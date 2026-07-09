//! Shared types used by game state, engines, and the GUI.

use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::move_generator::{CastlingOption, MoveWithPath};
use std::cell::Cell;

// ─── Move representation ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExpandedMove {
    pub from: Position,
    pub to: Position,
    pub move_with_path: MoveWithPath,
    pub castling_option: Option<CastlingOption>,
    pub promotion_target: Option<usize>,
    pub captures: Option<Piece>,
    pub captures_position: Option<Position>,
}

#[derive(Debug)]
pub enum MoveGenerationResult {
    Moves(Vec<ExpandedMove>),
    Checkmate { move_that_captures_royal: ExpandedMove },
}

// ─── Move history record ────────────────────────────────────────────────

const F_CAPTURED: u8 = 1 << 0;
const F_EP_CAPTURE: u8 = 1 << 1;
const F_CASTLING: u8 = 1 << 2;
const NO_PIECE: u16 = u16::MAX;

/// One frame of the event tape.
///
/// **64 bytes, `Copy`.** The previous version was ~230 bytes, `!Copy`, and
/// carried three `Option<Piece>`s plus a `SmallVec<[(Position,Piece);1]>` —
/// none of which `undo` ever read. Everything it *did* need is a scalar.
///
/// `move_history.push()` is now a 64-byte store with no drop glue, and
/// `pop()` is free. Engines that clone `GameState` (Diffusion,
/// ProbabilisticSearch) copy the history with a `memcpy`.
///
/// PGN and redo need a handful of derived facts; they read them through the
/// accessors below, which are bitfield tests rather than `Option<Piece>`
/// payloads. Anything richer is reconstructable from `tape[tape_start..]`.
#[derive(Debug, Clone, Copy)]
pub struct GameMove {
    pub from: Position,
    pub to: Position,

    // ── Frame scalars: everything `undo` needs ──
    pub piece_hash_before: u64,
    pub(crate) tape_start: u32,
    pub(crate) ghost_live_start: u32,
    pub fifty_move_counter_before_move: u32,

    // ── Derived metadata. Never read by `undo`. ──
    promoted_from_idx: u16,
    promoted_to_idx: u16,
    rook_from: (u8, u8),
    rook_to: (u8, u8),
    flags: u8,
    flight_count: u8,
}

const _: () = assert!(std::mem::size_of::<GameMove>() <= 64);

impl GameMove {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        from: Position,
        to: Position,
        piece_hash_before: u64,
        tape_start: u32,
        ghost_live_start: u32,
        fifty_move_counter_before_move: u32,
        captured: bool,
        en_passant_capture: bool,
        castling_rook_move: Option<(Position, Position)>,
                      promoted_from: Option<usize>,
                      promoted_to: Option<usize>,
                      flight_count: u8,
    ) -> Self {
        let mut flags = 0u8;
        if captured {
            flags |= F_CAPTURED;
        }
        if en_passant_capture {
            flags |= F_EP_CAPTURE;
        }
        let (rook_from, rook_to) = match castling_rook_move {
            Some((f, t)) => {
                flags |= F_CASTLING;
                ((f.0 as u8, f.1 as u8), (t.0 as u8, t.1 as u8))
            }
            None => ((0, 0), (0, 0)),
        };
        GameMove {
            from,
            to,
            piece_hash_before,
            tape_start,
            ghost_live_start,
            fifty_move_counter_before_move,
            promoted_from_idx: promoted_from.map(|x| x as u16).unwrap_or(NO_PIECE),
            promoted_to_idx: promoted_to.map(|x| x as u16).unwrap_or(NO_PIECE),
            rook_from,
            rook_to,
            flags,
            flight_count,
        }
    }

    #[inline]
    pub fn is_capture(&self) -> bool {
        self.flags & F_CAPTURED != 0
    }
    #[inline]
    pub fn is_en_passant_capture(&self) -> bool {
        self.flags & F_EP_CAPTURE != 0
    }
    #[inline]
    pub fn is_castling(&self) -> bool {
        self.flags & F_CASTLING != 0
    }
    #[inline]
    pub fn flight_capture_count(&self) -> u8 {
        self.flight_count
    }

    /// Kept as a method with the old shape so PGN / redo read unchanged.
    #[inline]
    pub fn castling_rook_move(&self) -> Option<(Position, Position)> {
        if !self.is_castling() {
            return None;
        }
        Some((
            (self.rook_from.0 as usize, self.rook_from.1 as usize),
              (self.rook_to.0 as usize, self.rook_to.1 as usize),
        ))
    }

    #[inline]
    pub fn promoted_from(&self) -> Option<usize> {
        (self.promoted_from_idx != NO_PIECE).then_some(self.promoted_from_idx as usize)
    }
    #[inline]
    pub fn promoted_to(&self) -> Option<usize> {
        (self.promoted_to_idx != NO_PIECE).then_some(self.promoted_to_idx as usize)
    }
}

// ─── Game result types ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameResult {
    Winner(PieceColor),
    Draw(DrawReason),
    Ongoing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawReason {
    FiftyMoveRule,
    Repetition,
    Stalemate,
    InsufficientMaterial,
    MutualElimination,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MateStatus {
    Ongoing,
    Checkmate,
    Stalemate,
    OpponentLostByCheck,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveAttemptResult {
    Success,
    Invalid,
    NeedsCastlingChoice,
    NeedsPromotion,
}

// ─── Pending move (GUI interaction) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PendingMove {
    Castling {
        king_from: Position,
        king_to: Position,
        king_move: MoveWithPath,
        options: Vec<CastlingOption>,
    },
    Promotion {
        from: Position,
        to: Position,
        move_rule: MoveWithPath,
        targets: Vec<usize>,
    },
}

// ─── Performance tracking ───────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct PerformanceTracker {
    pub moves_made: Cell<usize>,
    pub moves_undone: Cell<usize>,
    pub moves_generated: Cell<usize>,
    pub pseudo_legal_generations: Cell<usize>,
    pub legal_move_checks: Cell<usize>,
    pub check_tests: Cell<usize>,
    pub mate_status_checks: Cell<usize>,
}

impl PerformanceTracker {
    pub fn new() -> Self {
        Default::default()
    }
    #[inline(always)]
    pub fn inc(cell: &Cell<usize>) {
        cell.set(cell.get() + 1);
    }
    pub fn reset(&self) {
        self.moves_made.set(0);
        self.moves_undone.set(0);
        self.moves_generated.set(0);
        self.pseudo_legal_generations.set(0);
        self.legal_move_checks.set(0);
        self.check_tests.set(0);
        self.mate_status_checks.set(0);
    }
    pub fn snapshot(&self) -> PerformanceSnapshot {
        PerformanceSnapshot {
            moves_made: self.moves_made.get(),
            moves_undone: self.moves_undone.get(),
            moves_generated: self.moves_generated.get(),
            pseudo_legal_generations: self.pseudo_legal_generations.get(),
            legal_move_checks: self.legal_move_checks.get(),
            check_tests: self.check_tests.get(),
            mate_status_checks: self.mate_status_checks.get(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerformanceSnapshot {
    pub moves_made: usize,
    pub moves_undone: usize,
    pub moves_generated: usize,
    pub pseudo_legal_generations: usize,
    pub legal_move_checks: usize,
    pub check_tests: usize,
    pub mate_status_checks: usize,
}
