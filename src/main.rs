#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod network;

use eframe::egui::{self, ColorImage, TextureHandle};
use rand::Rng;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

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

struct CheckResults {
    checks: Vec<CheckResult>,
}

fn run_permission_checks() -> Vec<CheckResult> {
    let mut checks = Vec::new();

    // Check 1: evdev access on Linux
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
                                Ok(_) => {
                                    // Try opening for read
                                    match std::fs::File::open(&path) {
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
                                    }
                                }
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

    // Check 2: Port availability
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

    // Check 3: Screen size detected
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
    logo_texture: Option<TextureHandle>,
    ip_string: String,
    pin_string: String,
    connection_status: Arc<Mutex<String>>,
    server_task: Option<std::thread::JoinHandle<()>>,
    client_task: Option<std::thread::JoinHandle<()>>,
    discovered_servers: Vec<String>,
    discovered_raw: Vec<network::DiscoveredServer>,
    screen_width: f64,
    screen_height: f64,
    _discovery_rx: Option<mpsc::Receiver<network::DiscoveredServer>>,
    permission_checks: CheckResults,
}

impl Default for FreemouseApp {
    fn default() -> Self {
        let (sw, sh) = capture::get_screen_size();
        let checks = run_permission_checks();
        let all_pass = checks.iter().all(|c| c.pass);
        Self {
            mode: if all_pass { AppMode::Home } else { AppMode::Onboarding },
            logo_texture: None,
            ip_string: String::new(),
            pin_string: String::new(),
            connection_status: Arc::new(Mutex::new("Ready".to_string())),
            server_task: None,
            client_task: None,
            discovered_servers: Vec::new(),
            discovered_raw: Vec::new(),
            screen_width: sw,
            screen_height: sh,
            _discovery_rx: None,
            permission_checks: CheckResults { checks },
        }
    }
}

impl eframe::App for FreemouseApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let status = if let Ok(lock) = self.connection_status.try_lock() {
            lock.clone()
        } else {
            "...".to_string()
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered_justified(|ui| {
                ui.add_space(20.0);
                ui.heading(egui::RichText::new("Freemouse").size(32.0).strong());
                ui.label(
                    egui::RichText::new("Mouse, Keyboard & Clipboard Sharing")
                        .size(14.0)
                        .weak(),
                );
                ui.add_space(20.0);

                ui.label(
                    egui::RichText::new(format!(
                        "Display: {:.0}x{:.0}",
                        self.screen_width, self.screen_height
                    ))
                    .size(10.0)
                    .weak(),
                );
                ui.add_space(10.0);

                match self.mode.clone() {
                    AppMode::Onboarding => { self.render_onboarding(ctx, ui); }
                    AppMode::Home => {
                        let card_frame = egui::Frame {
                            fill: ctx.style().visuals.window_fill(),
                            rounding: egui::Rounding::same(12.0),
                            shadow: egui::epaint::Shadow {
                offset: [0.0, 2.0].into(),
                blur: 8.0,
                spread: 0.0,
                                color: egui::Color32::from_black_alpha(80),
                            },
                            ..Default::default()
                        };

                        ui.add_space(15.0);
                        card_frame.show(ui, |ui| {
                            ui.set_min_width(280.0);
                            ui.set_max_width(320.0);
                            ui.add_space(16.0);
                            ui.vertical_centered(|ui| {
                                ui.label(egui::RichText::new("📤").size(28.0));
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Share").size(18.0).strong());
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Share your mouse & keyboard\nwith another computer")
                                        .size(12.0)
                                        .color(egui::Color32::GRAY),
                                );
                                ui.add_space(12.0);
                                if ui
                                    .add_sized(
                                        egui::vec2(200.0, 44.0),
                                        egui::Button::new(
                                            egui::RichText::new("Start Sharing").size(16.0),
                                        ),
                                    )
                                    .clicked()
                                {
                                    capture::os::stop_capture();
                                    let pin = format!("{:06}", rand::thread_rng().gen_range(0..999999));
                                    self.mode = AppMode::Share(pin.clone());
                                    *self.connection_status.lock().unwrap() =
                                        "Waiting for connection...".to_string();

                                    let status_clone = self.connection_status.clone();
                                    let ctx_clone = ctx.clone();
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
                                                    *status_clone.lock().unwrap() =
                                                        "Connected!".to_string();
                                                    ctx_clone.request_repaint();

                                                    let (tx, rx) =
                                                        mpsc::channel::<network::NetworkEvent>(100);

                                                    capture::os::start_capture(tx.clone(), sw);
                                                    clipboard::start_clipboard_monitor(tx);

                                                    network::run_share_loop(conn, rx).await;

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
                            ui.add_space(16.0);
                        });

                        ui.add_space(16.0);

                        card_frame.show(ui, |ui| {
                            ui.set_min_width(280.0);
                            ui.set_max_width(320.0);
                            ui.add_space(16.0);
                            ui.vertical_centered(|ui| {
                                ui.label(egui::RichText::new("📥").size(28.0));
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Receive").size(18.0).strong());
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Take control of another\ncomputer's mouse & keyboard")
                                        .size(12.0)
                                        .color(egui::Color32::GRAY),
                                );
                                ui.add_space(12.0);
                                if ui
                                    .add_sized(
                                        egui::vec2(200.0, 44.0),
                                        egui::Button::new(
                                            egui::RichText::new("Start Receiving").size(16.0),
                                        ),
                                    )
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
                            ui.add_space(16.0);
                        });
                    }
                    AppMode::Share(pin) => {
                        let local_ip = local_ip_address::local_ip()
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|_| "Unknown IP".to_string());

                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("📤 Share Mode").size(24.0).strong());
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Share your mouse & keyboard with a remote computer")
                                .size(12.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(20.0);

                        let info_frame = egui::Frame {
                            fill: ctx.style().visuals.window_fill(),
                            rounding: egui::Rounding::same(12.0),
                            ..Default::default()
                        };

                        info_frame.show(ui, |ui| {
                            ui.add_space(16.0);
                            ui.vertical_centered(|ui| {
                                ui.label(egui::RichText::new("Connection Details").size(16.0).strong());
                                ui.add_space(12.0);

                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("IP Address").size(12.0).color(egui::Color32::GRAY));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.add_space(20.0);
                                        ui.label(
                                            egui::RichText::new(&local_ip)
                                                .color(egui::Color32::from_rgb(100, 200, 255))
                                                .size(18.0)
                                                .monospace(),
                                        );
                                    });
                                });
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);

                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("PIN Code").size(12.0).color(egui::Color32::GRAY));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.add_space(20.0);
                                        ui.label(
                                            egui::RichText::new(&pin)
                                                .color(egui::Color32::from_rgb(100, 220, 100))
                                                .size(24.0)
                                                .strong()
                                                .monospace(),
                                        );
                                    });
                                });
                            });
                            ui.add_space(16.0);
                        });

                        ui.add_space(16.0);

                        let status_color = if status.contains("Connected") {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else if status.contains("Error") {
                            egui::Color32::from_rgb(240, 100, 100)
                        } else {
                            egui::Color32::GRAY
                        };

                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("●").size(10.0).color(status_color));
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(&status).size(13.0).color(status_color));
                        });

                        ui.add_space(20.0);

                        if ui
                            .add_sized(
                                egui::vec2(120.0, 36.0),
                                egui::Button::new("← Back"),
                            )
                            .clicked()
                        {
                            self.server_task = None;
                            capture::os::stop_capture();
                            self.mode = AppMode::Home;
                            *self.connection_status.lock().unwrap() = "Ready".to_string();
                        }
                    }
                    AppMode::Receive => {
                        let info_frame = egui::Frame {
                            fill: ctx.style().visuals.window_fill(),
                            rounding: egui::Rounding::same(12.0),
                            ..Default::default()
                        };

                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("📥 Receive Mode").size(24.0).strong());
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Connect to a remote computer to control it")
                                .size(12.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(20.0);

                        // Poll for discovered servers
                        if let Some(rx) = &mut self._discovery_rx {
                            while let Ok(server) = rx.try_recv() {
                                let display = format!("{} ({})", server.hostname, server.ip);
                                if !self.discovered_servers.contains(&display) {
                                    self.discovered_servers.push(display.clone());
                                    self.discovered_raw.push(server);
                                }
                            }
                        }

                        if !self.discovered_servers.is_empty() {
                            info_frame.show(ui, |ui| {
                                ui.add_space(12.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(egui::RichText::new("Discovered Servers").size(14.0).strong());
                                });
                                ui.add_space(8.0);
                                for (i, _server_str) in self.discovered_servers.iter().enumerate() {
                                    let selected = self.ip_string == self.discovered_raw[i].ip;
                                    let bg = if selected {
                                        egui::Color32::from_rgba_premultiplied(60, 120, 200, 40)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    let response = egui::Frame::none()
                                        .fill(bg)
                                        .rounding(egui::Rounding::same(6.0))
                                        .show(ui, |ui| {
                                            ui.set_min_width(280.0);
                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                ui.add_space(12.0);
                                                ui.label(
                                                    egui::RichText::new("💻").size(18.0),
                                                );
                                                ui.add_space(8.0);
                                                ui.vertical(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(&self.discovered_raw[i].hostname)
                                                            .size(14.0)
                                                            .strong(),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(&self.discovered_raw[i].ip)
                                                            .size(11.0)
                                                            .color(egui::Color32::GRAY)
                                                            .monospace(),
                                                    );
                                                });
                                            });
                                            ui.add_space(8.0);
                                        });
                                    if response.response.clicked() {
                                        self.ip_string = self.discovered_raw[i].ip.clone();
                                    }
                                }
                                ui.add_space(12.0);
                            });
                            ui.add_space(16.0);
                        }

                        info_frame.show(ui, |ui| {
                            ui.add_space(16.0);
                            ui.vertical_centered(|ui| {
                                ui.label(egui::RichText::new("Manual Connection").size(14.0).strong());
                            });
                            ui.add_space(12.0);

                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(egui::RichText::new("IP Address").size(12.0).color(egui::Color32::GRAY));
                                ui.add_space(8.0);
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.add_sized(
                                    egui::vec2(ui.available_width() - 32.0, 28.0),
                                    egui::TextEdit::singleline(&mut self.ip_string)
                                        .hint_text("192.168.1.100"),
                                );
                            });
                            ui.add_space(12.0);

                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(egui::RichText::new("PIN Code").size(12.0).color(egui::Color32::GRAY));
                                ui.add_space(8.0);
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.add_sized(
                                    egui::vec2(ui.available_width() - 32.0, 28.0),
                                    egui::TextEdit::singleline(&mut self.pin_string)
                                        .hint_text("000000")
                                        .password(true),
                                );
                            });
                            ui.add_space(16.0);
                        });

                        ui.add_space(16.0);

                        if ui
                            .add_enabled(
                                !self.ip_string.is_empty() && !self.pin_string.is_empty(),
                                egui::Button::new(
                                    egui::RichText::new("🔗 Connect").size(16.0),
                                ),
                            )
                            .clicked()
                        {
                            self.client_task = None;
                            *self.connection_status.lock().unwrap() = "Connecting...".to_string();
                            let ip = self.ip_string.clone();
                            let pin = self.pin_string.clone();
                            let status_clone = self.connection_status.clone();
                            let ctx_clone = ctx.clone();
                            self._discovery_rx = None;

                            self.client_task = Some(std::thread::spawn(move || {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                rt.block_on(async {
                                    match network::start_client(&ip, 4444, &pin).await {
                                        Ok(conn) => {
                                            *status_clone.lock().unwrap() =
                                                "Connected Successfully!".to_string();
                                            ctx_clone.request_repaint();

                                            let (tx, rx) =
                                                mpsc::channel::<network::NetworkEvent>(100);

                                            let rt2 = tokio::runtime::Runtime::new().unwrap();
                                            rt2.spawn(capture::os::start_simulation(rx));

                                            network::run_receive_loop(conn, tx).await;

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

                        ui.add_space(10.0);

                        let status_color = if status.contains("Success") {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else if status.contains("Error") {
                            egui::Color32::from_rgb(240, 100, 100)
                        } else {
                            egui::Color32::GRAY
                        };

                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("●").size(10.0).color(status_color));
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(&status).size(13.0).color(status_color));
                        });

                        ui.add_space(20.0);

                        if ui
                            .add_sized(
                                egui::vec2(120.0, 36.0),
                                egui::Button::new("← Back"),
                            )
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
                    }
                }
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl FreemouseApp {
    fn render_onboarding(&mut self, _ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.add_space(30.0);

        // Logo
        if let Some(tex) = &self.logo_texture {
            ui.add(egui::Image::new(tex).max_height(128.0));
            ui.add_space(10.0);
        }

        ui.heading(egui::RichText::new("Welcome to Freemouse").size(28.0).strong());
        ui.label(egui::RichText::new("Mouse, Keyboard & Clipboard Sharing").size(14.0).weak());
        ui.add_space(20.0);

        // Permission checks
        ui.label(egui::RichText::new("System Checks").size(18.0).strong());
        ui.separator();
        ui.add_space(10.0);

        let all_pass = self.permission_checks.checks.iter().all(|c| c.pass);

        for check in &self.permission_checks.checks {
            ui.horizontal(|ui| {
                let icon = if check.pass { "✓" } else { "✗" };
                let color = if check.pass {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(240, 100, 100)
                };
                ui.label(egui::RichText::new(icon).size(18.0).color(color));
                ui.label(egui::RichText::new(check.name).strong());
            });

            if !check.pass {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(&check.detail)
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(2.0);
            }
        }

        ui.add_space(20.0);

        if all_pass {
            ui.label(
                egui::RichText::new("All checks passed! You're ready to go.")
                    .size(14.0)
                    .color(egui::Color32::from_rgb(100, 220, 100)),
            );
            ui.add_space(15.0);
            if ui
                .add_sized(egui::vec2(120.0, 40.0), egui::Button::new("Let's go!"))
                .clicked()
            {
                self.mode = AppMode::Home;
            }
        } else {
            ui.label(
                egui::RichText::new("Some checks failed. Please fix the issues above.")
                    .size(14.0)
                    .color(egui::Color32::from_rgb(240, 180, 60)),
            );
            ui.add_space(10.0);
            if ui
                .add_sized(egui::vec2(160.0, 40.0), egui::Button::new("Continue anyway"))
                .clicked()
            {
                self.mode = AppMode::Home;
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 600.0])
            .with_min_inner_size([400.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Freemouse",
        options,
        Box::new(|cc| {
            let mut style = (*cc.egui_ctx.style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.window_rounding = egui::Rounding::same(10.0);
            style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(8.0);
            style.visuals.widgets.inactive.rounding = egui::Rounding::same(8.0);
            style.visuals.widgets.hovered.rounding = egui::Rounding::same(8.0);
            style.visuals.widgets.active.rounding = egui::Rounding::same(8.0);
            cc.egui_ctx.set_style(style);

            let mut app = FreemouseApp::default();
            // Load logo texture
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
            Box::new(app)
        }),
    )
}
