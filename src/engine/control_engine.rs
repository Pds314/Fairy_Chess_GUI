// src/engine/control_engine.rs

use crate::core::GameState;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef, ParameterizedEngine};
use crate::engine::search::Search;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;

pub const PARAM_MATERIAL_WEIGHT: &str = "tc_material_weight";
pub const PARAM_TERRITORY_WEIGHT: &str = "tc_territory_weight";
pub const PARAM_ENEMY_OCC_BONUS: &str = "tc_enemy_occ_bonus";
pub const PARAM_DIMINISHING_POW: &str = "tc_diminishing_pow";
pub const PARAM_CENTER_WEIGHT: &str = "tc_center_weight";
pub const PARAM_CONTEMPT: &str = "tc_contempt";

pub static CONTROL_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MATERIAL_WEIGHT,
        "Material Weight",
        "Scales the raw intrinsic-value material difference.",
        0.0,
        5.0,
        1.0,
        0.05,
    ),
    ParameterDef::new(
        PARAM_TERRITORY_WEIGHT,
        "Territory Weight",
        "Scales the net territorial control score.",
        0.0,
        100.0,
        12.0,
        0.5,
    ),
    ParameterDef::new(
        PARAM_ENEMY_OCC_BONUS,
        "Enemy Piece Control Bonus",
        "Extra importance multiplier for controlling squares that hold enemy pieces.\
         Scales with the enemy piece's normalised intrinsic value.",
        0.0,
        20.0,
        4.0,
        0.1,
    ),
    ParameterDef::new(
        PARAM_DIMINISHING_POW,
        "Diminishing Power",
        "Exponent compressing stacked control on one square.\
         0.5 = sqrt (strong compression), 1.0 = linear (no compression).",
        0.2,
        1.0,
        0.6,
        0.05,
    ),
    ParameterDef::new(
        PARAM_CENTER_WEIGHT,
        "Center Weight",
        "Additional importance multiplier for central squares (0 = uniform).",
        0.0,
        5.0,
        0.5,
        0.1,
    ),
    ParameterDef::new(
        PARAM_CONTEMPT,
        "Contempt",
        "Centipawn penalty applied when the position has already occurred\
         twice and the eval is positive. Encourages avoiding repetition draws.",
        0.0,
        100.0,
        15.0,
        1.0,
    ),
];

struct ControlEvalData {
    board_size: (usize, usize),
    num_pieces: usize,
    intrinsic_values: Vec<f32>,
    control_weights: Vec<f32>,
    center_factors: Vec<f32>,
    max_intrinsic: f32,
}

impl ControlEvalData {
    fn new(
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let (rows, cols) = board_size;
        let num_pieces = config_manager.piece_order.len();
        let total_sq = (rows * cols) as f64;

        let mut raw_mob = vec![0.0f64; num_pieces];
        for pt in 0..num_pieces {
            let mut sum = 0.0f64;
            for r in 0..rows {
                for c in 0..cols {
                    let moves = move_generator.generate_theoretical_moves_for_pst(
                        (r, c),
                        pt,
                        PieceColor::White,
                        board_size,
                        1,
                    );
                    let cnt = moves
                        .iter()
                        .filter(|m| !m.rule.is_king_castle && !m.rule.is_rook_castle)
                        .count();
                    sum += cnt as f64;
                }
            }
            raw_mob[pt] = sum / total_sq;
        }

        let min_mob = raw_mob
            .iter()
            .copied()
            .filter(|&v| v > 0.0)
            .fold(f64::INFINITY, f64::min);
        let scale = if min_mob > 0.0 && min_mob.is_finite() {
            100.0 / min_mob
        } else {
            100.0
        };

        let mut intrinsic_values = vec![0.0f32; num_pieces];
        for i in 0..num_pieces {
            intrinsic_values[i] = if raw_mob[i] > 0.0 {
                (raw_mob[i] * scale) as f32
            } else {
                100.0
            };
        }

        let min_intr = intrinsic_values
            .iter()
            .copied()
            .filter(|&v| v > 0.0)
            .fold(f32::INFINITY, f32::min)
            .min(100.0);
        let max_intr = intrinsic_values
            .iter()
            .copied()
            .fold(0.0f32, f32::max)
            .max(100.0);

        let mut control_weights = vec![0.0f32; num_pieces];
        for i in 0..num_pieces {
            control_weights[i] = if intrinsic_values[i] > 0.0 {
                min_intr / intrinsic_values[i]
            } else {
                0.0
            };
        }

        let cr = (rows as f32 - 1.0) / 2.0;
        let cc = (cols as f32 - 1.0) / 2.0;
        let max_d = (cr * cr + cc * cc).sqrt();
        let mut center_factors = vec![0.0f32; rows * cols];
        for r in 0..rows {
            for c in 0..cols {
                let dr = r as f32 - cr;
                let dc = c as f32 - cc;
                let d = (dr * dr + dc * dc).sqrt();
                center_factors[r * cols + c] = if max_d > 0.0 { 1.0 - d / max_d } else { 1.0 };
            }
        }

        Self {
            board_size,
            num_pieces,
            intrinsic_values,
            control_weights,
            center_factors,
            max_intrinsic: max_intr,
        }
    }
}

struct ControlEvaluator<'a> {
    engine: &'a ControlEngine,
}

impl<'a> EvaluatorTrait for ControlEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        self.engine.evaluate_position(state, move_generator)
    }

    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        _pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        match &self.engine.eval_data {
            Some(d) if piece.piece_type < d.num_pieces => {
                d.intrinsic_values[piece.piece_type] as i32
            }
            _ => 100,
        }
    }
}

pub struct ControlEngine {
    eval_data: Option<ControlEvalData>,
    transposition_table: HashMap<u64, crate::engine::search::TTEntry>,
    parameters: EngineParameters,
    needs_reinit: bool,
}

impl ControlEngine {
    pub fn new() -> Self {
        Self {
            eval_data: None,
            transposition_table: HashMap::new(),
            parameters: EngineParameters::from_defaults(CONTROL_PARAMETERS),
            needs_reinit: true,
        }
    }

    #[inline]
    fn p(&self, id: &str, default: f64) -> f32 {
        self.parameters.get_or_default(id, default) as f32
    }

    fn initialize(
        &mut self,
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let needs = self.needs_reinit
            || self.eval_data.is_none()
            || self
                .eval_data
                .as_ref()
                .map_or(true, |d| d.board_size != board_size);
        if !needs {
            return;
        }

        println!(
            "🗺️  Building Control Engine tables for {}×{}, {} piece types…",
            board_size.0,
            board_size.1,
            config_manager.piece_order.len(),
        );
        let data = ControlEvalData::new(board_size, move_generator, config_manager);
        for (i, name) in config_manager.piece_order.iter().enumerate() {
            if i < data.intrinsic_values.len() {
                println!(
                    "  {:>10}: intrinsic {:>7.1} cp   control_wt {:.4}",
                    name, data.intrinsic_values[i], data.control_weights[i],
                );
            }
        }
        self.eval_data = Some(data);
        self.needs_reinit = false;
        println!("✅ Control Engine tables ready.");
    }

    fn evaluate_position(&self, state: &GameState, move_generator: &MoveGenerator) -> i32 {
        let data = match &self.eval_data {
            Some(d) => d,
            None => return 0,
        };
        let (rows, cols) = data.board_size;
        if (rows, cols) != state.board.size() {
            return 0;
        }
        if !state.board.has_pieces(state.current_turn) {
            return -999999;
        }

        let flat_size = rows * cols;
        let current = state.current_turn;
        let mat_w = self.p(PARAM_MATERIAL_WEIGHT, 1.0);
        let ter_w = self.p(PARAM_TERRITORY_WEIGHT, 12.0);
        let eocc = self.p(PARAM_ENEMY_OCC_BONUS, 4.0);
        let dim = self.p(PARAM_DIMINISHING_POW, 0.6);
        let cen_w = self.p(PARAM_CENTER_WEIGHT, 0.5);
        let contempt = self.p(PARAM_CONTEMPT, 15.0);

        let mut white_ctrl = vec![0.0f32; flat_size];
        let mut black_ctrl = vec![0.0f32; flat_size];
        let mut white_mat = 0.0f32;
        let mut black_mat = 0.0f32;

        for row in 0..rows {
            for col in 0..cols {
                let pos = (row, col);
                let piece = match state.board.get_piece(pos) {
                    Some(p) => p,
                    None => continue,
                };
                if piece.piece_type >= data.num_pieces {
                    continue;
                }

                let intr = data.intrinsic_values[piece.piece_type];
                match piece.color {
                    PieceColor::White => white_mat += intr,
                    PieceColor::Black => black_mat += intr,
                }

                let cw = data.control_weights[piece.piece_type];
                let moves = move_generator.generate_moves_with_database(
                    &state.board,
                    pos,
                    piece.piece_type,
                );
                let map = match piece.color {
                    PieceColor::White => &mut white_ctrl,
                    PieceColor::Black => &mut black_ctrl,
                };

                for mv in &moves {
                    if mv.rule.is_king_castle || mv.rule.is_rook_castle {
                        continue;
                    }
                    let d = mv.destination;
                    if d.0 < rows && d.1 < cols {
                        map[d.0 * cols + d.1] += cw;
                    }
                }
            }
        }

        let inv_max = if data.max_intrinsic > 0.0 {
            1.0 / data.max_intrinsic
        } else {
            1.0
        };

        let mut territory = 0.0f32;
        for row in 0..rows {
            for col in 0..cols {
                let flat = row * cols + col;
                let wc = white_ctrl[flat];
                let bc = black_ctrl[flat];

                let w_eff = if wc > 0.0 { wc.powf(dim) } else { 0.0 };
                let b_eff = if bc > 0.0 { bc.powf(dim) } else { 0.0 };

                let (my, their) = match current {
                    PieceColor::White => (w_eff, b_eff),
                    PieceColor::Black => (b_eff, w_eff),
                };
                let net = my - their;

                let mut importance = 1.0 + cen_w * data.center_factors[flat];
                if let Some(occ) = state.board.get_piece((row, col)) {
                    if occ.piece_type < data.num_pieces {
                        let norm_val = data.intrinsic_values[occ.piece_type] * inv_max;
                        if occ.color != current {
                            importance += norm_val * eocc;
                        }
                    }
                }
                territory += net * importance;
            }
        }

        let mat_diff = match current {
            PieceColor::White => white_mat - black_mat,
            PieceColor::Black => black_mat - white_mat,
        };

        let mut score = mat_w * mat_diff + ter_w * territory;

        if contempt > 0.0 && score > 0.0 {
            let hash = state.current_hash();
            if let Some(&count) = state.position_history.get(&hash) {
                if count >= 2 {
                    score -= contempt;
                }
            }
        }
        score as i32
    }
}

impl ParameterizedEngine for ControlEngine {
    fn parameter_definitions(&self) -> &'static [ParameterDef] {
        CONTROL_PARAMETERS
    }
    fn get_parameters(&self) -> &EngineParameters {
        &self.parameters
    }
    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
            self.needs_reinit = true;
        }
        changed
    }
    fn on_parameters_changed(&mut self) {
        self.needs_reinit = true;
        self.eval_data = None;
        self.transposition_table.clear();
        println!("🔄 Control Engine parameters changed — tables will be recomputed.");
    }
}

impl ChessEngine for ControlEngine {
    fn name(&self) -> &str {
        "Control Engine (Diminishing Territory)"
    }
    fn reset_cache(&mut self) {
        self.transposition_table.clear();
        self.eval_data = None;
        self.needs_reinit = true;
    }
    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.initialize(
            params.state.board.size(),
            params.move_generator,
            params.config_manager,
        );
        let evaluator = ControlEvaluator { engine: self };
        let mut search = Search::new(&evaluator);
        search.set_transposition_table(self.transposition_table.clone());

        let depth = if params.depth > 0 { params.depth } else { 4 };
        let result = if let Some(time_limit) = params.time_limit {
            search.find_best_move_iterative(
                params.state,
                params.move_generator,
                params.config_manager,
                depth,
                time_limit,
            )
        } else {
            search.find_best_move_with_depth(
                params.state,
                params.move_generator,
                params.config_manager,
                depth,
            )
        };
        self.transposition_table = search.get_transposition_table();
        let (best_move, evaluation, depth_reached) = result?;

        let mate_in = if evaluation >= 999_000 {
            Some(((999_999 - evaluation) / 2) as i32)
        } else if evaluation <= -999_000 {
            Some(-((-999_999 - evaluation) / 2) as i32)
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
