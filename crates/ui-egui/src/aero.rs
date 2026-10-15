//! Takery (Arch Quilly): the Aero look over FilmCraft's themes.
//!
//! The desktop's own greys (#1e1e1e panels on dark, #f6f6f6 on light) instead of Premiere's
//! near-black, rounder corners, and the desktop's accent colour read once from
//! `~/.cache/hypr-tema/cores` (written by `hypr-tema`). The desktop's "mono" accent becomes a
//! neutral graphite here, because FilmCraft draws white text on its accent.

use egui::Color32;
use std::sync::OnceLock;

use crate::theme::{ThemeKind, Tokens};

fn desktop_accent() -> Option<[u8; 3]> {
    static ACCENT: OnceLock<Option<[u8; 3]>> = OnceLock::new();
    *ACCENT.get_or_init(|| {
        let home = std::env::var_os("HOME")?;
        let text = std::fs::read_to_string(std::path::Path::new(&home).join(".cache/hypr-tema/cores")).ok()?;
        parse_hex(text.split_whitespace().nth(4)?)
    })
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let byte = |i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The accent and its hover shade; white text must read on both.
fn accent(dark: bool) -> (Color32, Color32) {
    match desktop_accent() {
        Some([r, g, b]) => {
            let c = Color32::from_rgb(r, g, b);
            // a very light accent would hide white text: darken it until it holds
            let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            let c = if lum > 150.0 { mix(c, Color32::BLACK, ((lum - 150.0) / 255.0 * 2.0).min(0.6)) } else { c };
            (c, mix(c, Color32::WHITE, 0.12))
        }
        None if dark => (Color32::from_gray(0x5c), Color32::from_gray(0x6a)),
        None => (Color32::from_gray(0x4a), Color32::from_gray(0x5a)),
    }
}

pub fn apply(mut t: Tokens) -> Tokens {
    let dark = t.kind != ThemeKind::Light;
    let (a, ah) = accent(dark);
    t.accent = a;
    t.accent_hover = ah;
    t.focus = if dark { mix(a, Color32::WHITE, 0.35) } else { a };
    t.hot_text = if dark { mix(a, Color32::WHITE, 0.45) } else { a };
    t.track_target = a;
    t.playhead = if dark { mix(a, Color32::WHITE, 0.45) } else { a };
    t.timecode = t.playhead;
    t.switch_fill = a;
    match t.kind {
        ThemeKind::Dark => {
            t.app_bg = Color32::from_gray(0x16);
            t.header_bg = Color32::from_gray(0x1e);
            t.panel_bg = Color32::from_gray(0x1e);
            t.tab_bg = Color32::from_gray(0x1e);
            t.text = Color32::from_gray(0xf0);
            t.text_dim = Color32::from_gray(0xc4);
            t.tab_text_active = Color32::from_gray(0xf0);
            t.icon_active = Color32::from_gray(0xf0);
        }
        ThemeKind::Medium => {}
        ThemeKind::Light => {
            t.header_bg = Color32::from_gray(0xf6);
            t.panel_bg = Color32::from_gray(0xf6);
            t.tab_bg = Color32::from_gray(0xf6);
            t.text = Color32::from_gray(0x1e);
        }
    }
    t.radius = t.radius.max(8.0);
    t.radius_sm = t.radius_sm.max(6.0);
    t
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex() {
        assert_eq!(super::parse_hex("3584e4"), Some([0x35, 0x84, 0xe4]));
        assert_eq!(super::parse_hex("mono"), None);
    }
}
