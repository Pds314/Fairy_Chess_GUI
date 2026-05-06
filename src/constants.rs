// src/constants.rs
use iced::Color as IcedColor;
use std::sync::LazyLock;

// Color constants
pub static LIGHT_SQUARE: LazyLock<IcedColor> =
    LazyLock::new(|| IcedColor::from_rgb8(238, 238, 210));
pub static DARK_SQUARE: LazyLock<IcedColor> = LazyLock::new(|| IcedColor::from_rgb8(118, 150, 86));
pub static HIGHLIGHT_COLOR: LazyLock<IcedColor> =
    LazyLock::new(|| IcedColor::from_rgba8(255, 255, 0, 0.4));
pub static PIECE_COLOR_WHITE: LazyLock<IcedColor> =
    LazyLock::new(|| IcedColor::from_rgb8(248, 248, 248));
pub static PIECE_COLOR_BLACK: LazyLock<IcedColor> =
    LazyLock::new(|| IcedColor::from_rgb8(80, 80, 80));

// Window size constants
pub const MAX_BOARD_SIZE: usize = 32; // Maximum supported board dimension
pub const MIN_BOARD_SIZE: usize = 3; // Minimum supported board dimension

// Update window constants
pub const DEFAULT_SQUARE_SIZE: f32 = 64.0;
pub const BOARD_PADDING: f32 = 64.0; // Space for coordinates
pub const CONTROL_PANEL_HEIGHT: f32 = 200.0; // Estimated height for controls
pub const STATUS_HEIGHT: f32 = 100.0; // Space for status and timer

// Calculate window size based on board
pub const WINDOW_WIDTH: f32 = DEFAULT_SQUARE_SIZE * 8.0 + BOARD_PADDING * 2.0 + 20.0;
pub const WINDOW_HEIGHT: f32 =
    DEFAULT_SQUARE_SIZE * 8.0 + BOARD_PADDING * 2.0 + CONTROL_PANEL_HEIGHT + STATUS_HEIGHT;

// Default board configuration
pub const DEFAULT_BOARD_SIZE: usize = 8; // Standard chess board
