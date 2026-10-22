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
    /// Title bar, tab strip and bottom bar.
    pub chrome: Color32,
    pub sidebar: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub input_bg: Color32,
    pub input_border: Color32,
    pub input_border_hover: Color32,
    pub button_bg: Color32,
    pub button_hover: Color32,
    pub accent_fill: Color32,
    pub accent_fill_hover: Color32,
    pub on_accent: Color32,
    /// Indent guides in code views.
    pub guide: Color32,
    pub line_number: Color32,
    pub code_key: Color32,
    pub code_string: Color32,
    pub code_number: Color32,
    pub code_keyword: Color32,
    pub code_comment: Color32,
    pub code_punct: Color32,
    pub code_variable: Color32,
    pub code_tag: Color32,
    pub code_attr: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Palette {
    pub const LIGHT: Palette = Palette {
        surface: hex(0xFFFFFF),
        surface_alt: hex(0xF7F7F8),
        surface_sunken: hex(0xF3F3F5),
        border: hex(0xE4E4E7),
        text: hex(0x1F2328),
        text_muted: hex(0x6A7380),
        accent: hex(0x2F6FEB),
        success: hex(0x178048),
        warning: hex(0x8A5A12),
        danger: hex(0xC03434),
        info: hex(0x1F6399),
        get: hex(0x178048),
        post: hex(0x8A5A12),
        put: hex(0x2F6FEB),
        patch: hex(0x7C5CC4),
        delete: hex(0xC03434),
        head: hex(0x1C7C72),
        options: hex(0xA63B80),
        chrome: hex(0xFAFAFA),
        sidebar: hex(0xFFFFFF),
        hover: hex(0xF2F2F3),
        selected: hex(0xEBEBED),
        input_bg: hex(0xFFFFFF),
        input_border: hex(0xDCDCDF),
        input_border_hover: hex(0xC4C4C8),
        button_bg: hex(0xF2F2F3),
        button_hover: hex(0xE6E6E8),
        accent_fill: hex(0x2F6FEB),
        accent_fill_hover: hex(0x2563D9),
        on_accent: hex(0xFFFFFF),
        guide: hex(0xE6E6E8),
        line_number: hex(0x9C9CA3),
        code_key: hex(0xA31515),
        code_string: hex(0x0451A5),
        code_number: hex(0x098658),
        code_keyword: hex(0x0000FF),
        code_comment: hex(0x6A737D),
        code_punct: hex(0x1F2328),
        code_variable: hex(0xAF00DB),
        code_tag: hex(0x800000),
        code_attr: hex(0xE50000),
    };

    pub const DARK: Palette = Palette {
        surface: hex(0x1C1C1C),
        surface_alt: hex(0x232323),
        surface_sunken: hex(0x141414),
        border: hex(0x2E2E2E),
        text: hex(0xE6E6E6),
        text_muted: hex(0xA0A0A0),
        accent: hex(0x5B93F5),
        success: hex(0x4CC38A),
        warning: hex(0xE0A458),
        danger: hex(0xF07178),
        info: hex(0x6BB3E8),
        get: hex(0x4CC38A),
        post: hex(0xE0A458),
        put: hex(0x5B93F5),
        patch: hex(0xA68BE8),
        delete: hex(0xF07178),
        head: hex(0x4FC3B4),
        options: hex(0xD97AC0),
        chrome: hex(0x212121),
        sidebar: hex(0x1C1C1C),
        hover: hex(0x2A2A2A),
        selected: hex(0x333333),
        input_bg: hex(0x202020),
        input_border: hex(0x3A3A3A),
        input_border_hover: hex(0x4A4A4A),
        button_bg: hex(0x2C2C2C),
        button_hover: hex(0x363636),
        accent_fill: hex(0x2F6FEB),
        accent_fill_hover: hex(0x2563D9),
        on_accent: hex(0xFFFFFF),
        guide: hex(0x333333),
        line_number: hex(0x6E6E6E),
        code_key: hex(0x9CDCFE),
        code_string: hex(0xCE9178),
        code_number: hex(0xB5CEA8),
        code_keyword: hex(0x569CD6),
        code_comment: hex(0x6A9955),
        code_punct: hex(0xD4D4D4),
        code_variable: hex(0xC586C0),
        code_tag: hex(0x569CD6),
        code_attr: hex(0x9CDCFE),
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
    pub url_height: f32,
    pub radius: f32,
    pub card_radius: f32,
    pub ui_font: f32,
    pub small_font: f32,
    pub heading_font: f32,
    pub code_font: f32,
    pub stroke: f32,
    pub icon: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            header_height: 40.0,
            bottom_bar_height: 28.0,
            sidebar_default: 300.0,
            right_panel: 340.0,
            row_height: 28.0,
            control_height: 30.0,
            url_height: 36.0,
            radius: 6.0,
            card_radius: 8.0,
            ui_font: 13.0,
            small_font: 11.5,
            heading_font: 15.0,
            code_font: 13.5,
            stroke: 1.5,
            icon: 16.0,
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
    pub custom_fonts_ready: bool,
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
        let ready = Self::ensure_fonts(ctx);
        let mut installed = self.clone();
        installed.custom_fonts_ready = ready;
        installed.apply(ctx);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("postguy-theme"), installed));
    }

    /// Resolve the colour family. `System` follows `ctx.style().visuals.dark_mode`, which eframe
    /// updates from the OS (and kittest lets tests choose).
    pub fn from_context(ctx: &Context, mode: ThemeMode) -> Theme {
        let ready = Self::ensure_fonts(ctx);
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => ctx.theme() == egui::Theme::Dark,
        };
        let mut theme = Theme::resolve_with(mode, dark);
        theme.custom_fonts_ready = ready;
        theme
    }

    pub fn resolve(mode: ThemeMode) -> Theme {
        // A non-system call in tests picks dark only for Dark mode; System defaults to light.
        Theme::resolve_with(mode, mode == ThemeMode::Dark)
    }

    pub fn resolve_with(mode: ThemeMode, dark: bool) -> Theme {
        Theme { mode, dark, palette: if dark { Palette::DARK } else { Palette::LIGHT }, metrics: Metrics::default(), custom_fonts_ready: false }
    }

    /// Register Poppins (Regular, Medium, SemiBold) once per [`Context`] so all proportional
    /// UI text renders in Poppins while code keeps Hack Mono and fallback glyphs stay intact.
    pub fn ensure_fonts(ctx: &Context) -> bool {
        let id = egui::Id::new("postguy-poppins-fonts");
        let pass = ctx.cumulative_pass_nr();
        if let Some(installed_pass) = ctx.data(|data| data.get_temp::<u64>(id)) {
            return pass > installed_pass;
        }
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Poppins-Regular".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Poppins-Regular.ttf"))),
        );
        fonts.font_data.insert(
            "Poppins-Medium".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Poppins-Medium.ttf"))),
        );
        fonts.font_data.insert(
            "Poppins-SemiBold".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Poppins-SemiBold.ttf"))),
        );
        if let Some(proportional) = fonts.families.get_mut(&FontFamily::Proportional) {
            proportional.insert(0, "Poppins-Regular".to_owned());
        }
        fonts.families.insert(
            FontFamily::Name("medium".into()),
            vec![
                "Poppins-Medium".to_owned(),
                "Poppins-Regular".to_owned(),
                "Ubuntu-Light".to_owned(),
                "NotoEmoji-Regular".to_owned(),
                "emoji-icon-font".to_owned(),
            ],
        );
        fonts.families.insert(
            FontFamily::Name("semibold".into()),
            vec![
                "Poppins-SemiBold".to_owned(),
                "Poppins-Regular".to_owned(),
                "Ubuntu-Light".to_owned(),
                "NotoEmoji-Regular".to_owned(),
                "emoji-icon-font".to_owned(),
            ],
        );
        ctx.set_fonts(fonts);
        ctx.request_repaint();
        ctx.data_mut(|data| data.insert_temp(id, pass));
        false
    }

    /// Apply the palette to egui's style. Call once per frame (idempotent and cheap).
    pub fn apply(&self, ctx: &Context) {
        Self::ensure_fonts(ctx);
        let palette = self.palette;
        let metrics = self.metrics;
        let control_rounding = egui::CornerRadius::same(metrics.radius as u8);
        let card_rounding = egui::CornerRadius::same(metrics.card_radius as u8);
        let egui_theme = if self.dark { egui::Theme::Dark } else { egui::Theme::Light };
        let mut style = (*ctx.style_of(egui_theme)).clone();
        let mut visuals = if self.dark { Visuals::dark() } else { Visuals::light() };
        visuals.override_text_color = Some(palette.text);
        visuals.panel_fill = palette.surface;
        visuals.window_fill = palette.surface;
        visuals.extreme_bg_color = palette.input_bg;
        visuals.faint_bg_color = palette.surface_alt;
        visuals.text_edit_bg_color = Some(palette.input_bg);
        visuals.window_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.noninteractive.bg_fill = palette.surface;
        visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text_muted);
        visuals.widgets.noninteractive.corner_radius = control_rounding;
        visuals.widgets.inactive.bg_fill = palette.input_bg;
        visuals.widgets.inactive.weak_bg_fill = palette.button_bg;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.input_border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.inactive.corner_radius = control_rounding;
        visuals.widgets.hovered.bg_fill = palette.input_bg;
        visuals.widgets.hovered.weak_bg_fill = palette.button_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.input_border_hover);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.hovered.expansion = 0.0;
        visuals.widgets.hovered.corner_radius = control_rounding;
        visuals.widgets.active.bg_fill = palette.input_bg;
        visuals.widgets.active.weak_bg_fill = palette.selected;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.active.corner_radius = control_rounding;
        visuals.widgets.open.bg_fill = palette.input_bg;
        visuals.widgets.open.weak_bg_fill = palette.selected;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.accent);
        visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.open.corner_radius = control_rounding;
        visuals.selection.bg_fill = palette.accent.gamma_multiply(0.22);
        visuals.selection.stroke = Stroke::new(1.0, palette.accent);
        visuals.hyperlink_color = palette.accent;
        visuals.warn_fg_color = palette.warning;
        visuals.error_fg_color = palette.danger;
        visuals.window_corner_radius = card_rounding;
        visuals.menu_corner_radius = card_rounding;
        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.window_margin = egui::Margin::same(14);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.interact_size = egui::vec2(40.0, metrics.control_height);
        style.spacing.scroll.bar_width = 8.0;
        style.text_styles = [
            (TextStyle::Heading, FontId::new(metrics.heading_font, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(metrics.ui_font, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(metrics.ui_font, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(metrics.small_font, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(metrics.code_font, FontFamily::Monospace)),
        ]
        .into();
        ctx.set_style_of(egui_theme, style);
    }

    /// Use the user's editor font size for code surfaces (settings > editor font size).
    pub fn with_code_font(mut self, size: f32) -> Self {
        self.metrics.code_font = size.clamp(9.0, 24.0);
        self
    }

    /// Medium-weight Poppins font (500 weight).
    pub fn medium_font(&self, size: f32) -> FontId {
        let family = if self.custom_fonts_ready { FontFamily::Name("medium".into()) } else { FontFamily::Proportional };
        FontId::new(size, family)
    }

    /// SemiBold-weight Poppins font (600 weight, used for section headers and method labels).
    pub fn semibold_font(&self, size: f32) -> FontId {
        let family = if self.custom_fonts_ready { FontFamily::Name("semibold".into()) } else { FontFamily::Proportional };
        FontId::new(size, family)
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
                ("code_key", palette.code_key),
                ("code_string", palette.code_string),
                ("code_number", palette.code_number),
                ("code_keyword", palette.code_keyword),
                ("code_comment", palette.code_comment),
                ("code_variable", palette.code_variable),
                ("code_tag", palette.code_tag),
                ("code_attr", palette.code_attr),
            ] {
                let ratio = contrast(color, surface);
                assert!(ratio >= 4.5, "{name} on surface is {ratio:.2}:1 (needs 4.5:1)");
            }
            for (name, color, bg) in [
                ("muted on chrome", palette.text_muted, palette.chrome),
                ("muted on sidebar", palette.text_muted, palette.sidebar),
                ("on_accent on accent_fill", palette.on_accent, palette.accent_fill),
                ("on_accent on accent_fill_hover", palette.on_accent, palette.accent_fill_hover),
            ] {
                let ratio = contrast(color, bg);
                assert!(ratio >= 4.5, "{name} is {ratio:.2}:1 (needs 4.5:1)");
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

    #[test]
    fn both_modes_install_the_compact_sans_type_scale_and_rounded_controls() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let context = egui::Context::default();
            let theme = Theme::resolve(mode);
            theme.install(&context);
            let dark = mode == ThemeMode::Dark;
            let style = context.style_of(if dark { egui::Theme::Dark } else { egui::Theme::Light });
            for text_style in [TextStyle::Heading, TextStyle::Body, TextStyle::Button, TextStyle::Small] {
                let font = style.text_styles.get(&text_style).cloned().unwrap_or_else(|| panic!("{text_style:?} is installed"));
                assert_eq!(font.family, FontFamily::Proportional, "{text_style:?} is sans-serif in {mode:?}");
            }
            let body = style.text_styles.get(&TextStyle::Body).cloned().unwrap_or_else(|| panic!("body is installed"));
            assert!((body.size - 13.0).abs() < f32::EPSILON, "body is 13 px in {mode:?}");
            let mono = style.text_styles.get(&TextStyle::Monospace).cloned().unwrap_or_else(|| panic!("monospace is installed"));
            assert_eq!(mono.family, FontFamily::Monospace, "code stays monospace in {mode:?}");
            assert!((style.spacing.interact_size.y - theme.metrics.control_height).abs() < f32::EPSILON);
            assert!(style.visuals.widgets.inactive.corner_radius.nw > 0, "controls are rounded in {mode:?}");
            assert!(style.visuals.window_corner_radius.nw > 0, "windows are rounded in {mode:?}");
            assert_eq!(style.visuals.text_edit_bg_color, Some(theme.palette.input_bg));
            assert!(
                context.data(|data| data.get_temp::<u64>(egui::Id::new("postguy-poppins-fonts")).is_some()),
                "Poppins font definitions are installed in {mode:?}"
            );
        }
    }

    #[test]
    fn the_editor_font_size_overrides_the_code_scale_within_bounds() {
        let theme = Theme::resolve(ThemeMode::Dark).with_code_font(18.0);
        assert!((theme.metrics.code_font - 18.0).abs() < f32::EPSILON);
        assert!((theme.clone().with_code_font(1.0).metrics.code_font - 9.0).abs() < f32::EPSILON);
        assert!((theme.with_code_font(999.0).metrics.code_font - 24.0).abs() < f32::EPSILON);
    }
}
