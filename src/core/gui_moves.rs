//src/core/gui_moves.rs
use crate::core::game_state::GameState;
use crate::core::game_types::*;
use crate::core::position::Position;
use crate::move_generator::{resolve_landing_capture, MoveGenerator};
use crate::piece_config::PieceConfigManager;
use crate::promotion::{PromotionManager, PromotionSelector, RandomPromotionSelector};

impl GameState {
    pub fn attempt_move(
        &mut self,
        from: Position,
        to: Position,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MoveAttemptResult {
        if !self.is_valid_turn(from) {
            return MoveAttemptResult::Invalid;
        }
        let Some(piece) = self.board.get_piece(from) else { return MoveAttemptResult::Invalid };
        let Some(mwp) = move_generator.get_move_rule(&self.board, from, to, piece.piece_type) else {
            return MoveAttemptResult::Invalid;
        };

        if mwp.rule.is_king_castle {
            let options = move_generator.get_castling_options(&self.board, from, to, &mwp);
            let legal: Vec<_> = options.into_iter().filter(|opt| {
                let cap = self.board.get_piece(opt.rook_to);
                let em = ExpandedMove {
                    from, to,
                    move_with_path: mwp.clone(),
                                                           castling_option: Some(opt.clone()),
                                                           promotion_target: None,
                                                           captures: cap,
                                                           captures_position: cap.map(|_| opt.rook_to),
                };
                self.is_move_legal(&em, move_generator, config_manager)
            }).collect();

            if legal.is_empty() { return MoveAttemptResult::Invalid; }
            if legal.len() == 1 {
                self.redo_stack.clear();
                self.execute_castling(from, to, &mwp, &legal[0], config_manager, move_generator);
                return MoveAttemptResult::Success;
            }
            self.pending_move = Some(PendingMove::Castling {
                king_from: from, king_to: to, king_move: mwp, options: legal,
            });
            return MoveAttemptResult::NeedsCastlingChoice;
        }

        let (captures, captures_position) = match resolve_landing_capture(&self.board, to, &mwp.rule) {
            Some((victim, sq)) => (Some(victim), Some(sq)),
            None => (None, None),
        };

        let can_promote = config_manager
        .get_piece_by_index(piece.piece_type)
        .map_or(false, |c| c.properties.can_promote);

        if can_promote && self.promotion_config.is_promotion_zone(to, piece.color) {
            let targets = PromotionManager::get_promotion_targets(piece.piece_type, config_manager);
            if !targets.is_empty() {
                let promo = RandomPromotionSelector.select_promotion(&targets, config_manager);
                let em = ExpandedMove {
                    from, to, move_with_path: mwp.clone(), castling_option: None,
                    promotion_target: promo, captures, captures_position,
                };
                if !self.is_move_legal(&em, move_generator, config_manager) {
                    return MoveAttemptResult::Invalid;
                }
                self.redo_stack.clear();
                self.make_move(from, to, &mwp, config_manager, promo);
                return MoveAttemptResult::Success;
            }
        }

        let em = ExpandedMove {
            from, to, move_with_path: mwp.clone(), castling_option: None,
            promotion_target: None, captures, captures_position,
        };
        if !self.is_move_legal(&em, move_generator, config_manager) {
            return MoveAttemptResult::Invalid;
        }
        self.redo_stack.clear();
        self.make_move(from, to, &mwp, config_manager, None);
        MoveAttemptResult::Success
    }

    pub fn undo_move_for_gui(&mut self, config_manager: &PieceConfigManager) -> bool {
        if let Some(gm) = self.move_history.last().copied() {
            self.redo_stack.push(gm);
        }
        self.undo_move(config_manager)
    }

    pub fn redo_move(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        let Some(redo) = self.redo_stack.pop() else { return false };
        let Some(piece) = self.board.get_piece(redo.from) else { return false };

        if let Some((rook_from, _)) = redo.castling_rook_move() {
            let Some(mwp) = move_generator.get_move_rule(&self.board, redo.from, redo.to, piece.piece_type) else {
                return false;
            };
            let options = move_generator.get_castling_options(&self.board, redo.from, redo.to, &mwp);

            if let Some(opt) = options.into_iter().find(|o| o.rook_from == rook_from) {
                self.execute_castling(redo.from, redo.to, &mwp, &opt, config_manager, move_generator);

                // `try_auto_promote` is nondeterministic; re-pin the recorded
                // outcome so redo reproduces the original game exactly.
                if let (Some(pt), Some(pf)) = (redo.promoted_to(), redo.promoted_from()) {
                    if let Some(last) = self.move_history.last_mut() {
                        *last = GameMove::new(
                            last.from, last.to, last.piece_hash_before,
                            last.tape_start, last.ghost_live_start,
                            last.fifty_move_counter_before_move,
                            last.is_capture(), last.is_en_passant_capture(),
                                              last.castling_rook_move(), Some(pf), Some(pt),
                                              last.flight_capture_count(),
                        );
                    }
                    if let Some(mut p) = self.board.get_piece(redo.to) {
                        if p.piece_type != pt {
                            let (r, ry) = config_manager.piece_flags(pt);
                            p.piece_type = pt;
                            p.is_royal = r;
                            p.is_royalty = ry;
                            self.board.set_piece(redo.to, Some(p));
                        }
                    }
                }
                return true;
            }
            return false;
        }

        if let Some(mwp) = move_generator.get_move_rule(&self.board, redo.from, redo.to, piece.piece_type) {
            self.make_move(redo.from, redo.to, &mwp, config_manager, redo.promoted_to());
            return true;
        }
        false
    }
}
