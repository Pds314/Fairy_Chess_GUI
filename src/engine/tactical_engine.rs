// src/engine/tactical_engine.rs
use crate::core::GameState;
use crate::core::game_state::{ExpandedMove, MateStatus};
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

pub struct TacticalEngine;

impl TacticalEngine {
    pub fn new() -> Self {
        TacticalEngine
    }

    fn is_checkmate_move(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);
        matches!(
            test_state.get_mate_status(move_generator, config_manager),
            MateStatus::Checkmate
        )
    }

    fn gives_check(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);
        test_state.is_in_check(move_generator, config_manager)
    }

    fn calculate_distance_score(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> f64 {
        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);

        let enemy_color = test_state.current_turn;
        let enemy_pieces = test_state.board.get_pieces_by_color(enemy_color);

        let mut royal_positions = Vec::new();
        for (pos, piece) in enemy_pieces {
            if let Some(piece_config) = config_manager.get_piece_by_index(piece.piece_type) {
                if piece_config.properties.is_royal || piece_config.properties.is_royalty {
                    royal_positions.push(pos);
                }
            }
        }

        if royal_positions.is_empty() {
            return f64::MAX;
        }

        let avg_row =
            royal_positions.iter().map(|p| p.0 as f64).sum::<f64>() / royal_positions.len() as f64;
        let avg_col =
            royal_positions.iter().map(|p| p.1 as f64).sum::<f64>() / royal_positions.len() as f64;

        let our_color = test_state.current_turn.opposite();
        let our_pieces = test_state.board.get_pieces_by_color(our_color);
        let mut total_distance = 0.0;
        for (pos, _) in our_pieces {
            let row_diff = pos.0 as f64 - avg_row;
            let col_diff = pos.1 as f64 - avg_col;
            total_distance += row_diff * row_diff + col_diff * col_diff;
        }

        total_distance
    }
}

impl ChessEngine for TacticalEngine {
    fn name(&self) -> &str {
        "Tactical Priority Engine (Mate/Check/Capture/Push)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);
        if legal_moves.is_empty() {
            return None;
        }

        // Priority 1: Check for mate in 1
        for mv in &legal_moves {
            if self.is_checkmate_move(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
            ) {
                return Some(SearchResult {
                    best_move: mv.clone(), // <-- FIX
                    evaluation: Evaluation {
                        score: 999999,
                        mate_in: Some(1),
                    },
                    depth_reached: 1,
                });
            }
        }

        // Priority 2: Check for moves that give check
        for mv in &legal_moves {
            if self.gives_check(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
            ) {
                return Some(SearchResult {
                    best_move: mv.clone(), // <-- FIX
                    evaluation: Evaluation {
                        score: 1000,
                        mate_in: None,
                    },
                    depth_reached: 1,
                });
            }
        }

        // Priority 3: Check for captures
        for mv in &legal_moves {
            if mv.captures.is_some() {
                return Some(SearchResult {
                    best_move: mv.clone(), // <-- FIX
                    evaluation: Evaluation {
                        score: 500,
                        mate_in: None,
                    },
                    depth_reached: 1,
                });
            }
        }

        // Priority 4: Choose move that minimizes distance to enemy royals
        let mut best_move = &legal_moves[0];
        let mut best_distance = self.calculate_distance_score(
            params.state,
            best_move,
            params.move_generator,
            params.config_manager,
        );

        for mv in &legal_moves[1..] {
            let distance = self.calculate_distance_score(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
            );
            if distance < best_distance {
                best_distance = distance;
                best_move = mv;
            }
        }

        Some(SearchResult {
            best_move: best_move.clone(), // <-- FIX
            evaluation: Evaluation {
                score: -(best_distance as i32),
                mate_in: None,
            },
            depth_reached: 1,
        })
    }

    fn stop(&mut self) {}
}
