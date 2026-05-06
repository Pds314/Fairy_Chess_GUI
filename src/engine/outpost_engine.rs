// src/engine/outpost_engine.rs
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

// Parameter IDs
pub const PARAM_OUTPOST_VALUE: &str = "outpost_value";
pub const PARAM_SUPPORT_BONUS: &str = "support_bonus";
pub const PARAM_ENEMY_TERRITORY_MULT: &str = "enemy_territory_mult";
pub const PARAM_CENTRAL_BONUS: &str = "central_bonus";

// Parameter definitions
pub static OUTPOST_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_OUTPOST_VALUE,
        "Outpost Base Value",
        "Base value for pieces in advanced positions. Higher = more aggressive.",
        0.0,
        100.0,
        20.0,
        1.0,
    ),
    ParameterDef::new(
        PARAM_SUPPORT_BONUS,
        "Support Bonus",
        "Bonus for outpost pieces protected by pawns or other pieces.",
        0.0,
        50.0,
        15.0,
        1.0,
    ),
    ParameterDef::new(
        PARAM_ENEMY_TERRITORY_MULT,
        "Enemy Territory Multiplier",
        "Multiplier for pieces deep in enemy territory.",
        1.0,
        3.0,
        1.5,
        0.1,
    ),
    ParameterDef::new(
        PARAM_CENTRAL_BONUS,
        "Central Files Bonus",
        "Extra bonus for outposts in central files.",
        0.0,
        30.0,
        10.0,
        1.0,
    ),
];

/// Engine that values establishing outposts in enemy territory
pub struct OutpostEngine {
    parameters: EngineParameters,
}

impl OutpostEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(OUTPOST_PARAMETERS),
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }
}

struct OutpostEvaluator<'a> {
    engine: &'a OutpostEngine,
}

impl<'a> OutpostEvaluator<'a> {
    /// Check if a position is an outpost (advanced and relatively safe)
    fn evaluate_outpost_quality(
        &self,
        pos: Position,
        piece: &Piece,
        board: &Board,
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
    ) -> f64 {
        let mut quality = 0.0;

        // How far into enemy territory?
        let advancement = match piece.color {
            PieceColor::White => (board_size.0 - 1 - pos.0) as f64 / board_size.0 as f64,
            PieceColor::Black => pos.0 as f64 / board_size.0 as f64,
        };

        // Only consider positions in enemy half
        if advancement < 0.5 {
            return 0.0;
        }

        let base_value = self.engine.get_param(PARAM_OUTPOST_VALUE, 20.0);
        quality += base_value * (advancement - 0.5) * 2.0; // Scale from 0 at midfield to full value at back rank

        // Check if supported by friendly pieces
        let support_bonus = self.engine.get_param(PARAM_SUPPORT_BONUS, 15.0);
        let supporters = move_generator.get_attackers_to_square(board, pos, piece.color);
        quality += support_bonus * (supporters.len() as f64).min(2.0); // Cap at 2 supporters

        // Check if in enemy territory (past 6th rank for white, past 3rd for black)
        let deep_advancement = match piece.color {
            PieceColor::White => pos.0 <= 2,
            PieceColor::Black => pos.0 >= board_size.0 - 3,
        };

        if deep_advancement {
            let territory_mult = self.engine.get_param(PARAM_ENEMY_TERRITORY_MULT, 1.5);
            quality *= territory_mult;
        }

        // Bonus for central files
        let center_col = board_size.1 / 2;
        let col_distance = (pos.1 as i32 - center_col as i32).abs();
        if col_distance <= 1 {
            let central_bonus = self.engine.get_param(PARAM_CENTRAL_BONUS, 10.0);
            quality += central_bonus * (2.0 - col_distance as f64) / 2.0;
        }

        quality
    }
}

impl<'a> EvaluatorTrait for OutpostEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let my_color = state.current_turn;
        let enemy_color = my_color.opposite();
        let board_size = state.board.size();

        let mut my_outpost_score = 0.0;
        let mut enemy_outpost_score = 0.0;

        // Evaluate outposts for both sides
        for (pos, piece) in state.board.get_pieces_by_color(my_color) {
            // Skip pawns and royalty from outpost consideration
            if let Some(piece_config) = config_manager.get_piece_by_index(piece.piece_type) {
                if piece_config.properties.is_royal || piece_config.properties.is_royalty {
                    continue;
                }
                // You might want to check if it's a pawn and skip those too
            }

            my_outpost_score += self.evaluate_outpost_quality(
                pos,
                &piece,
                &state.board,
                board_size,
                move_generator,
            );
        }

        for (pos, piece) in state.board.get_pieces_by_color(enemy_color) {
            if let Some(piece_config) = config_manager.get_piece_by_index(piece.piece_type) {
                if piece_config.properties.is_royal || piece_config.properties.is_royalty {
                    continue;
                }
            }

            enemy_outpost_score += self.evaluate_outpost_quality(
                pos,
                &piece,
                &state.board,
                board_size,
                move_generator,
            );
        }

        // Add a small material component to avoid sacrificing everything
        let material_weight = 0.1;
        let my_material = state.board.get_pieces_by_color(my_color).len() as f64;
        let enemy_material = state.board.get_pieces_by_color(enemy_color).len() as f64;
        let material_diff = (my_material - enemy_material) * 100.0 * material_weight;

        // Return combined score
        ((my_outpost_score - enemy_outpost_score + material_diff) * 10.0) as i32
    }
}

impl ParameterizedEngine for OutpostEngine {
    fn parameter_definitions(&self) -> &'static [ParameterDef] {
        OUTPOST_PARAMETERS
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
        // No special reinitialization needed
    }
}

impl ChessEngine for OutpostEngine {
    fn name(&self) -> &str {
        "Outpost Engine (Territory Control)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let evaluator = OutpostEvaluator { engine: self };
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
