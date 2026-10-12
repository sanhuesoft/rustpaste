//! Core data model: clipboard items, kinds, pinboards and classification helpers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ItemKind {
    #[default]
    Text,
    RichText,
    Code,
    Image,
    Link,
    File,
    Color,
}

impl ItemKind {
    pub const ALL: [ItemKind; 7] = [
        ItemKind::Text,
        ItemKind::RichText,
        ItemKind::Code,
        ItemKind::Image,
        ItemKind::Link,
        ItemKind::File,
        ItemKind::Color,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Text => "text",
            ItemKind::RichText => "richtext",
            ItemKind::Code => "code",
            ItemKind::Image => "image",
            ItemKind::Link => "link",
            ItemKind::File => "file",
            ItemKind::Color => "color",
        }
    }

    pub fn parse(s: &str) -> Option<ItemKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Human readable label shown in the card header.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Text => "Text",
            ItemKind::RichText => "Rich Text",
            ItemKind::Code => "Code",
            ItemKind::Image => "Image",
            ItemKind::Link => "Link",
            ItemKind::File => "File",
            ItemKind::Color => "Color",
        }
    }

    /// Header color (RGB) matching Paste / macOS system palette.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            ItemKind::Text => [0xFF, 0x9F, 0x0A], // Apple Notes Amber / Orange
            ItemKind::RichText => [0xFF, 0x64, 0x34], // Coral Orange
            ItemKind::Code => [0xAF, 0x52, 0xDE], // Apple Purple / Regex
            ItemKind::Image => [0xFF, 0x45, 0x3A], // Apple Photos / Red Coral
            ItemKind::Link => [0x34, 0xC7, 0x59], // Apple Maps / Safari Green
            ItemKind::File => [0x00, 0x7A, 0xFF], // Apple Finder Blue
            ItemKind::Color => [0xFF, 0x37, 0x5F], // Vivid Rose / Swatch Pink
        }
    }
}

/// A stored clipboard entry. `rtf` is only populated by `Store::get`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipItem {
    pub id: i64,
    pub kind: ItemKind,
    /// Plain text; for files, newline-separated paths; for images, empty.
    pub text: String,
    pub rtf: Option<Vec<u8>>,
    /// HTML flavor preserved for applications that prefer rich HTML paste.
    pub html: Option<Vec<u8>>,
    pub image_file: Option<String>,
    pub title: Option<String>,
    pub source_app: String,
    pub source_bundle: String,
    /// Unix epoch milliseconds of the last time this content was copied.
    pub created_at: i64,
    pub pinboard_id: Option<i64>,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

impl ClipItem {
    /// Footer metadata line, e.g. "120 characters" or "1024 × 768 · 84 KB".
    pub fn footer(&self) -> String {
        match self.kind {
            ItemKind::Image => format!(
                "{} × {} · {}",
                self.width,
                self.height,
                human_size(self.bytes)
            ),
            ItemKind::File => {
                let n = self.text.lines().count();
                if n == 1 {
                    "1 file".to_string()
                } else {
                    format!("{n} files")
                }
            }
            ItemKind::Link => self.text.trim().to_string(),
            ItemKind::Color => self.text.trim().to_uppercase(),
            _ => {
                let n = self.text.chars().count();
                if n == 1 {
                    "1 character".to_string()
                } else {
                    format!("{n} characters")
                }
            }
        }
    }

    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or("")
    }
}

/// Data captured by the monitor, before being persisted.
#[derive(Debug, Clone, Default)]
pub struct NewClip {
    pub kind: Option<ItemKind>,
    pub text: String,
    /// Source-provided title for URL or rich-link pasteboard items.
    pub title: Option<String>,
    pub rtf: Option<Vec<u8>>,
    /// HTML flavor captured from `public.html`, when available.
    pub html: Option<Vec<u8>>,
    pub image_png: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub source_app: String,
    pub source_bundle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinboard {
    pub id: i64,
    pub name: String,
    /// Hex color like `#FF9400`.
    pub color: String,
}

/// Palette offered when creating pinboards.
pub const BOARD_COLORS: [&str; 8] = [
    "#FF453A", "#FF9F0A", "#FFD60A", "#34C759", "#30B0C7", "#007AFF", "#AF52DE", "#8E8E93",
];

/// Parses a color string in various formats:
/// - `#RRGGBB`, `#RGB`, `#RRGGBBAA`
/// - `RRGGBB`, `RGB` without leading `#`
/// - `rgb(r, g, b)` or `rgba(r, g, b, a)`
/// - handles surrounding quotes and semicolons like `"#FF9400;"`
pub fn parse_color_str(s: &str) -> Option<([u8; 3], String)> {
    let raw = s
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == ';' || c == ',');
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Try rgb(r, g, b) or rgba(r, g, b, a)
    let lower = trimmed.to_ascii_lowercase();
    if (lower.starts_with("rgb(") || lower.starts_with("rgba(")) && lower.ends_with(')') {
        let inner = lower
            .trim_start_matches("rgba(")
            .trim_start_matches("rgb(")
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() >= 3 {
            let r: u8 = parts[0].parse().ok()?;
            let g: u8 = parts[1].parse().ok()?;
            let b: u8 = parts[2].parse().ok()?;
            let hex = format!("#{r:02X}{g:02X}{b:02X}");
            return Some(([r, g, b], hex));
        }
    }

    let h = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let expand = |c: char| c.to_digit(16).map(|d| (d * 17) as u8);
    match h.len() {
        3 => {
            let v: Vec<u8> = h.chars().filter_map(expand).collect();
            if v.len() == 3 {
                let hex = format!("#{0:02X}{1:02X}{2:02X}", v[0], v[1], v[2]);
                Some(([v[0], v[1], v[2]], hex))
            } else {
                None
            }
        }
        6 => {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            let r = p(0)?;
            let g = p(2)?;
            let b = p(4)?;
            let hex = format!("#{r:02X}{g:02X}{b:02X}");
            Some(([r, g, b], hex))
        }
        8 => {
            // #RRGGBBAA
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            let r = p(0)?;
            let g = p(2)?;
            let b = p(4)?;
            let hex = format!("#{r:02X}{g:02X}{b:02X}");
            Some(([r, g, b], hex))
        }
        _ => None,
    }
}

pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    parse_color_str(s).map(|(rgb, _)| rgb)
}

/// Heuristic to detect whether a snippet of text is source code, regex, or JSON.
pub fn is_code_snippet(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }

    // 1. Regex pattern e.g. `const url = new RegExp(...)` or `/^[a-z0-9_-]+$/`
    if t.contains("new RegExp(") || t.contains("RegExp(") {
        return true;
    }
    if t.starts_with('/')
        && (t.ends_with("/g")
            || t.ends_with("/i")
            || t.ends_with("/m")
            || t.ends_with("/gi")
            || t.ends_with('/'))
        && t.len() > 3
    {
        return true;
    }

    // 2. JSON check
    if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
        if serde_json::from_str::<serde_json::Value>(t).is_ok()
            && (t.contains(':') || t.contains('\n'))
        {
            return true;
        }
    }

    // 3. Programming language keywords & constructs
    const CODE_TRIGGERS: &[&str] = &[
        "const ",
        "let ",
        "var ",
        "function ",
        "function(",
        "def ",
        "class ",
        "import ",
        "export ",
        "return ",
        "async ",
        "await ",
        "fn ",
        "pub fn ",
        "pub struct ",
        "struct ",
        "enum ",
        "impl ",
        "interface ",
        "package ",
        "public class ",
        "private ",
        "SELECT ",
        "INSERT INTO ",
        "UPDATE ",
        "DELETE FROM ",
        "<!DOCTYPE ",
        "<html",
        "<script",
        "<style",
        "<div",
        "<span",
        "curl ",
        "chmod ",
        "#!/bin/",
        "console.log",
        "document.",
        "System.out.",
        "printf(",
        "println!",
        "std::",
        "<?php",
        "namespace ",
        "use crate",
    ];

    let has_keyword = CODE_TRIGGERS.iter().any(|k| t.contains(k));
    if has_keyword {
        return true;
    }

    // 4. Multi-line code patterns with indentations and programming symbols
    let lines: Vec<&str> = t.lines().collect();
    if lines.len() >= 2 {
        let syntax_symbols = [
            ";", "{", "}", "=>", "->", "::", "==", "!=", "++", "--", "()", "[]",
        ];
        let count = syntax_symbols.iter().filter(|s| t.contains(**s)).count();
        let indented = lines
            .iter()
            .filter(|l| l.starts_with("  ") || l.starts_with('\t'))
            .count();
        if count >= 2 && (indented > 0 || t.contains(';')) {
            return true;
        }
    }

    false
}

/// Try to extract a meaningful title from a code snippet (e.g. comment or function name).
pub fn extract_suggested_title(text: &str, kind: ItemKind) -> Option<String> {
    if kind != ItemKind::Code {
        return None;
    }
    for line in text.lines().take(5) {
        let l = line.trim();
        // Comment: `// Title` or `/* Title */` or `# Title`
        if let Some(c) = l.strip_prefix("//") {
            let t = c.trim().trim_matches('*').trim();
            if !t.is_empty() && t.len() <= 40 {
                return Some(t.to_string());
            }
        }
        if let Some(c) = l.strip_prefix("/*") {
            let t = c.trim_end_matches("*/").trim().trim_matches('*').trim();
            if !t.is_empty() && t.len() <= 40 {
                return Some(t.to_string());
            }
        }
        if let Some(c) = l.strip_prefix('#') {
            if !c.starts_with('!') {
                // not shebang
                let t = c.trim();
                if !t.is_empty() && t.len() <= 40 {
                    return Some(t.to_string());
                }
            }
        }
        // Variable / function declaration: `const check = ...` -> `check` or `Validate Email`
        for prefix in ["const ", "let ", "var ", "function ", "def ", "fn "] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let name = rest
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() && name.len() <= 30 {
                    return Some(format_identifier_to_title(name));
                }
            }
        }
    }
    None
}

fn format_identifier_to_title(s: &str) -> String {
    // Converts camelCase / snake_case into readable Title Case:
    // `validateEmail` -> `Validate Email`, `strip_html` -> `Strip Html`
    let mut out = String::new();
    let mut prev_is_lower = false;
    for c in s.chars() {
        if c == '_' {
            out.push(' ');
            prev_is_lower = false;
        } else if c.is_uppercase() && prev_is_lower {
            out.push(' ');
            out.push(c);
            prev_is_lower = false;
        } else {
            if out.is_empty() {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c);
            }
            prev_is_lower = c.is_lowercase();
        }
    }
    out
}

/// Decide whether plain text should be treated as a Color, Link, Code or Text.
pub fn classify_text(text: &str) -> ItemKind {
    let t = text.trim();
    if t.is_empty() {
        return ItemKind::Text;
    }

    // 1. Color check (single token or short string)
    if parse_color_str(t).is_some() {
        return ItemKind::Color;
    }

    // 2. Link check
    let lower = t.to_ascii_lowercase();
    if !t.contains(char::is_whitespace) {
        if (lower.starts_with("http://") || lower.starts_with("https://")) && t.len() > 8 {
            return ItemKind::Link;
        }
        if lower.starts_with("www.") && t.len() > 6 {
            return ItemKind::Link;
        }
    }

    // 3. Code check
    if is_code_snippet(t) {
        return ItemKind::Code;
    }

    ItemKind::Text
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

/// "just now", "3 minutes ago", "2 hours ago", "5 days ago"...
pub fn relative_time(now_ms: i64, then_ms: i64) -> String {
    let secs = ((now_ms - then_ms) / 1000).max(0);
    let unit = |n: i64, name: &str| {
        if n == 1 {
            format!("1 {name} ago")
        } else {
            format!("{n} {name}s ago")
        }
    };
    match secs {
        0..=9 => "just now".to_string(),
        10..=59 => unit(secs, "second"),
        60..=3599 => unit(secs / 60, "minute"),
        3600..=86399 => unit(secs / 3600, "hour"),
        86400..=2_591_999 => unit(secs / 86400, "day"),
        2_592_000..=31_535_999 => unit(secs / 2_592_000, "month"),
        _ => unit(secs / 31_536_000, "year"),
    }
}

/// FNV-1a 64-bit. Stable across Rust versions (unlike `DefaultHasher`), so it
/// is safe to persist for de-duplication.
pub fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in *part {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        // Separator so ("ab","c") != ("a","bc").
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl NewClip {
    /// Content hash used for de-duplication.
    pub fn content_hash(&self, kind: ItemKind) -> String {
        let h = match kind {
            ItemKind::Image => fnv1a(&[
                kind.as_str().as_bytes(),
                self.image_png.as_deref().unwrap_or_default(),
            ]),
            ItemKind::RichText => fnv1a(&[
                kind.as_str().as_bytes(),
                self.text.as_bytes(),
                self.rtf.as_deref().unwrap_or_default(),
                self.html.as_deref().unwrap_or_default(),
            ]),
            _ => fnv1a(&[kind.as_str().as_bytes(), self.text.as_bytes()]),
        };
        format!("{h:016x}")
    }

    /// Resolve the final kind (explicit, or classified from the text).
    pub fn resolved_kind(&self) -> ItemKind {
        match self.kind {
            Some(ItemKind::Text) | None => classify_text(&self.text),
            Some(k) => k,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify() {
        assert_eq!(classify_text("https://example.com/a?b=1"), ItemKind::Link);
        assert_eq!(classify_text("HTTP://X.ORG"), ItemKind::Link);
        assert_eq!(classify_text("see https://example.com"), ItemKind::Text);
        assert_eq!(classify_text("#FF9400"), ItemKind::Color);
        assert_eq!(classify_text("#F5AB82"), ItemKind::Color);
        assert_eq!(classify_text("D0EBFA"), ItemKind::Color);
        assert_eq!(classify_text("#fff"), ItemKind::Color);
        assert_eq!(classify_text("rgb(255, 148, 0)"), ItemKind::Color);
        assert_eq!(
            classify_text("const x = new RegExp(/abc/);"),
            ItemKind::Code
        );
        assert_eq!(
            classify_text("let text = input.toString();\ntext.replace(html, \"\")"),
            ItemKind::Code
        );
        assert_eq!(classify_text("hello"), ItemKind::Text);
        assert_eq!(classify_text("   "), ItemKind::Text);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex_color("#FF9400"), Some([255, 148, 0]));
        assert_eq!(parse_hex_color("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("FF9400"), Some([255, 148, 0]));
        assert_eq!(parse_hex_color("\"#F5AB82\""), Some([245, 171, 130]));
        assert_eq!(parse_hex_color("rgb(10, 20, 30)"), Some([10, 20, 30]));
        assert_eq!(parse_hex_color("#ggg"), None);
    }

    #[test]
    fn suggested_title() {
        let code = "// Extract URL\nconst url = new RegExp(/(http|https)/);";
        assert_eq!(
            extract_suggested_title(code, ItemKind::Code).as_deref(),
            Some("Extract URL")
        );

        let code2 = "const validateEmail = (e) => true;";
        assert_eq!(
            extract_suggested_title(code2, ItemKind::Code).as_deref(),
            Some("Validate Email")
        );
    }

    #[test]
    fn relative() {
        assert_eq!(relative_time(1_000_000, 1_000_000), "just now");
        assert_eq!(
            relative_time(1_000_000 + 180_000, 1_000_000),
            "3 minutes ago"
        );
        assert_eq!(
            relative_time(1_000_000 + 3_600_000, 1_000_000),
            "1 hour ago"
        );
        assert_eq!(relative_time(0, 5000), "just now");
    }

    #[test]
    fn hash_stable_and_separated() {
        assert_eq!(fnv1a(&[b"abc"]), fnv1a(&[b"abc"]));
        assert_ne!(fnv1a(&[b"ab", b"c"]), fnv1a(&[b"a", b"bc"]));
        let a = NewClip {
            text: "x".into(),
            ..Default::default()
        };
        assert_ne!(
            a.content_hash(ItemKind::Text),
            a.content_hash(ItemKind::Link)
        );
    }

    #[test]
    fn footer_text() {
        let mut it = ClipItem {
            id: 1,
            kind: ItemKind::Text,
            text: "héllo".into(),
            rtf: None,
            html: None,
            image_file: None,
            title: None,
            source_app: String::new(),
            source_bundle: String::new(),
            created_at: 0,
            pinboard_id: None,
            bytes: 6,
            width: 0,
            height: 0,
        };
        assert_eq!(it.footer(), "5 characters");
        it.kind = ItemKind::File;
        it.text = "/a\n/b".into();
        assert_eq!(it.footer(), "2 files");
        it.kind = ItemKind::Color;
        it.text = "#f5ab82".into();
        assert_eq!(it.footer(), "#F5AB82");
    }
}
