//! Standalone preferences window, styled pixel-perfect after Paste (pasteapp.io) on macOS.

use crate::monitor::SharedRef;
use crate::settings::{app_support_dir, Settings};
use egui::{
    pos2, vec2, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect,
    Response, RichText, Sense, Stroke, Ui, ViewportBuilder, ViewportClass, ViewportCommand,
    ViewportId,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    General,
    Privacy,
    Shortcuts,
}

#[derive(Debug)]
pub struct WindowState {
    open: bool,
    page: Page,
    draft: Settings,
    ignored_apps_text: String,
    hotkey_edit: String,
    new_app: String,
    error: Option<String>,
}

impl WindowState {
    pub fn new(settings: Settings) -> Self {
        Self {
            open: false,
            page: Page::General,
            ignored_apps_text: settings.ignored_apps.join("\n"),
            hotkey_edit: settings.hotkey.clone(),
            draft: settings,
            new_app: String::new(),
            error: None,
        }
    }

    pub fn open(&mut self, settings: Settings) {
        if !self.open {
            self.draft = settings;
            self.ignored_apps_text = self.draft.ignored_apps.join("\n");
            self.hotkey_edit = self.draft.hotkey.clone();
            self.error = None;
        }
        self.open = true;
    }

    pub fn hotkey_rejected(&mut self, error: String, accepted: String) {
        self.error = Some(error);
        self.draft.hotkey = accepted.clone();
        self.hotkey_edit = accepted;
    }
}

pub type StateRef = Arc<Mutex<WindowState>>;

pub fn show(ctx: &egui::Context, state: StateRef, shared: SharedRef) {
    let is_open = state.lock().map(|s| s.open).unwrap_or(false);
    if !is_open {
        return;
    }

    let viewport_id = ViewportId::from_hash_of("rustpaste-settings-window");
    let builder = ViewportBuilder::default()
        .with_title("Ajustes")
        .with_fullsize_content_view(true)
        .with_titlebar_shown(true)
        .with_titlebar_buttons_shown(true)
        .with_title_shown(false)
        .with_inner_size([580.0, 525.0])
        .with_min_inner_size([540.0, 490.0])
        .with_max_inner_size([660.0, 600.0])
        .with_resizable(true);
    let parent_ctx = ctx.clone();

    ctx.show_viewport_deferred(viewport_id, builder, move |ui, _class: ViewportClass| {
        let ctx = ui.ctx().clone();
        if ui.input(|i| i.viewport().close_requested()) {
            if let Ok(mut state) = state.lock() {
                state.open = false;
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        let Ok(mut state) = state.lock() else {
            return;
        };
        let before = state.draft.clone();
        let before_ignored_apps = state.ignored_apps_text.clone();
        crate::mac::configure_key_window_sharing(state.draft.show_in_screen_sharing);
        let dark = ctx.theme() == egui::Theme::Dark;
        let colors = Colors::new(dark);

        // Sidebar on left (175pt width) with macOS-native styling
        egui::Panel::left("settings_sidebar")
            .exact_size(175.0)
            .resizable(false)
            .show_separator_line(true)
            .frame(
                Frame::new()
                    .fill(colors.sidebar)
                    .inner_margin(Margin {
                        left: 10,
                        right: 10,
                        top: 14,
                        bottom: 14,
                    }),
            )
            .show(ui, |sidebar_ui| sidebar(&mut state, sidebar_ui, colors));

        // Content area on right
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(colors.content)
                    .inner_margin(Margin {
                        left: 20,
                        right: 20,
                        top: 12,
                        bottom: 16,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Title header with proper macOS titlebar offset
                        ui.add_space(26.0);
                        ui.label(
                            RichText::new(state.page.title())
                                .size(22.0)
                                .strong()
                                .color(colors.text),
                        );
                        ui.add_space(14.0);

                        match state.page {
                            Page::General => general(ui, &mut state, colors),
                            Page::Privacy => privacy(ui, &mut state, colors),
                            Page::Shortcuts => shortcuts(ui, &mut state, colors),
                        }

                        if let Some(error) = &state.error {
                            ui.add_space(12.0);
                            ui.colored_label(colors.error, error);
                        }
                        ui.add_space(16.0);
                    });
            });

        // Instant persistence on change
        state.draft.ignored_apps = state
            .ignored_apps_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();

        if before != state.draft || before_ignored_apps != state.ignored_apps_text {
            if let Err(error) = state.draft.save(&app_support_dir().join("settings.json")) {
                state.error = Some(format!("No se pudo guardar la configuración: {error}"));
            } else {
                if before.launch_at_login != state.draft.launch_at_login {
                    if let Err(error) = crate::autostart::set_enabled(state.draft.launch_at_login) {
                        state.error = Some(format!(
                            "No se pudo actualizar el inicio de sesión: {error}"
                        ));
                    }
                }
                if let Ok(mut current) = shared.settings.lock() {
                    *current = state.draft.clone();
                }
                crate::monitor::purge(&shared);
                shared
                    .revision
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                parent_ctx.request_repaint();
            }
        }
    });
}

#[derive(Clone, Copy)]
struct Colors {
    dark: bool,
    content: Color32,
    sidebar: Color32,
    card: Color32,
    card_border: Color32,
    text: Color32,
    secondary: Color32,
    accent: Color32,
    divider: Color32,
    error: Color32,
}

impl Colors {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                content: Color32::from_rgb(26, 26, 28),
                sidebar: Color32::from_rgb(34, 34, 37),
                card: Color32::from_rgb(42, 42, 45),
                card_border: Color32::from_rgba_unmultiplied(255, 255, 255, 18),
                text: Color32::from_rgb(245, 245, 247),
                secondary: Color32::from_rgb(155, 155, 162),
                accent: Color32::from_rgb(10, 132, 255),
                divider: Color32::from_rgba_unmultiplied(255, 255, 255, 18),
                error: Color32::from_rgb(255, 69, 58),
            }
        } else {
            Self {
                dark,
                content: Color32::WHITE,
                sidebar: Color32::from_rgb(236, 236, 239),
                card: Color32::from_rgb(246, 246, 248),
                card_border: Color32::from_rgba_unmultiplied(0, 0, 0, 15),
                text: Color32::from_rgb(28, 28, 30),
                secondary: Color32::from_rgb(138, 138, 144),
                accent: Color32::from_rgb(0, 122, 255),
                divider: Color32::from_rgba_unmultiplied(0, 0, 0, 20),
                error: Color32::from_rgb(255, 59, 48),
            }
        }
    }
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Privacy => "Privacidad",
            Self::Shortcuts => "Atajos de teclado",
        }
    }
}

// ----------------------------------------------------------------------------
// Sidebar
// ----------------------------------------------------------------------------

fn sidebar(state: &mut WindowState, ui: &mut Ui, colors: Colors) {
    ui.set_min_width(ui.available_width());
    ui.set_min_height(ui.available_height());

    ui.vertical(|ui| {
        // Space for macOS traffic-light buttons on top-left of sidebar
        ui.add_space(28.0);

        let access_needed = !crate::mac::accessibility_trusted();

        for page in [Page::General, Page::Privacy, Page::Shortcuts] {
            let selected = state.page == page;
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

            let p = ui.painter();
            if selected {
                p.rect_filled(rect, CornerRadius::same(7), colors.accent);
            } else if resp.hovered() {
                let hover_bg = if colors.dark {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 18)
                } else {
                    Color32::from_rgba_unmultiplied(0, 0, 0, 14)
                };
                p.rect_filled(rect, CornerRadius::same(7), hover_bg);
            }

            let icon_col = if selected {
                Color32::WHITE
            } else {
                colors.accent
            };
            let text_col = if selected {
                Color32::WHITE
            } else {
                colors.text
            };

            let icon_center = pos2(rect.left() + 18.0, rect.center().y);
            match page {
                Page::General => draw_gear_icon(&p, icon_center, 7.0, icon_col),
                Page::Privacy => draw_shield_icon(&p, icon_center, 7.0, icon_col),
                Page::Shortcuts => draw_keyboard_icon(&p, icon_center, 7.0, icon_col),
            }

            p.text(
                pos2(rect.left() + 35.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                page.title(),
                FontId::proportional(13.0),
                text_col,
            );

            // Red warning exclamation badge on General if accessibility is missing
            if page == Page::General && access_needed {
                let badge_c = pos2(rect.right() - 14.0, rect.center().y);
                draw_alert_badge(&p, badge_c, 6.0);
            }

            if resp.clicked() {
                state.page = page;
            }
            ui.add_space(2.0);
        }

        // Help Center at bottom of sidebar
        let rem = (ui.available_height() - 26.0).max(0.0);
        ui.add_space(rem);

        let (help_rect, help_resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
        let help_resp = help_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();
        let help_col = if help_resp.hovered() {
            colors.text
        } else {
            colors.secondary
        };

        draw_info_icon(&p, pos2(help_rect.left() + 10.0, help_rect.center().y), 6.0, help_col);
        p.text(
            pos2(help_rect.left() + 24.0, help_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Centro de ayuda",
            FontId::proportional(12.0),
            help_col,
        );

        if help_resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::same_tab("https://pasteapp.io/help"));
        }
    });
}

// ----------------------------------------------------------------------------
// General Page
// ----------------------------------------------------------------------------

fn general(ui: &mut Ui, state: &mut WindowState, colors: Colors) {
    // 1. System Toggles Card
    grouped_card(ui, colors, |ui| {
        card_row(ui, colors, "Abrir al iniciar sesión", None, |ui| {
            switch(ui, &mut state.draft.launch_at_login, colors);
        });
        card_divider(ui, colors);

        let mut run_in_background = !state.draft.paused;
        card_row(ui, colors, "Ejecutar en segundo plano", None, |ui| {
            switch(ui, &mut run_in_background, colors);
        });
        state.draft.paused = !run_in_background;
        card_divider(ui, colors);

        card_row(ui, colors, "Sincronización con iCloud", None, |ui| {
            let mut off = false;
            let _ = ui.add_enabled(false, |ui: &mut Ui| {
                switch(ui, &mut off, colors);
                ui.response()
            });
        });
        card_divider(ui, colors);

        card_row(ui, colors, "Efectos de sonido", None, |ui| {
            switch(ui, &mut state.draft.sound_effects, colors);
        });
    });

    // 2. Pegar elementos Card
    section_title(ui, "Pegar elementos", colors);
    grouped_card(ui, colors, |ui| {
        let mut direct = state.draft.paste_to_active_app;

        // Option 1: Direct Paste with illustration
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if radio(ui, direct, colors).clicked() {
                direct = true;
            }

            ui.vertical(|ui| {
                ui.label(
                    RichText::new("A la app activa")
                        .size(13.0)
                        .strong()
                        .color(colors.text),
                );
                ui.label(
                    RichText::new("Pega elementos seleccionados directamente en la aplicación que estés usando.")
                        .size(11.5)
                        .color(colors.secondary),
                );
                ui.add_space(4.0);

                if !crate::mac::accessibility_trusted() {
                    if accessibility_pill_button(ui, colors).clicked() {
                        crate::mac::request_accessibility();
                    }
                } else {
                    accessibility_granted_badge(ui, colors);
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                draw_paste_illustration(ui, colors);
            });
        });

        card_divider(ui, colors);

        // Option 2: Copy to Clipboard
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if radio(ui, !direct, colors).clicked() {
                direct = false;
            }

            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Al portapapeles")
                        .size(13.0)
                        .strong()
                        .color(colors.text),
                );
                ui.label(
                    RichText::new("Copia elementos seleccionados al portapapeles del sistema para pegarlos manualmente más tarde.")
                        .size(11.5)
                        .color(colors.secondary),
                );
            });
        });

        state.draft.paste_to_active_app = direct;
        card_divider(ui, colors);

        // Option 3: Always plain text checkbox
        checkbox(
            ui,
            &mut state.draft.always_plain_text,
            "Siempre pegar como texto plano",
            colors,
        );
    });

    // 3. Mantener Historial Card
    section_title(ui, "Mantener Historial", colors);
    grouped_card(ui, colors, |ui| {
        retention_slider(ui, &mut state.draft.retention_days, colors);
        card_divider(ui, colors);

        card_row(
            ui,
            colors,
            "Máximo de elementos",
            Some("Límite de clips (0 = ilimitado)"),
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut state.draft.max_items)
                        .range(0..=100_000)
                        .speed(50),
                );
            },
        );
        card_divider(ui, colors);

        card_row(
            ui,
            colors,
            "Altura del panel",
            Some("Tamaño vertical de la barra de historial"),
            |ui| {
                ui.add(
                    egui::Slider::new(&mut state.draft.panel_height, 260.0..=550.0).suffix(" pt"),
                );
            },
        );
    });
}

// ----------------------------------------------------------------------------
// Privacy Page
// ----------------------------------------------------------------------------

fn privacy(ui: &mut Ui, state: &mut WindowState, colors: Colors) {
    grouped_card(ui, colors, |ui| {
        card_row(
            ui,
            colors,
            "Mostrar al compartir pantalla",
            Some("Permitir que RustPaste sea visible durante capturas o llamadas."),
            |ui| {
                switch(ui, &mut state.draft.show_in_screen_sharing, colors);
            },
        );
        card_divider(ui, colors);

        card_row(
            ui,
            colors,
            "Vistas previas de enlaces",
            Some("Generar títulos y miniaturas visuales de enlaces web copiados."),
            |ui| {
                switch(ui, &mut state.draft.generate_link_previews, colors);
            },
        );
        card_divider(ui, colors);

        card_row(
            ui,
            colors,
            "Ignorar contenido confidencial",
            Some("Omitir clips marcados como contraseñas por gestores seguros."),
            |ui| {
                switch(ui, &mut state.draft.ignore_sensitive_clipboard, colors);
            },
        );
        card_divider(ui, colors);

        card_row(
            ui,
            colors,
            "Ignorar contenido transitorio",
            Some("Omitir datos temporales marcados para no guardarse en historial."),
            |ui| {
                switch(ui, &mut state.draft.ignore_transient_clipboard, colors);
            },
        );
    });

    section_title(ui, "Ignorar aplicaciones", colors);
    ui.label(
        RichText::new("No guardar contenido copiado desde estas aplicaciones.")
            .size(11.5)
            .color(colors.secondary),
    );
    ui.add_space(6.0);

    grouped_card(ui, colors, |ui| {
        let mut remove = None;
        let lines: Vec<_> = state.ignored_apps_text.lines().map(str::to_owned).collect();
        for (index, app) in lines.iter().enumerate() {
            ui.horizontal(|ui| {
                let p = ui.painter();
                let icon_c = pos2(ui.cursor().left() + 7.0, ui.cursor().top() + 10.0);
                p.rect_filled(
                    Rect::from_center_size(icon_c, vec2(14.0, 14.0)),
                    CornerRadius::same(3),
                    if colors.dark {
                        Color32::from_rgb(60, 60, 65)
                    } else {
                        Color32::from_rgb(220, 220, 225)
                    },
                );
                ui.add_space(20.0);

                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(app_display_name(app))
                            .size(12.5)
                            .strong()
                            .color(colors.text),
                    );
                    ui.label(RichText::new(app).size(10.5).color(colors.secondary));
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (btn_rect, btn_resp) =
                        ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    let btn_resp = btn_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let p = ui.painter();
                    let col = if btn_resp.hovered() {
                        colors.error
                    } else {
                        colors.secondary
                    };
                    p.circle_stroke(btn_rect.center(), 8.0, Stroke::new(1.0, col));
                    p.line_segment(
                        [
                            pos2(btn_rect.center().x - 4.0, btn_rect.center().y),
                            pos2(btn_rect.center().x + 4.0, btn_rect.center().y),
                        ],
                        Stroke::new(1.4, col),
                    );
                    if btn_resp.clicked() {
                        remove = Some(index);
                    }
                });
            });

            card_divider(ui, colors);
        }

        if let Some(index) = remove {
            let mut apps: Vec<_> = state.ignored_apps_text.lines().map(str::to_owned).collect();
            if index < apps.len() {
                apps.remove(index);
            }
            state.ignored_apps_text = apps.join("\n");
        }

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.new_app)
                    .hint_text("com.ejemplo.aplicacion")
                    .desired_width(ui.available_width() - 32.0),
            );
            if ui.button("+").clicked() && !state.new_app.trim().is_empty() {
                if !state.ignored_apps_text.is_empty() {
                    state.ignored_apps_text.push('\n');
                }
                state.ignored_apps_text.push_str(state.new_app.trim());
                state.new_app.clear();
            }
        });
    });
}

fn app_display_name(bundle: &str) -> &str {
    match bundle {
        "com.apple.keychainaccess" => "Acceso a Llaveros",
        "com.bitwarden.desktop" => "Bitwarden",
        "com.apple.Passwords" => "Contraseñas",
        "com.1password.1password"
        | "com.agilebits.onepassword7"
        | "com.agilebits.onepassword-osx" => "1Password",
        "org.keepassxc.keepassxc" => "KeePassXC",
        "com.lastpass.LastPass" => "LastPass",
        "com.dashlane.dashlanephonefinal" => "Dashlane",
        other => other,
    }
}

// ----------------------------------------------------------------------------
// Shortcuts Page
// ----------------------------------------------------------------------------

fn shortcuts(ui: &mut Ui, state: &mut WindowState, colors: Colors) {
    grouped_card(ui, colors, |ui| {
        card_row(
            ui,
            colors,
            "Activar RustPaste",
            Some("Atajo global para abrir o cerrar el panel"),
            |ui| {
                let (box_rect, _) = ui.allocate_exact_size(vec2(130.0, 26.0), Sense::hover());
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(box_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                let resp = child.add(
                    egui::TextEdit::singleline(&mut state.hotkey_edit)
                        .desired_width(130.0)
                        .horizontal_align(Align::Center),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    match state.hotkey_edit.parse::<global_hotkey::hotkey::HotKey>() {
                        Ok(_) => {
                            state.draft.hotkey = state.hotkey_edit.trim().to_owned();
                            state.error = None;
                        }
                        Err(e) => {
                            state.error = Some(format!("Atajo no válido: {e}"));
                        }
                    }
                }
            },
        );
        card_divider(ui, colors);

        shortcut_row(ui, colors, "Pegar rápido", "Pegar directamente uno de los clips recientes", &["⌘", "1…9"]);
        card_divider(ui, colors);

        shortcut_row(ui, colors, "Pegar como texto plano", "Pegar eliminando estilos y formato", &["⇧", "Return"]);
        card_divider(ui, colors);

        shortcut_row(ui, colors, "Vista previa rápida", "Ver clip completo con Quick Look", &["Espacio"]);
        card_divider(ui, colors);

        shortcut_row(ui, colors, "Editar elemento", "Editar el texto del clip seleccionado", &["⌘", "E"]);
        card_divider(ui, colors);

        shortcut_row(ui, colors, "Renombrar clip", "Asignar un título personalizado", &["⌘", "R"]);
    });

    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button("Restablecer atajos").clicked() {
            state.hotkey_edit = "shift+super+KeyV".to_owned();
            state.draft.hotkey = "shift+super+KeyV".to_owned();
            state.error = None;
        }
    });
}

fn shortcut_row(ui: &mut Ui, colors: Colors, title: &str, subtitle: &str, keys: &[&str]) {
    card_row(ui, colors, title, Some(subtitle), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for &k in keys {
                keycap(ui, k, colors);
            }
        });
    });
}

fn keycap(ui: &mut Ui, text: &str, colors: Colors) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        FontId::proportional(11.5),
        colors.text,
    );
    let size = vec2(galley.size().x + 12.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();

    let bg = if colors.dark {
        Color32::from_rgb(52, 52, 56)
    } else {
        Color32::WHITE
    };
    let border = if colors.dark {
        Color32::from_rgb(70, 70, 75)
    } else {
        Color32::from_rgb(212, 212, 218)
    };

    // Soft drop shadow
    p.rect_filled(
        Rect::from_min_size(pos2(rect.left(), rect.top() + 0.8), rect.size()),
        CornerRadius::same(5),
        Color32::from_rgba_unmultiplied(0, 0, 0, 18),
    );
    p.rect_filled(rect, CornerRadius::same(5), bg);
    p.rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(0.8, border),
        egui::StrokeKind::Inside,
    );

    p.galley(
        pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        colors.text,
    );
}

// ----------------------------------------------------------------------------
// Custom Controls & Containers
// ----------------------------------------------------------------------------

fn grouped_card(ui: &mut Ui, colors: Colors, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(colors.card)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(0.5, colors.card_border))
        .inner_margin(Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

fn section_title(ui: &mut Ui, title: &str, colors: Colors) {
    ui.add_space(16.0);
    ui.label(RichText::new(title).size(13.0).strong().color(colors.text));
    ui.add_space(6.0);
}

fn card_row(
    ui: &mut Ui,
    colors: Colors,
    title: &str,
    subtitle: Option<&str>,
    control: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(13.0).color(colors.text));
            if let Some(sub) = subtitle {
                ui.label(RichText::new(sub).size(11.0).color(colors.secondary));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            control(ui);
        });
    });
}

fn card_divider(ui: &mut Ui, colors: Colors) {
    ui.add_space(5.0);
    let rect = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [pos2(rect.left(), ui.cursor().top()), pos2(rect.right(), ui.cursor().top())],
        Stroke::new(0.5, colors.divider),
    );
    ui.add_space(5.0);
}

fn switch(ui: &mut Ui, value: &mut bool, colors: Colors) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        *value = !*value;
    }
    let p = ui.painter();
    let track = if *value {
        colors.accent
    } else if colors.dark {
        Color32::from_rgb(58, 58, 60)
    } else {
        Color32::from_rgb(229, 229, 234)
    };
    p.rect_filled(rect, CornerRadius::same(11), track);
    if !*value && !colors.dark {
        p.rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(0.5, Color32::from_rgba_unmultiplied(0, 0, 0, 30)),
            egui::StrokeKind::Inside,
        );
    }
    let thumb_x = if *value {
        rect.right() - 11.0
    } else {
        rect.left() + 11.0
    };
    let thumb_c = pos2(thumb_x, rect.center().y);
    // Subtle drop shadow
    p.circle_filled(
        pos2(thumb_c.x, thumb_c.y + 0.8),
        9.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 35),
    );
    // White thumb
    p.circle_filled(thumb_c, 9.0, Color32::WHITE);

    response.clicked()
}

fn radio(ui: &mut Ui, selected: bool, colors: Colors) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    let center = rect.center();
    if selected {
        p.circle_filled(center, 7.5, colors.accent);
        p.circle_filled(center, 2.8, Color32::WHITE);
    } else {
        let border_col = if colors.dark {
            Color32::from_rgb(90, 90, 95)
        } else {
            Color32::from_rgb(195, 195, 200)
        };
        p.circle_stroke(center, 7.0, Stroke::new(1.3, border_col));
    }
    resp
}

fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str, colors: Colors) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *checked = !*checked;
    }
    let p = ui.painter();
    let box_rect = Rect::from_min_size(pos2(rect.left(), rect.center().y - 7.5), vec2(15.0, 15.0));
    if *checked {
        p.rect_filled(box_rect, CornerRadius::same(4), colors.accent);
        let c = box_rect.center();
        let stroke = Stroke::new(1.6, Color32::WHITE);
        p.line_segment([pos2(c.x - 4.0, c.y), pos2(c.x - 1.0, c.y + 3.0)], stroke);
        p.line_segment(
            [pos2(c.x - 1.0, c.y + 3.0), pos2(c.x + 4.5, c.y - 3.5)],
            stroke,
        );
    } else {
        let fill = if colors.dark {
            Color32::from_rgb(50, 50, 55)
        } else {
            Color32::from_rgb(238, 238, 240)
        };
        let border = if colors.dark {
            Color32::from_rgb(80, 80, 85)
        } else {
            Color32::from_rgb(195, 195, 200)
        };
        p.rect_filled(box_rect, CornerRadius::same(4), fill);
        p.rect_stroke(
            box_rect,
            CornerRadius::same(4),
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        pos2(box_rect.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        colors.text,
    );
    resp
}

fn retention_slider(ui: &mut Ui, retention_days: &mut u32, colors: Colors) {
    let stops = [1_u32, 7, 30, 365, 0];
    let labels = ["Día", "Semana", "Mes", "Año", "Siempre"];
    let cur_idx = stops
        .iter()
        .position(|&d| d == *retention_days)
        .unwrap_or(2);

    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();

    let track_pad = 14.0;
    let track_y = rect.top() + 10.0;
    let x_left = rect.left() + track_pad;
    let x_right = rect.right() - track_pad;
    let width = x_right - x_left;

    let stop_x = |i: usize| -> f32 { x_left + (i as f32) * width / 4.0 };

    if resp.clicked() || resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let clamped_x = pos.x.clamp(x_left, x_right);
            let frac = (clamped_x - x_left) / width;
            let nearest = (frac * 4.0).round().clamp(0.0, 4.0) as usize;
            *retention_days = stops[nearest];
        }
    }

    let active_x = stop_x(cur_idx);

    // Active track (Blue)
    p.line_segment(
        [pos2(x_left, track_y), pos2(active_x, track_y)],
        Stroke::new(4.0, colors.accent),
    );
    // Inactive track (Gray)
    let inactive_track = if colors.dark {
        Color32::from_rgb(58, 58, 60)
    } else {
        Color32::from_rgb(229, 229, 234)
    };
    p.line_segment(
        [pos2(active_x, track_y), pos2(x_right, track_y)],
        Stroke::new(4.0, inactive_track),
    );

    // Stop dots
    for i in 0..5 {
        let sx = stop_x(i);
        let dot_col = if sx <= active_x {
            Color32::WHITE
        } else if colors.dark {
            Color32::from_rgb(100, 100, 105)
        } else {
            Color32::from_rgb(180, 180, 185)
        };
        p.circle_filled(pos2(sx, track_y), 1.5, dot_col);
    }

    // Thumb Knob
    let thumb_c = pos2(active_x, track_y);
    p.circle_filled(
        pos2(thumb_c.x, thumb_c.y + 0.8),
        9.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 35),
    );
    p.circle_filled(thumb_c, 9.0, Color32::WHITE);
    p.circle_stroke(
        thumb_c,
        9.0,
        Stroke::new(0.5, Color32::from_rgba_unmultiplied(0, 0, 0, 30)),
    );

    // Labels below
    let label_y = track_y + 16.0;
    for (i, label) in labels.iter().enumerate() {
        let sx = stop_x(i);
        let is_sel = i == cur_idx;
        let col = if is_sel {
            colors.text
        } else {
            colors.secondary
        };
        p.text(
            pos2(sx, label_y),
            egui::Align2::CENTER_CENTER,
            *label,
            FontId::proportional(11.5),
            col,
        );
    }
}

fn accessibility_pill_button(ui: &mut Ui, colors: Colors) -> Response {
    let text = "Permitir acceso de accesibilidad";
    let (rect, resp) = ui.allocate_exact_size(vec2(215.0, 24.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();

    let bg = if resp.hovered() {
        if colors.dark {
            Color32::from_rgb(60, 60, 64)
        } else {
            Color32::from_rgb(228, 228, 232)
        }
    } else if colors.dark {
        Color32::from_rgb(50, 50, 54)
    } else {
        Color32::from_rgb(238, 238, 242)
    };
    p.rect_filled(rect, CornerRadius::same(12), bg);

    // Red exclamation circle
    let badge_c = pos2(rect.left() + 13.0, rect.center().y);
    p.circle_filled(badge_c, 6.5, Color32::from_rgb(255, 59, 48));
    p.line_segment(
        [pos2(badge_c.x, badge_c.y - 3.0), pos2(badge_c.x, badge_c.y + 0.5)],
        Stroke::new(1.4, Color32::WHITE),
    );
    p.circle_filled(pos2(badge_c.x, badge_c.y + 2.8), 0.8, Color32::WHITE);

    // Text
    p.text(
        pos2(rect.left() + 25.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(11.5),
        colors.text,
    );

    // Chevron >
    p.text(
        pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        "›",
        FontId::proportional(14.0),
        colors.secondary,
    );

    resp
}

fn accessibility_granted_badge(ui: &mut Ui, colors: Colors) {
    let (rect, _) = ui.allocate_exact_size(vec2(140.0, 22.0), Sense::hover());
    let p = ui.painter();
    let bg = if colors.dark {
        Color32::from_rgb(34, 48, 38)
    } else {
        Color32::from_rgb(232, 245, 233)
    };
    p.rect_filled(rect, CornerRadius::same(11), bg);
    let green = Color32::from_rgb(52, 199, 89);
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 4.5, green);
    p.text(
        pos2(rect.left() + 21.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Acceso concedido",
        FontId::proportional(11.5),
        if colors.dark {
            Color32::from_rgb(180, 235, 195)
        } else {
            Color32::from_rgb(46, 125, 50)
        },
    );
}

// ----------------------------------------------------------------------------
// Vector Art / Illustrations / Icons
// ----------------------------------------------------------------------------

fn draw_gear_icon(p: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    p.circle_stroke(center, r * 0.65, Stroke::new(1.8, color));
    let teeth = 8;
    for i in 0..teeth {
        let angle = (i as f32) * std::f32::consts::PI / 4.0;
        let p1 = pos2(
            center.x + angle.cos() * (r * 0.62),
            center.y + angle.sin() * (r * 0.62),
        );
        let p2 = pos2(center.x + angle.cos() * r, center.y + angle.sin() * r);
        p.line_segment([p1, p2], Stroke::new(2.0, color));
    }
}

fn draw_shield_icon(p: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let top_y = center.y - r * 0.85;
    let bot_y = center.y + r * 0.85;
    let left_x = center.x - r * 0.75;
    let right_x = center.x + r * 0.75;
    let mid_y = center.y + r * 0.15;

    p.line_segment([pos2(left_x, top_y), pos2(right_x, top_y)], Stroke::new(1.5, color));
    p.line_segment([pos2(left_x, top_y), pos2(left_x, mid_y)], Stroke::new(1.5, color));
    p.line_segment([pos2(right_x, top_y), pos2(right_x, mid_y)], Stroke::new(1.5, color));
    p.line_segment([pos2(left_x, mid_y), pos2(center.x, bot_y)], Stroke::new(1.5, color));
    p.line_segment([pos2(right_x, mid_y), pos2(center.x, bot_y)], Stroke::new(1.5, color));

    // Check inside
    p.line_segment(
        [
            pos2(center.x - r * 0.35, center.y),
            pos2(center.x - r * 0.05, center.y + r * 0.3),
        ],
        Stroke::new(1.4, color),
    );
    p.line_segment(
        [
            pos2(center.x - r * 0.05, center.y + r * 0.3),
            pos2(center.x + r * 0.4, center.y - r * 0.25),
        ],
        Stroke::new(1.4, color),
    );
}

fn draw_keyboard_icon(p: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let w = r * 2.2;
    let h = r * 1.5;
    let rect = Rect::from_center_size(center, vec2(w, h));
    p.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.4, color),
        egui::StrokeKind::Inside,
    );
    // Keys
    for i in 0..4 {
        let kx = rect.left() + 3.5 + (i as f32) * (w - 7.0) / 3.0;
        p.circle_filled(pos2(kx, rect.top() + 4.2), 0.9, color);
    }
    // Spacebar
    let sp_y = rect.bottom() - 3.8;
    p.line_segment(
        [pos2(rect.left() + 4.5, sp_y), pos2(rect.right() - 4.5, sp_y)],
        Stroke::new(1.3, color),
    );
}

fn draw_info_icon(p: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    p.circle_stroke(center, r, Stroke::new(1.2, color));
    p.circle_filled(pos2(center.x, center.y - r * 0.45), 0.9, color);
    p.line_segment(
        [pos2(center.x, center.y - r * 0.1), pos2(center.x, center.y + r * 0.5)],
        Stroke::new(1.4, color),
    );
}

fn draw_alert_badge(p: &egui::Painter, center: Pos2, radius: f32) {
    p.circle_filled(center, radius, Color32::from_rgb(255, 59, 48));
    p.line_segment(
        [pos2(center.x, center.y - 2.8), pos2(center.x, center.y + 0.6)],
        Stroke::new(1.4, Color32::WHITE),
    );
    p.circle_filled(pos2(center.x, center.y + 2.6), 0.8, Color32::WHITE);
}

fn draw_paste_illustration(ui: &mut Ui, _colors: Colors) {
    let size = vec2(84.0, 52.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();

    // Vibrant Orange rounded background matching Paste illustration
    p.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(255, 149, 0));

    // Miniature document window
    let doc_w = 40.0;
    let doc_h = 28.0;
    let doc_rect = Rect::from_min_size(
        pos2(rect.right() - doc_w - 6.0, rect.top() + 6.0),
        vec2(doc_w, doc_h),
    );
    p.rect_filled(
        doc_rect,
        CornerRadius::same(4),
        Color32::from_rgb(255, 244, 230),
    );

    // Top window bar with 3 dots
    p.line_segment(
        [
            pos2(doc_rect.left(), doc_rect.top() + 6.0),
            pos2(doc_rect.right(), doc_rect.top() + 6.0),
        ],
        Stroke::new(0.8, Color32::from_rgb(240, 215, 185)),
    );
    for i in 0..3 {
        p.circle_filled(
            pos2(doc_rect.left() + 3.0 + (i as f32) * 3.0, doc_rect.top() + 3.0),
            0.8,
            Color32::from_rgb(220, 180, 140),
        );
    }

    // Simulated text lines in doc
    let line_col = Color32::from_rgba_unmultiplied(230, 180, 140, 180);
    p.line_segment(
        [
            pos2(doc_rect.left() + 4.0, doc_rect.top() + 11.0),
            pos2(doc_rect.right() - 8.0, doc_rect.top() + 11.0),
        ],
        Stroke::new(1.0, line_col),
    );
    p.line_segment(
        [
            pos2(doc_rect.left() + 4.0, doc_rect.top() + 16.0),
            pos2(doc_rect.right() - 4.0, doc_rect.top() + 16.0),
        ],
        Stroke::new(1.0, line_col),
    );
    p.line_segment(
        [
            pos2(doc_rect.left() + 4.0, doc_rect.top() + 21.0),
            pos2(doc_rect.right() - 12.0, doc_rect.top() + 21.0),
        ],
        Stroke::new(1.0, line_col),
    );

    // Miniature cards strip at bottom
    let card_w = 12.0;
    let card_h = 10.0;
    let strip_y = rect.bottom() - card_h - 5.0;
    for i in 0..4 {
        let cx = rect.left() + 6.0 + (i as f32) * 15.0;
        let c_rect = Rect::from_min_size(pos2(cx, strip_y), vec2(card_w, card_h));
        if i == 0 {
            // White highlighted card
            p.rect_filled(c_rect, CornerRadius::same(2), Color32::WHITE);
        } else {
            // Semi-transparent cards
            p.rect_filled(
                c_rect,
                CornerRadius::same(2),
                Color32::from_rgba_unmultiplied(255, 255, 255, 75),
            );
        }
    }

    // Smooth curved arrow pointing from card into document
    let p_start = pos2(rect.left() + 12.0, strip_y);
    let p_mid = pos2(rect.left() + 12.0, doc_rect.center().y + 4.0);
    let p_end = pos2(doc_rect.left() - 2.0, doc_rect.center().y + 4.0);
    let arrow_stroke = Stroke::new(1.6, Color32::from_rgb(70, 35, 0));
    p.line_segment([p_start, p_mid], arrow_stroke);
    p.line_segment([p_mid, p_end], arrow_stroke);
    // Arrowhead
    p.line_segment([pos2(p_end.x - 3.0, p_end.y - 2.5), p_end], arrow_stroke);
    p.line_segment([pos2(p_end.x - 3.0, p_end.y + 2.5), p_end], arrow_stroke);
}
