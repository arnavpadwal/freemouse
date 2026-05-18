#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, ColorImage, IconData};
use egui_shadcn::{
    BadgeProps, BadgeVariant, CardProps, SeparatorProps,
    badge::badge,
    button::{Button, ButtonSize, ButtonVariant},
    card::card,
    input::Input,
    label::Label,
    separator::separator,
    theme::Theme,
    tokens::ColorPalette,
    typography::{
        HeadingAs, HeadingProps, TextProps, TypographyColor, heading, text,
    },
};
use freemouse::{capture, clipboard, network};
use rand::Rng;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

// ── Theme colors from design spec ────────────────────────────────────

const BG_PRIMARY: Color32 = Color32::from_rgb(7, 9, 13);
const BG_SECONDARY: Color32 = Color32::from_rgb(15, 19, 27);
const BG_CARD: Color32 = Color32::from_rgb(17, 21, 29);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(245, 247, 250);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(163, 172, 185);
const TEXT_MUTED: Color32 = Color32::from_rgb(107, 114, 128);
const ACCENT_PRIMARY: Color32 = Color32::from_rgb(124, 77, 255);
const ACCENT_GREEN: Color32 = Color32::from_rgb(34, 197, 94);
const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
const BORDER: Color32 = Color32::from_rgb(31, 36, 48);
const INPUT_BG: Color32 = Color32::from_rgb(13, 17, 23);

#[derive(PartialEq, Clone)]
enum AppMode {
    Onboarding,
    Home,
    Share(String),
    Receive,
}

struct CheckResult {
    name: &'static str,
    pass: bool,
    detail: String,
}

fn custom_theme() -> Theme {
    Theme::new(ColorPalette {
        background: BG_PRIMARY,
        foreground: TEXT_PRIMARY,
        card: BG_CARD,
        card_foreground: TEXT_PRIMARY,
        popover: BG_CARD,
        popover_foreground: TEXT_PRIMARY,
        border: BORDER,
        input: INPUT_BG,
        ring: ACCENT_PRIMARY,
        primary: ACCENT_PRIMARY,
        primary_foreground: Color32::WHITE,
        secondary: Color32::from_rgb(26, 31, 43),
        secondary_foreground: TEXT_PRIMARY,
        accent: ACCENT_PRIMARY,
        accent_foreground: Color32::WHITE,
        muted: Color32::from_rgb(31, 36, 48),
        muted_foreground: TEXT_MUTED,
        destructive: DANGER,
        destructive_foreground: Color32::WHITE,
        chart_1: ACCENT_PRIMARY,
        chart_2: ACCENT_GREEN,
        chart_3: Color32::from_rgb(255, 183, 77),
        chart_4: Color32::from_rgb(100, 181, 246),
        chart_5: Color32::from_rgb(206, 147, 216),
        sidebar: BG_SECONDARY,
        sidebar_foreground: TEXT_PRIMARY,
        sidebar_primary: ACCENT_PRIMARY,
        sidebar_primary_foreground: Color32::WHITE,
        sidebar_accent: ACCENT_PRIMARY,
        sidebar_accent_foreground: Color32::WHITE,
        sidebar_border: BORDER,
        sidebar_ring: ACCENT_PRIMARY,
    })
}

fn run_permission_checks() -> Vec<CheckResult> {
    let mut checks = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let input_dir = std::path::Path::new("/dev/input");
        if !input_dir.exists() {
            checks.push(CheckResult {
                name: "Input devices",
                pass: false,
                detail: "/dev/input does not exist on this system.".into(),
            });
        } else {
            let entries = std::fs::read_dir(input_dir);
            match entries {
                Ok(entries) => {
                    let mut found_any = false;
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.file_name().and_then(|n| n.to_str()).unwrap_or("").starts_with("event") {
                            found_any = true;
                            match std::fs::metadata(&path) {
                                Ok(_) => match std::fs::File::open(&path) {
                                    Ok(_) => {
                                        checks.push(CheckResult {
                                            name: "Input devices",
                                            pass: true,
                                            detail: "evdev devices accessible.".into(),
                                        });
                                    }
                                    Err(_) => {
                                        checks.push(CheckResult {
                                            name: "Input devices (Permissions)",
                                            pass: false,
                                            detail: format!(
                                                "Cannot read {:?}.\nRun: sudo usermod -aG input $USER\nThen log out and back in.",
                                                path
                                            ),
                                        });
                                    }
                                },
                                Err(_) => {}
                            }
                            break;
                        }
                    }
                    if !found_any {
                        checks.push(CheckResult {
                            name: "Input devices",
                            pass: false,
                            detail: "No event devices found in /dev/input.".into(),
                        });
                    }
                }
                Err(e) => {
                    checks.push(CheckResult {
                        name: "Input devices",
                        pass: false,
                        detail: format!("Cannot read /dev/input: {}", e),
                    });
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        checks.push(CheckResult {
            name: "Input devices",
            pass: true,
            detail: "Platform does not require evdev.".into(),
        });
    }

    {
        match std::net::TcpListener::bind("0.0.0.0:4444") {
            Ok(_) => {
                checks.push(CheckResult {
                    name: "Network port 4444",
                    pass: true,
                    detail: "Port 4444 is available.".into(),
                });
            }
            Err(e) => {
                checks.push(CheckResult {
                    name: "Network port 4444",
                    pass: false,
                    detail: format!("Cannot bind port 4444: {}", e),
                });
            }
        }
    }

    {
        let (w, h) = capture::get_screen_size();
        if w > 0.0 && h > 0.0 {
            checks.push(CheckResult {
                name: "Display detected",
                pass: true,
                detail: format!("Screen: {:.0}x{:.0}", w, h),
            });
        } else {
            checks.push(CheckResult {
                name: "Display detected",
                pass: false,
                detail: "Could not detect screen size.".into(),
            });
        }
    }

    checks
}

struct FreemouseApp {
    mode: AppMode,
    theme: Theme,
    logo_texture: Option<egui::TextureHandle>,
    ip_string: String,
    pin_string: String,
    connection_status: Arc<Mutex<String>>,
    server_task: Option<std::thread::JoinHandle<()>>,
    client_task: Option<std::thread::JoinHandle<()>>,
    discovered_servers: Vec<String>,
    discovered_raw: Vec<network::DiscoveredServer>,
    screen_width: f64,
    _discovery_rx: Option<mpsc::Receiver<network::DiscoveredServer>>,
}

impl Default for FreemouseApp {
    fn default() -> Self {
        let (sw, _) = capture::get_screen_size();
        let checks = run_permission_checks();
        let all_pass = checks.iter().all(|c| c.pass);
        Self {
            mode: if all_pass { AppMode::Home } else { AppMode::Onboarding },
            theme: custom_theme(),
            logo_texture: None,
            ip_string: String::new(),
            pin_string: String::new(),
            connection_status: Arc::new(Mutex::new("Ready".to_string())),
            server_task: None,
            client_task: None,
            discovered_servers: Vec::new(),
            discovered_raw: Vec::new(),
            screen_width: sw,
            _discovery_rx: None,
        }
    }
}

impl eframe::App for FreemouseApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG_PRIMARY))
            .show(ctx, |ui| {
                let theme = self.theme.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // ── header ────────────────────────────────────
                    ui.add_space(32.0);
                    ui.vertical(|ui| {
                        heading(ui, &theme, HeadingProps::new("Freemouse").size(30.0));
                        ui.add_space(4.0);
                        text(ui, &theme, TextProps::new("Mouse, Keyboard & Clipboard Sharing").size(15.0).color(TypographyColor::Muted));
                    });

                    ui.add_space(24.0);

                    // ── page content ──────────────────────────────
                    match self.mode.clone() {
                        AppMode::Onboarding => self.render_onboarding(ui, &theme),
                        AppMode::Home => self.render_home(ui, &theme),
                        AppMode::Share(pin) => self.render_share(ui, &theme, &pin),
                        AppMode::Receive => self.render_receive(ui, &theme),
                    }

                    // ── footer ────────────────────────────────────
                    ui.add_space(32.0);
                    separator(ui, &theme, SeparatorProps::default());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        footer_badge(ui, &theme, "Secure");
                        ui.add_space(16.0);
                        footer_badge(ui, &theme, "Local Network");
                        ui.add_space(16.0);
                        footer_badge(ui, &theme, "Encrypted");
                    });
                    ui.add_space(8.0);
                });
            });

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn footer_badge(ui: &mut egui::Ui, theme: &Theme, label: &str) {
    badge(ui, theme, BadgeProps::new(label).variant(BadgeVariant::Outline).color(ACCENT_GREEN));
}

impl FreemouseApp {
    fn render_onboarding(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.add_space(24.0);
        if let Some(tex) = &self.logo_texture {
            ui.add(egui::Image::new(tex).max_height(96.0));
            ui.add_space(12.0);
        }

        heading(ui, theme, HeadingProps::new("Welcome to Freemouse").as_tag(HeadingAs::H3));
        text(ui, theme, TextProps::new("Mouse, Keyboard & Clipboard Sharing").color(TypographyColor::Muted));
        ui.add_space(20.0);

        heading(ui, theme, HeadingProps::new("System Checks").as_tag(HeadingAs::H4));
        ui.add_space(8.0);

        let all_pass = true;

        for check in &run_permission_checks() {
            card(ui, theme, CardProps::default(), |ui| {
                ui.horizontal(|ui| {
                    let badge_props = if check.pass {
                        BadgeProps::new("✓").variant(BadgeVariant::Default)
                    } else {
                        BadgeProps::new("✗").variant(BadgeVariant::Destructive)
                    };
                    badge(ui, theme, badge_props);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        Label::new(check.name).show(ui, theme);
                        if !check.pass {
                            text(ui, theme, TextProps::new(&check.detail).color(TypographyColor::Muted));
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }

        ui.add_space(20.0);

        if all_pass {
            text(ui, theme, TextProps::new("All checks passed! You're ready to go."));
            ui.add_space(12.0);
            if Button::new("Let's go!").show(ui, theme).clicked() {
                self.mode = AppMode::Home;
            }
        } else {
            text(ui, theme, TextProps::new("Some checks failed. Please fix the issues above."));
            ui.add_space(8.0);
            if Button::new("Continue anyway")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
            {
                self.mode = AppMode::Home;
            }
        }
    }

    fn render_home(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.add_space(8.0);

        let w = ui.available_width();
        let card_w = (w - 16.0) / 2.0;
        let card_h = 200.0;

        ui.horizontal(|ui| {
            // ── Share card ───────────────────────────────────────
            ui.allocate_ui_with_layout(
                egui::vec2(card_w, card_h),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    card(ui, theme, CardProps::default(), |ui| {
                        ui.add_space(16.0);
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(48.0, 48.0),
                            egui::Sense::hover(),
                        );
                        let painter = ui.painter();
                        painter.rect_filled(
                            icon_rect,
                            12.0,
                            Color32::from_rgba_premultiplied(42, 31, 82, 255),
                        );
                        painter.text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "📤",
                            egui::FontId::proportional(22.0),
                            TEXT_PRIMARY,
                        );
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| {
                            heading(ui, theme, HeadingProps::new("Share").size(18.0));
                            ui.add_space(4.0);
                            text(ui, theme, TextProps::new("Share your mouse & keyboard with another computer").size(14.0).color(TypographyColor::Muted));
                            ui.add_space(12.0);
                            if Button::new("Start Sharing")
                                .size(ButtonSize::Lg)
                                .show(ui, theme)
                                .clicked()
                            {
                                capture::os::stop_capture();
                                let pin = format!("{:06}", rand::thread_rng().gen_range(0..999999));
                                self.mode = AppMode::Share(pin.clone());
                                *self.connection_status.lock().unwrap() =
                                    "Waiting for connection...".to_string();

                                let status_clone = self.connection_status.clone();
                                let ctx_clone = ui.ctx().clone();
                                let pin_clone = pin.clone();
                                let sw = self.screen_width;

                                std::thread::spawn(move || {
                                    let rt = tokio::runtime::Runtime::new().unwrap();
                                    rt.block_on(network::start_discovery_broadcast(4444));
                                });

                                self.server_task = Some(std::thread::spawn(move || {
                                    let rt = match tokio::runtime::Runtime::new() {
                                        Ok(rt) => rt,
                                        Err(e) => {
                                            *status_clone.lock().unwrap() =
                                                format!("Runtime Error: {}", e);
                                            ctx_clone.request_repaint();
                                            return;
                                        }
                                    };
                                    rt.block_on(async {
                                        match network::start_server(4444, &pin_clone).await {
                                            Ok(conn) => {
                                                eprintln!("[share] server accepted connection, starting capture");
                                                *status_clone.lock().unwrap() =
                                                    "Connected!".to_string();
                                                ctx_clone.request_repaint();

                                                let (tx, rx) =
                                                    mpsc::channel::<network::NetworkEvent>(100);

                                                capture::os::start_capture(tx.clone(), sw);
                                                clipboard::start_clipboard_monitor(tx);

                                                eprintln!("[share] entering run_share_loop");
                                                network::run_share_loop(conn, rx).await;
                                                eprintln!("[share] run_share_loop exited");

                                                *status_clone.lock().unwrap() =
                                                    "Disconnected".to_string();
                                                ctx_clone.request_repaint();
                                            }
                                            Err(e) => {
                                                *status_clone.lock().unwrap() =
                                                    format!("Error: {}", e);
                                                ctx_clone.request_repaint();
                                            }
                                        }
                                    });
                                }));
                            }
                        });
                    });
                },
            );

            ui.add_space(16.0);

            // ── Receive card ─────────────────────────────────────
            ui.allocate_ui_with_layout(
                egui::vec2(card_w, card_h),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    card(ui, theme, CardProps::default(), |ui| {
                        ui.add_space(16.0);
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(48.0, 48.0),
                            egui::Sense::hover(),
                        );
                        let painter = ui.painter();
                        painter.rect_filled(
                            icon_rect,
                            12.0,
                            Color32::from_rgba_premultiplied(21, 56, 40, 255),
                        );
                        painter.text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "📥",
                            egui::FontId::proportional(22.0),
                            TEXT_PRIMARY,
                        );
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| {
                            heading(ui, theme, HeadingProps::new("Receive").size(18.0));
                            ui.add_space(4.0);
                            text(ui, theme, TextProps::new("Take control of another computer's mouse & keyboard").size(14.0).color(TypographyColor::Muted));
                            ui.add_space(12.0);
                            if Button::new("Start Receiving")
                                .size(ButtonSize::Lg)
                                .show(ui, theme)
                                .clicked()
                            {
                                self.client_task = None;
                                capture::os::stop_capture();
                                self.mode = AppMode::Receive;
                                *self.connection_status.lock().unwrap() =
                                    "Scanning network...".to_string();
                                let rx = network::start_discovery_listener();
                                self._discovery_rx = Some(rx);
                                *self.connection_status.lock().unwrap() =
                                    "Enter details or pick a discovered server.".to_string();
                            }
                        });
                    });
                },
            );
        });
    }

    fn render_share(&mut self, ui: &mut egui::Ui, theme: &Theme, pin: &str) {
        let status = if let Ok(lock) = self.connection_status.try_lock() {
            lock.clone()
        } else {
            "...".to_string()
        };
        let local_ip = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "Unknown IP".to_string());

        // Back button
        ui.horizontal(|ui| {
            if Button::new("← Back")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
            {
                self.server_task = None;
                capture::os::stop_capture();
                self.mode = AppMode::Home;
                *self.connection_status.lock().unwrap() = "Ready".to_string();
            }
        });

        ui.add_space(16.0);

        // Status row: connection + remote mode
        ui.horizontal(|ui| {
            if status.contains("Connected") {
                badge(ui, theme, BadgeProps::new("Connected").variant(BadgeVariant::Default).color(ACCENT_GREEN));
                ui.add_space(8.0);
                let remote = capture::os::is_remote();
                if remote {
                    badge(ui, theme, BadgeProps::new("REMOTE").variant(BadgeVariant::Default).color(ACCENT_PRIMARY));
                } else {
                    badge(ui, theme, BadgeProps::new("Local").variant(BadgeVariant::Outline).color(TEXT_MUTED));
                }
            } else {
                badge(ui, theme, BadgeProps::new("Waiting for connection...").variant(BadgeVariant::Secondary));
            }
        });

        ui.add_space(16.0);

        // Connection details card
        card(ui, theme, CardProps::default(), |ui| {
            ui.horizontal(|ui| {
                let icon_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(36.0, 36.0),
                );
                let painter = ui.painter();
                painter.rect_filled(icon_rect, 10.0, Color32::from_rgba_premultiplied(34, 26, 70, 255));
                painter.text(icon_rect.center(), egui::Align2::CENTER_CENTER, "🖥", egui::FontId::proportional(16.0), TEXT_PRIMARY);
                ui.add_space(44.0);
                heading(ui, theme, HeadingProps::new("Connection Details").size(18.0));
            });
            ui.add_space(12.0);

            separator(ui, theme, SeparatorProps::default());
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                text(ui, theme, TextProps::new("IP Address").size(14.0).color(TypographyColor::Muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    Label::new(&local_ip).show(ui, theme);
                });
            });
            ui.add_space(4.0);
            separator(ui, theme, SeparatorProps::default());
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                text(ui, theme, TextProps::new("PIN Code").size(14.0).color(TypographyColor::Muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    Label::new(pin).show(ui, theme);
                });
            });
        });

        ui.add_space(16.0);

        if status.contains("Connected") {
            // Remote toggle card
            card(ui, theme, CardProps::default(), |ui| {
                ui.horizontal(|ui| {
                    let icon_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(36.0, 36.0),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(icon_rect, 10.0, Color32::from_rgba_premultiplied(34, 26, 70, 255));
                    painter.text(icon_rect.center(), egui::Align2::CENTER_CENTER, "🔄", egui::FontId::proportional(16.0), TEXT_PRIMARY);
                    ui.add_space(44.0);
                    ui.vertical(|ui| {
                        let remote = capture::os::is_remote();
                        heading(ui, theme, HeadingProps::new(if remote { "Controlling remote computer" } else { "Controlling local computer" }).size(18.0));
                        ui.add_space(4.0);
                        text(ui, theme, TextProps::new("Mouse to right screen edge or tap toggle to switch").size(14.0).color(TypographyColor::Muted));
                        ui.add_space(8.0);
                        if Button::new(if remote { "⬅ Switch to Local" } else { "➡ Switch to Remote" })
                            .variant(ButtonVariant::Secondary)
                            .show(ui, theme)
                            .clicked()
                        {
                            let new_remote = capture::os::toggle_remote();
                            tracing::info!("Manual toggle remote: {}", if new_remote { "ON" } else { "OFF" });
                        }
                    });
                });
            });
        } else {
            // Info card
            card(ui, theme, CardProps::default(), |ui| {
                ui.horizontal(|ui| {
                    let icon_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(36.0, 36.0),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(icon_rect, 10.0, Color32::from_rgba_premultiplied(34, 26, 70, 255));
                    painter.text(icon_rect.center(), egui::Align2::CENTER_CENTER, "👥", egui::FontId::proportional(16.0), TEXT_PRIMARY);
                    ui.add_space(44.0);
                    ui.vertical(|ui| {
                        heading(ui, theme, HeadingProps::new("Waiting for connection...").size(18.0));
                        text(ui, theme, TextProps::new("Another computer can now connect using the details above.").size(14.0).color(TypographyColor::Muted));
                    });
                });
            });
        }

        ui.add_space(24.0);

        // Stop sharing button
        if Button::new("■ Stop Sharing")
            .variant(ButtonVariant::Destructive)
            .show(ui, theme)
            .clicked()
        {
            self.server_task = None;
            capture::os::stop_capture();
            self.mode = AppMode::Home;
            *self.connection_status.lock().unwrap() = "Ready".to_string();
        }
    }

    fn render_receive(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let status = if let Ok(lock) = self.connection_status.try_lock() {
            lock.clone()
        } else {
            "...".to_string()
        };

        // Back button
        ui.horizontal(|ui| {
            if Button::new("← Back")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
            {
                self.client_task = None;
                capture::os::stop_capture();
                self.mode = AppMode::Home;
                self._discovery_rx = None;
                self.discovered_servers.clear();
                self.discovered_raw.clear();
                *self.connection_status.lock().unwrap() = "Ready".to_string();
            }
        });

        ui.add_space(16.0);

        heading(ui, theme, HeadingProps::new("Connect to a Computer").size(18.0));
        ui.add_space(16.0);

        // Poll discovered servers
        if let Some(rx) = &mut self._discovery_rx {
            while let Ok(server) = rx.try_recv() {
                let display = format!("{} ({})", server.hostname, server.ip);
                if !self.discovered_servers.contains(&display) {
                    self.discovered_servers.push(display.clone());
                    self.discovered_raw.push(server);
                }
            }
        }

        // Discovered servers card
        if !self.discovered_servers.is_empty() {
            card(ui, theme, CardProps::default(), |ui| {
                ui.horizontal(|ui| {
                    heading(ui, theme, HeadingProps::new("Discovered Servers").size(18.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Button::new("⟳ Refresh")
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .show(ui, theme)
                            .clicked()
                        {
                            self._discovery_rx = Some(network::start_discovery_listener());
                            self.discovered_servers.clear();
                            self.discovered_raw.clear();
                        }
                    });
                });
                ui.add_space(8.0);

                for (i, _) in self.discovered_servers.iter().enumerate() {
                    let selected = self.ip_string == self.discovered_raw[i].ip;
                    let sense = egui::Sense::click();
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 52.0),
                        sense,
                    );
                    let bg = if selected {
                        Color32::from_rgba_premultiplied(60, 120, 200, 40)
                    } else if response.hovered() {
                        Color32::from_black_alpha(15)
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 8.0, bg);

                    // Icon circle
                    let icon_c = egui::pos2(rect.left() + 26.0, rect.center().y);
                    ui.painter().circle_filled(icon_c, 18.0, Color32::from_rgba_premultiplied(34, 26, 70, 255));
                    ui.painter().text(icon_c, egui::Align2::CENTER_CENTER, "💻", egui::FontId::proportional(14.0), TEXT_PRIMARY);

                    // Hostname
                    ui.painter().text(
                        egui::pos2(rect.left() + 56.0, rect.top() + 8.0),
                        egui::Align2::LEFT_TOP,
                        &self.discovered_raw[i].hostname,
                        egui::FontId::proportional(14.0),
                        TEXT_PRIMARY,
                    );
                    // IP
                    ui.painter().text(
                        egui::pos2(rect.left() + 56.0, rect.top() + 28.0),
                        egui::Align2::LEFT_TOP,
                        &self.discovered_raw[i].ip,
                        egui::FontId::monospace(12.0),
                        TEXT_MUTED,
                    );
                    // Chevron
                    ui.painter().text(
                        egui::pos2(rect.right() - 16.0, rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        "›",
                        egui::FontId::proportional(20.0),
                        TEXT_SECONDARY,
                    );

                    if response.clicked() {
                        self.ip_string = self.discovered_raw[i].ip.clone();
                    }
                }
            });
            ui.add_space(16.0);
        }

        // "or" separator
        ui.horizontal(|ui| {
            separator(ui, theme, SeparatorProps::default());
            ui.add_space(8.0);
            text(ui, theme, TextProps::new("or").color(TypographyColor::Muted));
            ui.add_space(8.0);
            separator(ui, theme, SeparatorProps::default());
        });

        ui.add_space(16.0);

        // Manual connection card
        card(ui, theme, CardProps::default(), |ui| {
            ui.horizontal(|ui| {
                let icon_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(36.0, 36.0),
                );
                let painter = ui.painter();
                painter.rect_filled(icon_rect, 10.0, Color32::from_rgba_premultiplied(34, 26, 70, 255));
                painter.text(icon_rect.center(), egui::Align2::CENTER_CENTER, "🔗", egui::FontId::proportional(16.0), TEXT_PRIMARY);
                ui.add_space(44.0);
                heading(ui, theme, HeadingProps::new("Manual Connection").size(18.0));
            });
            ui.add_space(16.0);

            text(ui, theme, TextProps::new("IP Address").size(14.0).color(TypographyColor::Muted));
            ui.add_space(4.0);
            Input::new("ip_input")
                .placeholder("192.168.1.100")
                .width(ui.available_width())
                .show(ui, theme, &mut self.ip_string);

            ui.add_space(12.0);

            text(ui, theme, TextProps::new("PIN Code").size(14.0).color(TypographyColor::Muted));
            ui.add_space(4.0);
            Input::new("pin_input")
                .placeholder("000000")
                .width(ui.available_width())
                .show(ui, theme, &mut self.pin_string);

            ui.add_space(20.0);

            let can_connect = !self.ip_string.is_empty() && !self.pin_string.is_empty();
            if Button::new("🔗 Connect")
                .enabled(can_connect)
                .show(ui, theme)
                .clicked()
            {
                *self.connection_status.lock().unwrap() = "Connecting...".to_string();
                let ip = self.ip_string.clone();
                let pin = self.pin_string.clone();
                let status_clone = self.connection_status.clone();
                let ctx_clone = ui.ctx().clone();
                self._discovery_rx = None;

                self.client_task = Some(std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        match network::start_client(&ip, 4444, &pin).await {
                            Ok(conn) => {
                                eprintln!("[receive] connected to share server");
                                *status_clone.lock().unwrap() =
                                    "Connected Successfully!".to_string();
                                ctx_clone.request_repaint();

                                let (tx, rx) =
                                    mpsc::channel::<network::NetworkEvent>(100);

                                let rt2 = tokio::runtime::Runtime::new().unwrap();
                                rt2.spawn(capture::os::start_simulation(rx));
                                eprintln!("[receive] simulation task spawned");

                                network::run_receive_loop(conn, tx).await;
                                eprintln!("[receive] run_receive_loop exited");

                                *status_clone.lock().unwrap() =
                                    "Disconnected".to_string();
                                ctx_clone.request_repaint();
                            }
                            Err(e) => {
                                *status_clone.lock().unwrap() =
                                    format!("Connection Error: {}", e);
                                ctx_clone.request_repaint();
                            }
                        }
                    });
                }));
            }
        });

        ui.add_space(8.0);
        text(ui, theme, TextProps::new("Enter the IP address and PIN code shown on the sharing computer to connect.").size(12.0).color(TypographyColor::Muted));

        ui.add_space(12.0);
        let badge_props = if status.contains("Success") {
            BadgeProps::new(&status).variant(BadgeVariant::Default).color(ACCENT_GREEN)
        } else if status.contains("Error") {
            BadgeProps::new(&status).variant(BadgeVariant::Destructive)
        } else {
            BadgeProps::new(&status).variant(BadgeVariant::Secondary)
        };
        badge(ui, theme, badge_props);
    }
}

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
        }
    }

    // Suppress egui-shadcn theme init spam
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,egui_shadcn::theme=off".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Load icon from PNG
    let icon_bytes = include_bytes!("../FreeMouse.png");
    let icon_data = image::load_from_memory(icon_bytes).ok().map(|img| {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        IconData {
            rgba: rgba.into_raw(),
            width: w,
            height: h,
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 720.0])
            .with_min_inner_size([560.0, 680.0])
            .with_icon(icon_data.map(Arc::new).unwrap_or_default()),
        ..Default::default()
    };

    eframe::run_native(
        "Freemouse",
        options,
        Box::new(|cc| {
            let mut app = FreemouseApp::default();
            let logo_bytes = include_bytes!("../FreeMouse.png");
            if let Ok(img) = image::load_from_memory(logo_bytes) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color_image = ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                app.logo_texture = Some(cc.egui_ctx.load_texture(
                    "freemouse_logo",
                    color_image,
                    egui::TextureOptions::default(),
                ));
            }
            Ok(Box::new(app))
        }),
    )
}
