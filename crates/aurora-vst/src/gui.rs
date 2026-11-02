//! Plugin editor GUI windows.
//!
//! ADR 0004 amendment: the audio chain stays out-of-process (crash-safe),
//! but a plugin *editor window* is a GUI-thread concern: the isolated
//! helper on Windows does not host GUIs, so the editor runs as a second
//! in-process instance in a dedicated GUI thread with its own Win32
//! message pump. Parameter changes the user makes in the editor are
//! forwarded to the audio chain through a callback (honest trade-off:
//! the GUI instance shares parameters, not DSP state).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Parameter-change callback: full (id, normalized value) snapshot from
/// the editor GUI.
pub type ParamSnapshot = Box<dyn Fn(Vec<(u32, f64)>) + Send + 'static>;

/// A running plugin editor window. Dropping it closes the window.
pub struct PluginGui {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PluginGui {
    /// Signal the GUI thread to close and wait for it.
    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PluginGui {
    fn drop(&mut self) {
        self.close();
    }
}

/// Open a plugin's editor window. `on_params` fires (on a GUI thread)
/// whenever the user changes a parameter in the editor, with the full
/// (id, normalized value) list — the host forwards these to the audio
/// chain.
#[cfg(target_os = "windows")]
pub fn open_gui(path: &str, class_id: &str, on_params: ParamSnapshot) -> Result<PluginGui, String> {
    use vst3_host::{PluginWindow, Vst3HostBuilder};

    let path = path.to_string();
    let class_id = class_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);

    // Everything plugin-related happens inside this thread: COM pointers
    // and the native window must not cross a thread boundary.
    let thread = std::thread::Builder::new()
        .name("aurora-vst-gui".into())
        .spawn(move || {
            let mut host = match Vst3HostBuilder::default()
                .with_process_isolation(false)
                .sample_rate(44_100.0)
                .block_size(crate::host::BLOCK_SIZE)
                .build()
            {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(%e, "vst gui: host build failed");
                    return;
                }
            };
            let plugin = match host.load_plugin_class(&path, &class_id) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(%e, %path, "vst gui: plugin load failed");
                    return;
                }
            };
            let plugin = Arc::new(std::sync::Mutex::new(plugin));
            let mut window = PluginWindow::new(Arc::clone(&plugin));
            if let Err(e) = window.open() {
                tracing::warn!(%e, "vst gui: editor open failed");
                return;
            }
            tracing::info!(%path, "vst gui: editor window open");

            let mut last_report = Instant::now() - Duration::from_secs(1);
            let mut last_params: Vec<(u32, f64)> = Vec::new();
            while !stop_thread.load(Ordering::Acquire) {
                pump_windows_messages();
                let _ = window.service_platform_events();
                if window.closed_by_user() || !window.is_open() {
                    break;
                }
                // Parameter sync: poll the editor's values ~5x/sec; the
                // plugin reports its own parameter state, so any control
                // the user moved shows up here.
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
                        on_params(params.clone());
                        last_params = params;
                    }
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            window.close();
            tracing::info!(%path, "vst gui: editor window closed");
        })
        .map_err(|e| e.to_string())?;

    Ok(PluginGui {
        stop,
        thread: Some(thread),
    })
}

#[cfg(not(target_os = "windows"))]
pub fn open_gui(
    _path: &str,
    _class_id: &str,
    _on_params: Box<dyn Fn(Vec<(u32, f64)>) + Send + 'static>,
) -> Result<PluginGui, String> {
    Err("plugin editor windows are only supported on Windows in this build".into())
}

/// Non-blocking Win32 message pump: dispatch everything queued for this
/// thread's windows. The editor's window was created on this thread, so
/// its messages (paint, input, DPI) are served here.
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
