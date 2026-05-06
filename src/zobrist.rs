// src/zobrist.rs
use crate::constants::MAX_BOARD_SIZE;
use crate::core::piece::PieceColor;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::LazyLock;

pub const MAX_PIECE_TYPES: usize = 32;

// Generate Zobrist keys for a maximum board size
// Actual board can be smaller, we'll just use a subset
pub static ZOBRIST_PIECES: LazyLock<
    [[[u64; MAX_BOARD_SIZE]; MAX_BOARD_SIZE]; MAX_PIECE_TYPES * 2],
> = LazyLock::new(|| {
    let seed: [u8; 32] = [
        0xad, 0x7e, 0x31, 0x8a, 0xe8, 0x12, 0x76, 0xe4, 0x90, 0x5d, 0x38, 0x3f, 0x59, 0xec, 0x15,
        0xb5, 0x63, 0x6b, 0xfb, 0x32, 0xee, 0xe1, 0x6f, 0xa1, 0x86, 0x3f, 0x08, 0xf2, 0x0c, 0x8d,
        0x7d, 0xa2,
    ];
    let mut rng = StdRng::from_seed(seed);

    let mut keys = [[[0; MAX_BOARD_SIZE]; MAX_BOARD_SIZE]; MAX_PIECE_TYPES * 2];
    for piece_index in 0..(MAX_PIECE_TYPES * 2) {
        for row in 0..MAX_BOARD_SIZE {
            for col in 0..MAX_BOARD_SIZE {
                keys[piece_index][row][col] = rng.random::<u64>();
            }
        }
    }
    keys
});

pub static ZOBRIST_TURN: LazyLock<[u64; 2]> = LazyLock::new(|| {
    let seed: [u8; 32] = [
        0x79, 0xb9, 0x7f, 0xb7, 0xac, 0xfe, 0x2f, 0x79, 0x16, 0x85, 0x19, 0x65, 0x46, 0x1b, 0x44,
        0x4a, 0x1f, 0x63, 0x51, 0x21, 0x1d, 0xec, 0xfd, 0x59, 0xc1, 0x6d, 0x1e, 0x44, 0xba, 0xb4,
        0xb2, 0x22,
    ];
    let mut rng = StdRng::from_seed(seed);

    [rng.random::<u64>(), rng.random::<u64>()]
});

/// Get the Zobrist index for a piece
#[inline]
pub fn get_zobrist_piece_index(piece_color: PieceColor, piece_type: usize) -> usize {
    let color_offset = match piece_color {
        PieceColor::White => 0,
        PieceColor::Black => MAX_PIECE_TYPES,
    };
    color_offset + piece_type.min(MAX_PIECE_TYPES - 1)
}

/// Get the Zobrist index for the turn
#[inline]
pub fn get_zobrist_turn_index(turn: PieceColor) -> usize {
    match turn {
        PieceColor::White => 0,
        PieceColor::Black => 1,
    }
}

/// Zobrist key for a piece of `color` and `type` standing on `pos`.
/// Returns 0 for positions outside the precomputed table (boards > MAX_BOARD_SIZE).
/// Since XOR with 0 is identity, oversized boards gracefully degrade rather than panic.
#[inline(always)]
pub fn piece_square_key(color: PieceColor, piece_type: usize, pos: (usize, usize)) -> u64 {
    if pos.0 >= MAX_BOARD_SIZE || pos.1 >= MAX_BOARD_SIZE {
        return 0;
    }
    let idx = get_zobrist_piece_index(color, piece_type);
    ZOBRIST_PIECES[idx][pos.0][pos.1]
}

/// Zobrist key for side-to-move.
#[inline(always)]
pub fn turn_key(turn: PieceColor) -> u64 {
    ZOBRIST_TURN[get_zobrist_turn_index(turn)]
}

/// Calculate Zobrist hash for a position
/// This is a helper that could be used by the Board
pub fn calculate_position_hash(
    pieces: &[(crate::core::position::Position, crate::core::piece::Piece)],
    board_size: (usize, usize),
) -> u64 {
    let mut hash = 0u64;

    for (pos, piece) in pieces {
        if pos.0 < MAX_BOARD_SIZE.min(board_size.0) && pos.1 < MAX_BOARD_SIZE.min(board_size.1) {
            let piece_idx = get_zobrist_piece_index(piece.color, piece.piece_type);
            hash ^= ZOBRIST_PIECES[piece_idx][pos.0][pos.1];
        }
    }

    hash
}
