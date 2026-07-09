//src/core/check_detection.rs
//! Check detection, mate classification, and move legality filtering.
//!
//! ── Ghost projection ─────────────────────────────────────────────────────
//!
//! A ghost whose owner is royal (or the last remaining royalty piece) makes
//! its square count as occupied by that piece. This is how "you may not
//! castle out of, or through, check" is implemented: the king's castle
//! pattern ghosts the square it departed on every step, so `{origin, transit}`
//! both get tested.
//!
//! Projection deliberately does **not** go through `can_land_at`. Doing so is
//! a real bug: for a ghost without `CAPTURE_OPEN`, `can_land_at` falls back to
//! `can_land_empty`, and a pawn that can merely *push* to the transit square
//! would report the king as being in check there. `is_square_attacked_as` asks
//! the right question — "could an enemy *capture* the owner if it stood here?"

use crate::core::game_state::GameState;
use crate::core::game_types::*;
use crate::core::piece::{Piece, PieceColor};
use crate::core::Position;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

impl GameState {
    pub fn is_in_check(
        &self,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> bool {
        PerformanceTracker::inc(&self.performance.check_tests);
        self.is_in_check_fast(move_generator)
    }

    /// Is the side to move currently in check?
    ///
    /// The live ghosts belong to the *opponent's* last move, so their owners
    /// are enemy pieces and none of them project onto us. Ghosts are therefore
    /// not consulted here.
    #[inline]
    pub fn is_in_check_fast(&self, move_generator: &MoveGenerator) -> bool {
        let attacker = self.current_turn.opposite();

        for &pos in self.board.get_royal_positions(self.current_turn) {
            if move_generator.is_square_attacked(&self.board, pos, attacker) {
                return true;
            }
        }

        let royalty = self.board.get_royalty_positions(self.current_turn);
        if royalty.len() == 1 && move_generator.is_square_attacked(&self.board, royalty[0], attacker)
        {
            return true;
        }

        false
    }

    /// Is `victim`'s royalty (real squares **or** projected ghost squares)
    /// capturable by `victim.opposite()` right now?
    ///
    /// Used by both `mover_king_in_check` and `opponent_can_capture_royal`,
    /// which are the same query asked from two directions: the side whose
    /// move just landed on the board.
    fn royalty_exposed(&self, move_generator: &MoveGenerator, victim: PieceColor) -> bool {
        let attacker = victim.opposite();

        for &pos in self.board.get_royal_positions(victim) {
            if move_generator.is_square_attacked(&self.board, pos, attacker) {
                return true;
            }
        }

        let royalty = self.board.get_royalty_positions(victim);
        let last_ry = royalty.len() == 1;
        if last_ry && move_generator.is_square_attacked(&self.board, royalty[0], attacker) {
            return true;
        }

        // Projected ghost squares. The live ghosts are exactly the ones this
        // move created (0-ply transit assertions for castling, 1-ply capture
        // aliases for en passant). Projection is derived from the owner, not
        // declared on the ghost.
        for g in self.board.live_ghosts() {
            let Some(owner) = self.board.get_piece(g.owner()) else {
                continue;
            };
            if owner.color != victim {
                continue;
            }
            if !(owner.is_royal || (owner.is_royalty && last_ry)) {
                continue;
            }
            if move_generator.is_square_attacked_as(&self.board, g.square(), attacker, owner) {
                return true;
            }
        }

        false
    }

    /// Is the player who just moved now in check (including on the squares
    /// their royal virtually passed through)?
    #[inline]
    pub fn mover_king_in_check(&self, move_generator: &MoveGenerator) -> bool {
        self.royalty_exposed(move_generator, self.current_turn.opposite())
    }

    pub fn opponent_can_capture_royal(
        &self,
        move_generator: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> bool {
        self.royalty_exposed(move_generator, self.current_turn.opposite())
    }

    /// Would this move's captures strip the MOVER's OPPONENT of their last
    /// royal protection? Scoped to opponent-coloured victims only: a move that
    /// destroys the mover's *own* royal is caught after the fact by
    /// `check_royal_elimination`, not short-circuited here.
    #[inline]
    pub fn is_fatal_capture_for(
        &self,
        mover_color: PieceColor,
        landing: Option<Piece>,
        flight: &[(Position, Piece)],
    ) -> bool {
        let opponent = mover_color.opposite();
        let mut royalty_removed: u32 = 0;

        if let Some(p) = landing {
            if p.color == opponent {
                if p.is_royal {
                    return true;
                }
                if p.is_royalty {
                    royalty_removed += 1;
                }
            }
        }
        for (_, p) in flight {
            if p.color == opponent {
                if p.is_royal {
                    return true;
                }
                if p.is_royalty {
                    royalty_removed += 1;
                }
            }
        }

        royalty_removed > 0 && royalty_removed as usize >= self.board.royalty_count(opponent)
    }

    /// Symmetric, turn-independent "has a colour lost all royal protection".
    /// This is what makes self-capture end the game.
    pub fn check_royal_elimination(&mut self) -> bool {
        if !self.uses_royal_system {
            return false;
        }

        let white_gone = self.board.get_royal_positions(PieceColor::White).is_empty()
        && self.board.royalty_count(PieceColor::White) == 0;
        let black_gone = self.board.get_royal_positions(PieceColor::Black).is_empty()
        && self.board.royalty_count(PieceColor::Black) == 0;

        match (white_gone, black_gone) {
            (true, true) => {
                self.game_result = Some(GameResult::Draw(DrawReason::MutualElimination));
                true
            }
            (true, false) => {
                self.game_result = Some(GameResult::Winner(PieceColor::Black));
                true
            }
            (false, true) => {
                self.game_result = Some(GameResult::Winner(PieceColor::White));
                true
            }
            (false, false) => false,
        }
    }

    /// Test legality by executing, checking for self-check, and undoing.
    ///
    /// The undo is a pure inverse of the event slice, so the board — and the
    /// ghost epoch — are restored exactly.
    pub fn is_move_legal(
        &mut self,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        self.execute_expanded_move(mv, move_generator, config_manager);
        let safe = !self.mover_king_in_check(move_generator);
        self.undo_move(config_manager);
        safe
    }

    pub fn opponent_in_check(
        &self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        PerformanceTracker::inc(&self.performance.check_tests);
        matches!(
            self.generate_pseudo_legal_moves(move_generator, config_manager),
                 MoveGenerationResult::Checkmate { .. }
        )
    }

    pub fn has_legal_moves(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        PerformanceTracker::inc(&self.performance.legal_move_checks);
        !self.get_legal_moves(move_generator, config_manager).is_empty()
    }

    pub fn get_mate_status(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MateStatus {
        PerformanceTracker::inc(&self.performance.mate_status_checks);

        if matches!(
            self.generate_pseudo_legal_moves(move_generator, config_manager),
                    MoveGenerationResult::Checkmate { .. }
        ) {
            return MateStatus::OpponentLostByCheck;
        }

        if self.get_legal_moves(move_generator, config_manager).is_empty() {
            if self.is_in_check(move_generator, config_manager) {
                MateStatus::Checkmate
            } else if !self.board.has_pieces(self.current_turn) {
                MateStatus::Checkmate // extinction
            } else {
                MateStatus::Stalemate
            }
        } else {
            MateStatus::Ongoing
        }
    }

    pub fn update_mate_status(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        if !matches!(self.game_result, Some(GameResult::Ongoing)) {
            return;
        }
        match self.get_mate_status(move_generator, config_manager) {
            MateStatus::Checkmate => {
                self.game_result = Some(GameResult::Winner(self.current_turn.opposite()));
            }
            MateStatus::Stalemate => {
                self.game_result = Some(GameResult::Draw(DrawReason::Stalemate));
            }
            MateStatus::OpponentLostByCheck => {
                self.game_result = Some(GameResult::Winner(self.current_turn));
            }
            MateStatus::Ongoing => {}
        }
    }
}
