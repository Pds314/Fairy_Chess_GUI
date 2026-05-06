// src/engine/evaluator.rs
use crate::core::game_state::ExpandedMove;
use crate::core::game_state::MoveGenerationResult;
use crate::core::{GameState, Piece, Position};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

/// A generic trait for any evaluation function.
pub trait EvaluatorTrait {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32;

    /// Gets the positional value of a specific piece on a specific square.
    fn get_piece_value_on_square(
        &self,
        _piece: &Piece,
        _pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        100
    }

    /// ⭐ ADDED: Calculates the change in score for a move.
    /// This default implementation is slow (it re-evaluates the board twice),
    /// but it allows any evaluator to work with an incremental search.
    /// Fast engines like PstEngine should provide their own optimized version.
    fn calculate_delta(
        &self,
        state: &mut GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let score_before = self.evaluate(state, move_generator, config_manager);

        state.execute_expanded_move(mv, move_generator, config_manager);
        let score_after = self.evaluate(state, move_generator, config_manager);
        state.undo_move(config_manager);

        // The score is from the current player's perspective. After they move, it's the
        // opponent's turn. The new score for the original player is the negative of the
        // opponent's score. The change is `new_score - old_score`, which is `-score_after - score_before`.
        -score_after - score_before
    }
}

pub struct Evaluator;

impl Evaluator {
    pub fn new() -> Self {
        Evaluator
    }

    pub fn evaluate_position_for_player(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let player_moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(moves) => moves,
            MoveGenerationResult::Checkmate { .. } => {
                return 999999;
            }
        };

        if player_moves.is_empty() {
            if state.is_in_check(move_generator, config_manager) {
                return -999999;
            }
            // Extinction: no pieces left is a loss in non-royal variants.
            if !state.board.has_pieces(state.current_turn) {
                return -999999;
            }
            return 0;
        }

        let player_mobility = player_moves.len() as i32;
        let mut min_opponent_mobility = i32::MAX;
        let mut valid_moves_tested = 0;

        for mv in player_moves {
            state.execute_expanded_move(&mv, move_generator, config_manager);
            match state.generate_pseudo_legal_moves(move_generator, config_manager) {
                MoveGenerationResult::Moves(opp_moves) => {
                    let opponent_mobility = opp_moves.len() as i32;
                    min_opponent_mobility = min_opponent_mobility.min(opponent_mobility);
                    valid_moves_tested += 1;
                }
                MoveGenerationResult::Checkmate { .. } => {
                    valid_moves_tested += 1;
                }
            };
            state.undo_move(config_manager);
        }

        if valid_moves_tested == 0 {
            return -999999;
        }
        player_mobility - min_opponent_mobility
    }
}

impl EvaluatorTrait for Evaluator {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Checkmate { .. } => 999999,
            MoveGenerationResult::Moves(moves) => {
                if moves.is_empty() {
                    if state.is_in_check(move_generator, config_manager) {
                        return -999999;
                    }
                    if !state.board.has_pieces(state.current_turn) {
                        return -999999;
                    }
                    0
                } else {
                    self.evaluate_position_for_player(state, move_generator, config_manager)
                }
            }
        }
    }
}
