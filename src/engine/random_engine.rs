// src/engine/random_engine.rs
use crate::core::game_state::ExpandedMove;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use rand::prelude::*; // FIX: This necessary import was missing or removed.

pub struct RandomEngine;

impl RandomEngine {
    pub fn new() -> Self {
        RandomEngine
    }
}

impl ChessEngine for RandomEngine {
    fn name(&self) -> &str {
        "Random Move Engine"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        // Get all legal moves
        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);

        if legal_moves.is_empty() {
            return None;
        }

        // Pick a random move, using the original full path
        let mut rng = rand::thread_rng();
        let chosen_move = legal_moves.choose(&mut rng)?;

        Some(SearchResult {
            best_move: chosen_move.clone(),
            evaluation: Evaluation {
                score: 0, // Random engine doesn't evaluate
                mate_in: None,
            },
            depth_reached: 0, // No search depth for random
        })
    }

    fn stop(&mut self) {
        // Nothing to stop for random engine
    }
}
