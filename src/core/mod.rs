// src/core/mod.rs
pub mod board;
pub mod game_state;
pub mod piece;
pub mod position;

pub use board::{Board, EnPassantTarget};
pub use game_state::{
    DrawReason, GameResult, GameState, MoveAttemptResult, PendingMove, PerformanceTracker,
};
pub use piece::{Piece, PieceColor};
pub use position::Position;
