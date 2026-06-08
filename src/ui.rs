//! Modern UI primitives and first-launch walkthrough.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use egui_shadcn::{
    BadgeProps, BadgeVariant, CardProps, SeparatorProps,
    badge::badge,
    button::{Button, ButtonSize, ButtonVariant},
    card::card,
    separator::separator,
    theme::Theme,
    typography::{
        HeadingProps, TextProps, TypographyColor, heading, text,
    },
};
use std::fs;
use std::path::PathBuf;

// ── Refined palette ──────────────────────────────────────────────────

pub const BG_PRIMARY: Color32 = Color32::from_rgb(6, 8, 14);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(14, 17, 28);
pub const BG_CARD: Color32 = Color32::from_rgb(18, 22, 36);
pub const BG_CARD_HOVER: Color32 = Color32::from_rgb(24, 28, 44);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(248, 250, 252);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(148, 163, 184);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(100, 116, 139);
pub const ACCENT: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(139, 92, 246, 40);
pub const SUCCESS: Color32 = Color32::from_rgb(52, 211, 153);
pub const SUCCESS_SOFT: Color32 = Color32::from_rgba_premultiplied(52, 211, 153, 35);
pub const DANGER: Color32 = Color32::from_rgb(248, 113, 113);
pub const BORDER: Color32 = Color32::from_rgb(38, 44, 62);
pub const BORDER_ACCENT: Color32 = Color32::from_rgb(88, 62, 170);

pub const WALKTHROUGH_STEPS: [&str; 5] = [
    "Welcome",
    "How it works",
    "System check",
    "Your PIN",
    "Ready",
];

pub fn walkthrough_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "freemouse")
        .map(|d| d.config_dir().join("walkthrough_complete"))
}

pub fn is_walkthrough_complete() -> bool {
    walkthrough_path().is_some_and(|p| p.exists())
}

pub fn mark_walkthrough_complete() {
    if let Some(path) = walkthrough_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, "1");
    }
}

pub fn reset_walkthrough() {
    if let Some(path) = walkthrough_path() {
        let _ = fs::remove_file(path);
    }
}

/// Subtle radial gradient backdrop.
pub fn paint_background(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, BG_PRIMARY);

    let center = Pos2::new(rect.center().x, rect.top() + 120.0);
    for (radius, alpha) in [(280.0, 18u8), (200.0, 28), (120.0, 40)] {
        painter.circle_filled(
            center,
            radius,
            Color32::from_rgba_premultiplied(124, 77, 255, alpha),
        );
    }
    painter.circle_filled(
        Pos2::new(rect.right() - 80.0, rect.bottom() - 60.0),
        100.0,
        Color32::from_rgba_premultiplied(52, 211, 153, 12),
    );
}

pub fn step_progress(ui: &mut egui::Ui, theme: &Theme, current: usize, labels: &[&str]) {
    let n = labels.len();
    ui.horizontal(|ui| {
        ui.add_space(ui.available_width() * 0.5 - (n as f32 * 36.0));
        for (i, label) in labels.iter().enumerate() {
            let active = i == current;
            let done = i < current;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), egui::Sense::hover());
            let fill = if active {
                ACCENT
            } else if done {
                SUCCESS
            } else {
                Color32::from_rgb(40, 46, 64)
            };
            ui.painter().circle_filled(rect.center(), 14.0, fill);
            if done && !active {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "✓",
                    egui::FontId::proportional(12.0),
                    TEXT_PRIMARY,
                );
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", i + 1),
                    egui::FontId::proportional(11.0),
                    if active { TEXT_PRIMARY } else { TEXT_MUTED },
                );
            }
            if i + 1 < n {
                let line = Rect::from_min_size(
                    Pos2::new(rect.right(), rect.center().y - 1.0),
                    Vec2::new(44.0, 2.0),
                );
                ui.painter().rect_filled(
                    line,
                    1.0,
                    if done { SUCCESS_SOFT } else { BORDER },
                );
                ui.add_space(44.0);
            }
            let _ = label;
        }
    });
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        text(
            ui,
            theme,
            TextProps::new(format!(
                "Step {} of {} — {}",
                current + 1,
                n,
                labels.get(current).unwrap_or(&"")
            ))
            .size(12.0)
            .color(TypographyColor::Muted),
        );
    });
    ui.add_space(16.0);
}

pub fn glass_card(
    ui: &mut egui::Ui,
    theme: &Theme,
    accent: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let outer = ui.available_rect_before_wrap();
    if accent {
        ui.painter().rect_stroke(
            outer.expand(1.0),
            14.0,
            Stroke::new(1.0, BORDER_ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    card(ui, theme, CardProps::default(), |ui| {
        ui.painter().rect_filled(ui.max_rect(), 12.0, BG_CARD);
        add_contents(ui);
    });
}

pub fn icon_circle(ui: &mut egui::Ui, emoji: &str, tint: Color32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 24.0, tint);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        emoji,
        egui::FontId::proportional(22.0),
        TEXT_PRIMARY,
    );
    rect
}

pub fn feature_tile(ui: &mut egui::Ui, theme: &Theme, emoji: &str, title: &str, desc: &str) {
    glass_card(ui, theme, false, |ui| {
        ui.horizontal(|ui| {
            icon_circle(ui, emoji, ACCENT_SOFT);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                heading(ui, theme, HeadingProps::new(title).size(15.0));
                ui.add_space(2.0);
                text(
                    ui,
                    theme,
                    TextProps::new(desc)
                        .size(13.0)
                        .color(TypographyColor::Muted),
                );
            });
        });
    });
    ui.add_space(8.0);
}

pub fn nav_tab(
    ui: &mut egui::Ui,
    _theme: &Theme,
    label: &str,
    icon: &str,
    active: bool,
) -> bool {
    let sense = egui::Sense::click();
    let size = Vec2::new(88.0, 52.0);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let bg = if active {
        ACCENT_SOFT
    } else if response.hovered() {
        BG_CARD_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if active {
        Stroke::new(1.0, ACCENT)
    } else {
        Stroke::new(1.0, Color32::TRANSPARENT)
    };
    ui.painter().rect_filled(rect, 10.0, bg);
    ui.painter()
        .rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
    ui.painter().text(
        Pos2::new(rect.center().x, rect.top() + 14.0),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(16.0),
        if active { ACCENT } else { TEXT_MUTED },
    );
    ui.painter().text(
        Pos2::new(rect.center().x, rect.bottom() - 12.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if active { TEXT_PRIMARY } else { TEXT_SECONDARY },
    );
    response.clicked()
}

pub fn status_pill(ui: &mut egui::Ui, theme: &Theme, label: &str, live: bool) {
    badge(
        ui,
        theme,
        if live {
            BadgeProps::new(label)
                .variant(BadgeVariant::Default)
                .color(SUCCESS)
        } else {
            BadgeProps::new(label).variant(BadgeVariant::Secondary)
        },
    );
}

pub fn pin_display(ui: &mut egui::Ui, theme: &Theme, pin: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 10.0, BG_ELEVATED);
    ui.painter().rect_stroke(
        rect,
        10.0,
        Stroke::new(1.0, BORDER_ACCENT),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        pin,
        egui::FontId::monospace(28.0),
        ACCENT,
    );
    ui.add_space(4.0);
    text(
        ui,
        theme,
        TextProps::new("Share this PIN with machines you want to connect")
            .size(12.0)
            .color(TypographyColor::Muted),
    );
}

pub fn how_it_works_diagram(ui: &mut egui::Ui, theme: &Theme) {
    let steps = [
        ("1", "🖥️", "Install Freemouse", "On every computer you want to link"),
        ("2", "🔗", "Start workspace", "Use the same PIN on your local network"),
        ("3", "📐", "Arrange layout", "Place machines in the 2×2 grid"),
        ("4", "🖱️", "Cross the edge", "Move your cursor off the screen border"),
    ];
    for (num, emoji, title, desc) in steps {
        glass_card(ui, theme, false, |ui| {
            ui.horizontal(|ui| {
                let (nrect, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                ui.painter().circle_filled(nrect.center(), 16.0, ACCENT);
                ui.painter().text(
                    nrect.center(),
                    egui::Align2::CENTER_CENTER,
                    num,
                    egui::FontId::proportional(13.0),
                    TEXT_PRIMARY,
                );
                ui.add_space(8.0);
                icon_circle(ui, emoji, SUCCESS_SOFT);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    heading(ui, theme, HeadingProps::new(title).size(15.0));
                    text(
                        ui,
                        theme,
                        TextProps::new(desc)
                            .size(13.0)
                            .color(TypographyColor::Muted),
                    );
                });
            });
        });
        ui.add_space(6.0);
    }
}

pub fn machine_cell(
    ui: &mut egui::Ui,
    theme: &Theme,
    hostname: Option<&str>,
    is_self: bool,
    connected: bool,
    size: Vec2,
) {
    ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Center), |ui| {
        let rect = ui.max_rect();
        let bg = if is_self {
            Color32::from_rgba_premultiplied(139, 92, 246, 50)
        } else if connected {
            Color32::from_rgba_premultiplied(52, 211, 153, 40)
        } else if hostname.is_some() {
            Color32::from_rgba_premultiplied(30, 36, 56, 200)
        } else {
            Color32::from_rgba_premultiplied(20, 24, 38, 120)
        };
        let border = if connected {
            SUCCESS
        } else if is_self {
            ACCENT
        } else {
            BORDER
        };
        ui.painter().rect_filled(rect, 12.0, bg);
        ui.painter()
            .rect_stroke(rect, 12.0, Stroke::new(1.5, border), egui::StrokeKind::Inside);

        if let Some(name) = hostname {
            ui.add_space(18.0);
            ui.painter().text(
                Pos2::new(rect.center().x, rect.top() + 36.0),
                egui::Align2::CENTER_CENTER,
                "💻",
                egui::FontId::proportional(20.0),
                TEXT_PRIMARY,
            );
            text(ui, theme, TextProps::new(name).size(13.0));
            let status = if is_self {
                "This machine"
            } else if connected {
                "● Connected"
            } else {
                "○ Waiting"
            };
            text(
                ui,
                theme,
                TextProps::new(status)
                    .size(11.0)
                    .color(TypographyColor::Muted),
            );
        } else {
            ui.add_space(38.0);
            text(
                ui,
                theme,
                TextProps::new("+ Empty slot")
                    .size(12.0)
                    .color(TypographyColor::Muted),
            );
        }
    });
}

pub fn section_header(ui: &mut egui::Ui, theme: &Theme, title: &str, subtitle: &str) {
    heading(ui, theme, HeadingProps::new(title).size(20.0));
    ui.add_space(4.0);
    text(
        ui,
        theme,
        TextProps::new(subtitle)
            .size(14.0)
            .color(TypographyColor::Muted),
    );
    ui.add_space(16.0);
}

pub fn walkthrough_nav(
    ui: &mut egui::Ui,
    theme: &Theme,
    step: usize,
    total: usize,
    on_back: bool,
    next_label: &str,
) -> (bool, bool) {
    let mut back = false;
    let mut next = false;
    ui.horizontal(|ui| {
        if on_back
            && Button::new("← Back")
                .variant(ButtonVariant::Outline)
                .show(ui, theme)
                .clicked()
        {
            back = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::new(next_label)
                .size(ButtonSize::Lg)
                .show(ui, theme)
                .clicked()
            {
                next = true;
            }
            if step + 1 < total {
                ui.add_space(8.0);
                if Button::new("Skip tour")
                    .variant(ButtonVariant::Ghost)
                    .show(ui, theme)
                    .clicked()
                {
                    next = true;
                }
            }
        });
    });
    (back, next)
}

pub fn footer_trust_badges(ui: &mut egui::Ui, theme: &Theme) {
    separator(ui, theme, SeparatorProps::default());
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        for label in ["End-to-end encrypted", "LAN only", "No cloud"] {
            badge(
                ui,
                theme,
                BadgeProps::new(label)
                    .variant(BadgeVariant::Outline)
                    .color(SUCCESS),
            );
            ui.add_space(10.0);
        }
    });
    ui.add_space(8.0);
}
