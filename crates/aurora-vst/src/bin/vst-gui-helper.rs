//! Out-of-process VST3 plugin editor host (ADR 0004, GUI amendment).
//!
//! A standalone binary: loads ONE plugin in-process (here a crash only
//! kills this window), opens its editor, pumps Win32 messages, and prints
//! parameter snapshots to stdout as JSON lines:
//!
//!     {"params":[{"id":0,"value":0.5}, ...]}
//!
//! Stdin lines control it: "close" shuts down. The parent (aurora-vst
//! gui module) spawns one process per editor window and forwards param
//! snapshots into the audio chain.
//!
//! Usage: vst-gui-helper <path-to-vst3> <class-id>

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use vst3_host::{PluginWindow, Vst3HostBuilder};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: vst-gui-helper <vst3-path> <class-id>");
        std::process::exit(2);
    }
    let path = &args[1];
    let class_id = &args[2];

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .try_init();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_writer = Arc::clone(&stop);

    // Stdin reader: "close" (or EOF = parent died) stops the loop.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if l.trim() == "close" => break,
                Err(_) => break,
                _ => continue,
            }
        }
        stop_writer.store(true, Ordering::Release);
    });

    let mut host = match Vst3HostBuilder::default()
        .with_process_isolation(false)
        .sample_rate(44_100.0)
        .block_size(512)
        .build()
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("host build failed: {e}");
            std::process::exit(3);
        }
    };
    let plugin = match host.load_plugin_class(path, class_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("plugin load failed: {e}");
            std::process::exit(4);
        }
    };
    let plugin = Arc::new(std::sync::Mutex::new(plugin));
    let mut window = PluginWindow::new(Arc::clone(&plugin));
    if let Err(e) = window.open() {
        eprintln!("editor open failed: {e}");
        std::process::exit(5);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let mut last_params: Vec<(u32, f64)> = Vec::new();

    while !stop.load(Ordering::Acquire) {
        pump_windows_messages();
        let _ = window.service_platform_events();
        if window.closed_by_user() || !window.is_open() {
            break;
        }
        if last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            let params: Vec<(u32, f64)> = plugin
                .lock()
                .ok()
                .and_then(|p| {
                    p.get_parameters()
                        .ok()
                        .map(|v| v.into_iter().map(|x| (x.id, x.value)).collect())
                })
                .unwrap_or_default();
            if params != last_params {
                last_params = params.clone();
                let json = params_to_json(&params);
                let _ = writeln!(out, "{json}");
                let _ = out.flush();
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    window.close();
}

fn params_to_json(params: &[(u32, f64)]) -> String {
    let items: Vec<String> = params
        .iter()
        .map(|(id, v)| format!("{{\"id\":{id},\"value\":{v}}}"))
        .collect();
    format!("{{\"params\":[{}]}}", items.join(","))
}

/// Non-blocking Win32 message pump for this thread's windows.
/// On non-Windows platforms the vst3-host crate services its native
/// windows through `service_platform_events` (xcb/NSWindow paths), so
/// nothing extra is needed — the pump is a Windows-only no-op elsewhere.
#[cfg(target_os = "windows")]
fn pump_windows_messages() {
    use std::mem;
    use std::ptr;
    use winapi::um::winuser::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

    unsafe {
        let mut msg: MSG = mem::zeroed();
        while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn pump_windows_messages() {}
