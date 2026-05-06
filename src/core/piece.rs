// src/core/piece.rs
use crate::piece_config::PieceConfigManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PieceColor {
    White,
    Black,
}

impl PieceColor {
    pub fn opposite(self) -> Self {
        match self {
            PieceColor::White => PieceColor::Black,
            PieceColor::Black => PieceColor::White,
        }
    }

    #[inline]
    pub fn index(self) -> usize {
        match self {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub color: PieceColor,
    pub piece_type: usize,
    pub move_count: u32,
    /// Cached copy of the R (royal) property. This piece is *individually*
    /// protected: it cannot be left in check, and capturing it is always
    /// checkmate. Maintained so Board/GameState never need a
    /// PieceConfigManager lookup in the hot path.
    pub is_royal: bool,
    /// Cached copy of the r (royalty) property. These pieces are
    /// *collectively* protected: losing the LAST one loses the game, but
    /// individual ones may be freely sacrificed while others remain.
    /// The last-remaining royalty piece dynamically inherits check
    /// protection (see GameState::is_in_check_fast).
    pub is_royalty: bool,
}

impl Piece {
    /// Create a piece with neither royal nor royalty flags set.
    /// Suitable for display-only pieces (promotion dialog, etc.) where
    /// the flags don't participate in any game logic.
    pub fn new(color: PieceColor, piece_type: usize) -> Self {
        Piece {
            color,
            piece_type,
            move_count: 0,
            is_royal: false,
            is_royalty: false,
        }
    }

    /// Create a piece with explicit royal and royalty flags.
    /// These should always be derived from the piece config's properties
    /// — callers typically pair this with a config lookup. The flags are
    /// cached on the piece so that Board's hot-path set_piece/move_piece
    /// can maintain royal_positions and royalty_positions without
    /// consulting the config manager.
    pub fn new_with_flags(
        color: PieceColor,
        piece_type: usize,
        is_royal: bool,
        is_royalty: bool,
    ) -> Self {
        Piece {
            color,
            piece_type,
            move_count: 0,
            is_royal,
            is_royalty,
        }
    }

    pub fn to_char(&self, config_manager: &PieceConfigManager) -> char {
        if let Some(config) = config_manager.get_piece_by_index(self.piece_type) {
            if let Some(first_char) = config.characters.first() {
                let ch = first_char.chars().next().unwrap_or('?');
                match self.color {
                    PieceColor::White => ch.to_ascii_uppercase(),
                    PieceColor::Black => ch.to_ascii_lowercase(),
                }
            } else {
                '?'
            }
        } else {
            '?'
        }
    }

    pub fn to_filename(&self, config_manager: &PieceConfigManager) -> String {
        if let Some(config) = config_manager.get_piece_by_index(self.piece_type) {
            let color_str = match self.color {
                PieceColor::White => "white",
                PieceColor::Black => "black",
            };
            for texture_name in &config.texture_names {
                return format!("{}_{}.png", color_str, texture_name);
            }
        }
        "unknown.png".to_string()
    }
}
