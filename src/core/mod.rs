// src/core/mod.rs
pub mod board;
pub mod chain;
pub mod game_state;
pub mod game_types;
pub mod ghost;
pub mod perft;
pub mod piece;
pub mod position;

mod check_detection;
mod gui_moves;
mod move_exec;
mod pseudo_legal;

pub use board::Board;
pub use chain::{BoardEvent, EventKind, MoveChain};
pub use game_state::GameState;
pub use game_types::*;
pub use ghost::{Ghost, GhostFlags};
pub use perft::{BenchResult, PerftStats};
pub use piece::{Piece, PieceColor};
pub use position::Position;
