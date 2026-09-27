//! `icons` -- the closed icon set. A widget may name an icon from this list; it may not supply SVG markup, which is the whole reason the module accepts a small schema and validates against it instead of accepting a view description.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

/// Icons are names from the static, hand-drawn frontend vocabulary. A schema
/// supplies a name only; it cannot supply SVG markup.
pub(super) const ALLOWED_ICONS: &[&str] = &[
    "calendar", "doc", "gauge", "journal", "memory", "rules", "watch",
];
