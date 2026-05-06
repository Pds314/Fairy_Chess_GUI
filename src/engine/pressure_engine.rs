// Create new file: src/engine/pressure_engine.rs

use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef, ParameterizedEngine};
use crate::engine::search::Search;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;

pub const PARAM_ZONE_SIZE: &str = "zone_size";
pub const PARAM_CONTROL_WEIGHT: &str = "control_weight";
pub const PARAM_CONTESTED_BONUS: &str = "contested_bonus";
pub const PARAM_EDGE_PENALTY: &str = "edge_penalty";

pub static PRESSURE_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_ZONE_SIZE,
        "Zone Size",
        "Size of pressure zones (2-4). Smaller = more granular analysis.",
        2.0,
        4.0,
        3.0,
        1.0,
    ),
    ParameterDef::new(
        PARAM_CONTROL_WEIGHT,
        "Control Weight",
        "How much zone control matters. Higher = dominance is more important.",
        0.5,
        3.0,
        1.0,
        0.1,
    ),
    ParameterDef::new(
        PARAM_CONTESTED_BONUS,
        "Contested Zone Bonus",
        "Bonus for zones where both sides have pieces. Rewards fighting for key squares.",
        0.0,
        2.0,
        0.5,
        0.1,
    ),
    ParameterDef::new(
        PARAM_EDGE_PENALTY,
        "Edge Zone Penalty",
        "Penalty for control of edge zones. Higher = prefers central control.",
        0.0,
        1.0,
        0.3,
        0.1,
    ),
];

/// Engine that evaluates based on board zone control and pressure
pub struct PressureEngine {
    parameters: EngineParameters,
}

impl PressureEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(PRESSURE_PARAMETERS),
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }
}

struct PressureEvaluator<'a> {
    engine: &'a PressureEngine,
}

#[derive(Debug, Clone)]
struct Zone {
    top_left: Position,
    size: usize,
    white_pieces: Vec<Position>,
    black_pieces: Vec<Position>,
    white_attacks: usize,
    black_attacks: usize,
}

impl<'a> PressureEvaluator<'a> {
    /// Divide board into zones and calculate control
    fn calculate_zones(&self, state: &GameState, zone_size: usize) -> Vec<Zone> {
        let (rows, cols) = state.board.size();
        let mut zones = Vec::new();

        for zone_row in (0..rows).step_by(zone_size) {
            for zone_col in (0..cols).step_by(zone_size) {
                let mut zone = Zone {
                    top_left: (zone_row, zone_col),
                    size: zone_size,
                    white_pieces: Vec::new(),
                    black_pieces: Vec::new(),
                    white_attacks: 0,
                    black_attacks: 0,
                };

                // Count pieces in zone
                for r in zone_row..zone_row.saturating_add(zone_size).min(rows) {
                    for c in zone_col..zone_col.saturating_add(zone_size).min(cols) {
                        if let Some(piece) = state.board.get_piece((r, c)) {
                            match piece.color {
                                PieceColor::White => zone.white_pieces.push((r, c)),
                                PieceColor::Black => zone.black_pieces.push((r, c)),
                            }
                        }
                    }
                }

                zones.push(zone);
            }
        }

        zones
    }

    /// Calculate attack/defense pressure on zones
    fn calculate_zone_pressure(
        &self,
        zones: &mut [Zone],
        state: &GameState,
        move_generator: &MoveGenerator,
    ) {
        // For each piece, count which zones it can attack
        let white_pieces = state.board.get_pieces_by_color(PieceColor::White);
        let black_pieces = state.board.get_pieces_by_color(PieceColor::Black);

        for (pos, piece) in white_pieces {
            let moves =
                move_generator.generate_moves_with_details(&state.board, pos, piece.piece_type);

            for mv in moves {
                for zone in zones.iter_mut() {
                    if self.position_in_zone(mv.destination, zone) {
                        zone.white_attacks += 1;
                    }
                }
            }
        }

        for (pos, piece) in black_pieces {
            let moves =
                move_generator.generate_moves_with_details(&state.board, pos, piece.piece_type);

            for mv in moves {
                for zone in zones.iter_mut() {
                    if self.position_in_zone(mv.destination, zone) {
                        zone.black_attacks += 1;
                    }
                }
            }
        }
    }

    fn position_in_zone(&self, pos: Position, zone: &Zone) -> bool {
        pos.0 >= zone.top_left.0
            && pos.0 < zone.top_left.0 + zone.size
            && pos.1 >= zone.top_left.1
            && pos.1 < zone.top_left.1 + zone.size
    }

    fn is_edge_zone(&self, zone: &Zone, board_size: (usize, usize)) -> bool {
        zone.top_left.0 == 0
            || zone.top_left.1 == 0
            || zone.top_left.0 + zone.size >= board_size.0
            || zone.top_left.1 + zone.size >= board_size.1
    }

    fn evaluate_zones(&self, zones: &[Zone], board_size: (usize, usize)) -> f64 {
        let control_weight = self.engine.get_param(PARAM_CONTROL_WEIGHT, 1.0);
        let contested_bonus = self.engine.get_param(PARAM_CONTESTED_BONUS, 0.5);
        let edge_penalty = self.engine.get_param(PARAM_EDGE_PENALTY, 0.3);

        let mut total_score = 0.0;

        for zone in zones {
            let white_control = zone.white_pieces.len() as f64 + zone.white_attacks as f64 * 0.5;
            let black_control = zone.black_pieces.len() as f64 + zone.black_attacks as f64 * 0.5;

            let zone_score = (white_control - black_control) * control_weight;

            // Bonus for contested zones (both sides present)
            let contested = if !zone.white_pieces.is_empty() && !zone.black_pieces.is_empty() {
                contested_bonus
            } else {
                0.0
            };

            // Penalty for edge control
            let edge_factor = if self.is_edge_zone(zone, board_size) {
                1.0 - edge_penalty
            } else {
                1.0
            };

            total_score += (zone_score + contested) * edge_factor;
        }

        total_score
    }
}

impl<'a> EvaluatorTrait for PressureEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        let zone_size = self.engine.get_param(PARAM_ZONE_SIZE, 3.0) as usize;
        let zone_size = zone_size.max(2).min(4);

        let mut zones = self.calculate_zones(state, zone_size);
        self.calculate_zone_pressure(&mut zones, state, move_generator);

        let score = self.evaluate_zones(&zones, state.board.size());
        (score * 100.0) as i32
    }
}

impl ParameterizedEngine for PressureEngine {
    fn parameter_definitions(&self) -> &'static [ParameterDef] {
        PRESSURE_PARAMETERS
    }

    fn get_parameters(&self) -> &EngineParameters {
        &self.parameters
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
        }
        changed
    }

    fn on_parameters_changed(&mut self) {
        // Pressure engine doesn't need reinitialization
    }
}

impl ChessEngine for PressureEngine {
    fn name(&self) -> &str {
        "Pressure Engine (Zone Control)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = PressureEvaluator { engine: self };
        let mut search = Search::new(&evaluator);
        let depth = if params.depth > 0 { params.depth } else { 3 };

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

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(ParameterizedEngine::parameter_definitions(self))
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(ParameterizedEngine::get_parameters(self).clone())
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        ParameterizedEngine::set_parameters(self, params)
    }
}
