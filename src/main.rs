// src/main.rs

use fairy_chess_gui::app::ChessGui;
use fairy_chess_gui::constants::{WINDOW_HEIGHT, WINDOW_WIDTH};
use iced::Settings;

pub fn main() -> iced::Result {
    use iced::window;

    iced::application("Fairy Chess GUI", ChessGui::update, ChessGui::view)
    .theme(ChessGui::theme)
    .subscription(ChessGui::subscription)
    .window(window::Settings {
        size: iced::Size::new(WINDOW_WIDTH * 1.5, WINDOW_HEIGHT * 1.2),
            min_size: Some(iced::Size::new(800.0, 600.0)),
            resizable: true,
            decorations: true,
            ..Default::default()
    })
    .settings(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .run()
}
