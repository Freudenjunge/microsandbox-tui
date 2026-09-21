//! Central color theme — one polished dark palette.
//!
//! Every view pulls colors from [`THEME`] instead of hardcoding
//! `ratatui::style::Color` values, so the palette is defined exactly once.
//! Palette derived from the Stitch mockups
//! (`design/stitch_microsandbox_tui_design_system/`): a GitHub-Dark base with
//! neon-cyberpunk accents. Semantics:
//!
//! - `bg` is the opaque base painted behind every frame; translucent
//!   terminal profiles (e.g. fish/zsh setups) must not bleed through.
//! - `fg`/`text`/`muted` form the three text tones (strong → regular →
//!   de-emphasized). Nothing else grays text.
//! - `accent` marks selection, focus, and interactive hints — nothing else.
//! - `ok`/`warn`/`err` are reserved for state with meaning (running/stopped,
//!   banners, errors), never for decoration.
//! - `border`/`panel`/`selection` are the structural tones: card and panel
//!   borders, raised panel backgrounds, and row/selection highlights.

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
    /// Card, panel, and table borders.
    pub border: Color,
    /// Raised panel / card background on top of `bg`.
    pub panel: Color,
    /// Focused table row / selection highlight background.
    pub selection: Color,
}

/// The active theme: GitHub-Dark base (`#0d1117`) with a neon-cyan accent
/// (`#00f0ff`) after the Stitch design system. Truecolor RGB is used
/// deliberately — every modern terminal (including Windows Terminal for
/// PowerShell) supports it, and it keeps the background opaque regardless of
/// the terminal's own palette.
pub static THEME: Theme = Theme {
    bg: Color::Rgb(13, 17, 23),        // #0d1117
    fg: Color::Rgb(240, 246, 252),     // #f0f6fc
    text: Color::Rgb(201, 209, 217),   // #c9d1d9
    muted: Color::Rgb(110, 118, 129),  // #6e7681
    accent: Color::Rgb(0, 240, 255),   // #00f0ff
    ok: Color::Rgb(0, 255, 136),       // #00ff88
    warn: Color::Rgb(255, 230, 0),     // #ffe600
    err: Color::Rgb(255, 42, 109),     // #ff2a6d
    border: Color::Rgb(48, 54, 61),    // #30363d
    panel: Color::Rgb(22, 27, 34),     // #161b22
    selection: Color::Rgb(15, 41, 55), // #0f2937
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

    #[test]
    fn theme_roles_are_distinct() {
        // Structural tones must not collide with each other or with the
        // semantic colors — a card border that renders like an error would
        // be a bug.
        let values = [
            THEME.bg,
            THEME.fg,
            THEME.text,
            THEME.muted,
            THEME.accent,
            THEME.ok,
            THEME.warn,
            THEME.err,
            THEME.border,
            THEME.panel,
            THEME.selection,
        ];
        for (i, a) in values.iter().enumerate() {
            for b in values.iter().skip(i + 1) {
                assert_ne!(a, b, "theme roles must be distinct: {a:?} == {b:?}");
            }
        }
    }
}
