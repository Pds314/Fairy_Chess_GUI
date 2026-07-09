//src/core/move_exec.rs
//! Move execution and undo, as event chains over `Board::set_piece`.

use crate::core::chain::MoveChain;
use crate::core::game_state::GameState;
use crate::core::game_types::*;
use crate::core::position::Position;
use crate::move_generator::{MoveGenerator, MoveWithPath};
use crate::piece_config::PieceConfigManager;

impl GameState {
    /// The entire cost of the attack-table feature when it is disabled: one
    /// perfectly-predicted, not-taken branch per touched square.
    #[inline(always)]
    pub(crate) fn attack_dirty(&mut self, pos: Position) {
        if let Some(t) = self.attack_table.as_mut() {
            t.mark_dirty(pos);
        }
    }

    pub fn execute_expanded_move(
        &mut self,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        PerformanceTracker::inc(&self.performance.moves_made);
        if let Some(ref option) = mv.castling_option {
            self.execute_castling(mv.from, mv.to, &mv.move_with_path, option, config_manager, move_generator);
        } else {
            self.make_move(mv.from, mv.to, &mv.move_with_path, config_manager, mv.promotion_target);
        }
    }

    pub fn make_move(
        &mut self,
        from: Position,
        to: Position,
        move_with_path: &MoveWithPath,
        config_manager: &PieceConfigManager,
        promotion_type: Option<usize>,
    ) {
        let mover_color = self.current_turn;
        let chain = match MoveChain::build_move(
            &self.board, from, to, move_with_path, promotion_type, mover_color, config_manager,
        ) {
            Some(c) => c,
            None => return,
        };
        self.commit(from, to, chain);
    }

    pub fn execute_castling(
        &mut self,
        king_from: Position,
        king_to: Position,
        king_move: &MoveWithPath,
        option: &crate::move_generator::CastlingOption,
        config_manager: &PieceConfigManager,
        _move_generator: &MoveGenerator,
    ) {
        let mover_color = self.current_turn;
        let chain = match MoveChain::build_castling(
            &self.board, king_from, king_to, king_move, option, mover_color,
            &self.promotion_config, config_manager,
        ) {
            Some(c) => c,
            None => return,
        };
        self.commit(king_from, king_to, chain);
    }

    /// Push a frame, apply the chain forward, push its ghosts.
    fn commit(&mut self, from: Position, to: Position, chain: MoveChain) {
        let fifty_before = self.fifty_move_counter;
        let hash_before = self.board.piece_hash();
        let tape_start = self.tape.len() as u32;
        let ghost_live_start = self.board.begin_ghost_epoch();

        for ev in &chain.events {
            ev.apply(&mut self.board);
            self.attack_dirty(ev.square_pos());
        }
        for g in &chain.ghosts {
            self.board.push_ghost(*g);
        }
        self.tape.extend_from_slice(&chain.events);

        self.fifty_move_counter = if chain.resets_fifty { 0 } else { fifty_before + 1 };

        // 64-byte POD store. No Option<Piece>, no SmallVec, no drop glue.
        self.move_history.push(GameMove::new(
            from,
            to,
            hash_before,
            tape_start,
            ghost_live_start,
            fifty_before,
            chain.captured_piece.is_some(),
                                             chain.captured_en_passant.is_some(),
                                             chain.castling_rook_move,
                                             chain.promoted_from,
                                             chain.promoted_to,
                                             chain.captured_in_flight.len() as u8,
        ));

        self.current_turn = self.current_turn.opposite();
        self.finalize_state_update();
    }

    /// `config_manager` is retained only because ~20 call sites pass it.
    /// **Nothing reads it.** Undo is a pure inverse of the event slice.
    pub fn undo_move(&mut self, _config_manager: &PieceConfigManager) -> bool {
        if let Some(last_move) = self.move_history.pop() {
            PerformanceTracker::inc(&self.performance.moves_undone);
            self.revert_position_history();
            self.revert_board_state(last_move);
            true
        } else {
            false
        }
    }

    fn revert_board_state(&mut self, gm: GameMove) {
        let start = gm.tape_start as usize;

        let mut i = self.tape.len();
        while i > start {
            i -= 1;
            let ev = self.tape[i]; // Copy: releases the borrow on `self.tape`
            ev.revert(&mut self.board);
            self.attack_dirty(ev.square_pos());
        }
        self.tape.truncate(start);
        self.board.rewind_ghosts(gm.ghost_live_start);

        self.current_turn = self.current_turn.opposite();
        self.fifty_move_counter = gm.fifty_move_counter_before_move;
        self.game_result = Some(GameResult::Ongoing);

        debug_assert_eq!(
            self.board.piece_hash(),
                         gm.piece_hash_before,
                         "XOR-inverted hash diverged from the frame's snapshot"
        );
        self.board.set_piece_hash(gm.piece_hash_before);
    }

    // ─── Internal helpers ───────────────────────────────────────────

    pub fn finalize_state_update(&mut self) {
        let h = self.current_hash();
        *self.position_history.entry(h).or_insert(0) += 1;
        self.check_draw_conditions();
    }

    fn revert_position_history(&mut self) {
        let h = self.current_hash();
        if let Some(count) = self.position_history.get_mut(&h) {
            if *count > 1 {
                *count -= 1;
            } else {
                self.position_history.remove(&h);
            }
        }
    }

    pub fn check_draw_conditions(&mut self) {
        if self.check_royal_elimination() {
            return;
        }
        if self.fifty_move_counter >= 100 {
            self.game_result = Some(GameResult::Draw(DrawReason::FiftyMoveRule));
            return;
        }
        // Repetition cannot exist before 4 reversible plies; this guard keeps
        // the HashMap probe off the tactical hot path.
        if self.fifty_move_counter >= 4 {
            let h = self.current_hash();
            if let Some(&count) = self.position_history.get(&h) {
                if count >= 3 {
                    self.game_result = Some(GameResult::Draw(DrawReason::Repetition));
                    return;
                }
            }
        }
        if self.insufficient_material.is_draw(&self.board) {
            self.game_result = Some(GameResult::Draw(DrawReason::InsufficientMaterial));
            return;
        }
        self.game_result = Some(GameResult::Ongoing);
    }
}
