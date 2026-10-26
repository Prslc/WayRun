// The same tables the core reads, so one locale covers both sides of the wire.
rust_i18n::i18n!("../locales", fallback = "en");

pub mod config;
pub mod geom;
pub mod icons;
pub mod present;
pub mod render;
pub mod text;
pub mod theme;
pub mod view;
