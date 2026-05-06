// src/engine/swarm_engine.rs
use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::search::Search;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

pub struct SwarmEngine;

impl SwarmEngine {
    pub fn new() -> Self {
        SwarmEngine
    }
}

struct SwarmEvaluator;

impl SwarmEvaluator {
    fn find_royal_positions(
        &self,
        board: &Board,
        color: PieceColor,
        config_manager: &PieceConfigManager,
    ) -> Vec<Position> {
        board
            .get_pieces_by_color(color)
            .into_iter()
            .filter(|(_, piece)| {
                config_manager
                    .get_piece_by_index(piece.piece_type)
                    .map_or(false, |cfg| {
                        cfg.properties.is_royal || cfg.properties.is_royalty
                    })
            })
            .map(|(pos, _)| pos)
            .collect()
    }

    fn distance(p1: Position, p2: Position) -> f64 {
        (((p1.0 as f64 - p2.0 as f64).powi(2) + (p1.1 as f64 - p2.1 as f64).powi(2)) as f64).sqrt()
    }
}

impl EvaluatorTrait for SwarmEvaluator {
    fn evaluate(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        // <-- FIX: Changed &mut self to &self
        let my_color = state.current_turn;
        let enemy_color = my_color.opposite();

        let my_royals = self.find_royal_positions(&state.board, my_color, config_manager);
        let enemy_royals = self.find_royal_positions(&state.board, enemy_color, config_manager);

        let my_pieces = state.board.get_pieces_by_color(my_color);
        let enemy_pieces = state.board.get_pieces_by_color(enemy_color);

        let mut my_score = 0.0;
        for (pos, _) in my_pieces {
            // Swarm bonus for attacking enemy royals
            for royal_pos in &enemy_royals {
                let dist = Self::distance(pos, *royal_pos);
                if dist > 0.0 {
                    my_score += 1.0 / dist;
                }
            }
        }

        let mut enemy_score = 0.0;
        for (pos, _) in enemy_pieces {
            // Swarm bonus for enemy attacking my royals
            for royal_pos in &my_royals {
                let dist = Self::distance(pos, *royal_pos);
                if dist > 0.0 {
                    enemy_score += 1.0 / dist;
                }
            }
        }

        // Return score scaled to be significant for the search
        ((my_score - enemy_score) * 1000.0) as i32
    }
}

impl ChessEngine for SwarmEngine {
    fn name(&self) -> &str {
        "Swarm Engine"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = SwarmEvaluator;
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
