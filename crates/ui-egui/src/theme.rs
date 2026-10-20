//! Theme tokens (PROMPT.md 6.7): semantic colours with light and dark values, spacing and type
//! scale. Widgets never name a raw colour; they ask the [`Theme`]. System mode follows the OS
//! via egui's `system_theme` (macOS/Windows live; Linux falls back to light as the brief says).

use egui::{Color32, Context, FontFamily, FontId, Stroke, TextStyle, Visuals};

/// The user's choice in Settings > Themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl From<postguy_model::Theme> for ThemeMode {
    fn from(theme: postguy_model::Theme) -> ThemeMode {
        match theme {
            postguy_model::Theme::System => ThemeMode::System,
            postguy_model::Theme::Light => ThemeMode::Light,
            postguy_model::Theme::Dark => ThemeMode::Dark,
        }
    }
}

/// The semantic palette. Hex values are the starting points from PROMPT.md 6.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub surface: Color32,
    pub surface_alt: Color32,
    pub surface_sunken: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,
    pub get: Color32,
    pub post: Color32,
    pub put: Color32,
    pub patch: Color32,
    pub delete: Color32,
    pub head: Color32,
    pub options: Color32,
}

impl Palette {
    pub const LIGHT: Palette = Palette {
        surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        surface_alt: Color32::from_rgb(0xF6, 0xF7, 0xF9),
        surface_sunken: Color32::from_rgb(0xEE, 0xF0, 0xF3),
        border: Color32::from_rgb(0xE2, 0xE5, 0xEA),
        text: Color32::from_rgb(0x1F, 0x23, 0x28),
        text_muted: Color32::from_rgb(0x6A, 0x73, 0x80),
        accent: Color32::from_rgb(0x2F, 0x6F, 0xEB),
        success: Color32::from_rgb(0x17, 0x80, 0x48),
        warning: Color32::from_rgb(0x8A, 0x5A, 0x12),
        danger: Color32::from_rgb(0xC0, 0x34, 0x34),
        info: Color32::from_rgb(0x1F, 0x63, 0x99),
        get: Color32::from_rgb(0x17, 0x80, 0x48),
        post: Color32::from_rgb(0x8A, 0x5A, 0x12),
        put: Color32::from_rgb(0x2F, 0x6F, 0xEB),
        patch: Color32::from_rgb(0x7C, 0x5C, 0xC4),
        delete: Color32::from_rgb(0xC0, 0x34, 0x34),
        head: Color32::from_rgb(0x1C, 0x7C, 0x72),
        options: Color32::from_rgb(0xA6, 0x3B, 0x80),
    };

    pub const DARK: Palette = Palette {
        surface: Color32::from_rgb(0x1E, 0x1F, 0x22),
        surface_alt: Color32::from_rgb(0x25, 0x27, 0x2B),
        surface_sunken: Color32::from_rgb(0x19, 0x1A, 0x1D),
        border: Color32::from_rgb(0x34, 0x36, 0x3B),
        text: Color32::from_rgb(0xE6, 0xE8, 0xEB),
        text_muted: Color32::from_rgb(0x9A, 0xA1, 0xAB),
        accent: Color32::from_rgb(0x5B, 0x93, 0xF5),
        success: Color32::from_rgb(0x4C, 0xC3, 0x8A),
        warning: Color32::from_rgb(0xE0, 0xA4, 0x58),
        danger: Color32::from_rgb(0xF0, 0x71, 0x78),
        info: Color32::from_rgb(0x6B, 0xB3, 0xE8),
        get: Color32::from_rgb(0x4C, 0xC3, 0x8A),
        post: Color32::from_rgb(0xE0, 0xA4, 0x58),
        put: Color32::from_rgb(0x5B, 0x93, 0xF5),
        patch: Color32::from_rgb(0xA6, 0x8B, 0xE8),
        delete: Color32::from_rgb(0xF0, 0x71, 0x78),
        head: Color32::from_rgb(0x4F, 0xC3, 0xB4),
        options: Color32::from_rgb(0xD9, 0x7A, 0xC0),
    };
}

/// Spacing and sizes shared by every screen (PROMPT.md 6.2: 32 px header, 320 px right panel).
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub header_height: f32,
    pub bottom_bar_height: f32,
    pub sidebar_default: f32,
    pub right_panel: f32,
    pub row_height: f32,
    pub control_height: f32,
    pub radius: f32,
    pub ui_font: f32,
    pub code_font: f32,
    pub stroke: f32,
    pub icon: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            header_height: 32.0,
            bottom_bar_height: 26.0,
            sidebar_default: 260.0,
            right_panel: 320.0,
            row_height: 26.0,
            control_height: 26.0,
            radius: 6.0,
            ui_font: 13.0,
            code_font: 12.5,
            stroke: 1.5,
            icon: 15.0,
        }
    }
}

/// Everything the widgets read: mode, resolved palette and metrics.
#[derive(Debug, Clone)]
pub struct Theme {
    pub mode: ThemeMode,
    pub dark: bool,
    pub palette: Palette,
    pub metrics: Metrics,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::resolve(ThemeMode::Light)
    }
}

impl Theme {
    /// The theme the shell installed this frame (widgets read this); falls back to the egui
    /// visual mode when a test or example draws widgets without the shell.
    pub fn current(ctx: &Context) -> Theme {
        if let Some(theme) = ctx.data(|data| data.get_temp::<Theme>(egui::Id::new("postguy-theme"))) {
            return theme;
        }
        Theme::from_context(ctx, ThemeMode::System)
    }

    /// Install the theme for this frame (the shell calls this once per frame): the preference
    /// (System/Light/Dark), the palette and the type scale.
    pub fn install(&self, ctx: &Context) {
        ctx.set_theme(match self.mode {
            ThemeMode::System => egui::ThemePreference::System,
            ThemeMode::Light => egui::ThemePreference::Light,
            ThemeMode::Dark => egui::ThemePreference::Dark,
        });
        self.apply(ctx);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("postguy-theme"), self.clone()));
    }

    /// Resolve the colour family. `System` follows `ctx.style().visuals.dark_mode`, which eframe
    /// updates from the OS (and kittest lets tests choose).
    pub fn from_context(ctx: &Context, mode: ThemeMode) -> Theme {
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => ctx.theme() == egui::Theme::Dark,
        };
        Theme::resolve_with(mode, dark)
    }

    pub fn resolve(mode: ThemeMode) -> Theme {
        // A non-system call in tests picks dark only for Dark mode; System defaults to light.
        Theme::resolve_with(mode, mode == ThemeMode::Dark)
    }

    pub fn resolve_with(mode: ThemeMode, dark: bool) -> Theme {
        Theme { mode, dark, palette: if dark { Palette::DARK } else { Palette::LIGHT }, metrics: Metrics::default() }
    }

    /// Apply the palette to egui's style. Call once per frame (idempotent and cheap).
    pub fn apply(&self, ctx: &Context) {
        let palette = self.palette;
        let metrics = self.metrics;
        let egui_theme = if self.dark { egui::Theme::Dark } else { egui::Theme::Light };
        let mut style = (*ctx.style_of(egui_theme)).clone();
        let mut visuals = if self.dark { Visuals::dark() } else { Visuals::light() };
        visuals.override_text_color = Some(palette.text);
        visuals.panel_fill = palette.surface;
        visuals.window_fill = palette.surface;
        visuals.extreme_bg_color = palette.surface_sunken;
        visuals.faint_bg_color = palette.surface_alt;
        visuals.window_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.noninteractive.bg_fill = palette.surface;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.inactive.bg_fill = palette.surface_alt;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.hovered.bg_fill = palette.surface_sunken;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.active.bg_fill = palette.surface_sunken;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
        visuals.selection.bg_fill = palette.accent.gamma_multiply(0.22);
        visuals.selection.stroke = Stroke::new(1.0, palette.accent);
        visuals.hyperlink_color = palette.accent;
        visuals.warn_fg_color = palette.warning;
        visuals.error_fg_color = palette.danger;
        visuals.window_corner_radius = egui::CornerRadius::same(metrics.radius as u8);
        visuals.menu_corner_radius = egui::CornerRadius::same(metrics.radius as u8);
        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = metrics.control_height;
        style.spacing.scroll.bar_width = 8.0;
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(metrics.ui_font, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(metrics.ui_font, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(metrics.code_font, FontFamily::Monospace)),
        ]
        .into();
        ctx.set_style_of(egui_theme, style);
    }

    /// Method text colour (universal REST conventions, PROMPT.md 6.7).
    pub fn method_color(&self, method: &str) -> Color32 {
        match method.to_ascii_uppercase().as_str() {
            "GET" => self.palette.get,
            "POST" => self.palette.post,
            "PUT" => self.palette.put,
            "PATCH" => self.palette.patch,
            "DELETE" => self.palette.delete,
            "HEAD" => self.palette.head,
            "OPTIONS" => self.palette.options,
            _ => self.palette.text_muted,
        }
    }

    /// Status chip colour: 2xx success, 3xx info, 4xx warning, 5xx danger.
    pub fn status_color(&self, status: u16) -> Color32 {
        match status {
            200..=299 => self.palette.success,
            300..=399 => self.palette.info,
            _ => self.palette.danger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_resolve_the_expected_palette() {
        assert_eq!(Theme::resolve(ThemeMode::Light).palette, Palette::LIGHT);
        assert_eq!(Theme::resolve(ThemeMode::Dark).palette, Palette::DARK);
        assert_eq!(Theme::resolve_with(ThemeMode::System, true).palette, Palette::DARK);
        assert_eq!(Theme::resolve_with(ThemeMode::System, false).palette, Palette::LIGHT);
    }

    fn relative_luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (high, low) = {
            let (a, b) = (relative_luminance(a), relative_luminance(b));
            if a > b { (a, b) } else { (b, a) }
        };
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn every_text_token_meets_wcag_aa_on_its_surface() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            let surface = palette.surface;
            for (name, color) in [
                ("text", palette.text),
                ("muted", palette.text_muted),
                ("accent", palette.accent),
                ("success", palette.success),
                ("warning", palette.warning),
                ("danger", palette.danger),
                ("info", palette.info),
                ("get", palette.get),
                ("post", palette.post),
                ("put", palette.put),
                ("patch", palette.patch),
                ("delete", palette.delete),
                ("head", palette.head),
                ("options", palette.options),
            ] {
                let ratio = contrast(color, surface);
                assert!(ratio >= 4.5, "{name} on surface is {ratio:.2}:1 (needs 4.5:1)");
            }
        }
    }

    #[test]
    fn method_and_status_colours_follow_the_conventions() {
        let theme = Theme::resolve(ThemeMode::Light);
        assert_eq!(theme.method_color("get"), theme.palette.get);
        assert_eq!(theme.method_color("DELETE"), theme.palette.delete);
        assert_eq!(theme.status_color(204), theme.palette.success);
        assert_eq!(theme.status_color(302), theme.palette.info);
        assert_eq!(theme.status_color(404), theme.palette.danger);
        assert_eq!(theme.status_color(503), theme.palette.danger);
    }
}
