//! Library folder watcher: auto-rescan on disk changes (kernel-side).
//!
//! One background thread watches all library roots (the same roots the
//! manual "rescan all" covers). Disk events are debounced, then a full
//! idempotent rescan runs. Results surface as `KernelEvent::LibraryChanged`
//! — quiet rescans (no adds/removals) emit nothing. The watcher stays
//! invisible until it actually matters.
//!
//! Lock discipline: the watcher NEVER touches `kernel.library` — the
//! scan runs through a cloned `SqlitePool`, so concurrent kernel calls
//! holding the library mutex never block it and vice versa. Roots are
//! re-read from the settings table on every rescan, so a root added via
//! the UI is picked up next time; the watch set itself is refreshed
//! after each rescan (new roots start being watched).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use aurora_storage::SqlitePool;
use notify::Watcher;

use crate::events::{EventSubscribers, KernelEvent};

/// How long disk activity must stay quiet before a rescan fires.
const DEBOUNCE: Duration = Duration::from_millis(1500);

/// Minimum gap between two rescans (a busy folder should not rescan in a
/// loop; the scan itself is idempotent so this is purely politeness).
const MIN_RESCAN_INTERVAL: Duration = Duration::from_secs(5);

/// Start the library watcher (call once, after the library is open).
///
/// The watcher holds a clone of the pool, so it survives independently
/// of the kernel's library slot (which may be swapped by `open_library`).
pub fn spawn_library_watcher(pool: SqlitePool, roots: Vec<PathBuf>, events: Arc<EventSubscribers>) {
    // Disk events funnel into this channel; the debounce loop reads it.
    let (tx, rx) = mpsc::channel::<()>();

    let mut watcher =
        match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            // Any event in a watched root nudges the debounce loop; event
            // detail does not matter (the rescan is idempotent and cheap
            // when nothing changed).
            let _ = match res {
                Ok(_) => tx.send(()),
                Err(e) => {
                    tracing::debug!(%e, "library watch: event error");
                    Ok(())
                }
            };
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(%e, "library watch: backend unavailable (watch disabled)");
                return;
            }
        };

    let mut watched: Vec<PathBuf> = Vec::new();
    for root in &roots {
        match watcher.watch(root, notify::RecursiveMode::Recursive) {
            Ok(()) => {
                watched.push(root.clone());
                tracing::info!(path = %root.display(), "library watch: watching root");
            }
            Err(e) => {
                tracing::warn!(path = %root.display(), %e, "library watch: cannot watch root");
            }
        }
    }

    if watched.is_empty() {
        tracing::info!("library watcher: no roots to watch yet (idle)");
    }

    std::thread::Builder::new()
        .name("aurora-library-watch".into())
        .spawn(move || {
            let mut last_rescan = std::time::Instant::now();
            loop {
                // Wait for the next disk event.
                if rx.recv().is_err() {
                    break; // watcher dropped — thread ends
                }
                // Debounce: keep draining while activity continues.
                loop {
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                // Rate limit.
                let since = last_rescan.elapsed();
                if since < MIN_RESCAN_INTERVAL {
                    std::thread::sleep(MIN_RESCAN_INTERVAL - since);
                }
                last_rescan = std::time::Instant::now();
                run_rescan(&pool, &mut watcher, &mut watched, &events);
            }
        })
        .ok();
}

/// One idempotent rescan across all roots; emit `LibraryChanged` only
/// when tracks were actually added or pruned.
fn run_rescan(
    pool: &SqlitePool,
    watcher: &mut notify::RecommendedWatcher,
    watched: &mut Vec<PathBuf>,
    events: &EventSubscribers,
) {
    let roots = match read_roots(pool) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "library watch: cannot read roots");
            return;
        }
    };
    if roots.is_empty() {
        return;
    }

    // The scan is async (sqlx/tokio); block this dedicated thread on it.
    let pool_for_scan = pool.clone();
    let roots_for_scan = roots.clone();
    let summary = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|rt| {
            rt.block_on(async move {
                let mut summary = aurora_library::ScanSummary::default();
                for root in &roots_for_scan {
                    let service = aurora_library::LibraryService::from_pool(pool_for_scan.clone());
                    match service.scan_folder(root).await {
                        Ok(s) => {
                            summary.discovered += s.discovered;
                            summary.added += s.added;
                            summary.updated += s.updated;
                            summary.pruned += s.pruned;
                            summary.failed += s.failed;
                        }
                        Err(e) => {
                            tracing::warn!(path = %root.display(), %e, "library watch: scan failed");
                        }
                    }
                }
                summary
            })
        })
        .unwrap_or_default();

    // Refresh the watch set: new roots start being watched, vanished
    // roots are dropped (their files are pruned by the scan anyway).
    for root in &roots {
        if !watched.contains(root) {
            match watcher.watch(root, notify::RecursiveMode::Recursive) {
                Ok(()) => {
                    watched.push(root.clone());
                    tracing::info!(path = %root.display(), "library watch: now watching");
                }
                Err(e) => {
                    tracing::warn!(path = %root.display(), %e, "library watch: cannot watch");
                }
            }
        }
    }
    for gone in watched.clone() {
        if !roots.contains(&gone) {
            let _ = watcher.unwatch(&gone);
        }
    }
    watched.retain(|r| roots.contains(r));

    if summary.added > 0 || summary.pruned > 0 {
        events.emit(&KernelEvent::LibraryChanged {
            added: summary.added,
            pruned: summary.pruned,
            updated: summary.updated,
        });
        tracing::info!(
            added = summary.added,
            pruned = summary.pruned,
            updated = summary.updated,
            "library watch: changed"
        );
    }
}

/// Read library roots from the settings table (from a sync context).
fn read_roots(pool: &SqlitePool) -> Result<Vec<PathBuf>, String> {
    let pool = pool.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(async move {
            aurora_storage::library_roots(&pool)
                .await
                .map_err(|e| e.to_string())
        })
        .map(|roots| roots.into_iter().map(PathBuf::from).collect())
}
