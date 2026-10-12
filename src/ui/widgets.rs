//! Painting helpers: theme, pills, cards, code syntax highlighting and tray icon bitmap.

use crate::model::{ClipItem, ItemKind, parse_hex_color, relative_time};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, ColorImage, Context, CornerRadius, FontId, Layout, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub bg: Color32,
    pub border: Color32,
    pub card: Color32,
    pub text: Color32,
    pub subtext: Color32,
    pub pill_hover: Color32,
    pub accent: Color32,
}

impl Theme {
    pub fn new(dark: bool) -> Theme {
        if dark {
            Theme {
                dark,
                bg: Color32::from_rgba_unmultiplied(26, 26, 28, 245),
                border: Color32::from_rgba_unmultiplied(255, 255, 255, 24),
                card: Color32::from_rgb(38, 38, 42),
                text: Color32::from_rgb(242, 242, 247),
                subtext: Color32::from_rgb(152, 152, 160),
                pill_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                accent: Color32::from_rgb(10, 132, 255),
            }
        } else {
            Theme {
                dark,
                bg: Color32::from_rgba_unmultiplied(242, 242, 247, 245),
                border: Color32::from_rgba_unmultiplied(0, 0, 0, 22),
                card: Color32::WHITE,
                text: Color32::from_rgb(28, 28, 32),
                subtext: Color32::from_rgb(120, 120, 128),
                pill_hover: Color32::from_rgba_unmultiplied(0, 0, 0, 28),
                accent: Color32::from_rgb(0, 122, 255),
            }
        }
    }
}

pub fn color_from_hex(hex: &str) -> Color32 {
    let [r, g, b] = parse_hex_color(hex).unwrap_or([142, 142, 147]);
    Color32::from_rgb(r, g, b)
}

fn luminance(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}

/// Black or white, whichever reads better on `bg`.
pub fn contrast_on(bg: Color32) -> Color32 {
    if luminance(bg) > 0.58 {
        Color32::from_rgb(20, 20, 22)
    } else {
        Color32::WHITE
    }
}

/// A rounded filter chip (optionally with a colored dot). Returns its response.
pub fn pill(ui: &mut Ui, th: &Theme, label: &str, dot: Option<Color32>, active: bool) -> Response {
    let text_color = if active { th.text } else { th.subtext };
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), FontId::proportional(13.0), text_color);
    let pad = 12.0;
    let dot_w = if dot.is_some() { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + pad * 2.0 + dot_w, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if resp.hovered() {
        th.pill_hover
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(13), fill);
    }
    if let Some(c) = dot {
        p.circle_filled(pos2(rect.left() + pad + 3.0, rect.center().y), 4.5, c);
    }
    p.galley(
        pos2(
            rect.left() + pad + dot_w,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text_color,
    );
    resp
}

/// Draw an Apple SF-style magnifying glass icon.
pub fn draw_search_icon(p: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let lens_offset = radius * 0.35;
    let lens_center = pos2(center.x - lens_offset, center.y - lens_offset);
    let r = radius * 0.72;
    p.circle_stroke(lens_center, r, Stroke::new(1.5, color));

    let cos45 = 0.70710678;
    let handle_start = pos2(lens_center.x + r * cos45, lens_center.y + r * cos45);
    let handle_len = radius * 0.85;
    let handle_end = pos2(
        handle_start.x + handle_len * cos45,
        handle_start.y + handle_len * cos45,
    );
    p.line_segment([handle_start, handle_end], Stroke::new(1.7, color));
}

/// macOS-style search bar matching Paste:
/// - Collapsed: clean, naked Apple SF-style magnifying glass toolbar icon (no background pill).
/// - Expanded: native macOS translucent capsule search field with internal icon and clear button.
pub fn search_bar(
    ui: &mut Ui,
    th: &Theme,
    query: &mut String,
    expanded: &mut bool,
    search_id: egui::Id,
    request_focus: &mut bool,
) -> (Response, bool) {
    let height = 26.0;
    let mut changed = false;

    if !*expanded {
        // Collapsed toolbar icon: naked glyph without background pill, matching Paste.
        let (rect, resp) = ui.allocate_exact_size(vec2(22.0, height), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();

        let icon_col = if resp.hovered() {
            th.text
        } else if th.dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 175)
        } else {
            Color32::from_rgba_unmultiplied(60, 60, 67, 175)
        };
        draw_search_icon(&p, rect.center(), 7.2, icon_col);

        let resp = resp.on_hover_text("Search (⌘F)");
        if resp.clicked() {
            *expanded = true;
            *request_focus = true;
        }
        return (resp, false);
    }

    // Expanded search capsule
    let width = 210.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let p = ui.painter();

    let bg = if th.dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 22)
    } else {
        Color32::from_rgba_unmultiplied(0, 0, 0, 16)
    };
    let border = if th.dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 38)
    } else {
        Color32::from_rgba_unmultiplied(0, 0, 0, 26)
    };

    p.rect_filled(rect, CornerRadius::same(13), bg);
    p.rect_stroke(
        rect,
        CornerRadius::same(13),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    // Left magnifying glass icon
    let icon_center = pos2(rect.left() + 14.0, rect.center().y);
    draw_search_icon(&p, icon_center, 6.0, th.subtext);

    // Click icon to collapse if query is empty
    let icon_rect = Rect::from_center_size(icon_center, vec2(20.0, 20.0));
    if ui.input(|i| {
        i.pointer.any_click()
            && i.pointer
                .interact_pos()
                .map(|pos| icon_rect.contains(pos))
                .unwrap_or(false)
    }) {
        if query.is_empty() {
            *expanded = false;
        }
    }

    // Right clear button (if query not empty)
    let has_query = !query.is_empty();
    let text_right_pad = if has_query { 26.0 } else { 10.0 };

    if has_query {
        let clear_center = pos2(rect.right() - 13.0, rect.center().y);
        let clear_rect = Rect::from_center_size(clear_center, vec2(16.0, 16.0));
        let hovered = ui.input(|i| {
            i.pointer
                .hover_pos()
                .map(|pos| clear_rect.contains(pos))
                .unwrap_or(false)
        });
        let clicked = ui.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .map(|pos| clear_rect.contains(pos))
                    .unwrap_or(false)
        });
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let circle_col = if hovered {
            if th.dark {
                Color32::from_rgba_unmultiplied(210, 210, 215, 220)
            } else {
                Color32::from_rgba_unmultiplied(110, 110, 115, 220)
            }
        } else if th.dark {
            Color32::from_rgba_unmultiplied(160, 160, 165, 160)
        } else {
            Color32::from_rgba_unmultiplied(140, 140, 145, 160)
        };
        p.circle_filled(clear_center, 6.5, circle_col);

        // White cross inside
        let x_size = 2.8;
        let x_stroke = Stroke::new(1.3, Color32::WHITE);
        p.line_segment(
            [
                pos2(clear_center.x - x_size, clear_center.y - x_size),
                pos2(clear_center.x + x_size, clear_center.y + x_size),
            ],
            x_stroke,
        );
        p.line_segment(
            [
                pos2(clear_center.x + x_size, clear_center.y - x_size),
                pos2(clear_center.x - x_size, clear_center.y + x_size),
            ],
            x_stroke,
        );

        if clicked {
            query.clear();
            changed = true;
            *request_focus = true;
        }
    }

    // Singleline TextEdit embedded in the capsule
    let text_rect = Rect::from_min_max(
        pos2(rect.left() + 24.0, rect.top() + 3.0),
        pos2(rect.right() - text_right_pad, rect.bottom() - 3.0),
    );

    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let te = egui::TextEdit::singleline(query)
        .id(search_id)
        .frame(egui::Frame::NONE)
        .margin(vec2(0.0, 0.0))
        .hint_text("Search")
        .font(FontId::proportional(13.0))
        .desired_width(text_rect.width());

    let te_resp = child.add(te);
    if te_resp.changed() {
        changed = true;
    }

    if *request_focus {
        te_resp.request_focus();
        *request_focus = false;
    }

    (resp, changed)
}

/// Naked '+' icon button matching macOS toolbar style for adding pinboards.
pub fn plus_button(ui: &mut Ui, th: &Theme) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 26.0), Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("New pinboard");
    let p = ui.painter();
    let col = if resp.hovered() {
        th.text
    } else if th.dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 175)
    } else {
        Color32::from_rgba_unmultiplied(60, 60, 67, 175)
    };
    let c = rect.center();
    let s = 4.5;
    let stroke = Stroke::new(1.6, col);
    p.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke);
    p.line_segment([pos2(c.x, c.y - s), pos2(c.x, c.y + s)], stroke);
    resp
}

/// Lazily created textures: image thumbnails and app icons.
pub struct Caches {
    pub thumbs: HashMap<i64, Option<TextureHandle>>,
    pub icons: HashMap<String, Option<TextureHandle>>,
    pending_thumbs: HashSet<i64>,
    pending_icons: HashSet<String>,
    tx: Sender<CacheResult>,
    rx: Receiver<CacheResult>,
}

enum CacheResult {
    Thumb(i64, Option<ColorImage>),
    Icon(String, Option<ColorImage>),
}

impl Default for Caches {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            thumbs: HashMap::new(),
            icons: HashMap::new(),
            pending_thumbs: HashSet::new(),
            pending_icons: HashSet::new(),
            tx,
            rx,
        }
    }
}

impl Caches {
    fn poll(&mut self, ctx: &Context) {
        while let Ok(result) = self.rx.try_recv() {
            match result {
                CacheResult::Thumb(key, image) => {
                    self.pending_thumbs.remove(&key);
                    let texture = image.map(|image| {
                        ctx.load_texture(format!("thumb-{key}"), image, TextureOptions::LINEAR)
                    });
                    self.thumbs.insert(key, texture);
                }
                CacheResult::Icon(bundle, image) => {
                    self.pending_icons.remove(&bundle);
                    let texture = image.map(|image| {
                        ctx.load_texture(format!("icon-{bundle}"), image, TextureOptions::LINEAR)
                    });
                    self.icons.insert(bundle, texture);
                }
            }
        }
    }

    pub fn thumb(
        &mut self,
        ctx: &Context,
        key: i64,
        path: Option<PathBuf>,
        max: u32,
    ) -> Option<TextureHandle> {
        self.poll(ctx);
        if self.thumbs.len() > 80 {
            self.thumbs
                .retain(|key, _| self.pending_thumbs.contains(key));
        }
        if let Some(cached) = self.thumbs.get(&key) {
            return cached.clone();
        }
        if self.pending_thumbs.insert(key) {
            let tx = self.tx.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = path
                    .and_then(|path| image::open(path).ok())
                    .map(|image| image.thumbnail(max, max).to_rgba8())
                    .map(|image| {
                        let (w, h) = image.dimensions();
                        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw())
                    });
                let _ = tx.send(CacheResult::Thumb(key, result));
                repaint.request_repaint();
            });
        }
        None
    }

    pub fn icon(&mut self, ctx: &Context, bundle: &str) -> Option<TextureHandle> {
        self.poll(ctx);
        if bundle.is_empty() {
            return None;
        }
        if let Some(cached) = self.icons.get(bundle) {
            return cached.clone();
        }
        let bundle = bundle.to_string();
        if self.pending_icons.insert(bundle.clone()) {
            let tx = self.tx.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let image = crate::mac::app_icon_rgba(&bundle).map(|(w, h, rgba)| {
                    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba)
                });
                let _ = tx.send(CacheResult::Icon(bundle, image));
                repaint.request_repaint();
            });
        }
        None
    }
}

/// First `max_lines` lines / `max_chars` chars of `s`, for cheap layout.
pub fn snippet(s: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        if i >= max_lines || out.chars().count() >= max_chars {
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out.extend(line.chars().take(max_chars));
    }
    out
}

fn link_parts(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let display_host = host.strip_prefix("www.").unwrap_or(host);
    let suffix = rest.get(host.len()..).unwrap_or_default();
    let footer = format!("{display_host}{suffix}");
    (display_host.to_string(), footer)
}

fn card_footer(item: &ClipItem) -> String {
    match item.kind {
        ItemKind::Link => link_parts(&item.text).1,
        ItemKind::File => item
            .text
            .lines()
            .next()
            .map(|path| {
                let path = PathBuf::from(path);
                path.strip_prefix(dirs::home_dir().unwrap_or_default())
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| item.footer()),
        _ => item.footer(),
    }
}

pub struct CardInput<'a> {
    pub item: &'a ClipItem,
    pub selected: bool,
    pub is_dragging: bool,
    pub number: Option<usize>,
    pub board_color: Option<Color32>,
    pub now_ms: i64,
    pub image_path: Option<PathBuf>,
    pub show_link_title: bool,
}

/// Syntax highlight code with a modern palette matching Paste (Regex/Code view).
pub fn highlight_code(ctx: &Context, code: &str, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.wrap.max_width = max_width;
    job.break_on_newline = true;

    let font = FontId::monospace(12.0);

    let col_keyword = Color32::from_rgb(255, 123, 114); // #FF7B72 Coral / Salmon
    let col_string = Color32::from_rgb(126, 231, 135); // #7EE787 Soft Green
    let col_regex = Color32::from_rgb(210, 168, 255); // #D2A8FF Light Violet
    let col_comment = Color32::from_rgb(139, 148, 158); // #8B949E Grey
    let col_number = Color32::from_rgb(121, 192, 255); // #79C0FF Light Blue
    let col_punct = Color32::from_rgb(121, 192, 255); // #79C0FF Punctuation / Operators
    let col_text = Color32::from_rgb(235, 235, 240); // #EBEBF0 Clean White

    for line in code.lines() {
        let trimmed_line = line.trim_start();
        if trimmed_line.starts_with("//")
            || trimmed_line.starts_with('#')
            || trimmed_line.starts_with("/*")
        {
            job.append(line, 0.0, TextFormat::simple(font.clone(), col_comment));
            job.append("\n", 0.0, TextFormat::simple(font.clone(), col_comment));
            continue;
        }

        let mut chars = line.chars().peekable();
        let mut token = String::new();

        while let Some(&c) = chars.peek() {
            if c == '"' || c == '\'' || c == '`' {
                chars.next();
                token.push(c);
                while let Some(&next) = chars.peek() {
                    chars.next();
                    token.push(next);
                    if next == c {
                        break;
                    }
                }
                job.append(&token, 0.0, TextFormat::simple(font.clone(), col_string));
                token.clear();
            } else if c == '/'
                && chars
                    .clone()
                    .nth(1)
                    .map(|x| x != '/' && x != ' ')
                    .unwrap_or(false)
                && (line.contains("/g")
                    || line.contains("/i")
                    || line.contains("/m")
                    || line.contains("RegExp("))
            {
                // Potential regex literal
                chars.next();
                token.push(c);
                while let Some(&next) = chars.peek() {
                    chars.next();
                    token.push(next);
                    if next == '/' {
                        // Include flags
                        while let Some(&flag) = chars.peek() {
                            if "gimsuy".contains(flag) {
                                chars.next();
                                token.push(flag);
                            } else {
                                break;
                            }
                        }
                        break;
                    }
                }
                job.append(&token, 0.0, TextFormat::simple(font.clone(), col_regex));
                token.clear();
            } else if c.is_alphanumeric() || c == '_' || c == '$' {
                chars.next();
                token.push(c);
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' || next == '$' {
                        chars.next();
                        token.push(next);
                    } else {
                        break;
                    }
                }
                let color =
                    match token.as_str() {
                        "const" | "let" | "var" | "function" | "return" | "if" | "else" | "new"
                        | "class" | "import" | "export" | "from" | "fn" | "pub" | "def"
                        | "match" | "struct" | "enum" | "impl" | "async" | "await" | "type"
                        | "interface" | "while" | "for" | "in" | "of" | "case" | "switch"
                        | "break" | "continue" | "try" | "catch" | "finally" | "throw"
                        | "SELECT" | "FROM" | "WHERE" => col_keyword,
                        "RegExp" | "String" | "Number" | "Boolean" | "Array" | "Object"
                        | "JSON" => col_regex,
                        "true" | "false" | "null" | "undefined" | "None" | "Some" | "Ok"
                        | "Err" => col_keyword,
                        _ if token.chars().all(|d| d.is_ascii_digit()) => col_number,
                        _ => col_text,
                    };
                job.append(&token, 0.0, TextFormat::simple(font.clone(), color));
                token.clear();
            } else if "=+-*/%&|^!~?:;,.<>()[]{}".contains(c) {
                chars.next();
                token.push(c);
                job.append(&token, 0.0, TextFormat::simple(font.clone(), col_punct));
                token.clear();
            } else {
                chars.next();
                token.push(c);
                job.append(&token, 0.0, TextFormat::simple(font.clone(), col_text));
                token.clear();
            }
        }
        job.append("\n", 0.0, TextFormat::simple(font.clone(), col_text));
    }

    ctx.fonts_mut(|f| f.layout_job(job))
}

/// Paint one card into `rect`.
pub fn paint_card(
    ui: &Ui,
    ctx: &Context,
    th: &Theme,
    caches: &mut Caches,
    rect: Rect,
    c: &CardInput,
) {
    caches.poll(ctx);
    let item = c.item;
    let p = ui.painter();

    // If card is inside a Pinboard, header adopts the Pinboard's color (matches Paste).
    // Otherwise, it uses the official Apple/Paste category color.
    let head_col = if let Some(bc) = c.board_color {
        bc
    } else {
        let kc = item.kind.rgb();
        Color32::from_rgb(kc[0], kc[1], kc[2])
    };
    let head_text = Color32::WHITE;
    let radius = 14u8;
    if c.is_dragging {
        p.rect_filled(
            rect,
            CornerRadius::same(radius),
            Color32::from_rgba_unmultiplied(20, 20, 24, 60),
        );
        p.rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 25)),
            StrokeKind::Inside,
        );
        return;
    }

    p.rect_filled(rect, CornerRadius::same(radius), th.card);
    let head_h = 48.0;
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), head_h));
    p.rect_filled(
        head,
        CornerRadius {
            nw: radius,
            ne: radius,
            sw: 0,
            se: 0,
        },
        head_col,
    );

    // The category label remains consistent; source-provided titles belong in the body.
    let head_title = item.kind.label();

    // Header title and relative time
    let title_clip = p.with_clip_rect(Rect::from_min_max(
        pos2(head.left() + 12.0, head.top() + 6.0),
        pos2(head.right() - 56.0, head.top() + 26.0),
    ));
    title_clip.text(
        pos2(head.left() + 12.0, head.top() + 7.0),
        Align2::LEFT_TOP,
        head_title,
        FontId::new(14.0, egui::FontFamily::Name("sf-pro-semibold".into())),
        head_text,
    );

    let sub = Color32::from_rgba_unmultiplied(255, 255, 255, 218);
    p.text(
        pos2(head.left() + 12.0, head.top() + 27.0),
        Align2::LEFT_TOP,
        relative_time(c.now_ms, item.created_at),
        FontId::proportional(11.0),
        sub,
    );

    // App icon with rounded corners on top right of the header
    let icon_rect = Rect::from_min_size(
        pos2(
            head.right() - 6.0 - 44.0,
            head.top() + (head_h - 44.0) / 2.0,
        ),
        vec2(44.0, 44.0),
    );
    if let Some(tex) = caches.icon(ctx, &item.source_bundle) {
        p.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else if !item.source_app.is_empty() {
        let initial: String = item.source_app.chars().take(1).collect();
        p.circle_filled(
            icon_rect.center(),
            22.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 40),
        );
        p.text(
            icon_rect.center(),
            Align2::CENTER_CENTER,
            initial,
            FontId::proportional(19.0),
            head_text,
        );
    }

    // Body & Footer
    match item.kind {
        ItemKind::Color => {
            // Full-bleed color swatch filling the whole bottom of the card, matching Paste
            let color_rect = Rect::from_min_max(
                pos2(rect.left(), head.bottom()),
                pos2(rect.right(), rect.bottom()),
            );
            let col = color_from_hex(&item.text);
            p.rect_filled(
                color_rect,
                CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: radius,
                    se: radius,
                },
                col,
            );

            // Centered clean hex label in monospace
            let hex_clean = item.text.trim().to_uppercase();
            p.text(
                color_rect.center(),
                Align2::CENTER_CENTER,
                hex_clean,
                FontId::monospace(17.0),
                contrast_on(col),
            );
        }
        ItemKind::Code => {
            // Full-bleed code editor box filling the whole bottom of the card, matching Paste
            let code_rect = Rect::from_min_max(
                pos2(rect.left(), head.bottom()),
                pos2(rect.right(), rect.bottom()),
            );
            let code_bg = Color32::from_rgb(26, 26, 30);
            p.rect_filled(
                code_rect,
                CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: radius,
                    se: radius,
                },
                code_bg,
            );

            // Integrated footer on dark background
            let foot_h = 26.0;
            let foot = Rect::from_min_max(
                pos2(code_rect.left(), code_rect.bottom() - foot_h),
                code_rect.max,
            );
            let mut fx = foot.left() + 14.0;
            if let Some(col) = c.board_color {
                p.circle_filled(pos2(fx + 4.0, foot.center().y), 4.0, col);
                fx += 14.0;
            }
            let footer_clip = p.with_clip_rect(Rect::from_min_max(
                pos2(foot.left(), foot.top()),
                pos2(foot.right() - 44.0, foot.bottom()),
            ));
            let code_subtext = Color32::from_rgb(148, 148, 156);
            footer_clip.text(
                pos2(fx, foot.center().y),
                Align2::LEFT_CENTER,
                card_footer(item),
                FontId::proportional(11.0),
                code_subtext,
            );
            if let Some(n) = c.number {
                p.text(
                    pos2(foot.right() - 14.0, foot.center().y),
                    Align2::RIGHT_CENTER,
                    format!("⌘{n}"),
                    FontId::proportional(11.0),
                    code_subtext,
                );
            }

            // Code text content
            let code_body = Rect::from_min_max(
                pos2(code_rect.left() + 14.0, code_rect.top() + 10.0),
                pos2(code_rect.right() - 14.0, foot.top() - 4.0),
            );
            let bp = p.with_clip_rect(code_body.intersect(ui.clip_rect()));
            let galley = highlight_code(ctx, &snippet(&item.text, 14, 800), code_body.width());
            bp.galley(code_body.min, galley, Color32::WHITE);
        }
        _ => {
            // Images use the full card body, with their resolution overlaid on the image.
            let is_image = item.kind == ItemKind::Image;
            let foot_h = if is_image { 0.0 } else { 26.0 };
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - foot_h), rect.max);
            if !is_image {
                let mut fx = foot.left() + 12.0;
                if let Some(col) = c.board_color {
                    p.circle_filled(pos2(fx + 4.0, foot.center().y), 4.0, col);
                    fx += 14.0;
                }
                let footer_clip = p.with_clip_rect(Rect::from_min_max(
                    pos2(foot.left(), foot.top()),
                    pos2(foot.right() - 44.0, foot.bottom()),
                ));
                footer_clip.text(
                    pos2(fx, foot.center().y),
                    Align2::LEFT_CENTER,
                    card_footer(item),
                    FontId::proportional(11.0),
                    th.subtext,
                );
                if let Some(n) = c.number {
                    p.text(
                        pos2(foot.right() - 12.0, foot.center().y),
                        Align2::RIGHT_CENTER,
                        format!("⌘{n}"),
                        FontId::proportional(11.0),
                        th.subtext,
                    );
                }
            }

            // Body
            let body = if is_image {
                Rect::from_min_max(
                    pos2(rect.left() + 1.0, head.bottom()),
                    pos2(rect.right() - 1.0, rect.bottom() - 1.0),
                )
            } else {
                Rect::from_min_max(
                    pos2(rect.left() + 12.0, head.bottom() + 8.0),
                    pos2(rect.right() - 12.0, foot.top() - 4.0),
                )
            };
            let bp = p.with_clip_rect(body.intersect(ui.clip_rect()));

            match item.kind {
                ItemKind::Code | ItemKind::Color => unreachable!(),
                ItemKind::Image => {
                    if let Some(tex) = caches.thumb(ctx, item.id, c.image_path.clone(), 480) {
                        let ts = tex.size_vec2();
                        let scale = (body.width() / ts.x).min(body.height() / ts.y);
                        let r = Rect::from_center_size(body.center(), ts * scale);
                        bp.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        if item.width > 0 && item.height > 0 {
                            let badge = format!("{} × {}", item.width, item.height);
                            let badge_rect = Rect::from_center_size(
                                pos2(body.center().x, body.bottom() - 18.0),
                                vec2(92.0, 22.0),
                            );
                            p.rect_filled(
                                badge_rect,
                                CornerRadius::same(11),
                                Color32::from_rgba_unmultiplied(0, 0, 0, 112),
                            );
                            p.text(
                                badge_rect.center(),
                                Align2::CENTER_CENTER,
                                badge,
                                FontId::proportional(11.0),
                                Color32::WHITE,
                            );
                        }
                    } else {
                        bp.text(
                            body.center(),
                            Align2::CENTER_CENTER,
                            "Image unavailable",
                            FontId::proportional(12.0),
                            th.subtext,
                        );
                    }
                }
                ItemKind::File => {
                    let names: Vec<String> = item
                        .text
                        .lines()
                        .take(8)
                        .map(|l| {
                            std::path::Path::new(l)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| l.to_string())
                        })
                        .collect();
                    let g = bp.layout(
                        names.join("\n"),
                        FontId::proportional(13.0),
                        th.text,
                        body.width(),
                    );
                    bp.galley(body.min, g, th.text);
                }
                ItemKind::Link => {
                    let url = item.text.trim();
                    let (host, _) = link_parts(url);
                    let title = item
                        .title
                        .as_deref()
                        .filter(|_| c.show_link_title)
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or(&host);
                    let g1 = bp.layout(
                        title.to_string(),
                        FontId::proportional(14.5),
                        th.text,
                        body.width(),
                    );
                    let h1 = g1.size().y;
                    bp.galley(body.min, g1, th.text);
                    let g2 = bp.layout(
                        url.to_string(),
                        FontId::proportional(11.5),
                        Color32::from_rgb(52, 199, 89),
                        body.width(),
                    );
                    bp.galley(
                        pos2(body.left(), body.top() + h1 + 6.0),
                        g2,
                        Color32::from_rgb(52, 199, 89),
                    );
                }
                ItemKind::Text | ItemKind::RichText => {
                    let g = bp.layout(
                        snippet(&item.text, 14, 900),
                        FontId::proportional(13.0),
                        th.text,
                        body.width(),
                    );
                    bp.galley(body.min, g, th.text);
                }
            }
        }
    }

    // Selection border
    if c.selected {
        p.rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(3.0, Color32::from_rgb(10, 132, 255)),
            StrokeKind::Inside,
        );
    } else {
        p.rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(1.0, th.border),
            StrokeKind::Inside,
        );
    }
}

/// 44×44 RGBA clipboard glyph for the menu bar (used as a template image).
pub fn tray_icon_rgba() -> (Vec<u8>, u32, u32) {
    const N: u32 = 44;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let mut set = |x: u32, y: u32| {
        let i = ((y * N + x) * 4) as usize;
        px[i + 3] = 255;
    };
    let in_round = |x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32| {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).pow(2) + (y - cy).pow(2) <= r * r
    };
    for y in 0..N as i32 {
        for x in 0..N as i32 {
            let outer = in_round(x, y, 8, 8, 35, 40, 5);
            let inner = in_round(x, y, 11, 11, 32, 37, 3);
            let clip = in_round(x, y, 15, 3, 28, 13, 3);
            let line = (x >= 15 && x <= 28) && ((y >= 20 && y <= 22) || (y >= 27 && y <= 29));
            if (outer && !inner) || clip || line {
                set(x as u32, y as u32);
            }
        }
    }
    (px, N, N)
}
