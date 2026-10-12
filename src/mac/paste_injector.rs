//! Synthetic keyboard input for direct paste on macOS.
//!
//! The caller must hide its panel and reactivate the previously focused app on
//! the main thread before invoking `paste_after_focus`. Posting CGEvents can
//! run on a worker thread after that AppKit work has completed.

use anyhow::{Result, anyhow};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::thread;
use std::time::Duration;

const KEY_V: u16 = 9;
const TARGET_FOCUS_DELAY: Duration = Duration::from_millis(140);

/// Wait for the target application to regain focus, then synthesize ⌘V.
pub fn paste_after_focus() -> Result<()> {
    thread::sleep(TARGET_FOCUS_DELAY);
    post_command_v()
}

/// Synthesize a Command+V keystroke through the HID event tap.
pub fn post_command_v() -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("could not create HID keyboard event source"))?;

    let down = CGEvent::new_keyboard_event(source.clone(), KEY_V, true)
        .map_err(|_| anyhow!("could not create Command+V key-down event"))?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source, KEY_V, false)
        .map_err(|_| anyhow!("could not create Command+V key-up event"))?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    up.post(CGEventTapLocation::HID);
    Ok(())
}
