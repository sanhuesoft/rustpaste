//! The panel UI (egui) plus the glue between hotkey, tray, monitor and store.

mod settings_window;
mod widgets;

use crate::mac::{self, AppInfo};
use crate::model::*;
use crate::monitor::{self, SharedRef};
use crate::now_ms;
use crate::settings::{Settings, app_support_dir};
use crate::store::Filter;
use egui::{
    Align, Color32, CornerRadius, Event, FontId, Key, Layout, Modifiers, Rect, RichText, Sense,
    UiBuilder, pos2, vec2,
};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use widgets::{Caches, CardInput, Theme, color_from_hex, paint_card, pill};

const ANIM: Duration = Duration::from_millis(190);
const LIST_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    Toggle,
    Settings,
    TogglePause,
    Quit,
    Refresh,
    DragEnded {
        id: i64,
        local_pos: Option<(f32, f32)>,
        success: bool,
    },
}

enum Mode {
    Browse,
    Preview(i64),
    Rename(i64, String),
    /// `None` = new item.
    Edit(Option<i64>, String),
    NewBoard(String, usize),
}

#[derive(Debug)]
enum Action {
    Select(usize),
    Paste { id: i64, plain: bool },
    CopyOnly(i64),
    Pin(i64, Option<i64>),
    Delete(i64),
    Rename(i64),
    Edit(i64),
    Preview(i64),
}

pub struct App {
    shared: SharedRef,
    rx: Receiver<Cmd>,
    tx: Sender<Cmd>,
    _tray: TrayIcon,
    pause_item: CheckMenuItem,
    hotkeys: GlobalHotKeyManager,
    hotkey: Option<HotKey>,
    registered_hotkey_spec: String,
    settings_window: settings_window::StateRef,

    visible: bool,
    shown_at: Instant,
    had_focus: bool,
    need_configure: bool,
    prev_app: AppInfo,
    asked_accessibility: bool,

    items: Vec<ClipItem>,
    boards: Vec<Pinboard>,
    apps: Vec<String>,
    last_rev: u64,
    dirty: bool,

    query: String,
    kind_filter: Option<ItemKind>,
    app_filter: Option<String>,
    board_filter: Option<i64>,
    selected: usize,
    scroll_to_selected: bool,
    mode: Mode,

    caches: Caches,
    dragging: Option<i64>,
    pill_rects: Vec<(i64, Rect)>,
    notice: Option<(String, Instant)>,
    search_id: egui::Id,
    search_expanded: bool,
    request_search_focus: bool,
}

/// Entry point: build the window and run the event loop.
pub fn run() -> eframe::Result {
    let dir = app_support_dir();
    let settings_path = dir.join("settings.json");
    let settings = Settings::load(&settings_path);
    let store = crate::store::Store::open(&dir).expect("open store");
    let shared = std::sync::Arc::new(monitor::Shared {
        store: std::sync::Mutex::new(store),
        settings: std::sync::Mutex::new(settings.clone()),
        ignore_change: std::sync::atomic::AtomicIsize::new(-1),
        revision: std::sync::atomic::AtomicU64::new(0),
        quit: std::sync::atomic::AtomicBool::new(false),
    });
    monitor::purge(&shared);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustPaste")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_has_shadow(false)
            .with_visible(false)
            .with_inner_size([1200.0, settings.panel_height]),
        ..Default::default()
    };

    eframe::run_native(
        "RustPaste",
        options,
        Box::new(move |cc| {
            mac::set_accessory_policy();
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone(), shared)?))
        }),
    )
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 1. Primary macOS proportional system font: SF Pro / SFNS / HelveticaNeue
    let sf_pro_candidates = [
        "/Library/Fonts/SF-Pro.ttf",
        "/Library/Fonts/SF-Pro-Text-Regular.otf",
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Helvetica.ttc",
    ];
    for path in sf_pro_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "sf-pro".to_string(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "sf-pro".to_string());
            break;
        }
    }

    let sf_semibold_candidates = [
        "/Library/Fonts/SF-Pro-Text-Semibold.otf",
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    ];
    let semibold = sf_semibold_candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok());
    if let Some(bytes) = semibold {
        fonts.font_data.insert(
            "sf-pro-semibold-data".to_string(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
    } else if let Some(regular) = fonts.font_data.get("sf-pro").cloned() {
        fonts
            .font_data
            .insert("sf-pro-semibold-data".to_string(), regular);
    }
    fonts.families.insert(
        egui::FontFamily::Name("sf-pro-semibold".into()),
        vec!["sf-pro-semibold-data".to_string()],
    );

    // 2. Primary macOS monospace font: SF Mono / SFNSMono / Menlo
    let sf_mono_candidates = [
        "/Library/Fonts/SF-Mono-Regular.otf",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Monaco.ttf",
    ];
    for path in sf_mono_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "sf-mono".to_string(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "sf-mono".to_string());
            break;
        }
    }

    // 3. Fallbacks for macOS symbols (⌘, ⌥, ⇧, etc.) and Unicode
    for (name, path) in [
        ("apple-symbols", "/System/Library/Fonts/Apple Symbols.ttf"),
        (
            "arial-unicode",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.to_string(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push(name.to_string());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(name.to_string());
        }
    }

    ctx.set_fonts(fonts);
}

impl App {
    fn new(
        ctx: egui::Context,
        shared: SharedRef,
    ) -> Result<App, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = channel::<Cmd>();

        let drag_tx = tx.clone();
        mac::drag::set_drag_end_callback(move |id, local_pos, success| {
            let _ = drag_tx.send(Cmd::DragEnded {
                id,
                local_pos,
                success,
            });
        });

        // Menu bar item.
        let menu = Menu::new();
        let show_item = MenuItem::new("Show Clipboard", true, None);
        let pause_item = CheckMenuItem::new("Pause Recording", true, false, None);
        let settings_item = MenuItem::new("Settings…", true, None);
        let quit_item = MenuItem::new("Quit RustPaste", true, None);
        menu.append_items(&[
            &show_item,
            &pause_item,
            &settings_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ])?;
        let (rgba, w, h) = widgets::tray_icon_rgba();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(Icon::from_rgba(rgba, w, h)?)
            .with_tooltip("RustPaste")
            .build()?;
        {
            let (show, pause, set, quit) = (
                show_item.id().clone(),
                pause_item.id().clone(),
                settings_item.id().clone(),
                quit_item.id().clone(),
            );
            let (tx, ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let cmd = if e.id == show {
                    Cmd::Toggle
                } else if e.id == pause {
                    Cmd::TogglePause
                } else if e.id == set {
                    Cmd::Settings
                } else if e.id == quit {
                    Cmd::Quit
                } else {
                    return;
                };
                let _ = tx.send(cmd);
                ctx.request_repaint();
            }));
        }

        // Global hotkey.
        let hotkeys = GlobalHotKeyManager::new()?;
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                if e.state == HotKeyState::Pressed {
                    let _ = tx.send(Cmd::Toggle);
                    ctx.request_repaint();
                }
            }));
        }

        // Clipboard monitor thread.
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            monitor::spawn(shared.clone(), move || {
                let _ = tx.send(Cmd::Refresh);
                ctx.request_repaint();
            })?;
        }

        let initial_settings = shared
            .settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default();
        let mut app = App {
            shared,
            rx,
            tx,
            _tray: tray,
            pause_item,
            hotkeys,
            hotkey: None,
            registered_hotkey_spec: String::new(),
            settings_window: std::sync::Arc::new(std::sync::Mutex::new(
                settings_window::WindowState::new(initial_settings),
            )),
            visible: false,
            shown_at: Instant::now(),
            had_focus: false,
            need_configure: true,
            prev_app: AppInfo::default(),
            asked_accessibility: false,
            items: vec![],
            boards: vec![],
            apps: vec![],
            last_rev: 0,
            dirty: true,
            query: String::new(),
            kind_filter: None,
            app_filter: None,
            board_filter: None,
            selected: 0,
            scroll_to_selected: false,
            mode: Mode::Browse,
            caches: Caches::default(),
            dragging: None,
            pill_rects: vec![],
            notice: None,
            search_id: egui::Id::new("search"),
            search_expanded: false,
            request_search_focus: false,
        };
        let (hk, paused) = {
            let s = app.shared.settings.lock().unwrap();
            (s.hotkey.clone(), s.paused)
        };
        app.pause_item.set_checked(paused);
        if std::env::args().any(|a| a == "--show") {
            let _ = app.tx.send(Cmd::Toggle); // handy for debugging/screenshots
            ctx.request_repaint();
        }
        if std::env::args().any(|a| a == "--settings") {
            let _ = app.tx.send(Cmd::Settings);
            ctx.request_repaint();
        }
        if let Err(e) = app.register_hotkey(&hk) {
            eprintln!("rustpaste: {e}");
        }
        Ok(app)
    }

    fn register_hotkey(&mut self, spec: &str) -> Result<(), String> {
        if self.registered_hotkey_spec == spec {
            return Ok(());
        }
        let hk: HotKey = spec
            .parse()
            .map_err(|e| format!("Invalid hotkey '{spec}': {e}"))?;
        self.hotkeys
            .register(hk)
            .map_err(|e| format!("Could not register hotkey '{spec}': {e}"))?;
        if let Some(old) = self.hotkey.take() {
            let _ = self.hotkeys.unregister(old);
        }
        self.hotkey = Some(hk);
        self.registered_hotkey_spec = spec.to_owned();
        Ok(())
    }

    // ---------- data ----------

    fn filter(&self) -> Filter {
        Filter {
            query: self.query.clone(),
            kind: self.kind_filter,
            board: self.board_filter,
            app: self.app_filter.clone(),
            limit: LIST_LIMIT,
        }
    }

    fn refresh(&mut self) {
        let f = self.filter();
        let store = self.shared.store.lock().unwrap();
        self.items = store.list(&f).unwrap_or_default();
        self.boards = store.boards().unwrap_or_default();
        self.apps = store.apps().unwrap_or_default();
        drop(store);
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
        self.dirty = false;
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now()));
    }

    // ---------- visibility ----------

    fn show(&mut self, ctx: &egui::Context) {
        let front = mac::frontmost_app();
        if front.pid != mac::own_pid() {
            self.prev_app = front;
        }
        self.query.clear();
        self.search_expanded = false;
        self.request_search_focus = false;
        self.kind_filter = None;
        self.app_filter = None;
        self.board_filter = None;
        self.selected = 0;
        self.mode = Mode::Browse;
        self.refresh();
        if !mac::accessibility_trusted() {
            self.notify("Grant Accessibility in Settings to paste directly");
        }

        let ph = self.shared.settings.lock().unwrap().panel_height;
        if let Some((x, y, w, h)) = mac::active_screen_frame() {
            // Keep the floating panel inset from the screen edges, like Paste.
            // The inset also leaves room for the rounded window corners to read
            // against the desktop instead of being clipped by the display edge.
            let side_inset = 12.0;
            let bottom_inset = 12.0;
            let panel_width = (w - side_inset * 2.0).max(320.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(panel_width, ph)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                x + side_inset,
                y + h - ph - bottom_inset,
            )));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        mac::activate_self();
        self.visible = true;
        self.shown_at = Instant::now();
        self.had_focus = false;
        self.need_configure = true;
        ctx.request_repaint();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.dragging = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        mac::activate_pid(self.prev_app.pid);
    }

    // ---------- actions ----------

    fn paste(&mut self, ctx: &egui::Context, id: i64, plain: bool, do_paste: bool) {
        let preferences = self.shared.settings.lock().unwrap().clone();
        let plain = plain || preferences.always_plain_text;
        let do_paste = do_paste && preferences.paste_to_active_app;
        let (item, png) = {
            let store = self.shared.store.lock().unwrap();
            let Ok(Some(item)) = store.get(id) else {
                return;
            };
            let png = item
                .image_file
                .as_ref()
                .and_then(|f| std::fs::read(store.image_path(f)).ok());
            (item, png)
        };
        let cc = mac::write_item(&item, png.as_deref(), plain);
        if preferences.sound_effects {
            mac::play_paste_sound();
        }
        self.shared.ignore_change.store(cc, Ordering::SeqCst);
        let _ = self.shared.store.lock().unwrap().touch(id, now_ms());
        self.dirty = true;
        self.hide(ctx);
        if !do_paste {
            return;
        }
        if mac::accessibility_trusted() {
            std::thread::spawn(|| {
                if let Err(e) = mac::paste_injector::paste_after_focus() {
                    eprintln!("rustpaste: could not synthesize paste: {e:#}");
                }
            });
        } else if !self.asked_accessibility {
            // The item is already on the clipboard; ask for permission once.
            self.asked_accessibility = true;
            mac::request_accessibility();
        }
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Select(i) => self.selected = i,
                Action::Paste { id, plain } => self.paste(ctx, id, plain, true),
                Action::CopyOnly(id) => {
                    self.paste(ctx, id, false, false);
                }
                Action::Pin(id, board) => {
                    let _ = self.shared.store.lock().unwrap().set_pin(id, board);
                    self.dirty = true;
                }
                Action::Delete(id) => {
                    let _ = self.shared.store.lock().unwrap().delete(id);
                    self.caches.thumbs.remove(&id);
                    self.dirty = true;
                }
                Action::Rename(id) => {
                    let cur = self
                        .items
                        .iter()
                        .find(|i| i.id == id)
                        .and_then(|i| i.title.clone());
                    self.mode = Mode::Rename(id, cur.unwrap_or_default());
                }
                Action::Edit(id) => {
                    if let Some(it) = self.items.iter().find(|i| i.id == id) {
                        if !matches!(it.kind, ItemKind::Image | ItemKind::File) {
                            self.mode = Mode::Edit(Some(id), it.text.clone());
                        }
                    }
                }
                Action::Preview(id) => self.mode = Mode::Preview(id),
            }
        }
        if self.dirty {
            self.refresh();
        }
    }

    fn selected_id(&self) -> Option<i64> {
        self.items.get(self.selected).map(|i| i.id)
    }

    // ---------- keyboard ----------

    fn handle_keys(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut actions = vec![];
        let browsing = matches!(self.mode, Mode::Browse);
        let query_empty = self.query.is_empty();
        let n = self.items.len();
        let mut esc = false;
        let mut settings_requested = false;

        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                esc = true;
            }
            if !browsing {
                if matches!(self.mode, Mode::Preview(_))
                    && i.consume_key(Modifiers::NONE, Key::Space)
                {
                    esc = true;
                }
                return;
            }
            let mut step = |d: isize| {
                if n > 0 {
                    self.selected = (self.selected as isize + d).clamp(0, n as isize - 1) as usize;
                    self.scroll_to_selected = true;
                }
            };
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                step(1);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                step(-1);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowRight) {
                step(isize::MAX / 2);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowLeft) {
                step(isize::MIN / 2);
            }
            let sel = self.items.get(self.selected).map(|x| x.id);
            if let Some(id) = sel {
                if i.consume_key(Modifiers::SHIFT, Key::Enter) {
                    actions.push(Action::Paste { id, plain: true });
                } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                    actions.push(Action::Paste { id, plain: false });
                }
                if i.consume_key(Modifiers::COMMAND, Key::Backspace)
                    || i.consume_key(Modifiers::COMMAND, Key::Delete)
                    || (query_empty
                        && (i.consume_key(Modifiers::NONE, Key::Backspace)
                            || i.consume_key(Modifiers::NONE, Key::Delete)))
                {
                    actions.push(Action::Delete(id));
                }
                if i.consume_key(Modifiers::COMMAND, Key::R) {
                    actions.push(Action::Rename(id));
                }
                if i.consume_key(Modifiers::COMMAND, Key::E) {
                    actions.push(Action::Edit(id));
                }
                if query_empty && i.consume_key(Modifiers::NONE, Key::Space) {
                    // Also drop the Text(" ") event so it isn't typed into search.
                    i.events
                        .retain(|e| !matches!(e, Event::Text(t) if t == " "));
                    actions.push(Action::Preview(id));
                }
            }
            for (k, idx) in [
                (Key::Num1, 0),
                (Key::Num2, 1),
                (Key::Num3, 2),
                (Key::Num4, 3),
                (Key::Num5, 4),
                (Key::Num6, 5),
                (Key::Num7, 6),
                (Key::Num8, 7),
                (Key::Num9, 8),
            ] {
                if i.consume_key(Modifiers::COMMAND, k) {
                    if let Some(item) = self.items.get(idx) {
                        actions.push(Action::Paste {
                            id: item.id,
                            plain: false,
                        });
                    }
                }
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                self.cycle_board(1);
            }
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                self.cycle_board(-1);
            }
            if i.consume_key(Modifiers::COMMAND, Key::N) {
                self.mode = Mode::Edit(None, String::new());
            }
            if i.consume_key(Modifiers::COMMAND, Key::Comma) {
                settings_requested = true;
            }
            if i.consume_key(Modifiers::COMMAND, Key::F) {
                self.search_expanded = true;
                self.request_search_focus = true;
            }
            if !self.search_expanded && browsing {
                let mut typed = String::new();
                for ev in &i.events {
                    if let Event::Text(t) = ev {
                        let s = t.trim();
                        if !s.is_empty()
                            && !t.contains('\n')
                            && !t.contains('\r')
                            && !t.contains('\t')
                        {
                            typed.push_str(t);
                        }
                    }
                }
                if !typed.is_empty() {
                    self.search_expanded = true;
                    self.request_search_focus = true;
                    self.query.push_str(&typed);
                    self.dirty = true;
                    self.selected = 0;
                }
            }
        });

        if settings_requested {
            self.open_settings(ctx);
        }

        if esc {
            if !matches!(self.mode, Mode::Browse) {
                self.mode = Mode::Browse;
            } else if !self.query.is_empty() {
                self.query.clear();
                self.dirty = true;
            } else if self.search_expanded {
                self.search_expanded = false;
            } else {
                self.hide(ctx);
            }
        }
        actions
    }

    /// Tab order: History → each board → History.
    fn cycle_board(&mut self, dir: isize) {
        let mut ids: Vec<Option<i64>> = vec![None];
        ids.extend(self.boards.iter().map(|b| Some(b.id)));
        let cur = ids
            .iter()
            .position(|x| *x == self.board_filter)
            .unwrap_or(0) as isize;
        let next = (cur + dir).rem_euclid(ids.len() as isize) as usize;
        self.board_filter = ids[next];
        self.selected = 0;
        self.dirty = true;
    }

    fn open_settings(&mut self, ctx: &egui::Context) {
        if let Ok(settings) = self.shared.settings.lock().map(|s| s.clone()) {
            if let Ok(mut window) = self.settings_window.lock() {
                window.open(settings);
            }
        }
        self.visible = false;
        self.dragging = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        mac::activate_self();
        ctx.request_repaint();
    }

    // ---------- drawing ----------

    fn toolbar(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.pill_rects.clear();
        let width = ui.available_width();
        let boards = self.boards.clone();

        // Measure the row before painting it so the whole toolbar group is
        // horizontally centered, independent of how many pinboards exist.
        // The two filters have fixed widths below, which keeps the center point
        // stable while their selected labels change.
        let text_width = |text: &str, size: f32| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), FontId::proportional(size), th.text)
                .size()
                .x
        };
        let pill_width = |label: &str, has_dot: bool| {
            text_width(label, 13.0) + 24.0 + if has_dot { 16.0 } else { 0.0 }
        };
        let search_width = if self.search_expanded { 210.0 } else { 22.0 };
        let app_width = 120.0;
        let kind_width = 112.0;
        let mut content_width = search_width
            + pill_width("Clipboard History", true)
            + 22.0 // add pinboard
            + app_width
            + kind_width
            + pill_width("Settings", false);
        content_width += boards
            .iter()
            .map(|board| pill_width(&board.name, true))
            .sum::<f32>();
        let control_count = 6 + boards.len();
        content_width += 8.0 * control_count.saturating_sub(1) as f32;
        // `horizontal` inserts its normal item spacing before the leading
        // spacer too, so account for that when calculating the first widget's
        // actual x coordinate.
        let leading_space = (((width - content_width) * 0.5) - 8.0).max(0.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(leading_space);
            let (_, changed) = widgets::search_bar(
                ui,
                th,
                &mut self.query,
                &mut self.search_expanded,
                self.search_id,
                &mut self.request_search_focus,
            );
            if changed {
                self.dirty = true;
                self.selected = 0;
            }

            let hist_dot = Some(Color32::from_rgb(142, 142, 147));
            if pill(
                ui,
                th,
                "Clipboard History",
                hist_dot,
                self.board_filter.is_none(),
            )
            .clicked()
            {
                self.board_filter = None;
                self.selected = 0;
                self.dirty = true;
            }
            for b in &boards {
                let resp = pill(
                    ui,
                    th,
                    &b.name,
                    Some(color_from_hex(&b.color)),
                    self.board_filter == Some(b.id),
                );
                self.pill_rects.push((b.id, resp.rect));
                if resp.clicked() {
                    self.board_filter = Some(b.id);
                    self.selected = 0;
                    self.dirty = true;
                }
                resp.context_menu(|ui| {
                    if ui.button("Delete Pinboard").clicked() {
                        let _ = self.shared.store.lock().unwrap().delete_board(b.id);
                        if self.board_filter == Some(b.id) {
                            self.board_filter = None;
                        }
                        self.dirty = true;
                        ui.close();
                    }
                });
            }
            if widgets::plus_button(ui, th).clicked() {
                self.mode = Mode::NewBoard(String::new(), 0);
            }

            let before = (self.kind_filter, self.app_filter.clone());
            egui::ComboBox::from_id_salt("app-filter")
                .width(app_width)
                .selected_text(self.app_filter.clone().unwrap_or_else(|| "All apps".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.app_filter, None, "All apps");
                    for a in &self.apps {
                        ui.selectable_value(&mut self.app_filter, Some(a.clone()), a);
                    }
                });
            egui::ComboBox::from_id_salt("kind-filter")
                .width(kind_width)
                .selected_text(self.kind_filter.map(|k| k.label()).unwrap_or("All types"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind_filter, None, "All types");
                    for k in ItemKind::ALL {
                        ui.selectable_value(&mut self.kind_filter, Some(k), k.label());
                    }
                });
            if before != (self.kind_filter, self.app_filter.clone()) {
                self.dirty = true;
                self.selected = 0;
            }
            if pill(ui, th, "Settings", None, false).clicked() {
                self.open_settings(ui.ctx());
            }
            if let Some((msg, at)) = &self.notice {
                if at.elapsed() < Duration::from_secs(4) {
                    ui.label(RichText::new(msg).color(th.subtext).size(12.0));
                }
            }
        });
    }

    fn cards(&mut self, ui: &mut egui::Ui, th: &Theme, card_h: f32) -> Vec<Action> {
        let ctx = ui.ctx().clone();
        let mut actions = vec![];
        if self.items.is_empty() {
            let msg =
                if self.query.is_empty() && self.kind_filter.is_none() && self.app_filter.is_none()
                {
                    if self.board_filter.is_some() {
                        "This pinboard is empty. Drag a card onto it, or right-click → Pin To."
                    } else {
                        "Nothing here yet. Copy something and it will show up."
                    }
                } else {
                    "No results."
                };
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(msg).color(th.subtext).size(14.0));
            });
            return actions;
        }

        let items = std::mem::take(&mut self.items);
        let boards = self.boards.clone();
        let store = self.shared.clone();
        let show_link_title = self
            .shared
            .settings
            .lock()
            .map(|settings| settings.generate_link_previews)
            .unwrap_or(true);
        let now = now_ms();
        let card_w = card_h.round();
        let selected = self.selected;
        let scroll_sel = std::mem::take(&mut self.scroll_to_selected);
        let mut dragging = self.dragging;

        egui::ScrollArea::horizontal()
            .id_salt("cards")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    for (idx, item) in items.iter().enumerate() {
                        let effective_board = item.pinboard_id.or(self.board_filter);
                        let board_color = effective_board
                            .and_then(|bid| boards.iter().find(|b| b.id == bid))
                            .map(|b| color_from_hex(&b.color));
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(card_w, card_h), Sense::click_and_drag());
                        if ui.is_rect_visible(rect) {
                            let image_path = item
                                .image_file
                                .as_ref()
                                .map(|f| store.store.lock().unwrap().image_path(f));
                            let is_dragging = dragging == Some(item.id)
                                || mac::drag::currently_dragging_id() == Some(item.id);
                            paint_card(
                                ui,
                                &ctx,
                                th,
                                &mut self.caches,
                                rect,
                                &CardInput {
                                    item,
                                    selected: idx == selected,
                                    is_dragging,
                                    number: (idx < 9).then_some(idx + 1),
                                    board_color,
                                    now_ms: now,
                                    image_path,
                                    show_link_title,
                                },
                            );
                        }
                        if scroll_sel && idx == selected {
                            ui.scroll_to_rect(rect.expand2(vec2(24.0, 0.0)), None);
                        }
                        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
                            actions.push(Action::Select(idx));
                        }
                        if resp.double_clicked() {
                            actions.push(Action::Paste {
                                id: item.id,
                                plain: false,
                            });
                        }
                        if resp.drag_started() {
                            dragging = Some(item.id);
                            let image_bytes = item.image_file.as_ref().and_then(|f| {
                                let p = store.store.lock().unwrap().image_path(f);
                                std::fs::read(p).ok()
                            });
                            let image_path = item
                                .image_file
                                .as_ref()
                                .map(|f| store.store.lock().unwrap().image_path(f));
                            let bcol = board_color.map(|c| [c.r(), c.g(), c.b()]);
                            mac::drag::start_drag(
                                item,
                                image_bytes.as_deref(),
                                image_path.as_deref(),
                                bcol,
                                (rect.min.x, rect.min.y, rect.width(), rect.height()),
                            );
                        }
                        resp.context_menu(|ui| {
                            let id = item.id;
                            if ui.button("Paste").clicked() {
                                actions.push(Action::Paste { id, plain: false });
                                ui.close();
                            }
                            if ui.button("Paste as Plain Text").clicked() {
                                actions.push(Action::Paste { id, plain: true });
                                ui.close();
                            }
                            if ui.button("Copy").clicked() {
                                actions.push(Action::CopyOnly(id));
                                ui.close();
                            }
                            ui.separator();
                            ui.menu_button("Pin To", |ui| {
                                if boards.is_empty() {
                                    ui.label("No pinboards yet");
                                }
                                for b in &boards {
                                    if ui.button(&b.name).clicked() {
                                        actions.push(Action::Pin(id, Some(b.id)));
                                        ui.close();
                                    }
                                }
                            });
                            if item.pinboard_id.is_some() && ui.button("Unpin").clicked() {
                                actions.push(Action::Pin(id, None));
                                ui.close();
                            }
                            if ui.button("Rename").clicked() {
                                actions.push(Action::Rename(id));
                                ui.close();
                            }
                            if !matches!(item.kind, ItemKind::Image | ItemKind::File)
                                && ui.button("Edit").clicked()
                            {
                                actions.push(Action::Edit(id));
                                ui.close();
                            }
                            if ui.button("Quick Look").clicked() {
                                actions.push(Action::Preview(id));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Delete").clicked() {
                                actions.push(Action::Delete(id));
                                ui.close();
                            }
                        });
                    }
                });
            });
        self.items = items;
        self.dragging = dragging;
        actions
    }

    /// Drag-and-drop pinning: ghost under the cursor, drop on a pinboard pill.
    fn drag_overlay(&mut self, ctx: &egui::Context, th: &Theme) -> Vec<Action> {
        let mut actions = vec![];
        if mac::drag::currently_dragging_id().is_some() {
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost"));
                let p = ctx.layer_painter(layer);
                for (_, r) in &self.pill_rects {
                    if r.expand(4.0).contains(pos) {
                        p.rect_stroke(
                            r.expand(2.0),
                            CornerRadius::same(15),
                            egui::Stroke::new(2.0, th.accent),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
            return actions;
        }
        let Some(id) = self.dragging else {
            return actions;
        };
        let (pos, released) = ctx.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
        let Some(pos) = pos else {
            self.dragging = None;
            return actions;
        };
        if released {
            if let Some((bid, _)) = self
                .pill_rects
                .iter()
                .find(|(_, r)| r.expand(4.0).contains(pos))
            {
                actions.push(Action::Pin(id, Some(*bid)));
            }
            self.dragging = None;
            return actions;
        }
        // Highlight the hovered drop target.
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost"));
        let p = ctx.layer_painter(layer);
        for (_, r) in &self.pill_rects {
            if r.expand(4.0).contains(pos) {
                p.rect_stroke(
                    r.expand(2.0),
                    CornerRadius::same(15),
                    egui::Stroke::new(2.0, th.accent),
                    egui::StrokeKind::Outside,
                );
            }
        }
        let ghost = Rect::from_center_size(pos + vec2(0.0, 6.0), vec2(80.0, 52.0));
        p.rect_filled(
            ghost,
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(10, 132, 255, 190),
        );
        p.text(
            ghost.center(),
            egui::Align2::CENTER_CENTER,
            "Pin to…",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        actions
    }

    fn overlays(&mut self, ctx: &egui::Context, th: &Theme, area: Rect) {
        let mode = std::mem::replace(&mut self.mode, Mode::Browse);
        let frame = egui::Frame::NONE
            .fill(if th.dark {
                Color32::from_rgb(38, 38, 42)
            } else {
                Color32::WHITE
            })
            .stroke(egui::Stroke::new(1.0, th.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::same(14));
        let place = |title: &'static str| {
            egui::Area::new(egui::Id::new(title))
                .order(egui::Order::Foreground)
                .fixed_pos(area.center_top() + vec2(-210.0, 0.0))
        };

        self.mode = match mode {
            Mode::Browse => Mode::Browse,
            Mode::Preview(id) => {
                let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else {
                    return;
                };
                let mut close = false;
                egui::Area::new(egui::Id::new("preview"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(area.min)
                    .show(ctx, |ui| {
                        frame.show(ui, |ui| {
                            ui.set_min_size(area.size() - vec2(28.0, 28.0));
                            ui.set_max_size(area.size() - vec2(28.0, 28.0));
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}",
                                        item.kind.label(),
                                        item.footer()
                                    ))
                                    .color(th.subtext),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button("Close (Space)").clicked() {
                                        close = true;
                                    }
                                });
                            });
                            ui.separator();
                            if item.kind == ItemKind::Image {
                                let path = item
                                    .image_file
                                    .as_ref()
                                    .map(|f| self.shared.store.lock().unwrap().image_path(f));
                                if let Some(tex) = self.caches.thumb(ctx, -item.id, path, 1600) {
                                    let avail = ui.available_size();
                                    let ts = tex.size_vec2();
                                    let sc = (avail.x / ts.x).min(avail.y / ts.y).min(1.0);
                                    ui.centered_and_justified(|ui| {
                                        ui.image((tex.id(), ts * sc));
                                    });
                                }
                            } else if item.kind == ItemKind::Color {
                                let col = color_from_hex(&item.text);
                                let hex = item.text.trim().to_uppercase();
                                ui.centered_and_justified(|ui| {
                                    let (r, resp) =
                                        ui.allocate_exact_size(vec2(280.0, 180.0), Sense::hover());
                                    ui.painter().rect_filled(r, CornerRadius::same(12), col);
                                    ui.painter().text(
                                        r.center(),
                                        egui::Align2::CENTER_CENTER,
                                        hex,
                                        FontId::monospace(22.0),
                                        widgets::contrast_on(col),
                                    );
                                    resp
                                });
                            } else if item.kind == ItemKind::Code {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let galley = widgets::highlight_code(
                                            ctx,
                                            &item.text,
                                            ui.available_width() - 16.0,
                                        );
                                        ui.label(RichText::new(&item.text).size(0.0)); // sizing layout
                                        ui.painter().galley(
                                            ui.cursor().min,
                                            galley,
                                            Color32::WHITE,
                                        );
                                    });
                            } else {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(&item.text).size(14.0));
                                    });
                            }
                        });
                    });
                if close {
                    Mode::Browse
                } else {
                    Mode::Preview(id)
                }
            }
            Mode::Rename(id, mut text) => {
                let mut result = None;
                place("rename").show(ctx, |ui| {
                    frame.show(ui, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new("Rename").strong());
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .hint_text("Name")
                                .desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked()
                                || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                            {
                                result = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                result = Some(false);
                            }
                        });
                    });
                });
                match result {
                    Some(true) => {
                        let _ = self.shared.store.lock().unwrap().rename(id, &text);
                        self.dirty = true;
                        Mode::Browse
                    }
                    Some(false) => Mode::Browse,
                    None => Mode::Rename(id, text),
                }
            }
            Mode::Edit(id, mut text) => {
                let mut result = None;
                place("edit").show(ctx, |ui| {
                    frame.show(ui, |ui| {
                        ui.set_width(520.0);
                        ui.label(
                            RichText::new(if id.is_some() {
                                "Edit item"
                            } else {
                                "New item"
                            })
                            .strong(),
                        );
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .desired_rows(6)
                                .desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        ui.horizontal(|ui| {
                            let cmd_enter =
                                ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
                            if ui.button("Save (⌘↩)").clicked() || cmd_enter {
                                result = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                result = Some(false);
                            }
                        });
                    });
                });
                match result {
                    Some(true) => {
                        let mut store = self.shared.store.lock().unwrap();
                        match id {
                            Some(id) => {
                                let _ = store.edit_text(id, &text);
                            }
                            None => {
                                let clip = NewClip {
                                    text: text.clone(),
                                    source_app: "RustPaste".into(),
                                    ..Default::default()
                                };
                                let _ = store.insert(&clip, now_ms());
                            }
                        }
                        drop(store);
                        self.dirty = true;
                        Mode::Browse
                    }
                    Some(false) => Mode::Browse,
                    None => Mode::Edit(id, text),
                }
            }
            Mode::NewBoard(mut name, mut color) => {
                let mut result = None;
                place("newboard").show(ctx, |ui| {
                    frame.show(ui, |ui| {
                        ui.set_width(380.0);
                        ui.label(RichText::new("New pinboard").strong());
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .hint_text("Name")
                                .desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        ui.horizontal(|ui| {
                            for (i, hex) in BOARD_COLORS.iter().enumerate() {
                                let (rect, resp) =
                                    ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                                ui.painter()
                                    .circle_filled(rect.center(), 9.0, color_from_hex(hex));
                                if i == color {
                                    ui.painter().circle_stroke(
                                        rect.center(),
                                        11.0,
                                        egui::Stroke::new(2.0, th.text),
                                    );
                                }
                                if resp.clicked() {
                                    color = i;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if (ui.button("Create").clicked() || enter) && !name.trim().is_empty() {
                                result = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                result = Some(false);
                            }
                        });
                    });
                });
                match result {
                    Some(true) => {
                        let mut store = self.shared.store.lock().unwrap();
                        if let Ok(id) = store.create_board(&name, BOARD_COLORS[color]) {
                            self.board_filter = Some(id);
                        }
                        drop(store);
                        self.dirty = true;
                        Mode::Browse
                    }
                    Some(false) => Mode::Browse,
                    None => Mode::NewBoard(name, color),
                }
            }
        };
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Runs even while the window is hidden (on repaint requests): this is where
    /// hotkey, tray and monitor events are handled.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Cmd::Toggle => {
                    if self.visible {
                        self.hide(ctx);
                    } else {
                        self.show(ctx);
                    }
                }
                Cmd::Settings => {
                    self.open_settings(ctx);
                }
                Cmd::TogglePause => {
                    let paused = {
                        let mut s = self.shared.settings.lock().unwrap();
                        s.paused = !s.paused;
                        let _ = s.save(&app_support_dir().join("settings.json"));
                        s.paused
                    };
                    self.pause_item.set_checked(paused);
                }
                Cmd::Quit => {
                    self.shared.quit.store(true, Ordering::Relaxed);
                    std::process::exit(0);
                }
                Cmd::Refresh => {}
                Cmd::DragEnded {
                    id,
                    local_pos,
                    success,
                } => {
                    self.dragging = None;
                    let mut pinned = false;
                    if let Some((lx, ly)) = local_pos {
                        let pt = egui::pos2(lx, ly);
                        if let Some((bid, _)) = self
                            .pill_rects
                            .iter()
                            .find(|(_, r)| r.expand(6.0).contains(pt))
                        {
                            let _ = self.shared.store.lock().unwrap().set_pin(id, Some(*bid));
                            let bname = self
                                .boards
                                .iter()
                                .find(|b| b.id == *bid)
                                .map(|b| b.name.as_str())
                                .unwrap_or("Pinboard");
                            self.notify(format!("Pinned to {bname}"));
                            self.dirty = true;
                            pinned = true;
                        }
                    }
                    if !pinned && success {
                        let _ = self.shared.store.lock().unwrap().touch(id, now_ms());
                        self.hide(ctx);
                        self.dirty = true;
                    }
                    ctx.request_repaint();
                }
            }
        }
        let (configured_hotkey, paused) = self
            .shared
            .settings
            .lock()
            .map(|settings| (settings.hotkey.clone(), settings.paused))
            .unwrap_or_else(|_| (self.registered_hotkey_spec.clone(), false));
        if configured_hotkey != self.registered_hotkey_spec {
            if let Err(error) = self.register_hotkey(&configured_hotkey) {
                eprintln!("rustpaste: could not register updated shortcut: {error}");
                if let Ok(mut state) = self.settings_window.lock() {
                    state.hotkey_rejected(error, self.registered_hotkey_spec.clone());
                }
                if let Ok(mut settings) = self.shared.settings.lock() {
                    settings.hotkey = self.registered_hotkey_spec.clone();
                    let _ = settings.save(&app_support_dir().join("settings.json"));
                }
            }
        }
        self.pause_item.set_checked(paused);
        let rev = self.shared.revision.load(Ordering::SeqCst);
        if rev != self.last_rev {
            self.last_rev = rev;
            self.dirty = true;
        }
        if self.dirty && self.visible {
            self.refresh();
        }
        let _ = &self.tx;
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        settings_window::show(&ctx, self.settings_window.clone(), self.shared.clone());
        if !self.visible {
            return;
        }
        if self.need_configure {
            let show_in_screen_sharing = self
                .shared
                .settings
                .lock()
                .map(|settings| settings.show_in_screen_sharing)
                .unwrap_or(true);
            mac::configure_window(frame, show_in_screen_sharing);
            self.need_configure = false;
        }

        // Hide when the panel loses focus (clicking elsewhere), like Paste.
        let is_dragging = mac::drag::currently_dragging_id().is_some() || self.dragging.is_some();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused {
            self.had_focus = true;
        } else if !is_dragging
            && self.had_focus
            && self.shown_at.elapsed() > Duration::from_millis(400)
        {
            self.hide(&ctx);
            return;
        }

        let mut actions = self.handle_keys(&ctx);
        if !self.visible {
            return;
        }

        let dark = ctx.theme() == egui::Theme::Dark;
        let th = Theme::new(dark);
        let full = ui.max_rect();

        // Slide-up animation: the panel is drawn offset inside a transparent window.
        let t = (self.shown_at.elapsed().as_secs_f32() / ANIM.as_secs_f32()).min(1.0);
        let ease = 1.0 - (1.0 - t).powi(3);
        let off = (1.0 - ease) * full.height();
        if t < 1.0 {
            ctx.request_repaint();
        }
        let panel = full.translate(vec2(0.0, off));
        ui.painter()
            .rect_filled(panel, CornerRadius::same(16), th.bg);
        ui.painter().line_segment(
            [
                panel.left_top() + vec2(14.0, 0.5),
                panel.right_top() + vec2(-14.0, 0.5),
            ],
            egui::Stroke::new(1.0, th.border),
        );

        let inner = panel.shrink2(vec2(18.0, 14.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::LEFT)),
        );

        self.toolbar(&mut child, &th);
        child.add_space(12.0);
        let remaining = inner.bottom() - child.cursor().top();
        let card_h = (remaining - 14.0).max(120.0);
        let cards_rect = Rect::from_min_size(child.cursor().min, vec2(inner.width(), remaining));
        actions.extend(self.cards(&mut child, &th, card_h));
        actions.extend(self.drag_overlay(&ctx, &th));
        self.overlays(
            &ctx,
            &th,
            Rect::from_min_size(cards_rect.min - vec2(0.0, 6.0), cards_rect.size()),
        );
        self.apply(&ctx, actions);

        if self.dirty {
            ctx.request_repaint();
        }
        // Keep relative timestamps fresh.
        ctx.request_repaint_after(Duration::from_secs(15));
        let _ = self.selected_id();
    }
}
