// src/engine/territory_engine.rs
//
// Territory-control engine.
//
// Changes from the original:
//   * `TerritoryEvaluator` no longer carries an unused `move_generator` field.
//   * `protected_positions` no longer allocates a `Vec` on every leaf.
//   * `tc_contempt` was being *added to the leaf score*, which makes it a
//     tempo bonus (it alternates sign through negamax), not contempt. It is
//     now routed through `EvaluatorTrait::contempt()`; a separate, honest
//     `tc_tempo` parameter provides the old behaviour.
//   * The transposition table is moved rather than cloned each move.

use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::core::GameState;
use crate::engine::api::{ChessEngine, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{combined_params, run_search, TTEntry};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use smallvec::SmallVec;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const PARAM_MATERIAL_WEIGHT: &str = "tc_material_weight";
pub const PARAM_TERRITORY_WEIGHT: &str = "tc_territory_weight";
pub const PARAM_CONTROL_SATURATION: &str = "tc_control_saturation";
pub const PARAM_OCCUPIED_BONUS: &str = "tc_occupied_bonus";
pub const PARAM_CENTRALITY_WEIGHT: &str = "tc_centrality_weight";
pub const PARAM_MOBILITY_INFLUENCE: &str = "tc_mobility_influence";
pub const PARAM_KING_ZONE_WEIGHT: &str = "tc_king_zone_weight";
pub const PARAM_TEMPO: &str = "tc_tempo";
pub const PARAM_CONTEMPT: &str = "tc_contempt";

pub static TERRITORY_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MATERIAL_WEIGHT,
        "Material Weight",
        "How much raw material difference matters relative to territory.",
        0.0, 10.0, 2.0, 0.01,
    ),
ParameterDef::new(
    PARAM_TERRITORY_WEIGHT,
    "Territory Weight",
    "How much board control matters relative to material.",
    0.0, 10.0, 1.0, 0.01,
),
ParameterDef::new(
    PARAM_CONTROL_SATURATION,
    "Control Saturation",
    "How quickly additional attackers on the same square have diminishing returns.",
    0.1, 5.0, 1.5, 0.01,
),
ParameterDef::new(
    PARAM_OCCUPIED_BONUS,
    "Occupied Square Bonus",
    "Extra importance multiplier for controlling squares that have pieces on them.",
    0.0, 5.0, 1.0, 0.01,
),
ParameterDef::new(
    PARAM_CENTRALITY_WEIGHT,
    "Centrality Weight",
    "Bonus importance for controlling squares near the center of the board.",
    0.0, 3.0, 0.3, 0.01,
),
ParameterDef::new(
    PARAM_MOBILITY_INFLUENCE,
    "Mobility Influence",
    "How much non-capture moves contribute to control. 0 = only captures count.",
    0.0, 1.0, 0.15, 0.01,
),
ParameterDef::new(
    PARAM_KING_ZONE_WEIGHT,
    "King Zone Weight",
    "Extra importance for squares near royal/protected pieces.",
    0.0, 5.0, 1.0, 0.01,
),
ParameterDef::new(
    PARAM_TEMPO,
    "Tempo",
    "Flat bonus (centipawns) for the side to move. This is what the old `tc_contempt` actually did.",
                  0.0, 50.0, 10.0, 1.0,
),
ParameterDef::new(
    PARAM_CONTEMPT,
    "Contempt",
    "Draw dislike in centipawns, applied by the search (not baked into the leaf score).",
                  0.0, 50.0, 10.0, 1.0,
),
];

struct TerritoryData {
    board_size: (usize, usize),
    num_pieces: usize,
    intrinsic_values: Vec<f32>,
    control_weights: Vec<f32>,
    centrality_map: Vec<f32>,
    initial_piece_count: f32,

    material_weight: f32,
    territory_weight: f32,
    control_saturation: f32,
    occupied_bonus: f32,
    centrality_weight: f32,
    mobility_influence: f32,
    king_zone_weight: f32,
    tempo: f32,
}

impl TerritoryData {
    fn new(
        board: &Board,
        params: &EngineParameters,
        move_generator: &MoveGenerator,
        num_pieces: usize,
    ) -> Self {
        let board_size = board.size();
        let (rows, cols) = board_size;

        let intrinsic_values = compute_intrinsic_values(move_generator, num_pieces, board_size);
        let control_weights: Vec<f32> = intrinsic_values
        .iter()
        .map(|&v| if v > 0.01 { 1.0 / v.sqrt() } else { 1.0 })
        .collect();

        let center_r = (rows as f32 - 1.0) / 2.0;
        let center_c = (cols as f32 - 1.0) / 2.0;
        let max_dist = (center_r * center_r + center_c * center_c).sqrt().max(1.0);
        let mut centrality_map = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                let dr = r as f32 - center_r;
                let dc = c as f32 - center_c;
                centrality_map.push(1.0 - (dr * dr + dc * dc).sqrt() / max_dist);
            }
        }

        let initial_piece_count = (board.count_pieces() as f32).max(8.0);

        TerritoryData {
            board_size,
            num_pieces,
            intrinsic_values,
            control_weights,
            centrality_map,
            initial_piece_count,
            material_weight: params.get_or_default(PARAM_MATERIAL_WEIGHT, 2.0) as f32,
            territory_weight: params.get_or_default(PARAM_TERRITORY_WEIGHT, 1.0) as f32,
            control_saturation: params.get_or_default(PARAM_CONTROL_SATURATION, 1.5) as f32,
            occupied_bonus: params.get_or_default(PARAM_OCCUPIED_BONUS, 1.0) as f32,
            centrality_weight: params.get_or_default(PARAM_CENTRALITY_WEIGHT, 0.3) as f32,
            mobility_influence: params.get_or_default(PARAM_MOBILITY_INFLUENCE, 0.15) as f32,
            king_zone_weight: params.get_or_default(PARAM_KING_ZONE_WEIGHT, 1.0) as f32,
            tempo: params.get_or_default(PARAM_TEMPO, 10.0) as f32 / 100.0,
        }
    }
}

fn compute_intrinsic_values(
    move_generator: &MoveGenerator,
    num_pieces: usize,
    board_size: (usize, usize),
) -> Vec<f32> {
    let (rows, cols) = board_size;
    let total_squares = (rows * cols).max(1) as f64;
    let mut values = vec![0.0f32; num_pieces];

    for piece_type in 0..num_pieces {
        let mut total_attacks = 0usize;
        for row in 0..rows {
            for col in 0..cols {
                let moves = move_generator.generate_theoretical_moves_for_pst(
                    (row, col),
                                                                              piece_type,
                                                                              PieceColor::White,
                                                                              board_size,
                                                                              1,
                );
                total_attacks += moves
                .iter()
                .filter(|m| {
                    m.rule.can_land_enemy && !m.rule.is_king_castle && !m.rule.is_rook_castle
                })
                .count();
            }
        }
        values[piece_type] = (total_attacks as f64 / total_squares) as f32;
    }

    let min_val = values
    .iter()
    .copied()
    .filter(|&v| v > 0.001)
    .fold(f32::INFINITY, f32::min);
    let min_val = if min_val.is_finite() && min_val > 0.0 {
        min_val
    } else {
        1.0
    };

    for v in values.iter_mut() {
        if *v > 0.001 {
            *v /= min_val;
        } else {
            *v = 0.5;
        }
    }
    values
}

/// Protected pieces: all 'R', plus the last remaining 'r'. Allocation-free.
#[inline]
fn protected(board: &Board, color: PieceColor, out: &mut SmallVec<[Position; 4]>) {
    out.extend_from_slice(board.get_royal_positions(color));
    let royalty = board.get_royalty_positions(color);
    if royalty.len() == 1 {
        out.push(royalty[0]);
    }
}

pub struct TerritoryEngine {
    eval_data: RefCell<Option<TerritoryData>>,
    transposition_table: HashMap<u64, TTEntry>,
    parameters: EngineParameters,
    needs_reinit: bool,
}

impl TerritoryEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            eval_data: RefCell::new(None),
            transposition_table: HashMap::new(),
            parameters: EngineParameters::from_defaults(combined_params(
                TERRITORY_PARAMETERS,
                &MERGED,
            )),
            needs_reinit: true,
        }
    }

    fn initialize(
        &mut self,
        board: &Board,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let board_size = board.size();
        let num_pieces = config_manager.piece_order.len();

        let needs_rebuild = {
            let d = self.eval_data.borrow();
            self.needs_reinit
            || d.is_none()
            || d.as_ref()
            .map_or(true, |d| d.board_size != board_size || d.num_pieces != num_pieces)
        };
        if !needs_rebuild {
            return;
        }

        println!(
            "🗺️  Building Territory tables for {}x{} board, {} piece types…",
            board_size.0, board_size.1, num_pieces
        );

        let data = TerritoryData::new(board, &self.parameters, move_generator, num_pieces);
        for (i, name) in config_manager.piece_order.iter().enumerate() {
            if i < data.intrinsic_values.len() {
                println!(
                    "   {} : intrinsic={:.2}  control_weight={:.3}",
                    name, data.intrinsic_values[i], data.control_weights[i]
                );
            }
        }

        *self.eval_data.borrow_mut() = Some(data);
        self.needs_reinit = false;
        println!("✅ Territory tables ready.");
    }

    /// Score in "pawn units", side-to-move relative.
    fn evaluate_position(&self, state: &GameState, move_generator: &MoveGenerator) -> f32 {
        let data_ref = self.eval_data.borrow();
        let Some(data) = data_ref.as_ref() else {
            return 0.0;
        };

        let board = &state.board;
        let (rows, cols) = data.board_size;
        if board.size() != (rows, cols) {
            return 0.0;
        }
        let flat_size = rows * cols;

        let current_pieces = board.count_pieces() as f32;
        let end_threshold = 4.0f32;
        let phase = if data.initial_piece_count > end_threshold {
            ((current_pieces - end_threshold) / (data.initial_piece_count - end_threshold))
            .clamp(0.0, 1.0)
        } else {
            1.0
        };
        let effective_territory_weight = data.territory_weight * (0.4 + 0.6 * phase);

        let mut white_control = vec![0.0f32; flat_size];
        let mut black_control = vec![0.0f32; flat_size];
        let mut white_material = 0.0f32;
        let mut black_material = 0.0f32;

        for row in 0..rows {
            for col in 0..cols {
                let pos = (row, col);
                let Some(piece) = board.get_piece(pos) else { continue };
                if piece.piece_type >= data.num_pieces {
                    continue;
                }

                if !piece.is_royal {
                    let iv = data.intrinsic_values[piece.piece_type];
                    match piece.color {
                        PieceColor::White => white_material += iv,
                        PieceColor::Black => black_material += iv,
                    }
                }

                let cw = data.control_weights[piece.piece_type];
                let moves = move_generator.generate_moves_with_database(board, pos, piece.piece_type);
                let control_map = match piece.color {
                    PieceColor::White => &mut white_control,
                    PieceColor::Black => &mut black_control,
                };
                for mv in &moves {
                    if mv.rule.is_king_castle || mv.rule.is_rook_castle {
                        continue;
                    }
                    let dest = mv.destination;
                    if dest.0 >= rows || dest.1 >= cols {
                        continue;
                    }
                    let dest_flat = dest.0 * cols + dest.1;
                    let weight = if mv.rule.can_land_enemy {
                        cw
                    } else {
                        cw * data.mobility_influence
                    };
                    control_map[dest_flat] += weight;
                }
            }
        }

        let mut prot: SmallVec<[Position; 4]> = SmallVec::new();
        protected(board, PieceColor::White, &mut prot);
        protected(board, PieceColor::Black, &mut prot);

        let sat = data.control_saturation;
        let mut territory_score = 0.0f32;

        for row in 0..rows {
            for col in 0..cols {
                let flat = row * cols + col;
                let wc = white_control[flat];
                let bc = black_control[flat];
                if wc < 0.001 && bc < 0.001 {
                    continue;
                }

                let w_eff = wc / (sat + wc);
                let b_eff = bc / (sat + bc);
                let net = w_eff - b_eff;

                let mut importance = 1.0f32;
                importance += data.centrality_map[flat] * data.centrality_weight;

                if let Some(occ) = board.get_piece((row, col)) {
                    if occ.piece_type < data.num_pieces {
                        let iv = data.intrinsic_values[occ.piece_type];
                        importance += iv.sqrt() * data.occupied_bonus;
                    }
                }

                if data.king_zone_weight > 0.0 {
                    for &kp in prot.iter() {
                        let dr = (row as i32 - kp.0 as i32).unsigned_abs();
                        let dc = (col as i32 - kp.1 as i32).unsigned_abs();
                        let chebyshev = dr.max(dc) as f32;
                        if chebyshev <= 3.0 {
                            importance += data.king_zone_weight / (1.0 + chebyshev);
                        }
                    }
                }

                territory_score += net * importance;
            }
        }

        let material_diff = white_material - black_material;
        let score_white =
        data.material_weight * material_diff + effective_territory_weight * territory_score;

        let score = match state.current_turn {
            PieceColor::White => score_white,
            PieceColor::Black => -score_white,
        };

        score + data.tempo
    }
}

impl Default for TerritoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct TerritoryEvaluator<'a> {
    engine: &'a TerritoryEngine,
}

impl EvaluatorTrait for TerritoryEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        if !state.board.has_pieces(state.current_turn) {
            return -999_999;
        }
        let score = self.engine.evaluate_position(state, move_generator);
        (score * 100.0) as i32
    }

    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        _pos: Position,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let data = self.engine.eval_data.borrow();
        if let Some(d) = data.as_ref() {
            if piece.piece_type < d.num_pieces {
                return (d.intrinsic_values[piece.piece_type] * 100.0) as i32;
            }
        }
        100
    }

    fn delta_pruning_margin(&self) -> i32 {
        400
    }
    fn aspiration_window(&self) -> i32 {
        100
    }
    fn contempt(&self) -> i32 {
        self.engine.parameters.get_or_default(PARAM_CONTEMPT, 10.0) as i32
    }
}

impl ChessEngine for TerritoryEngine {
    fn name(&self) -> &str {
        "Territory Control Engine"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
        *self.eval_data.borrow_mut() = None;
        self.needs_reinit = true;
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.initialize(
            &params.state.board,
            params.move_generator,
            params.config_manager,
        );

        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let evaluator = TerritoryEvaluator { engine: &*self };
            run_search(&evaluator, params, &self.parameters, &mut tt, 4)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}
    fn supports_analysis(&self) -> bool {
        false
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(TERRITORY_PARAMETERS, &MERGED))
    }
    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }
    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
            self.needs_reinit = true;
            self.transposition_table.clear();
        }
        changed
    }
}
