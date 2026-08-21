//! Fixed responsive surface policy: breakpoints, fit, and fallback order.
//!
//! Breakpoints are implementation-owned product behavior, not settings. A
//! forced preference is a preference, not permission to clip: when it does
//! not fit, the richest fitting surface wins. Smaller surfaces omit whole
//! semantic segments; nothing is clipped mid-segment.

use ratatui::layout::Rect;

use crate::config::ModePreference;

/// One approved responsive surface, richest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    /// Full selected-GPU instrument cluster.
    Mode,
    /// Header, strip, panels, support, and health without the logo block.
    Compact,
    /// The two graphs only.
    Mini,
    /// One status line.
    Tiny,
}

impl Surface {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Compact => "compact",
            Self::Mini => "mini",
            Self::Tiny => "tiny",
        }
    }

    /// Exact approved breakpoints.
    pub(super) fn fits(self, area: Rect) -> bool {
        match self {
            Self::Mode => area.width >= 72 && area.height >= 34,
            Self::Compact => area.width >= 62 && area.height >= 17,
            Self::Mini => area.width >= 48 && area.height >= 11,
            Self::Tiny => true,
        }
    }
}

/// The richest approved surface that fits `area`.
pub(super) fn automatic(area: Rect) -> Surface {
    [Surface::Mode, Surface::Compact, Surface::Mini]
        .into_iter()
        .find(|surface| surface.fits(area))
        .unwrap_or(Surface::Tiny)
}

/// The surface actually rendered: the preference when it fits, otherwise the
/// richest fitting surface. A later resize restores the preference.
pub(super) fn effective(preference: ModePreference, area: Rect) -> Surface {
    let forced = match preference {
        ModePreference::Auto => return automatic(area),
        ModePreference::Mode => Surface::Mode,
        ModePreference::Compact => Surface::Compact,
        ModePreference::Mini => Surface::Mini,
        ModePreference::Tiny => Surface::Tiny,
    };
    if forced.fits(area) {
        forced
    } else {
        automatic(area)
    }
}

/// A `width`×`height` rectangle centered inside `area`, clamped to it.
pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}
