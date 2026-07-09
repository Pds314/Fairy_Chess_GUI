// src/engine/random_engine.rs
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use rand::prelude::*; // rand 0.9: brings `IndexedRandom::choose`

pub struct RandomEngine;

impl RandomEngine {
    pub fn new() -> Self {
        RandomEngine
    }
}

impl Default for RandomEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ChessEngine for RandomEngine {
    fn name(&self) -> &str {
        "Random Move Engine"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let legal_moves = params
        .state
        .get_legal_moves(params.move_generator, params.config_manager);

        // rand 0.9 deprecates `thread_rng()` in favour of `rng()`, which is
        // what `promotion.rs` already uses. `choose` returns None on an empty
        // slice, so the explicit is_empty check is redundant.
        let mut rng = rand::rng();
        let chosen_move = legal_moves.choose(&mut rng)?;

        Some(SearchResult {
            best_move: chosen_move.clone(),
             evaluation: Evaluation {
                 score: 0,
                 mate_in: None,
             },
             depth_reached: 0,
        })
    }

    fn stop(&mut self) {}
}
