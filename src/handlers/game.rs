use crate::app::ChessGui;
use crate::core::game_state::MoveAttemptResult;
use crate::core::PendingMove;
use crate::core::Position;
use crate::promotion;
use crate::promotion_dialog::PromotionDialog;
use crate::clog;

impl ChessGui {
    pub(crate) fn handle_square_click(&mut self, clicked_pos: Position) {
        if self.promotion_dialog.is_some() {
            return;
        }
        // Castling choice resolution
        if let Some(PendingMove::Castling {
            king_from,
            king_to,
            king_move,
            options,
        }) = &self.game_state.pending_move
        {
            for option in options.iter() {
                if clicked_pos == option.rook_from || clicked_pos == option.rook_to {
                    let (option, king_from, king_to, king_move) =
                        (option.clone(), *king_from, *king_to, king_move.clone());
                    self.game_state.pending_move = None;
                    self.game_state.execute_castling(
                        king_from,
                        king_to,
                        &king_move,
                        &option,
                        &self.piece_config,
                        &self.move_generator,
                    );
                    self.clear_selection();
                    self.castling_highlights.clear();
                    self.game_controller
                        .end_turn(self.game_state.current_turn.opposite());
                    self.check_for_engine_move();
                    return;
                }
            }
        }
        if !self.is_game_ongoing() {
            return;
        }
        if let Some(from_pos) = self.selected_square {
            self.try_make_move(from_pos, clicked_pos);
        } else {
            self.try_select_piece(clicked_pos);
        }
    }

    pub(crate) fn try_make_move(&mut self, from: Position, to: Position) {
        if from == to {
            self.clear_selection();
            return;
        }
        let current_turn = self.game_state.current_turn;

        // Check for promotion before attempting the move
        if !self
            .game_controller
            .is_engine_turn(current_turn)
        {
            if let Some(piece) = self.game_state.board.get_piece(from) {
                if self
                    .move_generator
                    .get_move_rule(&self.game_state.board, from, to, piece.piece_type)
                    .is_some()
                    && promotion::PromotionManager::can_promote(piece.piece_type, &self.piece_config)
                    && self
                        .game_state
                        .promotion_config
                        .is_promotion_zone(to, piece.color)
                {
                    let targets = promotion::PromotionManager::get_promotion_targets(
                        piece.piece_type,
                        &self.piece_config,
                    );
                    if !targets.is_empty() {
                        self.promotion_dialog = Some(PromotionDialog::new(from, to, targets));
                        self.board_cache.clear();
                        return;
                    }
                }
            }
        }

        match self
            .game_state
            .attempt_move(from, to, &self.move_generator, &self.piece_config)
        {
            MoveAttemptResult::Success => {
                self.game_state.clear_redo();
                self.clear_selection();
                self.castling_highlights.clear();
                self.position_analysis = None;
                self.last_move_highlight = Some((from, to));
                self.game_controller.end_turn(current_turn);
                self.check_for_engine_move();
            }
            MoveAttemptResult::Invalid => self.try_select_piece(to),
            MoveAttemptResult::NeedsCastlingChoice => {
                if let Some(PendingMove::Castling { options, .. }) = &self.game_state.pending_move {
                    self.castling_highlights = options
                        .iter()
                        .map(|opt| (opt.rook_from, opt.rook_to))
                        .collect();
                    self.board_cache.clear();
                }
            }
            MoveAttemptResult::NeedsPromotion => {
                clog!("Unexpected promotion state");
                self.clear_selection();
                self.game_controller.end_turn(current_turn);
                self.check_for_engine_move();
            }
        }
    }

    pub(crate) fn try_select_piece(&mut self, pos: Position) {
        if let Some(piece) = self.game_state.board.get_piece(pos) {
            if piece.color == self.game_state.current_turn {
                self.selected_square = Some(pos);
                self.board_cache.clear();
                return;
            }
        }
        self.clear_selection();
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_square = None;
        self.promotion_dialog = None;
        self.board_cache.clear();
    }

    pub(crate) fn handle_undo(&mut self) {
        if self.game_state.undo_move_for_gui(&self.piece_config) {
            self.clear_selection();
            self.promotion_dialog = None;
        }
    }

    pub(crate) fn handle_reset(&mut self) {
        if let Some(game_file) = self.current_game_file.clone() {
            if let Err(e) = self.load_game_from_file(&game_file) {
                clog!("Failed to reload game file '{}': {}", game_file, e);
                let board_config = crate::handlers::loading::load_board_config(&self.asset_manager);
                self.game_state = crate::core::GameState::from_config(
                    board_config,
                    &self.piece_config,
                    std::sync::Arc::make_mut(&mut self.move_generator),
                );
            }
        } else {
            let board_config = crate::handlers::loading::load_board_config(&self.asset_manager);
            self.game_state = crate::core::GameState::from_config(
                board_config,
                &self.piece_config,
                std::sync::Arc::make_mut(&mut self.move_generator),
            );
        }
        self.game_controller.reset_engine_caches();
        self.clear_selection();
        self.promotion_dialog = None;
        self.position_analysis = None;
        self.last_move_highlight = None;
    }

    pub(crate) fn handle_generate_moves(&self) {
        let result = self
            .game_state
            .generate_pseudo_legal_moves(&self.move_generator, &self.piece_config);
        let board_size = self.game_state.board.size();
        crate::helpers::print_move_generation_results(&result, &self.piece_config, board_size);
    }
}
