#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use eframe::egui::{self, Color32, ColorImage, IconData};
use egui_shadcn::{
    BadgeProps, BadgeVariant,
    badge::badge,
    button::{Button, ButtonSize, ButtonVariant},
    input::Input,
    label::Label,
    theme::Theme,
    tokens::ColorPalette,
    typography::{
        HeadingAs, HeadingProps, TextProps, TypographyColor, heading, text,
    },
};
use freemouse::{
    capture, clipboard, connection::ConnectionManager, file_transfer, layout::{MachineLayout, WorkspaceLayout},
    network::{self, DiscoveredServer, DEFAULT_PORT},
    router::EdgeRouter,
};
use rand::Rng;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(PartialEq, Clone)]
enum AppMode {
    Walkthrough,
    Workspace,
}

#[derive(PartialEq, Clone, Copy)]
enum WorkspaceTab {
    Overview,
    Layout,
    Connect,
    Settings,
}

struct CheckResult {
    name: &'static str,
    pass: bool,
    detail: String,
}

fn custom_theme() -> Theme {
    Theme::new(ColorPalette {
        background: ui::BG_PRIMARY,
        foreground: ui::TEXT_PRIMARY,
        card: ui::BG_CARD,
        card_foreground: ui::TEXT_PRIMARY,
        popover: ui::BG_CARD,
        popover_foreground: ui::TEXT_PRIMARY,
        border: ui::BORDER,
        input: ui::BG_ELEVATED,
        ring: ui::ACCENT,
        primary: ui::ACCENT,
        primary_foreground: egui::Color32::WHITE,
        secondary: ui::BG_ELEVATED,
        secondary_foreground: ui::TEXT_PRIMARY,
        accent: ui::ACCENT,
        accent_foreground: egui::Color32::WHITE,
        muted: ui::BG_ELEVATED,
        muted_foreground: ui::TEXT_MUTED,
        destructive: ui::DANGER,
        destructive_foreground: egui::Color32::WHITE,
        chart_1: ui::ACCENT,
        chart_2: ui::SUCCESS,
        chart_3: Color32::from_rgb(255, 183, 77),
        chart_4: Color32::from_rgb(100, 181, 246),
        chart_5: Color32::from_rgb(206, 147, 216),
        sidebar: ui::BG_ELEVATED,
        sidebar_foreground: ui::TEXT_PRIMARY,
        sidebar_primary: ui::ACCENT,
        sidebar_primary_foreground: egui::Color32::WHITE,
        sidebar_accent: ui::ACCENT,
        sidebar_accent_foreground: egui::Color32::WHITE,
        sidebar_border: ui::BORDER,
        sidebar_ring: ui::ACCENT,
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
                detail: "/dev/input does not exist.".into(),
            });
        } else {
            let mut found = false;
            if let Ok(entries) = std::fs::read_dir(input_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .starts_with("event")
                    {
                        found = true;
                        let pass = std::fs::File::open(&path).is_ok();
                        checks.push(CheckResult {
                            name: "Input devices",
                            pass,
                            detail: if pass {
                                "evdev devices accessible.".into()
                            } else {
                                format!("Cannot read {:?}. Run: sudo usermod -aG input $USER", path)
                            },
                        });
                        break;
                    }
                }
            }
            if !found {
                checks.push(CheckResult {
                    name: "Input devices",
                    pass: false,
                    detail: "No event devices found.".into(),
                });
            }
        }

        let uinput = std::path::Path::new("/dev/uinput");
        checks.push(CheckResult {
            name: "uinput device",
            pass: uinput.exists() && std::fs::OpenOptions::new().write(true).open(uinput).is_ok(),
            detail: if uinput.exists() {
                "Run: sudo usermod -aG uinput $USER then log out/in.".into()
            } else {
                "/dev/uinput missing. Load uinput module.".into()
            },
        });
    }

    #[cfg(target_os = "macos")]
    {
        checks.push(CheckResult {
            name: "Accessibility",
            pass: true,
            detail: "Grant Accessibility permission in System Settings > Privacy & Security.".into(),
        });
    }

    #[cfg(target_os = "windows")]
    {
        checks.push(CheckResult {
            name: "Input hooks",
            pass: true,
            detail: "Run as administrator if input capture fails.".into(),
        });
    }

    match std::net::TcpListener::bind(format!("0.0.0.0:{}", DEFAULT_PORT)) {
        Ok(_) => checks.push(CheckResult {
            name: "Network port",
            pass: true,
            detail: format!("Port {} available.", DEFAULT_PORT),
        }),
        Err(e) => checks.push(CheckResult {
            name: "Network port",
            pass: false,
            detail: format!("Cannot bind port {}: {}", DEFAULT_PORT, e),
        }),
    }

    let (w, h) = capture::get_screen_size();
    checks.push(CheckResult {
        name: "Display detected",
        pass: w > 0.0 && h > 0.0,
        detail: format!("Screen: {:.0}x{:.0}", w, h),
    });

    checks
}

struct FreemouseApp {
    mode: AppMode,
    theme: Theme,
    logo_texture: Option<egui::TextureHandle>,
    machine_id: Uuid,
    pin: String,
    connection_status: Arc<Mutex<String>>,
    conn_manager: Option<ConnectionManager>,
    discovery: Option<network::DiscoveryHandle>,
    workspace_shutdown: Arc<AtomicBool>,
    workspace_thread: Option<std::thread::JoinHandle<()>>,
    discovered: Vec<DiscoveredServer>,
    layout: WorkspaceLayout,
    ip_string: String,
    pin_connect: String,
    screen_width: f64,
    workspace_active: bool,
    checks_cache: Vec<CheckResult>,
    walkthrough_step: usize,
    workspace_tab: WorkspaceTab,
}

impl Default for FreemouseApp {
    fn default() -> Self {
        let checks = run_permission_checks();
        let mut layout = WorkspaceLayout::load();
        let machine_id = Uuid::new_v4();
        let pin = format!("{:06}", rand::thread_rng().gen_range(0..999999));
        let (sw, _) = capture::get_screen_size();
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "This Machine".into());
        let local_ip = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "0.0.0.0".into());

        if !layout.machines.iter().any(|m| m.hostname == hostname) {
            if let Some(pos) = layout.next_free_grid_pos() {
                layout.add_or_update_machine(MachineLayout {
                    machine_id,
                    hostname: hostname.clone(),
                    ip: local_ip,
                    grid_pos: pos,
                    screens: capture::get_screens(),
                });
                let _ = layout.save();
            }
        }

        Self {
            mode: if ui::is_walkthrough_complete() {
                AppMode::Workspace
            } else {
                AppMode::Walkthrough
            },
            theme: custom_theme(),
            logo_texture: None,
            machine_id,
            pin,
            connection_status: Arc::new(Mutex::new("Ready".into())),
            conn_manager: None,
            discovery: None,
            workspace_shutdown: Arc::new(AtomicBool::new(false)),
            workspace_thread: None,
            discovered: Vec::new(),
            layout,
            ip_string: String::new(),
            pin_connect: String::new(),
            screen_width: sw,
            workspace_active: false,
            checks_cache: checks,
            walkthrough_step: 0,
            workspace_tab: WorkspaceTab::Overview,
        }
    }
}

impl eframe::App for FreemouseApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_discovery();
        self.poll_inbound_events();

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ui::BG_PRIMARY))
            .show(ctx, |ui| {
                ui::paint_background(ui);
                let theme = self.theme.clone();
                let max_w = 640.0;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(max_w);
                        self.render_header(ui, &theme);
                        ui.add_space(20.0);

                        match self.mode {
                            AppMode::Walkthrough => self.render_walkthrough(ui, &theme),
                            AppMode::Workspace => self.render_workspace_shell(ui, &theme),
                        }

                        ui.add_space(28.0);
                        ui::footer_trust_badges(ui, &theme);
                    });
                });
            });

        ctx.request_repaint_after(Duration::from_millis(300));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_workspace();
    }
}

impl FreemouseApp {
    fn poll_discovery(&mut self) {
        if let Some(handle) = &mut self.discovery {
            while let Ok(server) = handle.rx.try_recv() {
                if !self.discovered.iter().any(|s| s.machine_id == server.machine_id) {
                    self.discovered.push(server.clone());
                    if let Some(cm) = &mut self.conn_manager {
                        cm.add_discovered_to_layout(&server);
                        cm.connect_neighbors(&self.discovered);
                    }
                }
            }
        }
    }

    fn poll_inbound_events(&mut self) {
        let events: Vec<_> = if let Some(cm) = &mut self.conn_manager {
            let mut evts = Vec::new();
            while let Ok(e) = cm.ui_rx.try_recv() {
                evts.push(e);
            }
            evts
        } else {
            return;
        };

        for event in events {
            match event {
                network::NetworkEvent::Hello {
                    machine_id,
                    hostname,
                    screens,
                    ..
                } => {
                    if let Some(pos) = self.layout.next_free_grid_pos() {
                        self.layout.add_or_update_machine(MachineLayout {
                            machine_id,
                            hostname,
                            ip: String::new(),
                            grid_pos: pos,
                            screens,
                        });
                        let _ = self.layout.save();
                    }
                }
                network::NetworkEvent::LayoutUpdate { machines } => {
                    self.layout.machines = machines;
                    let _ = self.layout.save();
                    capture::set_edge_router(EdgeRouter::new(
                        self.machine_id,
                        self.layout.clone(),
                    ));
                }
                _ => {}
            }
        }
    }

    fn start_workspace(&mut self) {
        if self.workspace_active {
            return;
        }
        self.workspace_active = true;
        self.workspace_shutdown.store(false, Ordering::SeqCst);

        capture::set_edge_router(EdgeRouter::new(
            self.machine_id,
            self.layout.clone(),
        ));

        let mut cm = ConnectionManager::new(
            self.machine_id,
            self.pin.clone(),
            self.layout.clone(),
        );
        cm.start_discovery(capture::get_screens());
        cm.start_listener();

        let shutdown = self.workspace_shutdown.clone();
        let status = self.connection_status.clone();
        let sw = self.screen_width;
        let (capture_tx, capture_rx) = tokio::sync::mpsc::channel(100);
        let (sim_tx, sim_rx) = tokio::sync::mpsc::channel(100);

        cm.start_event_bridge(capture_rx, sim_tx, shutdown.clone());
        self.conn_manager = Some(cm);

        self.discovery = Some(network::start_discovery_listener());

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                capture::os::start_capture(capture_tx.clone(), sw);
                clipboard::start_clipboard_monitor(capture_tx);
                capture::os::start_simulation(sim_rx).await;
            });
            *status.lock().unwrap() = "Stopped".into();
        });
        self.workspace_thread = Some(handle);
        *self.connection_status.lock().unwrap() = "Workspace active".into();
    }

    fn stop_workspace(&mut self) {
        self.workspace_shutdown.store(true, Ordering::SeqCst);
        if let Some(d) = &self.discovery {
            d.stop();
        }
        self.discovery = None;
        if let Some(mut cm) = self.conn_manager.take() {
            cm.shutdown_all();
        }
        capture::os::stop_capture();
        if let Some(h) = self.workspace_thread.take() {
            let _ = h.join();
        }
        self.workspace_active = false;
        *self.connection_status.lock().unwrap() = "Ready".into();
    }

    fn render_header(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.add_space(28.0);
        ui.horizontal(|ui| {
            if let Some(tex) = &self.logo_texture {
                ui.add(egui::Image::new(tex).max_height(44.0));
                ui.add_space(12.0);
            }
            ui.vertical(|ui| {
                heading(ui, theme, HeadingProps::new("Freemouse").size(26.0));
                text(
                    ui,
                    theme,
                    TextProps::new("One mouse. Every screen.")
                        .size(14.0)
                        .color(TypographyColor::Muted),
                );
            });
        });
    }

    fn render_walkthrough(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let step = self.walkthrough_step;
        let labels: Vec<&str> = ui::WALKTHROUGH_STEPS.to_vec();
        ui::step_progress(ui, theme, step, &labels);

        match step {
            0 => {
                ui.vertical_centered(|ui| {
                    if let Some(tex) = &self.logo_texture {
                        ui.add(egui::Image::new(tex).max_height(120.0));
                        ui.add_space(16.0);
                    }
                    heading(
                        ui,
                        theme,
                        HeadingProps::new("Welcome to Freemouse")
                            .as_tag(HeadingAs::H2)
                            .size(24.0),
                    );
                    ui.add_space(8.0);
                    text(
                        ui,
                        theme,
                        TextProps::new(
                            "Control up to 4 computers with one keyboard and mouse — \
                             like Mouse Without Borders, but for Windows, macOS, and Linux.",
                        )
                        .size(15.0)
                        .color(TypographyColor::Muted),
                    );
                });
                ui.add_space(20.0);
                ui::feature_tile(
                    ui,
                    theme,
                    "🔒",
                    "Encrypted & local",
                    "All traffic stays on your LAN with ChaCha20 encryption.",
                );
                ui::feature_tile(
                    ui,
                    theme,
                    "⚡",
                    "Seamless switching",
                    "Move your cursor off the screen edge to jump to the next machine.",
                );
                ui::feature_tile(
                    ui,
                    theme,
                    "📋",
                    "Clipboard & files",
                    "Copy-paste and send files across machines instantly.",
                );
            }
            1 => {
                ui::section_header(
                    ui,
                    theme,
                    "How Freemouse works",
                    "Set up once, then forget it's there.",
                );
                ui::how_it_works_diagram(ui, theme);
            }
            2 => {
                ui::section_header(
                    ui,
                    theme,
                    "System check",
                    "We'll verify your machine is ready.",
                );
                self.checks_cache = run_permission_checks();
                for check in &self.checks_cache {
                    ui::glass_card(ui, theme, check.pass, |ui| {
                        ui.horizontal(|ui| {
                            let badge_props = if check.pass {
                                BadgeProps::new("✓").variant(BadgeVariant::Default)
                            } else {
                                BadgeProps::new("✗").variant(BadgeVariant::Destructive)
                            };
                            badge(ui, theme, badge_props);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                Label::new(check.name).show(ui, theme);
                                text(
                                    ui,
                                    theme,
                                    TextProps::new(&check.detail)
                                        .size(13.0)
                                        .color(TypographyColor::Muted),
                                );
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            }
            3 => {
                ui::section_header(
                    ui,
                    theme,
                    "Your connection details",
                    "Other machines need this PIN to join your workspace.",
                );
                ui::glass_card(ui, theme, true, |ui| {
                    ui::pin_display(ui, theme, &self.pin);
                    ui.add_space(12.0);
                    let local_ip = local_ip_address::local_ip()
                        .map(|ip| ip.to_string())
                        .unwrap_or_else(|_| "Unknown".into());
                    text(
                        ui,
                        theme,
                        TextProps::new(format!("IP address: {}", local_ip))
                            .size(14.0),
                    );
                    text(
                        ui,
                        theme,
                        TextProps::new(format!("Port: {}", DEFAULT_PORT))
                            .size(13.0)
                            .color(TypographyColor::Muted),
                    );
                });
            }
            _ => {
                ui::section_header(
                    ui,
                    theme,
                    "You're all set!",
                    "Start your workspace and add other machines from the grid.",
                );
                ui::glass_card(ui, theme, true, |ui| {
                    ui.vertical_centered(|ui| {
                        ui::icon_circle(ui, "🚀", ui::SUCCESS_SOFT);
                        ui.add_space(12.0);
                        heading(ui, theme, HeadingProps::new("Ready to launch").size(18.0));
                        text(
                            ui,
                            theme,
                            TextProps::new(
                                "Click Start Workspace, then move your mouse to the screen edge \
                                 to take control of a linked machine.",
                            )
                            .size(14.0)
                            .color(TypographyColor::Muted),
                        );
                    });
                });
            }
        }

        ui.add_space(24.0);
        let next_label = if step + 1 >= ui::WALKTHROUGH_STEPS.len() {
            "Enter workspace →"
        } else {
            "Continue →"
        };
        let (back, next) = ui::walkthrough_nav(
            ui,
            theme,
            step,
            ui::WALKTHROUGH_STEPS.len(),
            step > 0,
            next_label,
        );
        if back && step > 0 {
            self.walkthrough_step -= 1;
        }
        if next {
            if step + 1 >= ui::WALKTHROUGH_STEPS.len() {
                ui::mark_walkthrough_complete();
                self.mode = AppMode::Workspace;
                self.start_workspace();
            } else {
                self.walkthrough_step += 1;
            }
        }
    }

    fn render_workspace_shell(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            if ui::nav_tab(
                ui,
                theme,
                "Overview",
                "◉",
                self.workspace_tab == WorkspaceTab::Overview,
            ) {
                self.workspace_tab = WorkspaceTab::Overview;
            }
            if ui::nav_tab(
                ui,
                theme,
                "Layout",
                "▦",
                self.workspace_tab == WorkspaceTab::Layout,
            ) {
                self.workspace_tab = WorkspaceTab::Layout;
            }
            if ui::nav_tab(
                ui,
                theme,
                "Connect",
                "🔗",
                self.workspace_tab == WorkspaceTab::Connect,
            ) {
                self.workspace_tab = WorkspaceTab::Connect;
            }
            if ui::nav_tab(
                ui,
                theme,
                "Settings",
                "⚙",
                self.workspace_tab == WorkspaceTab::Settings,
            ) {
                self.workspace_tab = WorkspaceTab::Settings;
            }
        });
        ui.add_space(20.0);

        match self.workspace_tab {
            WorkspaceTab::Overview => self.render_workspace_overview(ui, theme),
            WorkspaceTab::Layout => self.render_layout_editor(ui, theme),
            WorkspaceTab::Connect => self.render_connect(ui, theme),
            WorkspaceTab::Settings => self.render_settings(ui, theme),
        }
    }

    fn render_workspace_overview(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let status = self.connection_status.lock().unwrap().clone();

        ui.horizontal(|ui| {
            ui::section_header(
                ui,
                theme,
                "Workspace",
                "Manage linked machines and control sharing.",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui::status_pill(ui, theme, &status, self.workspace_active);
            });
        });

        if !self.workspace_active {
            ui::glass_card(ui, theme, true, |ui| {
                ui.horizontal(|ui| {
                    ui::icon_circle(ui, "💡", ui::ACCENT_SOFT);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        heading(ui, theme, HeadingProps::new("Get started").size(16.0));
                        text(
                            ui,
                            theme,
                            TextProps::new(
                                "Start the workspace to broadcast on your network and accept connections.",
                            )
                            .size(13.0)
                            .color(TypographyColor::Muted),
                        );
                    });
                });
            });
            ui.add_space(12.0);
        }

        ui::glass_card(ui, theme, false, |ui| {
            heading(ui, theme, HeadingProps::new("Machine grid").size(16.0));
            ui.add_space(4.0);
            text(
                ui,
                theme,
                TextProps::new("Arrange up to 4 machines — edges connect neighbors.")
                    .size(12.0)
                    .color(TypographyColor::Muted),
            );
            ui.add_space(14.0);
            let cell_size = egui::vec2(148.0, 108.0);
            for row in 0..2u8 {
                ui.horizontal(|ui| {
                    for col in 0..2u8 {
                        let machine = self
                            .layout
                            .machines
                            .iter()
                            .find(|m| m.grid_pos == (col, row));
                        let connected = machine
                            .map(|m| {
                                self.conn_manager
                                    .as_ref()
                                    .map(|cm| cm.is_connected(m.machine_id))
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false);
                        ui::machine_cell(
                            ui,
                            theme,
                            machine.map(|m| m.hostname.as_str()),
                            machine.map(|m| m.machine_id) == Some(self.machine_id),
                            connected,
                            cell_size,
                        );
                        ui.add_space(10.0);
                    }
                });
                ui.add_space(10.0);
            }
        });

        ui.add_space(16.0);

        if !self.discovered.is_empty() {
            ui::glass_card(ui, theme, false, |ui| {
                heading(ui, theme, HeadingProps::new("Nearby machines").size(16.0));
                ui.add_space(10.0);
                for server in self.discovered.clone() {
                    ui.horizontal(|ui| {
                        ui::icon_circle(ui, "🖥", ui::BG_ELEVATED);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            text(ui, theme, TextProps::new(&server.hostname).size(14.0));
                            text(
                                ui,
                                theme,
                                TextProps::new(&server.ip)
                                    .size(12.0)
                                    .color(TypographyColor::Muted),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Button::new("Add")
                                .size(ButtonSize::Sm)
                                .show(ui, theme)
                                .clicked()
                            {
                                if let Some(cm) = &mut self.conn_manager {
                                    cm.add_discovered_to_layout(&server);
                                    let _ = cm.connect_to_peer(&server);
                                }
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
            });
            ui.add_space(16.0);
        }

        ui.horizontal(|ui| {
            if !self.workspace_active {
                if Button::new("▶  Start Workspace")
                    .size(ButtonSize::Lg)
                    .show(ui, theme)
                    .clicked()
                {
                    self.start_workspace();
                }
            } else if Button::new("■  Stop")
                .variant(ButtonVariant::Destructive)
                .show(ui, theme)
                .clicked()
            {
                self.stop_workspace();
            }

            if Button::new("📁  Send file")
                .variant(ButtonVariant::Secondary)
                .show(ui, theme)
                .clicked()
            {
                if let Some(path) = file_transfer::pick_file_to_send() {
                    if let Some(cm) = self.conn_manager.as_ref() {
                        if let Some(peer) = cm.peers_first() {
                            let out = peer.outbound.clone();
                            std::thread::spawn(move || {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                rt.block_on(async {
                                    let _ = file_transfer::send_file(&out, &path).await;
                                });
                            });
                        }
                    }
                }
            }

            if Button::new("?  Replay tour")
                .variant(ButtonVariant::Ghost)
                .show(ui, theme)
                .clicked()
            {
                ui::reset_walkthrough();
                self.walkthrough_step = 0;
                self.mode = AppMode::Walkthrough;
            }
        });

        ui.add_space(14.0);
        ui::glass_card(ui, theme, false, |ui| {
            text(
                ui,
                theme,
                TextProps::new("Your PIN")
                    .size(13.0)
                    .color(TypographyColor::Muted),
            );
            ui::pin_display(ui, theme, &self.pin);
        });
    }

    fn render_layout_editor(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui::section_header(
            ui,
            theme,
            "Layout editor",
            "Position machines in the grid — connected edges share mouse control.",
        );

        let mut moves: Vec<(Uuid, (u8, u8))> = Vec::new();
        for row in 0..2u8 {
            ui.horizontal(|ui| {
                for col in 0..2u8 {
                    let pos = (col, row);
                    let machine = self
                        .layout
                        .machines
                        .iter()
                        .find(|m| m.grid_pos == pos);
                    ui::glass_card(ui, theme, machine.is_some(), |ui| {
                        ui.set_min_size(egui::vec2(140.0, 96.0));
                        if let Some(m) = machine {
                            text(ui, theme, TextProps::new(&m.hostname).size(14.0));
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                for (label, new_pos) in [
                                    ("←", (col.saturating_sub(1), row)),
                                    ("→", (col + 1, row)),
                                    ("↑", (col, row.saturating_sub(1))),
                                    ("↓", (col, row + 1)),
                                ] {
                                    if new_pos.0 < 2
                                        && new_pos.1 < 2
                                        && Button::new(label)
                                            .size(ButtonSize::Sm)
                                            .variant(ButtonVariant::Secondary)
                                            .show(ui, theme)
                                            .clicked()
                                    {
                                        moves.push((m.machine_id, new_pos));
                                    }
                                }
                            });
                        } else {
                            ui.vertical_centered(|ui| {
                                text(
                                    ui,
                                    theme,
                                    TextProps::new("Empty")
                                        .size(13.0)
                                        .color(TypographyColor::Muted),
                                );
                            });
                        }
                    });
                    ui.add_space(8.0);
                }
            });
            ui.add_space(8.0);
        }

        for (id, new_pos) in moves {
            let occupied = self
                .layout
                .machines
                .iter()
                .any(|x| x.grid_pos == new_pos && x.machine_id != id);
            if !occupied {
                if let Some(m) = self.layout.machines.iter_mut().find(|m| m.machine_id == id) {
                    m.grid_pos = new_pos;
                }
            }
        }

        ui.add_space(16.0);
        if Button::new("Save layout").size(ButtonSize::Lg).show(ui, theme).clicked() {
            let _ = self.layout.save();
            capture::set_edge_router(EdgeRouter::new(
                self.machine_id,
                self.layout.clone(),
            ));
            if let Some(cm) = &mut self.conn_manager {
                cm.update_layout(self.layout.clone());
            }
            self.workspace_tab = WorkspaceTab::Overview;
        }
    }

    fn render_settings(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui::section_header(ui, theme, "Settings", "Security and device preferences.");

        ui::glass_card(ui, theme, false, |ui| {
            text(ui, theme, TextProps::new("Machine ID").size(14.0));
            Label::new(self.machine_id.to_string()).show(ui, theme);
            ui.add_space(12.0);
            text(ui, theme, TextProps::new("PIN").size(14.0));
            ui::pin_display(ui, theme, &self.pin);
            ui.add_space(12.0);
            if Button::new("Regenerate PIN")
                .variant(ButtonVariant::Secondary)
                .show(ui, theme)
                .clicked()
            {
                self.pin = format!("{:06}", rand::thread_rng().gen_range(0..999999));
            }
        });

        ui.add_space(12.0);
        ui::glass_card(ui, theme, false, |ui| {
            heading(ui, theme, HeadingProps::new("Help").size(15.0));
            ui.add_space(8.0);
            if Button::new("Replay setup tour")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
            {
                ui::reset_walkthrough();
                self.walkthrough_step = 0;
                self.mode = AppMode::Walkthrough;
            }
        });
    }

    fn render_connect(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui::section_header(
            ui,
            theme,
            "Manual connect",
            "Join another machine using its IP address and PIN.",
        );

        ui::glass_card(ui, theme, true, |ui| {
            text(ui, theme, TextProps::new("IP Address").size(14.0));
            Input::new("ip_input")
                .placeholder("192.168.1.100")
                .width(ui.available_width())
                .show(ui, theme, &mut self.ip_string);
            ui.add_space(12.0);
            text(ui, theme, TextProps::new("PIN").size(14.0));
            Input::new("pin_input")
                .placeholder("000000")
                .width(ui.available_width())
                .show(ui, theme, &mut self.pin_connect);

            ui.add_space(16.0);
            if Button::new("Connect →")
                .size(ButtonSize::Lg)
                .enabled(!self.ip_string.is_empty() && !self.pin_connect.is_empty())
                .show(ui, theme)
                .clicked()
            {
                let ip = self.ip_string.clone();
                let ip_status = ip.clone();
                let pin = self.pin_connect.clone();
                let shutdown = Arc::new(AtomicBool::new(false));
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        match network::start_client(&ip, DEFAULT_PORT, &pin).await {
                            Ok(conn) => {
                                let (sim_tx, sim_rx) = tokio::sync::mpsc::channel(100);
                                let rt2 = tokio::runtime::Runtime::new().unwrap();
                                rt2.spawn(capture::os::start_simulation(sim_rx));
                                network::run_receive_loop(conn, sim_tx, shutdown).await;
                            }
                            Err(e) => tracing::warn!("Connect failed: {}", e),
                        }
                    });
                });
                *self.connection_status.lock().unwrap() = format!("Connecting to {}...", ip_status);
                self.workspace_tab = WorkspaceTab::Overview;
            }
        });
    }
}

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
        }
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,egui_shadcn::theme=off".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

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
            .with_inner_size([720.0, 860.0])
            .with_min_inner_size([600.0, 700.0])
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
                let color_image =
                    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
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
