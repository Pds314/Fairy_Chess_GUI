// src/engine/simple_engine.rs
use crate::engine::api::{ChessEngine, SearchParams, SearchResult};
use crate::engine::evaluator::Evaluator;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{run_search, TTEntry, SEARCH_PARAMETER_DEFS};
use std::collections::HashMap;

pub struct SimpleEngine {
    /// Persisted between moves so the TT accumulates across the game.
    transposition_table: HashMap<u64, TTEntry>,
    parameters: EngineParameters,
}

impl SimpleEngine {
    pub fn new() -> Self {
        SimpleEngine {
            transposition_table: HashMap::new(),
            parameters: EngineParameters::from_defaults(SEARCH_PARAMETER_DEFS),
        }
    }
}

impl Default for SimpleEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ChessEngine for SimpleEngine {
    fn name(&self) -> &str {
        "Simple Minimax Engine"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = Evaluator::new();
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = run_search(&evaluator, params, &self.parameters, &mut tt, 4);
        self.transposition_table = tt;
        result
    }

    /// `Search` has no external cancellation hook — its only stopping
    /// mechanisms are the hard/soft deadlines in
    /// `find_best_move_iterative`. The old `stop_flag: Arc<AtomicBool>` was
    /// write-only and has been removed rather than left as a lie.
    fn stop(&mut self) {}

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(SEARCH_PARAMETER_DEFS)
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
            self.transposition_table.clear();
        }
        changed
    }
}
