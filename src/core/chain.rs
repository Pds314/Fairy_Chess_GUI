// src/core/chain.rs
//! Moves as chains of atomic board events.
//!
//! Exactly two primitives:
//!
//! ```text
//! Lift { square, piece }   // remove; remember the piece in full
//! Drop { square, piece }   // place; carries move_count and the royal flags
//! ```
//!
//! `Drop` carries the piece it places, so a promotion is a `Drop` of a
//! different piece type and a `move_count` bump is a `Drop` of a different
//! `move_count`. Neither needs its own event kind. A null move (`?`, where
//! `from == to`) is `Lift(from, p)` then `Drop(from, p')`.
//!
//! ── The one invariant ────────────────────────────────────────────────────
//!
//! **All `Lift`s precede all `Drop`s.** Chess960 forces this: the rook can
//! start on the king's destination (K b1→c1, R c1→d1), or neither piece may
//! move at all (K g1→g1, R f1→f1).
//!
//! ── Why undo needs nothing ───────────────────────────────────────────────
//!
//! Each event carries its complete payload, so reverting is `Lift⁻¹ = place`,
//! `Drop⁻¹ = clear`, walked backwards. No move generator, no
//! `PieceConfigManager`, no `ExpandedMove`.
//!
//! `Board::set_piece` XORs the Zobrist key in and out, so the hash is restored
//! by the reverse walk alone.

use crate::core::board::Board;
use crate::core::ghost::Ghost;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::move_generator::{
    flight_captures, ghosts_for, resolve_landing_capture, CastlingOption, MoveWithPath,
};
use crate::notation::position_to_algebraic;
use crate::piece_config::PieceConfigManager;
use crate::promotion::{
    PromotionConfig, PromotionManager, PromotionSelector, RandomPromotionSelector,
};
use smallvec::SmallVec;

// ─────────────────────────────────────────────────────────────────────────
// Atomic events
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Lift,
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardEvent {
    pub kind: EventKind,
    square: (u8, u8),
    pub piece: Piece,
}

impl BoardEvent {
    #[inline]
    pub fn lift(square: Position, piece: Piece) -> Self {
        BoardEvent { kind: EventKind::Lift, square: (square.0 as u8, square.1 as u8), piece }
    }
    #[inline]
    pub fn drop_at(square: Position, piece: Piece) -> Self {
        BoardEvent { kind: EventKind::Drop, square: (square.0 as u8, square.1 as u8), piece }
    }

    #[inline(always)]
    pub fn square_pos(&self) -> Position {
        (self.square.0 as usize, self.square.1 as usize)
    }

    /// Forward. `Board::set_piece` maintains royal/royalty lists, piece counts
    /// and the Zobrist hash, so it is the only primitive we need.
    #[inline]
    pub fn apply(&self, board: &mut Board) {
        match self.kind {
            EventKind::Lift => board.set_piece(self.square_pos(), None),
            EventKind::Drop => board.set_piece(self.square_pos(), Some(self.piece)),
        }
    }

    /// Backward. Walk the slice in reverse and call this on each event.
    #[inline]
    pub fn revert(&self, board: &mut Board) {
        match self.kind {
            EventKind::Lift => board.set_piece(self.square_pos(), Some(self.piece)),
            EventKind::Drop => board.set_piece(self.square_pos(), None),
        }
    }

    /// `"Lift e2   P  mc=0"`
    pub fn describe(&self, board_size: (usize, usize), cm: &PieceConfigManager) -> String {
        let kind = match self.kind {
            EventKind::Lift => "Lift",
            EventKind::Drop => "Drop",
        };
        let name = cm
        .get_piece_by_index(self.piece.piece_type)
        .map(|c| c.display_name.clone())
        .unwrap_or_else(|| "?".to_string());
        format!(
            "{} {:<4} {}  {:<10} mc={}{}{}",
            kind,
            position_to_algebraic(self.square_pos(), board_size),
                self.piece.to_char(cm),
                name,
                self.piece.move_count,
                if self.piece.is_royal { " R" } else { "" },
                    if self.piece.is_royalty { " r" } else { "" },
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────
// MoveChain
// ─────────────────────────────────────────────────────────────────────────

/// The full expansion of one move: the events to apply, the ghosts it leaves
/// behind, and the small amount of derived metadata PGN / the GUI / redo want.
/// `undo` reads **none** of the metadata.
#[derive(Clone, Debug, Default)]
pub struct MoveChain {
    pub events: SmallVec<[BoardEvent; 6]>,
    pub ghosts: SmallVec<[Ghost; 4]>,
    pub resets_fifty: bool,

    pub captured_piece: Option<Piece>,
    pub captured_en_passant: Option<Position>,
    pub castling_rook_move: Option<(Position, Position)>,
    pub promoted_from: Option<usize>,
    pub promoted_to: Option<usize>,
    pub captured_in_flight: SmallVec<[(Position, Piece); 1]>,
}

impl MoveChain {
    #[inline]
    fn already_lifted(&self, sq: Position) -> bool {
        self.events
        .iter()
        .any(|e| e.kind == EventKind::Lift && e.square_pos() == sq)
    }

    #[cfg(debug_assertions)]
    fn assert_ordering(&self) {
        let mut seen_drop = false;
        for e in &self.events {
            match e.kind {
                EventKind::Drop => seen_drop = true,
                EventKind::Lift => debug_assert!(
                    !seen_drop,
                    "MoveChain invariant violated: a Lift follows a Drop. Chess960 \
(rook starting on the king's destination) depends on all Lifts \
preceding all Drops."
                ),
            }
        }
    }
    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn assert_ordering(&self) {}

    pub fn describe(&self, board_size: (usize, usize), cm: &PieceConfigManager) -> String {
        let mut s = String::new();
        for e in &self.events {
            s.push_str("      ");
            s.push_str(&e.describe(board_size, cm));
            s.push('\n');
        }
        for g in &self.ghosts {
            s.push_str(&format!(
                "     ghost {} -> {}  [{}]  {}\n",
                position_to_algebraic(g.square(), board_size),
                                position_to_algebraic(g.owner(), board_size),
                                g.flags().dsl_marks(),
                                g.flags()
            ));
        }
        s
    }

    // ── Ordinary move (null, capture, en passant, flight, promotion) ────

    pub fn build_move(
        board: &Board,
        from: Position,
        to: Position,
        mwp: &MoveWithPath,
        promotion: Option<usize>,
        mover_color: PieceColor,
        config_manager: &PieceConfigManager,
    ) -> Option<MoveChain> {
        let mover = board.get_piece(from)?;
        let is_null = from == to;
        let mut c = MoveChain::default();

        // 1. Flight captures (`%`) — intermediates of the path. May include
        //    friendly pieces; `flight_captures` structurally excludes `to`.
        if !is_null {
            for (pos, victim) in flight_captures(board, mwp, mover_color) {
                c.events.push(BoardEvent::lift(pos, victim));
                c.captured_in_flight.push((pos, victim));
            }
        }

        // 2. Landing capture. `resolve_landing_capture` is the one rule:
        //    real occupant first, ghost alias only on an empty square.
        if !is_null {
            if let Some((victim, victim_sq)) = resolve_landing_capture(board, to, &mwp.rule) {
                // A `%` sweep may already have taken the ghost's owner.
                // Lifting twice would XOR the Zobrist key in twice; the board
                // outcome is identical either way, so we simply skip and let
                // the flight bookkeeping own it.
                if !c.already_lifted(victim_sq) {
                    c.events.push(BoardEvent::lift(victim_sq, victim));
                    c.captured_piece = Some(victim);
                    if victim_sq != to {
                        c.captured_en_passant = Some(victim_sq);
                    }
                }
            }
        }

        // 3. The mover itself. Promotion is just a different `Drop`.
        let mut landed = mover;
        landed.move_count += 1;
        if let Some(new_type) = promotion {
            let (r, ry) = config_manager.piece_flags(new_type);
            c.promoted_from = Some(landed.piece_type);
            c.promoted_to = Some(new_type);
            landed.piece_type = new_type;
            landed.is_royal = r;
            landed.is_royalty = ry;
        }
        c.events.push(BoardEvent::lift(from, mover));
        c.events.push(BoardEvent::drop_at(to, landed));

        // 4. Ghosts: one per flagged step, on the square that step departed.
        c.ghosts = ghosts_for(mwp, from, to);

        c.resets_fifty = c.captured_piece.is_some()
        || !c.captured_in_flight.is_empty()
        || c.promoted_from.is_some()
        || mwp.rule.is_irreversible;

        c.assert_ordering();
        Some(c)
    }

    // ── Castling ────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn build_castling(
        board: &Board,
        king_from: Position,
        king_to: Position,
        king_move: &MoveWithPath,
        option: &CastlingOption,
        mover_color: PieceColor,
        promotion_config: &PromotionConfig,
        config_manager: &PieceConfigManager,
    ) -> Option<MoveChain> {
        let king = board.get_piece(king_from)?;
        let rook = board.get_piece(option.rook_from)?;
        let mut c = MoveChain::default();

        // Flight captures along the king's own path. Exclude both castling
        // participants' squares: the rook is lifted explicitly below, and
        // double-lifting would corrupt the hash.
        for (pos, victim) in flight_captures(board, king_move, mover_color) {
            if pos == option.rook_to || pos == option.rook_from || pos == king_from {
                continue;
            }
            c.events.push(BoardEvent::lift(pos, victim));
            c.captured_in_flight.push((pos, victim));
        }

        // Anything on the rook's destination that isn't a castling participant.
        let rook_victim = if option.rook_to != king_from && option.rook_to != option.rook_from {
            board.get_piece(option.rook_to)
        } else {
            None
        };
        if let Some(v) = rook_victim {
            if !c.already_lifted(option.rook_to) {
                c.events.push(BoardEvent::lift(option.rook_to, v));
            }
        }

        // ALL LIFTS BEFORE ALL DROPS. Chess960 overlap depends on this.
        c.events.push(BoardEvent::lift(king_from, king));
        c.events.push(BoardEvent::lift(option.rook_from, rook));

        let mut king_landed = king;
        king_landed.move_count += 1;
        let mut rook_landed = rook;
        rook_landed.move_count += 1;

        let king_promo = auto_promote(&mut king_landed, king_to, promotion_config, config_manager);
        let rook_promo =
        auto_promote(&mut rook_landed, option.rook_to, promotion_config, config_manager);

        c.events.push(BoardEvent::drop_at(king_to, king_landed));
        c.events.push(BoardEvent::drop_at(option.rook_to, rook_landed));

        c.ghosts = ghosts_for(king_move, king_from, king_to);

        c.captured_piece = rook_victim;
        c.castling_rook_move = Some((option.rook_from, option.rook_to));
        let (pf, pt) = king_promo.or(rook_promo).unzip();
        c.promoted_from = pf;
        c.promoted_to = pt;

        c.resets_fifty = rook_victim.is_some()
        || king_promo.is_some()
        || rook_promo.is_some()
        || !c.captured_in_flight.is_empty();

        c.assert_ordering();
        Some(c)
    }
}

/// Promote `p` in place if it is standing in a promotion zone. The *result* is
/// recorded on the tape, which is what makes the nondeterministic
/// `RandomPromotionSelector` survive both undo and redo exactly.
fn auto_promote(
    p: &mut Piece,
    pos: Position,
    promotion_config: &PromotionConfig,
    config_manager: &PieceConfigManager,
) -> Option<(usize, usize)> {
    if !PromotionManager::can_promote(p.piece_type, config_manager) {
        return None;
    }
    if !promotion_config.is_promotion_zone(pos, p.color) {
        return None;
    }
    let targets = PromotionManager::get_promotion_targets(p.piece_type, config_manager);
    if targets.is_empty() {
        return None;
    }
    let new_type = RandomPromotionSelector.select_promotion(&targets, config_manager)?;
    let old_type = p.piece_type;
    let (r, ry) = config_manager.piece_flags(new_type);
    p.piece_type = new_type;
    p.is_royal = r;
    p.is_royalty = ry;
    Some((old_type, new_type))
}
