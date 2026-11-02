//! Plugin editor GUI windows — **out-of-process** (ADR 0004 amendment).
//!
//! The first in-process attempt crashed the whole player when a plugin's
//! editor initialization faulted (thread isolation does not protect from
//! a segfault). Now the editor runs in the `vst-gui-helper` binary: it
//! loads the plugin, opens the native window, pumps messages, and prints
//! parameter snapshots (JSON lines) on stdout. A crashing editor kills
//! the helper window — never the player.
//!
//! The helper lives next to the executable (cargo builds it into
//! `target/<profile>/`, same tree as the app in dev; deployment ships it
//! beside the binary).

use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};

/// A running plugin editor window. `Drop` closes it (kills the helper).
pub struct PluginGui {
    child: Option<Child>,
}

impl PluginGui {
    /// Ask the helper to close (its window teardown runs in the helper)
    /// and wait for a clean exit, then force-kill.
    pub fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Best-effort graceful: the helper's stdin thread reads this.
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = writeln!(stdin, "close");
                let _ = stdin.flush();
            }
            let _ = child.wait();
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Whether the helper process is still alive (editor window open).
    pub fn is_alive(&mut self) -> bool {
        self.child
            .as_mut()
            .map(|c| matches!(c.try_wait(), Ok(None)))
            .unwrap_or(false)
    }
}

impl Drop for PluginGui {
    fn drop(&mut self) {
        self.close();
    }
}

/// Parameter-change callback: full (id, normalized value) snapshot from
/// the editor.
pub type ParamSnapshot = Box<dyn Fn(Vec<(u32, f64)>) + Send + 'static>;

/// Open a plugin's editor window in a helper process. `on_params` fires
/// (on a reader thread) whenever the user changes parameters in the
/// editor; the host forwards these to the audio chain.
pub fn open_gui(path: &str, class_id: &str, on_params: ParamSnapshot) -> Result<PluginGui, String> {
    let helper = find_helper_binary()
        .ok_or_else(|| "vst-gui-helper not found (build it beside the application)".to_string())?;

    let mut child = Command::new(helper)
        .arg(path)
        .arg(class_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn vst-gui-helper: {e}"))?;

    let stdout = child.stdout.take().ok_or("helper stdout unavailable")?;
    std::thread::Builder::new()
        .name("aurora-vst-gui-reader".into())
        .spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(params) = parse_params_line(&line) {
                    on_params(params);
                }
            }
        })
        .map_err(|e| format!("reader thread spawn failed: {e}"))?;

    Ok(PluginGui { child: Some(child) })
}

/// Parse `{"params":[{"id":0,"value":0.5},...]}` (the helper's output
/// format). Hand-rolled to keep the helper/JSON surface minimal.
fn parse_params_line(line: &str) -> Option<Vec<(u32, f64)>> {
    let start = line.find("\"params\":")?;
    let mut rest = &line[start + 9..];
    let mut out = Vec::new();
    while let Some(id_pos) = rest.find("\"id\":") {
        rest = &rest[id_pos + 5..];
        let id_end = rest.find(|c: char| !c.is_ascii_digit())?;
        let id: u32 = rest[..id_end].parse().ok()?;
        let val_pos = rest.find("\"value\":")?;
        rest = &rest[val_pos + 8..];
        let val_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
        let value: f64 = rest[..val_end].parse().ok()?;
        out.push((id, value));
    }
    Some(out)
}

/// Locate `vst-gui-helper(.exe)`: beside the current executable first
/// (deployed layout), then the cargo target tree (dev layout).
fn find_helper_binary() -> Option<std::path::PathBuf> {
    let name = format!("vst-gui-helper{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let beside = dir.join(&name);
    if beside.exists() {
        return Some(beside);
    }
    // Dev: the app lives in target/debug/ (or deps/); helpers build
    // into the same tree.
    for ancestor in dir.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("target") {
            for profile in ["debug", "release"] {
                let p = ancestor.join(profile).join(&name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_helper_param_lines() {
        let line = r#"{"params":[{"id":0,"value":0.5},{"id":12,"value":0.75}]}"#;
        let params = parse_params_line(line).expect("parse");
        assert_eq!(params, vec![(0, 0.5), (12, 0.75)]);
    }

    #[test]
    fn empty_params_line() {
        assert_eq!(parse_params_line(r#"{"params":[]}"#), Some(Vec::new()));
    }
}
