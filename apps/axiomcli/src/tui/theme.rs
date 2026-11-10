//! Desktop-aligned semantic colors, independent of terminal color capability.
use std::env;

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    Terminal,
}

impl Appearance {
    pub(super) fn from_environment() -> Self {
        env::var("AXIOMCLI_THEME")
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Terminal => "Terminal",
        }
    }

    pub(super) const fn terminal_background(self, mode: ColorMode) -> Option<&'static str> {
        match (self, mode) {
            (Self::Terminal, _) | (_, ColorMode::NoColor) => None,
            (Self::Light, ColorMode::Ansi16) => Some("#ffffff"),
            (Self::Dark, ColorMode::Ansi16) => Some("#000000"),
            (Self::Light, _) => Some("#f2f2f4"),
            (Self::Dark, _) => Some("#141416"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    TrueColor,
    Ansi256,
    Ansi16,
    NoColor,
}

impl ColorMode {
    pub(super) fn detect() -> Self {
        Self::detect_from(
            env::var("AXIOMCLI_COLOR").ok().as_deref(),
            env::var_os("NO_COLOR").is_some(),
            env::var("TERM").ok().as_deref(),
            env::var("COLORTERM").ok().as_deref(),
        )
    }

    pub(super) fn detect_from(
        explicit: Option<&str>,
        no_color: bool,
        term: Option<&str>,
        color_term: Option<&str>,
    ) -> Self {
        match explicit {
            Some("none" | "never") => return Self::NoColor,
            Some("16") => return Self::Ansi16,
            Some("256") => return Self::Ansi256,
            Some("truecolor" | "24bit") => return Self::TrueColor,
            Some(_) | None => {}
        }
        if no_color || term == Some("dumb") {
            Self::NoColor
        } else if color_term.is_some_and(|v| v.contains("truecolor") || v.contains("24bit")) {
            Self::TrueColor
        } else if term.is_some_and(|v| v.contains("256color")) {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Theme {
    pub brand: Color,
    pub selection_bg: Color,
    pub base: Color,
    pub surface: Color,
    pub border: Color,
    pub on_accent: Color,
    pub text: Color,
    pub muted: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}

impl Theme {
    pub(super) fn selection(self) -> Style {
        if self.selection_bg == Color::Reset {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().bg(self.selection_bg)
        }
    }
    #[cfg(test)]
    pub(super) fn for_mode(mode: ColorMode) -> Self {
        Self::for_appearance(mode, Appearance::Dark)
    }

    pub(super) fn for_appearance(mode: ColorMode, appearance: Appearance) -> Self {
        let light = appearance == Appearance::Light;
        let mut theme = if light {
            Self {
                // The logo retains brand orange; small text stays neutral for contrast.
                brand: Color::Rgb(255, 118, 83),
                selection_bg: Color::Rgb(220, 220, 226),
                base: Color::Rgb(242, 242, 244),
                surface: Color::Rgb(255, 255, 255),
                border: Color::Rgb(199, 199, 207),
                on_accent: Color::Rgb(255, 255, 255),
                text: Color::Rgb(16, 16, 20),
                muted: Color::Rgb(86, 86, 95),
                success: Color::Rgb(42, 120, 49),
                warning: Color::Rgb(146, 64, 14),
                error: Color::Rgb(185, 38, 44),
            }
        } else {
            Self {
                brand: Color::Rgb(255, 118, 83),
                selection_bg: Color::Rgb(55, 55, 62),
                base: Color::Rgb(20, 20, 22),
                surface: Color::Rgb(30, 30, 34),
                border: Color::Rgb(62, 62, 70),
                on_accent: Color::Rgb(20, 20, 22),
                text: Color::Rgb(244, 244, 246),
                muted: Color::Rgb(160, 160, 170),
                success: Color::Rgb(92, 184, 106),
                warning: Color::Rgb(240, 163, 58),
                error: Color::Rgb(240, 86, 91),
            }
        };
        match mode {
            ColorMode::TrueColor => {}
            ColorMode::Ansi256 => theme.map_colors(nearest_ansi256),
            ColorMode::Ansi16 => {
                theme = Self {
                    brand: Color::LightRed,
                    selection_bg: if light { Color::Gray } else { Color::DarkGray },
                    base: if light { Color::White } else { Color::Black },
                    surface: if light { Color::White } else { Color::Black },
                    border: Color::DarkGray,
                    on_accent: if light { Color::White } else { Color::Black },
                    text: if light { Color::Black } else { Color::White },
                    muted: if light { Color::DarkGray } else { Color::Gray },
                    success: if light {
                        Color::Green
                    } else {
                        Color::LightGreen
                    },
                    warning: Color::Yellow,
                    error: if light { Color::Red } else { Color::LightRed },
                };
            }
            ColorMode::NoColor => theme.map_colors(|_| Color::Reset),
        }
        if appearance == Appearance::Terminal {
            theme.base = Color::Reset;
            theme.surface = Color::Reset;
            theme.text = Color::Reset;
            theme.on_accent = Color::Reset;
            theme.selection_bg = Color::Reset;
            theme.muted = Color::Reset;
            theme.border = Color::Reset;
        }
        theme
    }

    fn map_colors(&mut self, map: impl Fn(Color) -> Color) {
        for color in [
            &mut self.brand,
            &mut self.selection_bg,
            &mut self.base,
            &mut self.surface,
            &mut self.border,
            &mut self.on_accent,
            &mut self.text,
            &mut self.muted,
            &mut self.success,
            &mut self.warning,
            &mut self.error,
        ] {
            *color = map(*color);
        }
    }
}

fn nearest_ansi256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let levels = [0_i32, 95, 135, 175, 215, 255];
    let nearest = (16_u8..=255)
        .min_by_key(|&index| {
            let rgb = if index >= 232 {
                [8 + 10 * i32::from(index - 232); 3]
            } else {
                let n = usize::from(index - 16);
                [levels[n / 36], levels[(n / 6) % 6], levels[n % 6]]
            };
            (i32::from(r) - rgb[0]).pow(2)
                + (i32::from(g) - rgb[1]).pow(2)
                + (i32::from(b) - rgb[2]).pow(2)
        })
        .unwrap_or(7);
    Color::Indexed(nearest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("expected true color")
        };
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    #[test]
    fn reading_text_is_legible_on_every_desktop_surface() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::for_appearance(ColorMode::TrueColor, appearance);
            for foreground in [theme.text, theme.muted] {
                for background in [theme.base, theme.surface, theme.selection_bg] {
                    let a = luminance(foreground);
                    let b = luminance(background);
                    let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(
                        contrast >= 4.5,
                        "{appearance:?}: {foreground:?} on {background:?}: {contrast}"
                    );
                }
            }
        }
    }

    #[test]
    fn terminal_and_no_color_modes_preserve_host_colors_and_visible_selection() {
        for mode in [
            ColorMode::TrueColor,
            ColorMode::Ansi256,
            ColorMode::Ansi16,
            ColorMode::NoColor,
        ] {
            let theme = Theme::for_appearance(mode, Appearance::Terminal);
            assert_eq!(theme.base, Color::Reset);
            assert_eq!(theme.text, Color::Reset);
            assert_eq!(Appearance::Terminal.terminal_background(mode), None);
            assert!(theme.selection().add_modifier.contains(Modifier::REVERSED));
        }
        for appearance in [Appearance::Dark, Appearance::Light, Appearance::Terminal] {
            let theme = Theme::for_appearance(ColorMode::NoColor, appearance);
            assert_eq!(theme.brand, Color::Reset);
            assert_eq!(theme.error, Color::Reset);
            assert_eq!(appearance.terminal_background(ColorMode::NoColor), None);
        }
    }
}
