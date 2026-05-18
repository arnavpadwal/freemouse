#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod network;

use eframe::egui;
use rand::Rng;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

#[derive(PartialEq, Clone)]
enum AppMode {
    Home,
    Share(String),
    Receive,
}

struct FreemouseApp {
    mode: AppMode,
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
}

impl Default for FreemouseApp {
    fn default() -> Self {
        let (sw, sh) = capture::get_screen_size();
        Self {
            mode: AppMode::Home,
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
                    AppMode::Home => {
                        let btn_size = egui::vec2(220.0, 50.0);
                        if ui
                            .add_sized(
                                btn_size,
                                egui::Button::new(egui::RichText::new("📤 Share").size(20.0)),
                            )
                            .clicked()
                        {
                            self.server_task = None;
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
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                rt.block_on(async {
                                    match network::start_server(4444, &pin_clone).await {
                                        Ok(conn) => {
                                            *status_clone.lock().unwrap() = "Connected!".to_string();
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
                        ui.add_space(10.0);
                        if ui
                            .add_sized(
                                btn_size,
                                egui::Button::new(egui::RichText::new("📥 Receive").size(20.0)),
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
                    }
                    AppMode::Share(pin) => {
                        let local_ip = local_ip_address::local_ip()
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|_| "Unknown IP".to_string());

                        ui.label(egui::RichText::new("📤 Share Mode Active").size(24.0));
                        ui.add_space(10.0);
                        ui.label("Tell the Receiver to enter these details:");

                        ui.add_space(15.0);
                        ui.group(|ui| {
                            ui.set_width(260.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("IP Address:").strong());
                                ui.label(
                                    egui::RichText::new(&local_ip)
                                        .color(egui::Color32::from_rgb(100, 200, 255))
                                        .size(18.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("PIN Code:  ").strong());
                                ui.label(
                                    egui::RichText::new(&pin)
                                        .color(egui::Color32::from_rgb(100, 255, 150))
                                        .size(18.0)
                                        .strong(),
                                );
                            });
                        });

                        ui.add_space(20.0);
                        ui.label(status);

                        ui.add_space(10.0);
                        if ui.button("<< Back").clicked() {
                            self.server_task = None;
                            capture::os::stop_capture();
                            self.mode = AppMode::Home;
                            *self.connection_status.lock().unwrap() = "Ready".to_string();
                        }
                    }
                    AppMode::Receive => {
                        ui.label(egui::RichText::new("📥 Receive Mode").size(24.0));
                        ui.add_space(10.0);

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
                            ui.label("Discovered servers:");
                            ui.add_space(5.0);
                            for (i, server_str) in self.discovered_servers.iter().enumerate() {
                                if ui
                                    .selectable_label(
                                        self.ip_string == self.discovered_raw[i].ip,
                                        server_str,
                                    )
                                    .clicked()
                                {
                                    self.ip_string = self.discovered_raw[i].ip.clone();
                                }
                            }
                            ui.add_space(10.0);
                        }

                        ui.horizontal(|ui| {
                            ui.label("IP Address:");
                            ui.text_edit_singleline(&mut self.ip_string);
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.label("PIN Code:  ");
                            ui.text_edit_singleline(&mut self.pin_string);
                        });

                        ui.add_space(15.0);
                        if ui
                            .add_enabled(
                                !self.ip_string.is_empty() && !self.pin_string.is_empty(),
                                egui::Button::new(egui::RichText::new("🔗 Connect").size(18.0)),
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
                        ui.label(status);

                        ui.add_space(10.0);
                        if ui.button("<< Back").clicked() {
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

fn main() -> Result<(), eframe::Error> {
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 400.0])
            .with_min_inner_size([350.0, 300.0]),
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

            Box::<FreemouseApp>::default()
        }),
    )
}
