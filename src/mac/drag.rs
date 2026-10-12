//! Native macOS Drag & Drop for RustPaste items.
//! Allows dragging cards out of RustPaste directly into any macOS application
//! (Notes, Slack, Messages, Terminal, Finder, VS Code, Browser, etc.)
//! with native drag image, green copy badge, and automatic paste/drop.

use crate::model::{ClipItem, ItemKind};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSImage, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardTypeURL,
    NSPasteboardWriting, NSView,
};
use objc2_foundation::{
    NSArray, NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicPtr, Ordering};

static MAIN_VIEW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static DRAGGING_ID: AtomicI64 = AtomicI64::new(0);

type DragEndCallback = Box<dyn Fn(i64, Option<(f32, f32)>, bool) + Send + Sync>;
static DRAG_END_CB: Mutex<Option<DragEndCallback>> = Mutex::new(None);

pub fn set_main_view(view: *mut c_void) {
    MAIN_VIEW.store(view, Ordering::SeqCst);
}

pub fn set_drag_end_callback<F>(f: F)
where
    F: Fn(i64, Option<(f32, f32)>, bool) + Send + Sync + 'static,
{
    let mut cb = DRAG_END_CB.lock().unwrap();
    *cb = Some(Box::new(f));
}

pub fn currently_dragging_id() -> Option<i64> {
    let id = DRAGGING_ID.load(Ordering::SeqCst);
    if id != 0 { Some(id) } else { None }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RustPasteDraggingSource"]
    pub struct RustPasteDraggingSource;

    unsafe impl NSObjectProtocol for RustPasteDraggingSource {}

    #[allow(non_snake_case)]
    unsafe impl NSDraggingSource for RustPasteDraggingSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn draggingSession_sourceOperationMaskForDraggingContext(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn draggingSession_endedAtPoint_operation(
            &self,
            _session: &NSDraggingSession,
            screen_point: NSPoint,
            operation: NSDragOperation,
        ) {
            let id = DRAGGING_ID.swap(0, Ordering::SeqCst);
            let success = operation != NSDragOperation::None;

            let view_ptr = MAIN_VIEW.load(Ordering::SeqCst);
            let mut local_pos = None;
            if !view_ptr.is_null() {
                let view: &NSView = unsafe { &*(view_ptr as *const NSView) };
                if let Some(win) = view.window() {
                    let win_pt = win.convertPointFromScreen(screen_point);
                    let view_pt = view.convertPoint_fromView(win_pt, None);
                    let h = view.bounds().size.height;
                    local_pos = Some((view_pt.x as f32, (h - view_pt.y) as f32));
                }
            }

            if let Some(cb) = DRAG_END_CB.lock().unwrap().as_ref() {
                cb(id, local_pos, success);
            }
        }
    }
);

impl RustPasteDraggingSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Render a clean preview image of the card for the macOS drag session.
pub fn create_card_drag_image(
    item: &ClipItem,
    image_bytes: Option<&[u8]>,
    board_color: Option<[u8; 3]>,
    width: u32,
    height: u32,
) -> Option<Retained<NSImage>> {
    let w = width.max(120);
    let h = height.max(120);
    let mut img = RgbaImage::new(w, h);

    let head_rgb = board_color.unwrap_or_else(|| item.kind.rgb());
    let head_h = 44u32.min(h / 3);

    let card_bg = Rgba([32, 32, 36, 255]);
    let head_bg = Rgba([head_rgb[0], head_rgb[1], head_rgb[2], 255]);

    // Fill base background
    for y in 0..h {
        for x in 0..w {
            if y < head_h {
                img.put_pixel(x, y, head_bg);
            } else {
                img.put_pixel(x, y, card_bg);
            }
        }
    }

    // Body content filling
    match item.kind {
        ItemKind::Image => {
            if let Some(bytes) = image_bytes {
                if let Ok(src_img) = image::load_from_memory(bytes) {
                    let max_bw = (w - 16).max(10);
                    let max_bh = (h - head_h - 24).max(10);
                    let resized = src_img.thumbnail(max_bw, max_bh).to_rgba8();
                    let rw = resized.width();
                    let rh = resized.height();
                    let ox = (w - rw) / 2;
                    let oy = head_h + (h - head_h - rh) / 2;
                    for py in 0..rh {
                        for px in 0..rw {
                            let tx = ox + px;
                            let ty = oy + py;
                            if tx < w && ty < h {
                                img.put_pixel(tx, ty, *resized.get_pixel(px, py));
                            }
                        }
                    }
                }
            }
        }
        ItemKind::Color => {
            // Swatch color fills whole body
            let col = parse_hex_color(&item.text).unwrap_or([200, 200, 200]);
            let swatch_bg = Rgba([col[0], col[1], col[2], 255]);
            for y in head_h..h {
                for x in 0..w {
                    img.put_pixel(x, y, swatch_bg);
                }
            }
        }
        ItemKind::Code => {
            let code_bg = Rgba([26, 26, 30, 255]);
            for y in head_h..h {
                for x in 0..w {
                    img.put_pixel(x, y, code_bg);
                }
            }
        }
        _ => {}
    }

    // Apply corner rounding
    let radius = 14.0f32;
    for y in 0..h {
        for x in 0..w {
            let xf = x as f32 + 0.5;
            let yf = y as f32 + 0.5;
            let wf = w as f32;
            let hf = h as f32;

            let in_tl = xf < radius && yf < radius;
            let in_tr = xf > wf - radius && yf < radius;
            let in_bl = xf < radius && yf > hf - radius;
            let in_br = xf > wf - radius && yf > hf - radius;

            let d = if in_tl {
                ((xf - radius).powi(2) + (yf - radius).powi(2)).sqrt()
            } else if in_tr {
                ((xf - (wf - radius)).powi(2) + (yf - radius).powi(2)).sqrt()
            } else if in_bl {
                ((xf - radius).powi(2) + (yf - (hf - radius)).powi(2)).sqrt()
            } else if in_br {
                ((xf - (wf - radius)).powi(2) + (yf - (hf - radius)).powi(2)).sqrt()
            } else {
                0.0
            };

            if in_tl || in_tr || in_bl || in_br {
                if d > radius {
                    img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                } else if d > radius - 1.0 {
                    let factor = (radius - d).clamp(0.0, 1.0);
                    let p = img.get_pixel_mut(x, y);
                    p[3] = (p[3] as f32 * factor) as u8;
                }
            }
        }
    }

    let mut png_buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png_buf)
        .write_image(&img, w, h, ExtendedColorType::Rgba8)
        .ok()?;

    let ns_data = NSData::with_bytes(&png_buf);
    let ns_image = NSImage::initWithData(NSImage::alloc(), &ns_data)?;
    ns_image.setSize(NSSize {
        width: w as f64,
        height: h as f64,
    });
    Some(ns_image)
}

fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some([r, g, b])
    } else {
        None
    }
}

/// Start native macOS drag session for the given card.
pub fn start_drag(
    item: &ClipItem,
    image_bytes: Option<&[u8]>,
    image_path: Option<&std::path::Path>,
    board_color: Option<[u8; 3]>,
    card_rect: (f32, f32, f32, f32), // (x, y, w, h) in egui coordinates
) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };

    let view_ptr = MAIN_VIEW.load(Ordering::SeqCst);
    let app = NSApplication::sharedApplication(mtm);

    // SAFETY: view_ptr was obtained from RawWindowHandle on the main thread and is live.
    let view: &NSView = if !view_ptr.is_null() {
        unsafe { &*(view_ptr as *const NSView) }
    } else if let Some(win) = app.keyWindow().or_else(|| app.windows().iter().next()) {
        if let Some(content) = win.contentView() {
            // SAFETY: content view is retained by the window
            unsafe { &*(Retained::as_ptr(&content)) }
        } else {
            return false;
        }
    } else {
        return false;
    };

    let Some(event) = app.currentEvent() else {
        return false;
    };

    let mut drag_items: Vec<Retained<NSDraggingItem>> = Vec::new();

    match item.kind {
        ItemKind::File => {
            for path in item.text.lines().filter(|s| !s.trim().is_empty()) {
                let ns_str = NSString::from_str(path);
                let url = NSURL::fileURLWithPath(&ns_str);
                let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                    ProtocolObject::from_retained(url);
                let di = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
                drag_items.push(di);
            }
        }
        ItemKind::Image => {
            let pb_item = NSPasteboardItem::new();
            if let Some(png) = image_bytes {
                let ns_png = NSData::with_bytes(png);
                pb_item.setData_forType(&ns_png, unsafe { NSPasteboardTypePNG });
                pb_item.setData_forType(&ns_png, unsafe { NSPasteboardTypeTIFF });
            }
            if let Some(p) = image_path {
                let ns_path = NSString::from_str(&p.to_string_lossy());
                let url = NSURL::fileURLWithPath(&ns_path);
                if let Some(abs_str) = url.absoluteString() {
                    pb_item.setString_forType(&abs_str, unsafe { NSPasteboardTypeFileURL });
                }
            }
            let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                ProtocolObject::from_retained(pb_item);
            let di = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
            drag_items.push(di);
        }
        ItemKind::Link => {
            let pb_item = NSPasteboardItem::new();
            let url_str = item.text.trim();
            let ns_text = NSString::from_str(url_str);
            pb_item.setString_forType(&ns_text, unsafe { NSPasteboardTypeString });
            pb_item.setString_forType(&ns_text, unsafe { NSPasteboardTypeURL });
            let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                ProtocolObject::from_retained(pb_item);
            let di = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
            drag_items.push(di);
        }
        _ => {
            // Text, Code, Color
            let pb_item = NSPasteboardItem::new();
            let ns_text = NSString::from_str(&item.text);
            pb_item.setString_forType(&ns_text, unsafe { NSPasteboardTypeString });
            if let Some(rtf) = &item.rtf {
                let ns_rtf = NSData::with_bytes(rtf);
                pb_item.setData_forType(&ns_rtf, unsafe { NSPasteboardTypeRTF });
            }
            let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                ProtocolObject::from_retained(pb_item);
            let di = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
            drag_items.push(di);
        }
    }

    if drag_items.is_empty() {
        return false;
    }

    let (rx, ry, rw, rh) = card_rect;
    let view_bounds = view.bounds();
    let view_h = view_bounds.size.height;
    let appkit_y = view_h - (ry as f64 + rh as f64);
    let frame = NSRect {
        origin: NSPoint {
            x: rx as f64,
            y: appkit_y,
        },
        size: NSSize {
            width: rw as f64,
            height: rh as f64,
        },
    };

    let drag_img = create_card_drag_image(
        item,
        image_bytes,
        board_color,
        rw.round() as u32,
        rh.round() as u32,
    );

    let primary = &drag_items[0];
    if let Some(img) = &drag_img {
        let any_obj: &AnyObject = &*img;
        unsafe {
            primary.setDraggingFrame_contents(frame, Some(any_obj));
        }
    } else {
        primary.setDraggingFrame(frame);
    }

    DRAGGING_ID.store(item.id, Ordering::SeqCst);

    let source = RustPasteDraggingSource::new(mtm);
    let source_proto = ProtocolObject::from_ref(&*source);
    let arr = NSArray::from_retained_slice(&drag_items);

    let session = view.beginDraggingSessionWithItems_event_source(&arr, &event, source_proto);
    session.setAnimatesToStartingPositionsOnCancelOrFail(true);

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_image_generation_works() {
        let item = ClipItem {
            id: 1,
            kind: ItemKind::Link,
            text: "https://bluebottlecoffee.com".into(),
            ..Default::default()
        };
        let img = create_card_drag_image(&item, None, Some([52, 199, 89]), 180, 200);
        assert!(img.is_some());
    }

    #[test]
    fn drag_image_color_swatch() {
        let item = ClipItem {
            id: 2,
            kind: ItemKind::Color,
            text: "#D0EBFA".into(),
            ..Default::default()
        };
        let img = create_card_drag_image(&item, None, None, 180, 200);
        assert!(img.is_some());
    }

    #[test]
    fn hex_parser_for_drag() {
        assert_eq!(parse_hex_color("#FF9F0A"), Some([255, 159, 10]));
        assert_eq!(parse_hex_color("34C759"), Some([52, 199, 89]));
        assert_eq!(parse_hex_color("invalid"), None);
    }
}
