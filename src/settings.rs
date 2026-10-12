//! User settings persisted as JSON in the app support directory.

use crate::privacy::default_ignored_apps;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Global hotkey in `global-hotkey` syntax, e.g. `shift+super+KeyV`.
    pub hotkey: String,
    /// Delete unpinned items older than this many days. `0` = keep forever.
    pub retention_days: u32,
    /// Keep at most this many unpinned items. `0` = unlimited.
    pub max_items: u32,
    pub ignored_apps: Vec<String>,
    pub launch_at_login: bool,
    pub paused: bool,
    /// Paste selected clips directly into the previously focused application.
    pub paste_to_active_app: bool,
    /// Strip rich formats before placing a clip on the pasteboard.
    pub always_plain_text: bool,
    pub sound_effects: bool,
    /// Allow macOS screen capture APIs to read RustPaste windows.
    pub show_in_screen_sharing: bool,
    /// Show the source-provided page title on link cards when available.
    pub generate_link_previews: bool,
    /// Skip clips marked as concealed by their source application.
    pub ignore_sensitive_clipboard: bool,
    /// Skip temporary/transient pasteboard contents.
    pub ignore_transient_clipboard: bool,
    pub panel_height: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: "shift+super+KeyV".into(),
            retention_days: 30,
            max_items: 1000,
            ignored_apps: default_ignored_apps(),
            launch_at_login: false,
            paused: false,
            paste_to_active_app: true,
            always_plain_text: false,
            sound_effects: true,
            show_in_screen_sharing: true,
            generate_link_previews: true,
            ignore_sensitive_clipboard: true,
            ignore_transient_clipboard: true,
            panel_height: 330.0,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic save: write to a temp file, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub const APP_DIR_NAME: &str = "RustPaste";

/// `~/Library/Application Support/RustPaste` (via `dirs`).
pub fn app_support_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_json() {
        let dir = std::env::temp_dir().join(format!("rp-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.retention_days = 7;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);

        // Missing fields fall back to defaults (forward compatibility).
        std::fs::write(&path, br#"{"retention_days": 3}"#).unwrap();
        let l = Settings::load(&path);
        assert_eq!(l.retention_days, 3);
        assert_eq!(l.hotkey, Settings::default().hotkey);

        // Corrupt file falls back to defaults.
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
