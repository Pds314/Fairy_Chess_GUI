// src/engine/api.rs
use crate::core::{GameState, Position, game_state::ExpandedMove};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::time::Duration;

pub trait ChessEngine: Send {
    fn name(&self) -> &str;
    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult>;
    fn stop(&mut self);

    /// Clears any persistent caches (like transposition tables or evaluation tables).
    /// Should be called when the game rules, board size, or variant changes.
    fn reset_cache(&mut self) {}

    fn analyze_position(
        &mut self,
        _state: &mut GameState,
        _move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::analysis::PositionAnalysis> {
        None
    }

    fn supports_analysis(&self) -> bool {
        false
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        None
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        None
    }

    fn set_parameters(&mut self, _params: EngineParameters) -> bool {
        false
    }
}

pub struct SearchParams<'a> {
    pub state: &'a mut GameState,
    pub move_generator: &'a MoveGenerator,
    pub config_manager: &'a PieceConfigManager,
    pub time_limit: Option<Duration>,
    pub depth: u32,
}

pub struct SearchResult {
    pub best_move: ExpandedMove,
    pub evaluation: Evaluation,
    pub depth_reached: u32,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub score: i32,
    pub mate_in: Option<i32>,
}
