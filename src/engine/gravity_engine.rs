// src/engine/gravity_engine.rs
use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::search::Search;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

/// Engine that targets the center of mass of all pieces on the board
pub struct GravityEngine;

impl GravityEngine {
    pub fn new() -> Self {
        GravityEngine
    }
}

struct GravityEvaluator;

impl GravityEvaluator {
    /// Calculate the center of mass of all pieces on the board
    fn calculate_center_of_mass(&self, board: &Board) -> (f64, f64) {
        let mut total_row = 0.0;
        let mut total_col = 0.0;
        let mut piece_count = 0;

        let board_size = board.size();
        for row in 0..board_size.0 {
            for col in 0..board_size.1 {
                if board.get_piece((row, col)).is_some() {
                    total_row += row as f64;
                    total_col += col as f64;
                    piece_count += 1;
                }
            }
        }

        if piece_count > 0 {
            (
                total_row / piece_count as f64,
                total_col / piece_count as f64,
            )
        } else {
            // If no pieces, use board center
            (
                (board_size.0 - 1) as f64 / 2.0,
                (board_size.1 - 1) as f64 / 2.0,
            )
        }
    }

    fn distance(p1: Position, p2: (f64, f64)) -> f64 {
        let dx = p1.0 as f64 - p2.0;
        let dy = p1.1 as f64 - p2.1;
        (dx * dx + dy * dy).sqrt()
    }
}

impl EvaluatorTrait for GravityEvaluator {
    fn evaluate(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        let my_color = state.current_turn;
        let enemy_color = my_color.opposite();

        // Calculate the center of mass of all pieces
        let center_of_mass = self.calculate_center_of_mass(&state.board);

        // Get all pieces by color
        let my_pieces = state.board.get_pieces_by_color(my_color);
        let enemy_pieces = state.board.get_pieces_by_color(enemy_color);

        // Calculate gravity score for my pieces (closer to center of mass is better)
        let mut my_gravity_score = 0.0;
        for (pos, _) in my_pieces {
            let dist = Self::distance(pos, center_of_mass);
            // Inverse distance scoring: closer pieces get higher scores
            // Add 1.0 to avoid division by zero for pieces at the exact center
            my_gravity_score += 10.0 / (dist + 1.0);
        }

        // Calculate gravity score for enemy pieces
        let mut enemy_gravity_score = 0.0;
        for (pos, _) in enemy_pieces {
            let dist = Self::distance(pos, center_of_mass);
            enemy_gravity_score += 10.0 / (dist + 1.0);
        }

        // Additional bonus for controlling the actual center of mass square
        let center_square = (
            center_of_mass.0.round() as usize,
            center_of_mass.1.round() as usize,
        );
        let board_size = state.board.size();

        let mut center_control_bonus = 0.0;
        if center_square.0 < board_size.0 && center_square.1 < board_size.1 {
            if let Some(piece) = state.board.get_piece(center_square) {
                if piece.color == my_color {
                    center_control_bonus = 5.0;
                } else {
                    center_control_bonus = -5.0;
                }
            }
        }

        // Return the difference in gravity scores scaled appropriately
        ((my_gravity_score - enemy_gravity_score + center_control_bonus) * 100.0) as i32
    }
}

impl ChessEngine for GravityEngine {
    fn name(&self) -> &str {
        "Gravity Engine (Center of Mass)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = GravityEvaluator;
        let mut search = Search::new(&evaluator);
        let depth = if params.depth > 0 { params.depth } else { 3 };

        // Use iterative deepening if time limit is specified
        let (best_move, evaluation, depth_reached) = if let Some(time_limit) = params.time_limit {
            search.find_best_move_iterative(
                params.state,
                params.move_generator,
                params.config_manager,
                depth,
                time_limit,
            )?
        } else {
            search.find_best_move_with_depth(
                params.state,
                params.move_generator,
                params.config_manager,
                depth,
            )?
        };

        let mate_in = if evaluation >= 999000 {
            Some(((999999 - evaluation) / 2) as i32)
        } else if evaluation <= -999000 {
            Some(-((-999999 - evaluation) / 2) as i32)
        } else {
            None
        };

        Some(SearchResult {
            best_move,
            evaluation: Evaluation {
                score: evaluation,
                mate_in,
            },
            depth_reached,
        })
    }

    fn stop(&mut self) {}
}
