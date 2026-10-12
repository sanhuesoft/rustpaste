//! Background clipboard monitor: polls `NSPasteboard.changeCount`.

use crate::mac;
use crate::now_ms;
use crate::privacy::should_ignore_with_options;
use crate::settings::Settings;
use crate::store::Store;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const POLL_INTERVAL: Duration = Duration::from_millis(400);

/// State shared between the monitor thread and the UI.
pub struct Shared {
    pub store: Mutex<Store>,
    pub settings: Mutex<Settings>,
    /// Change count produced by our own pasteboard writes (ignored once).
    pub ignore_change: AtomicIsize,
    /// Bumped whenever history changes so the UI can refresh.
    pub revision: std::sync::atomic::AtomicU64,
    pub quit: AtomicBool,
}

pub type SharedRef = Arc<Shared>;

pub fn spawn(
    shared: SharedRef,
    wake: impl Fn() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("clipboard-monitor".into())
        .spawn(move || run(shared, wake))
}

fn run(shared: SharedRef, wake: impl Fn()) {
    let mut last = mac::change_count();
    let mut ticks: u32 = 0;
    while !shared.quit.load(Ordering::Relaxed) {
        std::thread::sleep(POLL_INTERVAL);
        ticks = ticks.wrapping_add(1);

        let cc = mac::change_count();
        if cc != last {
            last = cc;
            if shared.ignore_change.swap(-1, Ordering::SeqCst) == cc {
                continue; // our own write
            }
            capture(&shared);
            wake();
        }
        // Retention roughly every 5 minutes.
        if ticks % 750 == 0 {
            purge(&shared);
        }
    }
}

fn capture(shared: &Shared) {
    let Ok(settings) = shared.settings.lock().map(|guard| guard.clone()) else {
        eprintln!("rustpaste: settings lock poisoned; skipping clipboard capture");
        return;
    };
    if settings.paused {
        return;
    }
    let app = mac::frontmost_app();
    let types = mac::pasteboard_types();
    if should_ignore_with_options(
        &types,
        &app.bundle,
        &settings.ignored_apps,
        settings.ignore_sensitive_clipboard,
        settings.ignore_transient_clipboard,
    ) {
        return;
    }
    let Some(mut clip) = mac::read_pasteboard() else {
        return;
    };
    clip.source_app = app.name;
    clip.source_bundle = app.bundle;
    let Ok(mut store) = shared.store.lock() else {
        eprintln!("rustpaste: store lock poisoned; skipping clipboard capture");
        return;
    };
    match store.insert(&clip, now_ms()) {
        Ok(Some(_)) => {
            let _ = store.purge(settings.retention_days, settings.max_items, now_ms());
            shared.revision.fetch_add(1, Ordering::SeqCst);
        }
        Ok(None) => {}
        Err(e) => eprintln!("rustpaste: failed to store clip: {e}"),
    }
}

pub fn purge(shared: &Shared) {
    let Ok(s) = shared.settings.lock().map(|guard| guard.clone()) else {
        eprintln!("rustpaste: settings lock poisoned; skipping retention cleanup");
        return;
    };
    let Ok(mut store) = shared.store.lock() else {
        eprintln!("rustpaste: store lock poisoned; skipping retention cleanup");
        return;
    };
    if let Ok(n) = store.purge(s.retention_days, s.max_items, now_ms()) {
        if n > 0 {
            shared.revision.fetch_add(1, Ordering::SeqCst);
        }
    }
}
