//! Central color theme — one polished dark palette.
//!
//! Every view pulls colors from [`THEME`] instead of hardcoding
//! `ratatui::style::Color` values, so the palette is defined exactly once.
//! Semantics:
//!
//! - `bg` is the opaque base painted behind every frame; translucent
//!   terminal profiles (e.g. fish/zsh setups) must not bleed through.
//! - `fg`/`text`/`muted` form the three text tones (strong → regular →
//!   de-emphasized). Nothing else grays text.
//! - `accent` marks selection, focus, and interactive hints — nothing else.
//! - `ok`/`warn`/`err` are reserved for state with meaning (running/stopped,
//!   banners, errors), never for decoration.

use ratatui::style::Color;

/// The color palette of the active theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Opaque base background behind every view.
    pub bg: Color,
    /// Primary text: sandbox names, headers, strong values.
    pub fg: Color,
    /// Regular values: metrics, table rows, log lines.
    pub text: Color,
    /// De-emphasized text: field labels, image names, borders, hints.
    pub muted: Color,
    /// Selection, focus, and interactive elements.
    pub accent: Color,
    /// Running sandbox, success, stdout.
    pub ok: Color,
    /// Caution: paused states, runtime banner, pending confirmations.
    pub warn: Color,
    /// Errors, failed states, destructive actions.
    pub err: Color,
}

/// The active theme: a near-black background with a cyan accent. Truecolor
/// RGB is used deliberately — every modern terminal (including Windows
/// Terminal for PowerShell) supports it, and it keeps the background opaque
/// regardless of the terminal's own palette.
pub static THEME: Theme = Theme {
    bg: Color::Rgb(16, 16, 20),
    fg: Color::Rgb(226, 229, 239),
    text: Color::Rgb(178, 183, 198),
    muted: Color::Rgb(108, 113, 129),
    accent: Color::Rgb(103, 205, 250),
    ok: Color::Rgb(92, 222, 132),
    warn: Color::Rgb(255, 193, 88),
    err: Color::Rgb(245, 90, 102),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_background_is_opaque() {
        // `Reset` means "terminal default", which is translucent on many
        // setups — the base background must be a real color.
        assert_ne!(THEME.bg, Color::Reset);
    }
}
