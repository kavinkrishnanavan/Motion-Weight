//! Palette and metrics, carried over unchanged from the CSS build so the native
//! app is visually identical to what it replaces.

use super::paint::{rgb, Color};

pub const BG_APP: Color = rgb(0x0c0c0e);
pub const BG_PANEL: Color = rgb(0x161619);
pub const BG_PANEL_2: Color = rgb(0x1c1c20);
pub const BG_ELEV: Color = rgb(0x232328);
pub const BG_HOVER: Color = rgb(0x2b2b31);
pub const BORDER: Color = rgb(0x26262b);
pub const BORDER_SOFT: Color = rgb(0x202024);

pub const TEXT: Color = rgb(0xeceef2);
pub const TEXT_2: Color = rgb(0x9c9ca6);
pub const TEXT_3: Color = rgb(0x64646e);

pub const ACCENT: Color = rgb(0x3d7eff);
pub const ACCENT_HI: Color = rgb(0x5590ff);
pub const DANGER: Color = rgb(0xec5b53);
pub const WARN: Color = rgb(0xffb020);

pub const CLIP_VIDEO: Color = rgb(0x2c3e63);
pub const CLIP_IMAGE: Color = rgb(0x2a4a44);
pub const CLIP_AUDIO: Color = rgb(0x1f4a3a);
pub const CLIP_TEXT: Color = rgb(0x46356b);

pub const R_SM: f32 = 4.0;
pub const R_MD: f32 = 6.0;
pub const R_LG: f32 = 10.0;

pub const FS_SMALL: f32 = 11.5;
pub const FS_BODY: f32 = 13.0;
pub const FS_TITLE: f32 = 15.0;
pub const FS_BRAND: f32 = 18.0;

/// Height of a standard control row.
pub const ROW_H: f32 = 26.0;
pub const FIELD_H: f32 = 28.0;
pub const TOPBAR_H: f32 = 46.0;
pub const RAIL_W: f32 = 68.0;
/// Default sizes for the three resizable splits — the starting point for
/// `App::library_w`/`inspector_w`/`timeline_h`, which the user can then drag.
pub const LIBRARY_W: f32 = 250.0;
pub const INSPECTOR_W: f32 = 268.0;
pub const TIMELINE_H: f32 = 300.0;
pub const LIBRARY_W_MIN: f32 = 180.0;
pub const INSPECTOR_W_MIN: f32 = 220.0;
pub const TIMELINE_H_MIN: f32 = 140.0;
/// Just tall enough to grab for the tear-off drag — these headers carry no
/// title text any more, so there's nothing else to give them height for.
pub const PANEL_HEAD_H: f32 = 14.0;
/// Breathing room between the four detachable panes (library, player,
/// inspector, timeline) and the window edge around them.
pub const PANEL_GAP: f32 = 10.0;

pub fn clip_color(kind: crate::model::ClipKind) -> Color {
    use crate::model::ClipKind::*;
    match kind {
        Video => CLIP_VIDEO,
        Image => CLIP_IMAGE,
        Audio => CLIP_AUDIO,
        Text => CLIP_TEXT,
    }
}
