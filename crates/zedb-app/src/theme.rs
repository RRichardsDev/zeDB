//! The palette. Semantic constants only; a real theme system can swap
//! these wholesale later. One-off decorative shades may stay local to
//! their view, but anything reused or meaningful belongs here.

// Surfaces.
pub const BG: u32 = 0x1e2227;
pub const BG_SIDEBAR: u32 = 0x23272e;
pub const BG_STATUS: u32 = 0x191c20;
pub const BG_SUNKEN: u32 = 0x191c21;
pub const BG_INSET: u32 = 0x1b1e23;
pub const ROW_STRIPE: u32 = 0x21252b;
pub const BORDER: u32 = 0x33383f;

// Text.
pub const TEXT: u32 = 0xaab2bd;
pub const TEXT_DIM: u32 = 0x6b7380;
pub const TEXT_BRIGHT: u32 = 0xffffff;

// Interaction states.
pub const HOVER: u32 = 0x303640;
pub const ROW_HOVER: u32 = 0x2a2f37;
pub const SELECTED: u32 = 0x2c3a4d;
pub const ACCENT: u32 = 0x6f8fac;
pub const PRIMARY: u32 = 0x2f6f9f;
pub const PRIMARY_HOVER: u32 = 0x3884bd;
pub const DISABLED: u32 = 0x454b55;
pub const DISABLED_BORDER: u32 = 0x22262c;

// Meaning.
pub const SUCCESS: u32 = 0x76a981;
pub const DANGER: u32 = 0xc4737b;
pub const DANGER_HOVER: u32 = 0x3d2528;
pub const WARNING: u32 = 0xd7a65f;
pub const ALERT: u32 = 0xe0806f;

// Toggle switches.
pub const TOGGLE_ON: u32 = 0x3f6650;
pub const TOGGLE_OFF: u32 = 0x343941;
pub const TOGGLE_KNOB_ON: u32 = 0x9ab7a1;
pub const TOGGLE_KNOB_OFF: u32 = 0x777e88;

// Results grid accents.
pub const SORT_INDICATOR: u32 = 0xc08a52;
pub const FILTER_BORDER: u32 = 0x6f5b99;
pub const FILTER_TINT: u32 = 0x9d84cc;
pub const DATE_TINT: u32 = 0xb56b6b;
