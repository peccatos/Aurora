//! Kernel events: how the kernel tells shells what happened.
//!
//! Shells subscribe once and project events into their own transport
//! (Tauri emit, WebSocket, wasm callback). The kernel never knows who
//! is listening.

use std::sync::Mutex;

/// Events the kernel emits.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum KernelEvent {
    /// The library watcher rescan changed the library.
    LibraryChanged {
        added: u64,
        pruned: u64,
        updated: u64,
    },
}

type Listener = Box<dyn Fn(&KernelEvent) + Send + Sync>;

/// Subscription registry (lock-protected; listeners are cheap).
#[derive(Default)]
pub struct EventSubscribers {
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_id: Mutex<u64>,
}

impl EventSubscribers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe; returns a guard that unsubscribes on drop.
    pub fn subscribe(&self, f: impl Fn(&KernelEvent) + Send + Sync + 'static) -> Subscription {
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(|p| p.into_inner());
            *next += 1;
            *next
        };
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((id, Box::new(f)));
        Subscription::new(id, self)
    }

    fn unsubscribe(&self, id: u64) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(i, _)| *i != id);
    }

    pub(crate) fn emit(&self, event: &KernelEvent) {
        let listeners = self.listeners.lock().unwrap_or_else(|p| p.into_inner());
        for (_, f) in listeners.iter() {
            f(event);
        }
    }
}

/// Unsubscribes on drop.
pub struct Subscription {
    id: u64,
    inner: *const EventSubscribers,
}

impl Subscription {
    fn new(id: u64, inner: &EventSubscribers) -> Self {
        Self {
            id,
            inner: inner as *const _,
        }
    }
}

// SAFETY: the subscription only touches the registry, which outlives all
// subscriptions (both live in the kernel).
unsafe impl Send for Subscription {}
unsafe impl Sync for Subscription {}

impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe { &*self.inner }.unsubscribe(self.id);
    }
}
