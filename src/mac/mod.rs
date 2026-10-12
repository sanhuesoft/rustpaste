//! macOS integration: NSPasteboard reading/writing, frontmost app, synthetic ⌘V,
//! Accessibility permission checks.

pub mod drag;
pub mod paste_injector;

use crate::model::{ClipItem, ItemKind, NewClip};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard,
    NSPasteboardWriting, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};
use std::io::Cursor;

const T_STRING: &str = "public.utf8-plain-text";
const T_RTF: &str = "public.rtf";
const T_HTML: &str = "public.html";
const T_PNG: &str = "public.png";
const T_JPEG: &str = "public.jpeg";
const T_TIFF: &str = "public.tiff";
const T_FILE_URL: &str = "public.file-url";
const T_URL_NAME: &str = "public.url-name";

/// Skip images bigger than this to protect memory/disk.
const MAX_IMAGE_BYTES: usize = 40 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct AppInfo {
    pub name: String,
    pub bundle: String,
    pub pid: i32,
}

pub fn frontmost_app() -> AppInfo {
    let ws = NSWorkspace::sharedWorkspace();
    match ws.frontmostApplication() {
        Some(app) => AppInfo {
            name: app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            bundle: app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            pid: app.processIdentifier(),
        },
        None => AppInfo::default(),
    }
}

pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

pub fn pasteboard_types() -> Vec<String> {
    NSPasteboard::generalPasteboard()
        .types()
        .map(|a| a.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn data_to_vec(d: &NSData) -> Vec<u8> {
    d.to_vec()
}

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let data = NSData::with_bytes(tiff);
    let rep = NSBitmapImageRep::imageRepWithData(&data)?;
    let props: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> = NSDictionary::new();
    // SAFETY: empty properties dictionary is valid for PNG representation.
    let png =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(data_to_vec(&png))
}

fn png_dims(png: &[u8]) -> (u32, u32) {
    image::ImageReader::new(Cursor::new(png))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .unwrap_or((0, 0))
}

fn image_as_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

/// Read the current general pasteboard into a `NewClip` (without source info).
pub fn read_pasteboard() -> Option<NewClip> {
    let pb = NSPasteboard::generalPasteboard();
    let types: Vec<String> = pb
        .types()
        .map(|a| a.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let has = |t: &str| types.iter().any(|x| x == t);

    // 1. Files
    if has(T_FILE_URL) {
        let mut paths = vec![];
        if let Some(items) = pb.pasteboardItems() {
            for item in items.iter() {
                if let Some(s) = item.stringForType(&NSString::from_str(T_FILE_URL)) {
                    if let Some(url) = NSURL::URLWithString(&s) {
                        if let Some(p) = url.path() {
                            paths.push(p.to_string());
                        }
                    }
                }
            }
        }
        if !paths.is_empty() {
            return Some(NewClip {
                kind: Some(ItemKind::File),
                text: paths.join("\n"),
                ..Default::default()
            });
        }
    }

    let text = pb
        .stringForType(&NSString::from_str(T_STRING))
        .map(|s| s.to_string())
        .filter(|s| s.len() <= MAX_TEXT_BYTES);

    // 2. Images (only when there's no plain text, e.g. a copied picture)
    if text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true)
        && (has(T_PNG) || has(T_JPEG) || has(T_TIFF))
    {
        let png = if has(T_PNG) {
            pb.dataForType(&NSString::from_str(T_PNG))
                .map(|d| data_to_vec(&d))
        } else if has(T_JPEG) {
            pb.dataForType(&NSString::from_str(T_JPEG))
                .and_then(|d| image_as_png(&data_to_vec(&d)))
        } else {
            pb.dataForType(&NSString::from_str(T_TIFF))
                .and_then(|d| tiff_to_png(&data_to_vec(&d)))
        };
        if let Some(png) = png.filter(|p| !p.is_empty() && p.len() <= MAX_IMAGE_BYTES) {
            let (width, height) = png_dims(&png);
            return Some(NewClip {
                kind: Some(ItemKind::Image),
                image_png: Some(png),
                width,
                height,
                ..Default::default()
            });
        }
    }

    // 3. Text / rich text
    let html = if has(T_HTML) {
        pb.dataForType(&NSString::from_str(T_HTML))
            .map(|d| data_to_vec(&d))
            .filter(|d| d.len() <= MAX_TEXT_BYTES)
    } else {
        None
    };
    let rtf = if has(T_RTF) {
        pb.dataForType(&NSString::from_str(T_RTF))
            .map(|d| data_to_vec(&d))
            .filter(|d| d.len() <= MAX_TEXT_BYTES)
    } else {
        None
    };
    let text = text.or_else(|| {
        html.as_deref()
            .and_then(|data| std::str::from_utf8(data).ok().map(str::to_owned))
    })?;
    let rich = rtf.is_some() || html.is_some();
    let title = if has(T_URL_NAME) {
        pb.stringForType(&NSString::from_str(T_URL_NAME))
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
    } else {
        None
    };
    Some(NewClip {
        kind: Some(if rich {
            ItemKind::RichText
        } else {
            ItemKind::Text
        }),
        text,
        title,
        rtf,
        html,
        ..Default::default()
    })
}

/// Put an item on the general pasteboard. `plain` strips formatting.
/// Returns the pasteboard change count after writing, so the monitor can
/// ignore our own write.
pub fn write_item(item: &ClipItem, image_png: Option<&[u8]>, plain: bool) -> isize {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    match item.kind {
        ItemKind::Image => {
            if let Some(png) = image_png {
                pb.setData_forType(Some(&NSData::with_bytes(png)), &NSString::from_str(T_PNG));
            }
        }
        ItemKind::File if !plain => {
            let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = item
                .text
                .lines()
                .map(|p| {
                    ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(p)))
                })
                .collect();
            pb.writeObjects(&NSArray::from_retained_slice(&urls));
        }
        _ => {
            if !plain {
                if let Some(rtf) = &item.rtf {
                    pb.setData_forType(Some(&NSData::with_bytes(rtf)), &NSString::from_str(T_RTF));
                }
                if let Some(html) = &item.html {
                    pb.setData_forType(
                        Some(&NSData::with_bytes(html)),
                        &NSString::from_str(T_HTML),
                    );
                }
            }
            pb.setString_forType(
                &NSString::from_str(&item.text),
                &NSString::from_str(T_STRING),
            );
        }
    }
    pb.changeCount()
}

pub fn activate_pid(pid: i32) {
    if pid <= 0 {
        return;
    }
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

/// Synthesize ⌘V for the frontmost application.
pub fn post_cmd_v() {
    let _ = paste_injector::post_command_v();
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
    -> bool;
}

pub fn accessibility_trusted() -> bool {
    // SAFETY: plain C call without arguments.
    unsafe { AXIsProcessTrusted() }
}

/// Shows the system prompt that sends the user to Accessibility settings.
pub fn request_accessibility() -> bool {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let dict =
        CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    // SAFETY: dict is a valid CFDictionary for the duration of the call.
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) }
}

/// Frame (egui/top-left coordinates) of the screen currently containing the
/// mouse: `(x, y, width, height)`.
pub fn active_screen_frame() -> Option<(f32, f32, f32, f32)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let primary_h = screens.iter().next()?.frame().size.height;
    let mouse = NSEvent::mouseLocation();
    let pick = screens
        .iter()
        .find(|s| {
            let f = s.frame();
            mouse.x >= f.origin.x
                && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y
                && mouse.y < f.origin.y + f.size.height
        })
        .or_else(|| screens.iter().next())?;
    let f = pick.frame();
    Some((
        f.origin.x as f32,
        (primary_h - (f.origin.y + f.size.height)) as f32,
        f.size.width as f32,
        f.size.height as f32,
    ))
}

/// Make the egui window behave like a panel: above the Dock, visible on all
/// Spaces (including over full-screen apps), no shadow.
pub fn configure_window(frame: &eframe::Frame, show_in_screen_sharing: bool) {
    use objc2_app_kit::{NSView, NSWindowCollectionBehavior, NSWindowSharingType};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle points to the live NSView owned by the winit window,
    // and this runs on the main thread inside the event loop.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    drag::set_main_view(h.ns_view.as_ptr());
    if let Some(win) = view.window() {
        win.setLevel(25); // NSStatusWindowLevel
        win.setHasShadow(false);
        win.setSharingType(if show_in_screen_sharing {
            NSWindowSharingType::ReadOnly
        } else {
            NSWindowSharingType::None
        });
        win.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    }
}

/// Style the independent Settings viewport sharing preference.
pub fn configure_key_window_sharing(show_in_screen_sharing: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindowSharingType};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() {
        let target = if show_in_screen_sharing {
            NSWindowSharingType::ReadOnly
        } else {
            NSWindowSharingType::None
        };
        if window.sharingType() != target {
            window.setSharingType(target);
        }
    }
}

/// Play the short macOS pasteboard confirmation sound when enabled by the user.
pub fn play_paste_sound() {
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;
    let name = NSString::from_str("Pop");
    if let Some(sound) = NSSound::soundNamed(&name) {
        let _ = sound.play();
    }
}

/// Set the app as an accessory (menu bar only, no Dock icon).
pub fn set_accessory_policy() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
}

/// Bring our own app to the front (needed so the panel receives keystrokes).
pub fn activate_self() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

/// App icon as 128×128 RGBA, for crisp scaling in the card header.
pub fn app_icon_rgba(bundle: &str) -> Option<(u32, u32, Vec<u8>)> {
    let ws = NSWorkspace::sharedWorkspace();
    let url = ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle))?;
    let path = url.path()?;
    let img = ws.iconForFile(&path);
    let tiff = img.TIFFRepresentation()?;
    let png = tiff_to_png(&data_to_vec(&tiff))?;
    let rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?;
    let small = image::imageops::resize(
        &rgba.to_rgba8(),
        128,
        128,
        image::imageops::FilterType::Triangle,
    );
    Some((128, 128, small.into_raw()))
}
