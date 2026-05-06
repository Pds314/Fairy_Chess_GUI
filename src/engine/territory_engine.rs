// src/engine/territory_engine.rs
//
// Territory-control chess engine for fairy chess variants.
//
// Evaluates positions by computing which side has more influence over
// each square of the board, weighted by the importance of that square
// (centrality, piece occupancy, king proximity). Combined with a
// material term based on intrinsic piece values derived from average
// attack footprint.
//
// Key design principles:
//   • Low-value pieces contribute MORE control per attack than high-value
//     pieces (a pawn attacking a square is stronger control than a queen).
//   • Overdefending a single square has diminishing returns — breadth of
//     control is preferred over depth.
//   • Controlling squares with enemy pieces is valued proportionally to
//     the piece's value (threatening it).
//   • Entirely variant-agnostic: intrinsic values are derived from
//     average mobility, not hard-coded.

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
use std::cell::RefCell;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────
// Parameter IDs
// ─────────────────────────────────────────────────────────────────────────

pub const PARAM_MATERIAL_WEIGHT: &str = "tc_material_weight";
pub const PARAM_TERRITORY_WEIGHT: &str = "tc_territory_weight";
pub const PARAM_CONTROL_SATURATION: &str = "tc_control_saturation";
pub const PARAM_OCCUPIED_BONUS: &str = "tc_occupied_bonus";
pub const PARAM_CENTRALITY_WEIGHT: &str = "tc_centrality_weight";
pub const PARAM_MOBILITY_INFLUENCE: &str = "tc_mobility_influence";
pub const PARAM_KING_ZONE_WEIGHT: &str = "tc_king_zone_weight";
pub const PARAM_CONTEMPT: &str = "tc_contempt";

pub static TERRITORY_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MATERIAL_WEIGHT,
        "Material Weight",
        "How much raw material difference matters relative to territory.",
        0.0,
        10.0,
        2.0,
        0.01,
    ),
    ParameterDef::new(
        PARAM_TERRITORY_WEIGHT,
        "Territory Weight",
        "How much board control matters relative to material.",
        0.0,
        10.0,
        1.0,
        0.01,
    ),
    ParameterDef::new(
        PARAM_CONTROL_SATURATION,
        "Control Saturation",
        "How quickly additional attackers on the same square have diminishing returns. \
         Higher = faster saturation, more reward for breadth.",
        0.1,
        5.0,
        1.5,
        0.01,
    ),
    ParameterDef::new(
        PARAM_OCCUPIED_BONUS,
        "Occupied Square Bonus",
        "Extra importance multiplier for controlling squares that have pieces on them.",
        0.0,
        5.0,
        1.0,
        0.01,
    ),
    ParameterDef::new(
        PARAM_CENTRALITY_WEIGHT,
        "Centrality Weight",
        "Bonus importance for controlling squares near the center of the board.",
        0.0,
        3.0,
        0.3,
        0.01,
    ),
    ParameterDef::new(
        PARAM_MOBILITY_INFLUENCE,
        "Mobility Influence",
        "How much non-capture moves (can_land_empty only) contribute to control. \
         0 = only captures count, 1 = movement fully counts.",
        0.0,
        1.0,
        0.15,
        0.01,
    ),
    ParameterDef::new(
        PARAM_KING_ZONE_WEIGHT,
        "King Zone Weight",
        "Extra importance for squares near royal/protected pieces.",
        0.0,
        5.0,
        1.0,
        0.01,
    ),
    ParameterDef::new(
        PARAM_CONTEMPT,
        "Contempt",
        "Bias against draws (centipawns). Higher = avoids draws more aggressively.",
        0.0,
        50.0,
        10.0,
        1.0,
    ),
];

// ─────────────────────────────────────────────────────────────────────────
// Precomputed evaluation data
// ─────────────────────────────────────────────────────────────────────────

struct TerritoryData {
    board_size: (usize, usize),
    num_pieces: usize,

    /// Intrinsic piece values derived from average attack footprint.
    /// Normalized so the weakest attacking piece ≈ 1.0.
    intrinsic_values: Vec<f32>,

    /// Control weight per piece type: `1 / sqrt(intrinsic_value)`.
    /// Low-value pieces contribute MORE control per attack — modelling
    /// the "a pawn attacking a square is more dangerous than a queen
    /// attacking it" intuition from static exchange evaluation.
    control_weights: Vec<f32>,

    /// Precomputed centrality value for each square ∈ [0, 1].
    /// 1.0 = dead center, 0.0 = corner.
    centrality_map: Vec<f32>,

    /// Initial piece count, used for game-phase interpolation.
    initial_piece_count: f32,

    // Cached parameter values as f32 for the hot loop.
    material_weight: f32,
    territory_weight: f32,
    control_saturation: f32,
    occupied_bonus: f32,
    centrality_weight: f32,
    mobility_influence: f32,
    king_zone_weight: f32,
    /// Contempt in evaluation-score units (parameter is centipawns,
    /// stored here as centipawns / 100.0 since we work in "pawn units"
    /// internally and multiply by 100 at the very end).
    contempt: f32,
}

impl TerritoryData {
    fn new(
        board: &Board,
        params: &EngineParameters,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
        num_pieces: usize,
    ) -> Self {
        let board_size = board.size();
        let (rows, cols) = board_size;

        // ── Intrinsic values from average attack footprint ──────────────
        let intrinsic_values = compute_intrinsic_values(move_generator, num_pieces, board_size);

        // ── Control weights: inverse sqrt of intrinsic ──────────────────
        let control_weights: Vec<f32> = intrinsic_values
            .iter()
            .map(|&v| if v > 0.01 { 1.0 / v.sqrt() } else { 1.0 })
            .collect();

        // ── Centrality map ──────────────────────────────────────────────
        let center_r = (rows as f32 - 1.0) / 2.0;
        let center_c = (cols as f32 - 1.0) / 2.0;
        let max_dist = (center_r * center_r + center_c * center_c).sqrt().max(1.0);
        let centrality_map: Vec<f32> = (0..rows)
            .flat_map(|r| {
                (0..cols).map(move |c| {
                    let dr = r as f32 - center_r;
                    let dc = c as f32 - center_c;
                    1.0 - (dr * dr + dc * dc).sqrt() / max_dist
                })
            })
            .collect();

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
            contempt: params.get_or_default(PARAM_CONTEMPT, 10.0) as f32 / 100.0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Intrinsic value computation
//
// For each piece type, compute the average number of squares it can
// attack (`can_land_enemy`) from every position on the board (empty-
// board theoretical moves, treated as already-moved to exclude first-
// move-only patterns like castling or pawn double-push).
//
// The result is normalized so the weakest attacking piece ≈ 1.0.
// ─────────────────────────────────────────────────────────────────────────

fn compute_intrinsic_values(
    move_generator: &MoveGenerator,
    num_pieces: usize,
    board_size: (usize, usize),
) -> Vec<f32> {
    let (rows, cols) = board_size;
    let total_squares = (rows * cols) as f64;
    let mut values = vec![0.0_f32; num_pieces];

    for piece_type in 0..num_pieces {
        let mut total_attacks = 0_usize;
        for row in 0..rows {
            for col in 0..cols {
                let moves = move_generator.generate_theoretical_moves_for_pst(
                    (row, col),
                    piece_type,
                    PieceColor::White,
                    board_size,
                    1, // treat as moved — normal capability
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

    // Normalize: minimum positive value → 1.0
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
            // Pieces with negligible attack (walls, immovable pieces)
            // still need a small positive value so control_weight
            // doesn't blow up.
            *v = 0.5;
        }
    }

    values
}

// ─────────────────────────────────────────────────────────────────────────
// Protected-piece positions (mirrors GameState check logic)
// ─────────────────────────────────────────────────────────────────────────

/// Collect positions of pieces that are "protected" for a color:
/// all 'R' (royal) pieces plus the last-remaining 'r' (royalty) piece.
#[inline]
fn protected_positions(board: &Board, color: PieceColor) -> Vec<Position> {
    let mut out = Vec::with_capacity(4);
    for &pos in board.get_royal_positions(color) {
        out.push(pos);
    }
    let royalty = board.get_royalty_positions(color);
    if royalty.len() == 1 {
        out.push(royalty[0]);
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// The engine
// ─────────────────────────────────────────────────────────────────────────

pub struct TerritoryEngine {
    eval_data: RefCell<Option<TerritoryData>>,
    transposition_table: HashMap<u64, crate::engine::search::TTEntry>,
    parameters: EngineParameters,
    needs_reinit: bool,
}

impl TerritoryEngine {
    pub fn new() -> Self {
        Self {
            eval_data: RefCell::new(None),
            transposition_table: HashMap::new(),
            parameters: EngineParameters::from_defaults(TERRITORY_PARAMETERS),
            needs_reinit: true,
        }
    }

    /// Ensure evaluation tables are built for the current board geometry.
    fn initialize(
        &mut self,
        board: &Board,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let board_size = board.size();
        let num_pieces = config_manager.piece_order.len();

        let needs_rebuild = {
            let data = self.eval_data.borrow();
            self.needs_reinit
                || data.is_none()
                || data.as_ref().map_or(true, |d| {
                    d.board_size != board_size || d.num_pieces != num_pieces
                })
        };

        if !needs_rebuild {
            return;
        }

        println!(
            "🗺️  Building Territory tables for {}×{} board, {} piece types…",
            board_size.0, board_size.1, num_pieces
        );

        let data = TerritoryData::new(
            board,
            &self.parameters,
            move_generator,
            config_manager,
            num_pieces,
        );

        // Log intrinsic values for debugging
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

    /// Core evaluation: computes a score in "pawn units" (not centipawns).
    /// Positive = good for White.
    fn evaluate_position(&self, state: &GameState, move_generator: &MoveGenerator) -> f32 {
        let data_ref = self.eval_data.borrow();
        let data = match data_ref.as_ref() {
            Some(d) => d,
            None => return 0.0,
        };

        let board = &state.board;
        let (rows, cols) = data.board_size;
        if board.size() != (rows, cols) {
            return 0.0;
        }

        let flat_size = rows * cols;

        // ── Phase: 1.0 = opening/midgame, 0.0 = deep endgame ────────────
        let current_pieces = board.count_pieces() as f32;
        let end_threshold = 4.0_f32;
        let phase = if data.initial_piece_count > end_threshold {
            ((current_pieces - end_threshold) / (data.initial_piece_count - end_threshold))
                .clamp(0.0, 1.0)
        } else {
            1.0
        };

        // Territory weight tapers in the endgame (material becomes king).
        // Floor of 0.4 ensures king-zone and occupancy control still matter.
        let effective_territory_weight = data.territory_weight * (0.4 + 0.6 * phase);

        // ── Pass 1: accumulate control maps + material ───────────────────
        let mut white_control = vec![0.0_f32; flat_size];
        let mut black_control = vec![0.0_f32; flat_size];
        let mut white_material = 0.0_f32;
        let mut black_material = 0.0_f32;

        for row in 0..rows {
            for col in 0..cols {
                let pos = (row, col);
                let piece = match board.get_piece(pos) {
                    Some(p) => p,
                    None => continue,
                };
                if piece.piece_type >= data.num_pieces {
                    continue;
                }

                // Material (skip pure-royal pieces — they can't be traded,
                // so their "material" is irrelevant to the balance).
                if !piece.is_royal {
                    let iv = data.intrinsic_values[piece.piece_type];
                    match piece.color {
                        PieceColor::White => white_material += iv,
                        PieceColor::Black => black_material += iv,
                    }
                }

                // Control: generate this piece's actual moves on the
                // current board and distribute control to destinations.
                let cw = data.control_weights[piece.piece_type];
                let moves =
                    move_generator.generate_moves_with_database(board, pos, piece.piece_type);

                let control_map = match piece.color {
                    PieceColor::White => &mut white_control,
                    PieceColor::Black => &mut black_control,
                };

                for mv in &moves {
                    // Castling doesn't represent square control.
                    if mv.rule.is_king_castle || mv.rule.is_rook_castle {
                        continue;
                    }
                    let dest = mv.destination;
                    if dest.0 >= rows || dest.1 >= cols {
                        continue;
                    }
                    let dest_flat = dest.0 * cols + dest.1;

                    // The move was generated, so the piece CAN go there.
                    // If the pattern can capture enemies, this represents
                    // full attack control. If the pattern can only land on
                    // empty squares, it's weaker mobility-based influence.
                    let weight = if mv.rule.can_land_enemy {
                        cw
                    } else {
                        cw * data.mobility_influence
                    };

                    control_map[dest_flat] += weight;
                }
            }
        }

        // ── Precompute protected-piece positions for king-zone bonus ─────
        let white_protected = protected_positions(board, PieceColor::White);
        let black_protected = protected_positions(board, PieceColor::Black);

        // ── Pass 2: score each square ────────────────────────────────────
        let sat = data.control_saturation;
        let mut territory_score = 0.0_f32;

        for row in 0..rows {
            for col in 0..cols {
                let flat = row * cols + col;
                let wc = white_control[flat];
                let bc = black_control[flat];

                // Skip squares with zero control from both sides — common
                // on large or sparse boards. Avoids unnecessary work.
                if wc < 0.001 && bc < 0.001 {
                    continue;
                }

                // Saturating control: x / (sat + x)  ∈ [0, 1)
                // Ensures diminishing returns for stacking attackers.
                let w_eff = wc / (sat + wc);
                let b_eff = bc / (sat + bc);
                let net = w_eff - b_eff;

                // ── Square importance ────────────────────────────────────
                let mut importance = 1.0_f32;

                // Centrality
                importance += data.centrality_map[flat] * data.centrality_weight;

                // Occupancy: squares with pieces are more important.
                // Uses sqrt(intrinsic) to avoid queen-dominated eval.
                if let Some(occ) = board.get_piece((row, col)) {
                    if occ.piece_type < data.num_pieces {
                        let iv = data.intrinsic_values[occ.piece_type];
                        importance += iv.sqrt() * data.occupied_bonus;
                    }
                }

                // King zone: squares near protected pieces are critical.
                // Proximity to EITHER side's king matters — net_control
                // determines who benefits.
                if data.king_zone_weight > 0.0 {
                    for &kp in white_protected.iter().chain(black_protected.iter()) {
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

        // ── Combine terms ────────────────────────────────────────────────
        let material_diff = white_material - black_material;
        let score_white =
            data.material_weight * material_diff + effective_territory_weight * territory_score;

        // Convert to current-player perspective.
        let score = match state.current_turn {
            PieceColor::White => score_white,
            PieceColor::Black => -score_white,
        };

        // Contempt: slight positive bias to prefer playing over drawing.
        score + data.contempt
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Evaluator wrapper (borrows the engine for use with Search<E>)
// ─────────────────────────────────────────────────────────────────────────

struct TerritoryEvaluator<'a> {
    engine: &'a TerritoryEngine,
    move_generator: &'a MoveGenerator,
}

impl<'a> EvaluatorTrait for TerritoryEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        // Quick terminal checks
        if !state.board.has_pieces(state.current_turn) {
            return -999999; // Extinction
        }

        let score = self.engine.evaluate_position(state, move_generator);
        (score * 100.0) as i32
    }

    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        _pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        // Return intrinsic value (position-independent) for MVV-LVA
        // capture ordering. Higher = more valuable victim.
        let data = self.engine.eval_data.borrow();
        if let Some(data) = data.as_ref() {
            if piece.piece_type < data.num_pieces {
                return (data.intrinsic_values[piece.piece_type] * 100.0) as i32;
            }
        }
        100
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ParameterizedEngine impl
// ─────────────────────────────────────────────────────────────────────────

impl ParameterizedEngine for TerritoryEngine {
    fn parameter_definitions(&self) -> &'static [ParameterDef] {
        TERRITORY_PARAMETERS
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
        *self.eval_data.borrow_mut() = None;
        self.transposition_table.clear();
        println!("🔄 Territory Engine parameters changed — tables will be recomputed.");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ChessEngine impl
// ─────────────────────────────────────────────────────────────────────────

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

        let evaluator = TerritoryEvaluator {
            engine: self,
            move_generator: params.move_generator,
        };
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

    fn supports_analysis(&self) -> bool {
        false
    }

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
