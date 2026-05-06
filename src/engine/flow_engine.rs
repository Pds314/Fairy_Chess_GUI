// src/engine/flow_engine.rs
use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::search::Search;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashSet;

/// Engine that evaluates positions based on piece connectivity and flow patterns
/// It rewards positions where pieces protect each other and form connected networks
pub struct FlowEngine;

impl FlowEngine {
    pub fn new() -> Self {
        FlowEngine
    }
}

struct FlowEvaluator;

impl FlowEvaluator {
    /// Check if two positions are adjacent (including diagonals)
    fn are_adjacent(p1: Position, p2: Position) -> bool {
        let row_diff = (p1.0 as i32 - p2.0 as i32).abs();
        let col_diff = (p1.1 as i32 - p2.1 as i32).abs();
        row_diff <= 1 && col_diff <= 1 && (row_diff > 0 || col_diff > 0)
    }

    /// Calculate connectivity score for a color
    /// Pieces that can move to the same square or protect each other score points
    fn calculate_connectivity(
        &self,
        state: &GameState,
        color: PieceColor,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> f64 {
        let pieces = state.board.get_pieces_by_color(color);
        let mut connectivity_score = 0.0;

        // Create a map of which squares each piece can reach
        let mut piece_reach_map: Vec<(Position, HashSet<Position>)> = Vec::new();

        // Get moves for pieces of this color
        for (pos, piece) in &pieces {
            let mut reachable = HashSet::new();

            // Generate moves for this specific piece
            let moves =
                move_generator.generate_moves_with_details(&state.board, *pos, piece.piece_type);

            for mv in moves {
                reachable.insert(mv.destination);
            }

            piece_reach_map.push((*pos, reachable));
        }

        // Calculate connectivity based on shared target squares and mutual protection
        for i in 0..piece_reach_map.len() {
            for j in i + 1..piece_reach_map.len() {
                let (pos1, reach1) = &piece_reach_map[i];
                let (pos2, reach2) = &piece_reach_map[j];

                // Bonus for pieces that can reach the same squares (coordination)
                let shared_squares = reach1.intersection(reach2).count();
                connectivity_score += shared_squares as f64 * 0.5;

                // Bonus for pieces protecting each other
                if reach1.contains(pos2) {
                    connectivity_score += 2.0;
                }
                if reach2.contains(pos1) {
                    connectivity_score += 2.0;
                }

                // Small bonus for pieces being adjacent (forming chains)
                if Self::are_adjacent(*pos1, *pos2) {
                    connectivity_score += 1.0;
                }
            }
        }

        // Bonus for having multiple pieces that can reach the center
        let board_size = state.board.size();
        let center_row = board_size.0 / 2;
        let center_col = board_size.1 / 2;
        let center_squares = vec![
            (center_row, center_col),
            (center_row.saturating_sub(1), center_col),
            (center_row + 1, center_col.min(board_size.0 - 1)),
            (center_row, center_col.saturating_sub(1)),
            (center_row, (center_col + 1).min(board_size.1 - 1)),
        ];

        for (_pos, reachable) in &piece_reach_map {
            for &center in &center_squares {
                if reachable.contains(&center) {
                    connectivity_score += 0.3;
                }
            }
        }

        connectivity_score
    }

    /// Calculate flow patterns - rewards pieces moving in coordinated directions
    fn calculate_flow_pattern(&self, state: &GameState, color: PieceColor) -> f64 {
        let pieces = state.board.get_pieces_by_color(color);
        if pieces.len() < 2 {
            return 0.0;
        }

        let mut flow_score = 0.0;
        let board_size = state.board.size();

        // Calculate average position (centroid)
        let mut avg_row = 0.0;
        let mut avg_col = 0.0;
        for (pos, _) in &pieces {
            avg_row += pos.0 as f64;
            avg_col += pos.1 as f64;
        }
        avg_row /= pieces.len() as f64;
        avg_col /= pieces.len() as f64;

        // Reward pieces that form patterns relative to centroid
        let mut variance_row = 0.0;
        let mut variance_col = 0.0;

        for (pos, _) in &pieces {
            variance_row += (pos.0 as f64 - avg_row).powi(2);
            variance_col += (pos.1 as f64 - avg_col).powi(2);
        }

        variance_row /= pieces.len() as f64;
        variance_col /= pieces.len() as f64;

        // Reward moderate spread (not too clustered, not too scattered)
        let ideal_variance = ((board_size.0.min(board_size.1) as f64) / 4.0).powi(2);
        let total_variance = variance_row + variance_col;

        if total_variance > 0.0 {
            // Score peaks when variance equals ideal_variance
            let variance_ratio = if total_variance < ideal_variance {
                total_variance / ideal_variance
            } else {
                ideal_variance / total_variance
            };
            flow_score += variance_ratio * 10.0;
        }

        // Bonus for diagonal alignments (creates dynamic patterns)
        for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                let (pos1, _) = pieces[i];
                let (pos2, _) = pieces[j];

                let row_diff = (pos1.0 as i32 - pos2.0 as i32).abs();
                let col_diff = (pos1.1 as i32 - pos2.1 as i32).abs();

                // Perfect diagonal alignment
                if row_diff == col_diff && row_diff > 0 {
                    flow_score += 1.5;
                }
                // Straight line alignment (row or column)
                else if row_diff == 0 && col_diff > 0 {
                    flow_score += 1.0;
                } else if col_diff == 0 && row_diff > 0 {
                    flow_score += 1.0;
                }
            }
        }

        flow_score
    }
}

impl EvaluatorTrait for FlowEvaluator {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let my_color = state.current_turn;
        let enemy_color = my_color.opposite();

        // Calculate connectivity scores
        let my_connectivity =
            self.calculate_connectivity(state, my_color, move_generator, config_manager);
        let enemy_connectivity =
            self.calculate_connectivity(state, enemy_color, move_generator, config_manager);

        // Calculate flow pattern scores
        let my_flow = self.calculate_flow_pattern(state, my_color);
        let enemy_flow = self.calculate_flow_pattern(state, enemy_color);

        // Combine scores with appropriate weights
        let connectivity_weight = 10.0;
        let flow_weight = 5.0;

        let my_total = my_connectivity * connectivity_weight + my_flow * flow_weight;
        let enemy_total = enemy_connectivity * connectivity_weight + enemy_flow * flow_weight;

        // Return the difference scaled to centipawns
        ((my_total - enemy_total) * 10.0) as i32
    }
}

impl ChessEngine for FlowEngine {
    fn name(&self) -> &str {
        "Flow Engine (Connectivity & Patterns)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = FlowEvaluator;
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
