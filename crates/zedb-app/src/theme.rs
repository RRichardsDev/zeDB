//! The palette, resolved from the gpui-component theme system (the
//! Zed-style Global `Theme` with JSON theme configs).
//!
//! Accessors read a resolved snapshot instead of `cx.theme()` because
//! most call sites are style closures with no `cx`. [`apply`] rebuilds
//! the snapshot from the active theme; it runs at startup and on every
//! theme change, immediately before the window repaints.

use std::sync::{OnceLock, RwLock};

use gpui::{rgb, App, Hsla};
use gpui_component::Theme;

/// Every color the zeDB chrome paints with.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    pub bg_sidebar: Hsla,
    pub bg_status: Hsla,
    pub bg_sunken: Hsla,
    pub bg_inset: Hsla,
    pub row_stripe: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_dim: Hsla,
    pub text_bright: Hsla,
    pub hover: Hsla,
    pub row_hover: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub disabled: Hsla,
    pub disabled_border: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub warning: Hsla,
    pub alert: Hsla,
    pub toggle_on: Hsla,
    pub toggle_off: Hsla,
    pub toggle_knob_on: Hsla,
    pub toggle_knob_off: Hsla,
    pub sort_indicator: Hsla,
    pub filter_border: Hsla,
    pub filter_tint: Hsla,
    pub date_tint: Hsla,
}

/// zeDB-specific shades with no gpui-component token, per mode.
struct Accents {
    bg_status: u32,
    bg_sunken: u32,
    bg_inset: u32,
    text_bright: u32,
    disabled: u32,
    disabled_border: u32,
    danger_hover: u32,
    alert: u32,
    toggle_on: u32,
    toggle_off: u32,
    toggle_knob_on: u32,
    toggle_knob_off: u32,
    sort_indicator: u32,
    filter_border: u32,
    filter_tint: u32,
    date_tint: u32,
}

/// Today's dark values, unchanged.
const DARK: Accents = Accents {
    bg_status: 0x191c20,
    bg_sunken: 0x191c21,
    bg_inset: 0x1b1e23,
    text_bright: 0xffffff,
    disabled: 0x454b55,
    disabled_border: 0x22262c,
    danger_hover: 0x3d2528,
    alert: 0xe0806f,
    toggle_on: 0x3f6650,
    toggle_off: 0x343941,
    toggle_knob_on: 0x9ab7a1,
    toggle_knob_off: 0x777e88,
    sort_indicator: 0xc08a52,
    filter_border: 0x6f5b99,
    filter_tint: 0x9d84cc,
    date_tint: 0xb56b6b,
};

/// First-draft light values; expect soak-mode tuning.
const LIGHT: Accents = Accents {
    bg_status: 0xe8eaed,
    bg_sunken: 0xe4e6ea,
    bg_inset: 0xeceef1,
    text_bright: 0x14161a,
    disabled: 0xb0b6bf,
    disabled_border: 0xdadde2,
    danger_hover: 0xf3dcdc,
    alert: 0xc25f4e,
    toggle_on: 0x9ac8a5,
    toggle_off: 0xd4d8de,
    toggle_knob_on: 0x4d7a5c,
    toggle_knob_off: 0x8b929c,
    sort_indicator: 0xa96f2e,
    filter_border: 0x8a76b8,
    filter_tint: 0x6d51a1,
    date_tint: 0xa14f4f,
};

impl Palette {
    /// Pre-[`apply`] fallback: the dark chrome exactly as shipped.
    fn fallback_dark() -> Palette {
        let a = &DARK;
        Palette {
            bg: rgb(0x1e2227).into(),
            bg_sidebar: rgb(0x23272e).into(),
            bg_status: rgb(a.bg_status).into(),
            bg_sunken: rgb(a.bg_sunken).into(),
            bg_inset: rgb(a.bg_inset).into(),
            row_stripe: rgb(0x21252b).into(),
            border: rgb(0x33383f).into(),
            text: rgb(0xaab2bd).into(),
            text_dim: rgb(0x6b7380).into(),
            text_bright: rgb(a.text_bright).into(),
            hover: rgb(0x303640).into(),
            row_hover: rgb(0x2a2f37).into(),
            selected: rgb(0x2c3a4d).into(),
            accent: rgb(0x6f8fac).into(),
            primary: rgb(0x2f6f9f).into(),
            primary_hover: rgb(0x3884bd).into(),
            disabled: rgb(a.disabled).into(),
            disabled_border: rgb(a.disabled_border).into(),
            success: rgb(0x76a981).into(),
            danger: rgb(0xc4737b).into(),
            danger_hover: rgb(a.danger_hover).into(),
            warning: rgb(0xd7a65f).into(),
            alert: rgb(a.alert).into(),
            toggle_on: rgb(a.toggle_on).into(),
            toggle_off: rgb(a.toggle_off).into(),
            toggle_knob_on: rgb(a.toggle_knob_on).into(),
            toggle_knob_off: rgb(a.toggle_knob_off).into(),
            sort_indicator: rgb(a.sort_indicator).into(),
            filter_border: rgb(a.filter_border).into(),
            filter_tint: rgb(a.filter_tint).into(),
            date_tint: rgb(a.date_tint).into(),
        }
    }
}

fn palette_lock() -> &'static RwLock<Palette> {
    static PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();
    PALETTE.get_or_init(|| RwLock::new(Palette::fallback_dark()))
}

fn get(read: impl Fn(&Palette) -> Hsla) -> Hsla {
    read(&palette_lock().read().expect("palette lock"))
}

/// Rebuild the palette snapshot from the active gpui-component theme.
/// Call after every `Theme::change` and once at startup.
pub fn apply(cx: &mut App) {
    let theme = Theme::global(cx);
    let accents = if theme.is_dark() { &DARK } else { &LIGHT };
    let palette = Palette {
        bg: theme.colors.background,
        bg_sidebar: theme.colors.sidebar,
        bg_status: rgb(accents.bg_status).into(),
        bg_sunken: rgb(accents.bg_sunken).into(),
        bg_inset: rgb(accents.bg_inset).into(),
        row_stripe: theme.colors.list_even,
        border: theme.colors.border,
        text: theme.colors.foreground,
        text_dim: theme.colors.muted_foreground,
        text_bright: rgb(accents.text_bright).into(),
        hover: theme.colors.accent,
        row_hover: theme.colors.list_hover,
        selected: theme.colors.list_active,
        accent: theme.colors.ring,
        primary: theme.colors.primary,
        primary_hover: theme.colors.primary_hover,
        disabled: rgb(accents.disabled).into(),
        disabled_border: rgb(accents.disabled_border).into(),
        success: theme.colors.success,
        danger: theme.colors.danger,
        danger_hover: rgb(accents.danger_hover).into(),
        warning: theme.colors.warning,
        alert: rgb(accents.alert).into(),
        toggle_on: rgb(accents.toggle_on).into(),
        toggle_off: rgb(accents.toggle_off).into(),
        toggle_knob_on: rgb(accents.toggle_knob_on).into(),
        toggle_knob_off: rgb(accents.toggle_knob_off).into(),
        sort_indicator: rgb(accents.sort_indicator).into(),
        filter_border: rgb(accents.filter_border).into(),
        filter_tint: rgb(accents.filter_tint).into(),
        date_tint: rgb(accents.date_tint).into(),
    };
    *palette_lock().write().expect("palette lock") = palette;
}

macro_rules! accessors {
    ($($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name() -> Hsla {
                get(|palette| palette.$name)
            }
        )*
    };
}

accessors!(
    bg,
    bg_sidebar,
    bg_status,
    bg_sunken,
    bg_inset,
    row_stripe,
    border,
    text,
    text_dim,
    text_bright,
    hover,
    row_hover,
    selected,
    accent,
    primary,
    primary_hover,
    disabled,
    disabled_border,
    success,
    danger,
    danger_hover,
    warning,
    alert,
    toggle_on,
    toggle_off,
    toggle_knob_on,
    toggle_knob_off,
    sort_indicator,
    filter_border,
    filter_tint,
    date_tint,
);
