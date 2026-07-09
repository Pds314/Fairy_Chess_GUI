//! UI primitives shared across the `ui/` modules.
//!
//! `etext`/`rtext` wrap `iced::widget::text` with `Shaping::Advanced`, which
//! enables font fallback to system emoji fonts. Without it iced renders 🏆,
//! ⏱, 🤔, ⚙️, ➕ etc. as boxes/tofu.

use iced::widget::text::{IntoFragment, Shaping, Text};

/// Emoji-capable `text(...)`. Drop-in replacement for `iced::widget::text`.
pub fn etext<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    iced::widget::text(content).shaping(Shaping::Advanced)
}
