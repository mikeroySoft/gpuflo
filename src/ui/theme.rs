//! Semantic palette roles and the built-in theme mappings.
//!
//! A theme maps roles — background, foreground, muted, dim, accent, warning,
//! fault, border, graph — to terminal colors. It never changes wording,
//! layout content, health priority, or unavailable-state semantics. With
//! color disabled every style collapses to the terminal default and required
//! distinctions survive textually.

use ratatui::style::{Color, Modifier, Style};

use crate::config::Theme;

/// Resolved semantic role colors for one theme.
#[derive(Debug, Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: Color,
    pub(super) fg: Color,
    pub(super) muted: Color,
    pub(super) dim: Color,
    pub(super) accent: Color,
    pub(super) warning: Color,
    pub(super) fault: Color,
    pub(super) border: Color,
    pub(super) graph: Color,
}

/// Role colors for one built-in theme, with an ANSI fallback when terminal
/// environment evidence does not advertise truecolor.
pub(super) fn palette(theme: Theme, truecolor: bool) -> Palette {
    if !truecolor {
        return match theme {
            Theme::Buffalo => Palette {
                bg: Color::Black,
                fg: Color::LightYellow,
                muted: Color::DarkGray,
                dim: Color::DarkGray,
                accent: Color::Yellow,
                warning: Color::LightRed,
                fault: Color::Red,
                border: Color::DarkGray,
                graph: Color::Yellow,
            },
            Theme::Nord => Palette {
                bg: Color::Black,
                fg: Color::White,
                muted: Color::Cyan,
                dim: Color::DarkGray,
                accent: Color::LightCyan,
                warning: Color::Yellow,
                fault: Color::Red,
                border: Color::Blue,
                graph: Color::Cyan,
            },
            Theme::Monochrome => Palette {
                bg: Color::Black,
                fg: Color::White,
                muted: Color::Gray,
                dim: Color::DarkGray,
                accent: Color::White,
                warning: Color::Gray,
                fault: Color::White,
                border: Color::DarkGray,
                graph: Color::Gray,
            },
        };
    }
    match theme {
        Theme::Buffalo => Palette {
            bg: Color::Rgb(20, 20, 19),
            fg: Color::Rgb(244, 225, 192),
            muted: Color::Rgb(166, 146, 120),
            dim: Color::Rgb(102, 89, 74),
            accent: Color::Rgb(245, 158, 11),
            warning: Color::Rgb(190, 76, 37),
            fault: Color::Rgb(239, 68, 68),
            border: Color::Rgb(114, 91, 64),
            graph: Color::Rgb(245, 158, 11),
        },
        Theme::Nord => Palette {
            bg: Color::Rgb(46, 52, 64),
            fg: Color::Rgb(236, 239, 244),
            muted: Color::Rgb(129, 161, 193),
            dim: Color::Rgb(76, 86, 106),
            accent: Color::Rgb(136, 192, 208),
            warning: Color::Rgb(235, 203, 139),
            fault: Color::Rgb(191, 97, 106),
            border: Color::Rgb(76, 86, 106),
            graph: Color::Rgb(136, 192, 208),
        },
        Theme::Monochrome => Palette {
            bg: Color::Rgb(16, 16, 16),
            fg: Color::Rgb(232, 232, 232),
            muted: Color::Rgb(168, 168, 168),
            dim: Color::Rgb(104, 104, 104),
            accent: Color::Rgb(255, 255, 255),
            warning: Color::Rgb(200, 200, 200),
            fault: Color::Rgb(255, 255, 255),
            border: Color::Rgb(120, 120, 120),
            graph: Color::Rgb(208, 208, 208),
        },
    }
}

/// Style factory honoring color disablement: with color off every produced
/// style is `Style::default()` — no foreground, background, or emphasis.
#[derive(Debug, Clone, Copy)]
pub(super) struct Styler {
    pub(super) palette: Palette,
    color: bool,
}

impl Styler {
    pub(super) fn new(theme: Theme, color_enabled: bool, truecolor: bool) -> Self {
        Self {
            palette: palette(theme, truecolor),
            color: color_enabled,
        }
    }

    /// Foreground style in the given role color.
    pub(super) fn fg(&self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    /// Bold foreground style in the given role color.
    pub(super) fn bold(&self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }
    }

    /// Whole-frame background fill.
    pub(super) fn background(&self) -> Style {
        if self.color {
            Style::default().bg(self.palette.bg)
        } else {
            Style::default()
        }
    }

    /// Inverted highlight for the selected GPU strip label. Selection never
    /// depends on this alone: the label always carries a `▶` marker.
    pub(super) fn selected(&self) -> Style {
        if self.color {
            Style::default()
                .fg(self.palette.bg)
                .bg(self.palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }
    }
}
