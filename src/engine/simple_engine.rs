// src/engine/simple_engine.rs
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::{Evaluator, EvaluatorTrait};
use crate::engine::search::Search;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SimpleEngine {
    stop_flag: Arc<AtomicBool>,
    search: Option<Search<'static, Evaluator>>, // Use the concrete Evaluator type
}

impl SimpleEngine {
    pub fn new() -> Self {
        SimpleEngine {
            stop_flag: Arc::new(AtomicBool::new(false)),
            search: None,
        }
    }
}

impl ChessEngine for SimpleEngine {
    fn name(&self) -> &str {
        "Simple Minimax Engine"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.stop_flag.store(false, Ordering::Relaxed);

        if self.search.is_none() {
            let evaluator = Box::leak(Box::new(Evaluator::new()));
            self.search = Some(Search::new(evaluator));
        }

        let search = self.search.as_mut().unwrap();
        let depth = if params.depth > 0 { params.depth } else { 4 };

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
            best_move, // Changed to pass the whole struct
            evaluation: Evaluation {
                score: evaluation,
                mate_in,
            },
            depth_reached,
        })
    }

    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    fn reset_cache(&mut self) {
        self.search = None;
    }
}
