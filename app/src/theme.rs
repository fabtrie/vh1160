//! Colours of the Manson app (`app/src/style.css`), light and dark.

use gpui::{App, Global, Hsla, rgb, rgba};
use serde::{Deserialize, Serialize};

pub const UI_FONT: &str = "Segoe UI";
pub const MONO_FONT: &str = "Share Tech Mono";
pub const RADIUS: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub bg1: Hsla,
    pub bd: Hsla,
    pub tx: Hsla,
    pub tx2: Hsla,
    pub tx3: Hsla,
    pub input: Hsla,
    pub hover: Hsla,
    pub lcd: Hsla,
    pub seg: Hsla,
    pub grid: Hsla,
    pub axis: Hsla,
    pub led_off: Hsla,
    pub ok_bg: Hsla,
    pub ok_tx: Hsla,
    pub err_bg: Hsla,
    pub err_tx: Hsla,
    pub warn_bg: Hsla,
    pub warn_tx: Hsla,
    pub go: Hsla,
    pub go_hover: Hsla,
    pub stop: Hsla,
    pub stop_hover: Hsla,
    pub on: Hsla,
    pub fault: Hsla,
    pub din: Hsla,
    pub series_v: Hsla,
    pub series_v2: Hsla,
    pub series_i: Hsla,
}

impl Global for Palette {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

impl Palette {
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: c(0xffffff),
            bg1: c(0xf6f5f1),
            bd: ca(0x0b0b0b1f),
            tx: c(0x0b0b0b),
            tx2: c(0x52514e),
            tx3: c(0x898781),
            input: c(0xffffff),
            hover: ca(0x0b0b0b0d),
            lcd: c(0xb9d2e8),
            seg: c(0x0c2a44),
            grid: c(0xe1e0d9),
            axis: c(0x898781),
            led_off: c(0xc3c2b7),
            ok_bg: c(0xe6f4ea),
            ok_tx: c(0x0f6e3a),
            err_bg: c(0xfcebeb),
            err_tx: c(0xa32d2d),
            warn_bg: c(0xfdf3dc),
            warn_tx: c(0x8a5a00),
            go: c(0x1f8a4c),
            go_hover: c(0x1a7a43),
            stop: c(0xc93a3a),
            stop_hover: c(0xb33232),
            on: c(0x1d9e75),
            fault: c(0xe24b4a),
            din: c(0x2a78d6),
            series_v: c(0x2a78d6),
            series_v2: c(0x7a5bd0),
            series_i: c(0xeb6834),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: c(0x1f1f1e),
            bg1: c(0x171716),
            bd: ca(0xffffff1f),
            tx: c(0xf0efec),
            tx2: c(0xc3c2b7),
            tx3: c(0x898781),
            input: c(0x2a2a28),
            hover: ca(0xffffff0f),
            lcd: c(0x061524),
            seg: c(0x6fc3ff),
            grid: c(0x2c2c2a),
            axis: c(0x898781),
            led_off: c(0x3a3a37),
            ok_bg: c(0x0f2e1c),
            ok_tx: c(0x7fd8a4),
            err_bg: c(0x3a1414),
            err_tx: c(0xf09595),
            warn_bg: c(0x3a2a0c),
            warn_tx: c(0xf0c36a),
            go: c(0x22a05a),
            go_hover: c(0x26b365),
            stop: c(0xd94848),
            stop_hover: c(0xe35a5a),
            on: c(0x1d9e75),
            fault: c(0xe24b4a),
            din: c(0x3987e5),
            series_v: c(0x3987e5),
            series_v2: c(0x9b7fe8),
            series_i: c(0xd95926),
        }
    }
}

/// Makes `palette` current, for our widgets and gpui-component's text inputs.
pub fn apply(palette: Palette, cx: &mut App) {
    use gpui_component::theme::{Theme, ThemeMode};
    let mode = if palette.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, None, cx);
    let t = Theme::global_mut(cx);
    t.font_family = UI_FONT.into();
    t.font_size = gpui::px(14.0);
    t.mono_font_family = MONO_FONT.into();
    t.radius = gpui::px(RADIUS);
    let k = &mut t.colors;
    k.background = palette.input;
    k.foreground = palette.tx;
    k.border = palette.bd;
    k.input = palette.bd;
    k.ring = palette.series_v;
    k.caret = palette.tx;
    k.selection = palette.series_v.opacity(0.3);
    k.muted_foreground = palette.tx3;
    k.popover = palette.bg;
    k.popover_foreground = palette.tx;
    k.accent = palette.hover;
    k.accent_foreground = palette.tx;
    cx.set_global(palette);
}
