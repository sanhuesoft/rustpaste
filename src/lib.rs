//! RustPaste: an open source clipboard manager for macOS inspired by Paste.
//!
//! The platform-independent core (model, store, privacy, settings) lives here
//! and is unit-tested; the macOS integration is in [`mac`] and the UI in [`ui`].

pub mod autostart;
pub mod model;
pub mod privacy;
pub mod settings;
pub mod store;

#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(target_os = "macos")]
pub mod monitor;
#[cfg(target_os = "macos")]
pub mod ui;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
