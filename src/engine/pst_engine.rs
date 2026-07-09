use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{combined_params, Search, SearchConfig};
use crate::move_generator::{MoveGenerator, MoveWithPath};
use crate::piece_config::PieceConfigManager;
use crate::promotion::PromotionManager;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

// ─────────────────────────────────────────────────────────────────────────
// Parameter IDs
// ─────────────────────────────────────────────────────────────────────────
pub const PARAM_MULTIPLICATIVE_SWARM: &str = "multiplicative_swarm";
pub const PARAM_ADDITIVE_SWARM: &str = "additive_swarm";
pub const PARAM_HUDDLE_BONUS: &str = "huddle_bonus";
pub const PARAM_MOBILITY_WEIGHT: &str = "mobility_weight";
pub const PARAM_FUTURE_MOBILITY_DISCOUNT: &str = "future_mobility_discount";
pub const PARAM_PIECE_INTRINSIC_WEIGHT: &str = "piece_intrinsic_weight";
pub const PARAM_GRAVITY_BONUS: &str = "gravity_bonus";

pub const PARAM_PROMOTION_DAMPENER: &str = "promotion_dampener";
pub const PARAM_MIDGAME_STRUCTURAL_FACTOR: &str = "midgame_structural_factor";
pub const PARAM_ENDGAME_STRUCTURAL_FACTOR: &str = "endgame_structural_factor";

// ── Piece Safety Parameters ───────────────────────────────────────────────
pub const PARAM_PIECE_PROXIMITY_OFFENSIVE: &str = "piece_proximity_offensive";
pub const PARAM_PIECE_PROXIMITY_DEFENSIVE: &str = "piece_proximity_defensive";
pub const PARAM_PST_CROWDING_PENALTY: &str = "pst_crowding_penalty";

pub static PST_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MULTIPLICATIVE_SWARM,
        "Swarm Multiplier",
        "Multiplies base PST value near enemy king. Higher = stronger pieces attack more.",
        0.0,
        0.5,
        0.05,
        0.01,
    ),
ParameterDef::new(
    PARAM_ADDITIVE_SWARM,
    "Swarm Bonus",
    "Flat bonus added near enemy king. Higher = any piece joins attacks.",
    0.0,
    2.0,
    0.5,
    0.01,
),
ParameterDef::new(
    PARAM_HUDDLE_BONUS,
    "Huddle Bonus",
    "Bonus for pieces near your own king. Higher = more defensive play.",
    0.0,
    1.0,
    0.0,
    0.01,
),
ParameterDef::new(
    PARAM_MOBILITY_WEIGHT,
    "Mobility Weight",
    "How much current position mobility matters vs future potential.",
    0.0,
    2.0,
    1.0,
    0.01,
),
ParameterDef::new(
    PARAM_FUTURE_MOBILITY_DISCOUNT,
    "Future Discount",
    "Discount factor for future moves (lower = less long-term thinking).",
                  0.01,
                  0.99,
                  0.65,
                  0.01,
),
ParameterDef::new(
    PARAM_PIECE_INTRINSIC_WEIGHT,
    "Intrinsic Value Weight",
    "Weight for piece's inherent value vs positional value. Higher = piece type matters more.",
    0.0,
    2.0,
    1.0,
    0.01,
),
ParameterDef::new(
    PARAM_GRAVITY_BONUS,
    "Gravity Bonus",
    "Bonus for pieces closer to center of mass of all pieces. Higher = prefers central clustering.",
    0.0,
    5.0,
    0.0,
    0.01,
),
ParameterDef::new(
    PARAM_PROMOTION_DAMPENER,
    "Promotion Dampener",
    "Multiplier for promotion bonuses. 0.5 = cautious, 1.0 = highly aggressive.",
    0.0,
    2.0,
    0.55,
    0.01,
),
ParameterDef::new(
    PARAM_MIDGAME_STRUCTURAL_FACTOR,
    "Midgame Density Discount",
    "Discounts midgame blocking probability.",
    0.0,
    1.0,
    0.8,
    0.01,
),
ParameterDef::new(
    PARAM_ENDGAME_STRUCTURAL_FACTOR,
    "Endgame Density Discount",
    "Discounts endgame blocking.",
    0.0,
    1.0,
    1.0,
    0.01,
),
// ── Safety Parameters ────────────────────────────────────────────────
ParameterDef::new(
    PARAM_PIECE_PROXIMITY_OFFENSIVE,
    "Piece Proximity Offensive",
    "Bonus for pieces near high-value enemy pieces (swarming targets). 0 = disabled.",
                  0.0,
                  2.0,
                  0.0,
                  0.01,
),
ParameterDef::new(
    PARAM_PIECE_PROXIMITY_DEFENSIVE,
    "Piece Proximity Defensive",
    "Penalty when enemy pieces are near our high-value pieces (vulnerability). 0 = disabled.",
                  0.0,
                  2.0,
                  0.0,
                  0.01,
),
ParameterDef::new(
    PARAM_PST_CROWDING_PENALTY,
    "PST Crowding Penalty",
    "Penalizes valuable pieces on high-value squares when many enemy pieces can reach those squares. \
Scales quadratically with piece value, so queens are penalized much more than pawns. 0 = disabled.",
0.0,
100.0,
0.0,
0.01,
),
];

// ─────────────────────────────────────────────────────────────────────────
// Flat PST storage
// ─────────────────────────────────────────────────────────────────────────
struct FlatPst {
    values: Vec<f32>,
    rows: usize,
    cols: usize,
    stride_piece: usize,
}

impl FlatPst {
    fn new(num_pieces: usize, rows: usize, cols: usize) -> Self {
        let stride_piece = 2 * rows * cols;
        Self {
            values: vec![0.0_f32; num_pieces * stride_piece],
            rows,
            cols,
            stride_piece,
        }
    }

    #[inline(always)]
    fn idx(&self, piece_type: usize, color_idx: usize, row: usize, col: usize) -> usize {
        piece_type * self.stride_piece + color_idx * self.rows * self.cols + row * self.cols + col
    }

    #[inline(always)]
    fn get(&self, piece_type: usize, color_idx: usize, row: usize, col: usize) -> f32 {
        let idx = self.idx(piece_type, color_idx, row, col);
        if idx < self.values.len() {
            unsafe { *self.values.get_unchecked(idx) }
        } else {
            0.0
        }
    }

    fn set(&mut self, piece_type: usize, color_idx: usize, row: usize, col: usize, v: f32) {
        let idx = self.idx(piece_type, color_idx, row, col);
        if idx < self.values.len() {
            self.values[idx] = v;
        }
    }

    fn add(&mut self, piece_type: usize, color_idx: usize, row: usize, col: usize, delta: f32) {
        let idx = self.idx(piece_type, color_idx, row, col);
        if idx < self.values.len() {
            self.values[idx] += delta;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Flat swarm tables
// ─────────────────────────────────────────────────────────────────────────
struct SwarmTables {
    mult: [Vec<f32>; 2],
    add: [Vec<f32>; 2],
    huddle: [Vec<f32>; 2],
    cached_white_royals: smallvec::SmallVec<[Position; 2]>,
    cached_black_royals: smallvec::SmallVec<[Position; 2]>,
}

impl SwarmTables {
    fn new(flat_size: usize) -> Self {
        Self {
            mult: [vec![0.0_f32; flat_size], vec![0.0_f32; flat_size]],
            add: [vec![0.0_f32; flat_size], vec![0.0_f32; flat_size]],
            huddle: [vec![0.0_f32; flat_size], vec![0.0_f32; flat_size]],
            cached_white_royals: smallvec::SmallVec::new(),
            cached_black_royals: smallvec::SmallVec::new(),
        }
    }

    #[inline(always)]
    fn get_mult(&self, color_idx: usize, flat: usize) -> f32 {
        unsafe { *self.mult[color_idx].get_unchecked(flat) }
    }

    #[inline(always)]
    fn get_add(&self, color_idx: usize, flat: usize) -> f32 {
        unsafe { *self.add[color_idx].get_unchecked(flat) }
    }

    #[inline(always)]
    fn get_huddle(&self, color_idx: usize, flat: usize) -> f32 {
        unsafe { *self.huddle[color_idx].get_unchecked(flat) }
    }

    fn is_cache_valid(&self, white_royals: &[Position], black_royals: &[Position]) -> bool {
        self.cached_white_royals.as_slice() == white_royals
        && self.cached_black_royals.as_slice() == black_royals
    }

    fn recompute(
        &mut self,
        white_royals: &[Position],
        black_royals: &[Position],
        rows: usize,
        cols: usize,
        swarm_mult: f32,
        swarm_add: f32,
        huddle_bonus: f32,
    ) {
        for r in 0..rows {
            for c in 0..cols {
                let pos = (r, c);
                let flat = r * cols + c;

                let (wm, wa, wh) = accumulate_royal_bonuses(
                    pos,
                    black_royals,
                    white_royals,
                    swarm_mult,
                    swarm_add,
                    huddle_bonus,
                );
                self.mult[0][flat] = wm;
                self.add[0][flat] = wa;
                self.huddle[0][flat] = wh;

                let (bm, ba, bh) = accumulate_royal_bonuses(
                    pos,
                    white_royals,
                    black_royals,
                    swarm_mult,
                    swarm_add,
                    huddle_bonus,
                );
                self.mult[1][flat] = bm;
                self.add[1][flat] = ba;
                self.huddle[1][flat] = bh;
            }
        }

        self.cached_white_royals.clear();
        self.cached_white_royals.extend_from_slice(white_royals);
        self.cached_black_royals.clear();
        self.cached_black_royals.extend_from_slice(black_royals);
    }
}

#[inline(always)]
fn royal_distance(pos: Position, royal: Position) -> f32 {
    let dr = pos.0 as f32 - royal.0 as f32;
    let dc = pos.1 as f32 - royal.1 as f32;
    (dr * dr + dc * dc).sqrt()
}

#[inline]
fn accumulate_royal_bonuses(
    pos: Position,
    attack_royals: &[Position],
    defend_royals: &[Position],
    swarm_mult: f32,
    swarm_add: f32,
    huddle_bonus: f32,
) -> (f32, f32, f32) {
    let mut mult = 0.0_f32;
    let mut add = 0.0_f32;
    let mut huddle = 0.0_f32;

    for &royal in attack_royals {
        let dist = royal_distance(pos, royal).max(0.01);
        mult += swarm_mult / dist;
        add += swarm_add / dist;
    }
    for &royal in defend_royals {
        let dist = royal_distance(pos, royal).max(0.01);
        huddle += huddle_bonus / dist;
    }

    (mult, add, huddle)
}

// ─────────────────────────────────────────────────────────────────────────
// Piece Proximity Tables
//
// Performance design:
//   - Cache key is a cheap u64 fingerprint (XOR of packed positions),
//     not a Vec comparison. O(k) where k = number of HVPs, not O(board).
//   - Board scan for HVPs is O(rows*cols) but only runs when fingerprint
//     changes. Fingerprint itself is computed in the same O(rows*cols)
//     pass as collecting HVPs, so there's no extra scan.
//   - "High value" is defined relative to the piece being evaluated
//     (enemy piece value > our piece value * threshold_ratio), not an
//     arbitrary median. This is computed per-square in the hot loop but
//     it's a single float multiply+compare with no allocation.
// ─────────────────────────────────────────────────────────────────────────

/// A high-value enemy piece position + its intrinsic value.
/// Stored compactly: position as u16 (row*256+col), value as f32.
#[derive(Clone, Copy)]
struct HvpEntry {
    flat_pos: u16, // row * cols + col, fits u16 for boards up to 256x256
    row: u8,
    col: u8,
    value: f32,
}

/// Per-square proximity field: sum of (enemy_hvp_value / distance) for
/// all HVPs of the opposing color.
///
/// We store two fields per color:
///   offensive[color_idx][flat] = sum over enemy HVPs of (hvp_val / dist)
///     → used as a bonus for color's pieces at that square (attack target)
///   The defensive penalty is computed per-piece in the hot loop using
///     the same field, scaled by the piece's own intrinsic value.
///     This avoids a separate defensive table entirely.
struct PieceProximityTables {
    /// offensive[color_idx][flat] = proximity bonus for color_idx pieces
    /// being near enemy (1-color_idx) HVPs.
    offensive: [Vec<f32>; 2],

    /// Fingerprint of HVP positions per color. Cheap change detection.
    cached_fingerprint: [u64; 2],

    /// Cached HVP list per color (for distance computation).
    cached_hvps: [Vec<HvpEntry>; 2],
}

impl PieceProximityTables {
    fn new(flat_size: usize) -> Self {
        Self {
            offensive: [vec![0.0; flat_size], vec![0.0; flat_size]],
            cached_fingerprint: [u64::MAX, u64::MAX], // MAX = "never valid"
            cached_hvps: [Vec::new(), Vec::new()],
        }
    }

    /// Collect HVPs for a color and compute a fingerprint in one board scan.
    /// `value_threshold`: minimum intrinsic value to qualify as HVP.
    fn collect_hvps_with_fingerprint(
        board: &Board,
        color: PieceColor,
        intrinsic_values: &[f32],
        value_threshold: f32,
        num_pieces: usize,
    ) -> (Vec<HvpEntry>, u64) {
        let mut hvps = Vec::new();
        let cols = board.cols() as u32;
        let mut fingerprint: u64 = 0xcbf29ce484222325u64; // FNV offset basis

        let (rows, board_cols) = board.size();
        for r in 0..rows {
            for c in 0..board_cols {
                if let Some(piece) = board.get_piece((r, c)) {
                    if piece.color == color && piece.piece_type < num_pieces && !piece.is_royal {
                        let val = intrinsic_values[piece.piece_type];
                        if val >= value_threshold {
                            let flat = (r as u32 * cols + c as u32) as u16;
                            hvps.push(HvpEntry {
                                flat_pos: flat,
                                row: r as u8,
                                col: c as u8,
                                value: val,
                            });
                            // FNV-1a hash: mix position and value bits
                            fingerprint ^= (flat as u64).wrapping_mul(0x100000001b3u64)
                            ^ (val.to_bits() as u64).wrapping_mul(0x517cc1b727220a95u64);
                            fingerprint = fingerprint.wrapping_mul(0x100000001b3u64);
                        }
                    }
                }
            }
        }
        (hvps, fingerprint)
    }

    /// Update tables if HVP positions have changed.
    /// Returns true if a recompute was needed.
    fn update_if_needed(
        &mut self,
        board: &Board,
        intrinsic_values: &[f32],
        value_threshold: f32,
        num_pieces: usize,
        offensive_weight: f32,
    ) -> bool {
        let (rows, cols) = board.size();

        // Collect HVPs and fingerprints for both colors.
        let (white_hvps, white_fp) = Self::collect_hvps_with_fingerprint(
            board,
            PieceColor::White,
            intrinsic_values,
            value_threshold,
            num_pieces,
        );
        let (black_hvps, black_fp) = Self::collect_hvps_with_fingerprint(
            board,
            PieceColor::Black,
            intrinsic_values,
            value_threshold,
            num_pieces,
        );

        // Check if either changed.
        if white_fp == self.cached_fingerprint[0] && black_fp == self.cached_fingerprint[1] {
            return false;
        }

        // Recompute offensive tables.
        let flat_size = rows * cols;

        // White's offensive field = proximity to black HVPs
        let off_w = &mut self.offensive[0];
        for v in off_w.iter_mut() {
            *v = 0.0;
        }
        if offensive_weight > 0.0 {
            for r in 0..rows {
                for c in 0..cols {
                    let flat = r * cols + c;
                    let mut bonus = 0.0f32;
                    for hvp in &black_hvps {
                        let dr = r as f32 - hvp.row as f32;
                        let dc = c as f32 - hvp.col as f32;
                        let dist = (dr * dr + dc * dc).sqrt().max(0.5);
                        bonus += offensive_weight * hvp.value / dist;
                    }
                    off_w[flat] = bonus;
                }
            }
        }

        // Black's offensive field = proximity to white HVPs
        let off_b = &mut self.offensive[1];
        for v in off_b.iter_mut() {
            *v = 0.0;
        }
        if offensive_weight > 0.0 {
            for r in 0..rows {
                for c in 0..cols {
                    let flat = r * cols + c;
                    let mut bonus = 0.0f32;
                    for hvp in &white_hvps {
                        let dr = r as f32 - hvp.row as f32;
                        let dc = c as f32 - hvp.col as f32;
                        let dist = (dr * dr + dc * dc).sqrt().max(0.5);
                        bonus += offensive_weight * hvp.value / dist;
                    }
                    off_b[flat] = bonus;
                }
            }
        }

        self.cached_fingerprint[0] = white_fp;
        self.cached_fingerprint[1] = black_fp;
        self.cached_hvps[0] = white_hvps;
        self.cached_hvps[1] = black_hvps;
        true
    }

    #[inline(always)]
    fn get_offensive(&self, color_idx: usize, flat: usize) -> f32 {
        unsafe { *self.offensive[color_idx].get_unchecked(flat) }
    }

    /// Compute the defensive penalty for a specific piece at `flat`.
    /// This is called per-piece in the hot loop.
    ///
    /// The penalty = defensive_weight * piece_intrinsic_value
    ///               * sum(enemy_hvp_value / dist) at this square
    ///
    /// We reuse the offensive table of the *enemy* color as the
    /// "enemy piece density near this square" signal: a square that
    /// is attractive to enemy HVPs is dangerous for us.
    ///
    /// Scaling by piece_intrinsic_value means queens get a much larger
    /// penalty than pawns for standing near enemy queens.
    #[inline(always)]
    fn defensive_penalty(
        &self,
        our_color_idx: usize,
        flat: usize,
        piece_intrinsic: f32,
        max_intrinsic: f32,
        defensive_weight: f32,
    ) -> f32 {
        if defensive_weight <= 0.0 {
            return 0.0;
        }
        // Enemy's offensive field at our square = how much enemy HVPs
        // want to be near here = how dangerous this square is for us.
        let enemy_idx = 1 - our_color_idx;
        let danger = unsafe { *self.offensive[enemy_idx].get_unchecked(flat) };
        // Scale by normalized piece value so valuable pieces suffer more.
        let value_factor = piece_intrinsic / max_intrinsic;
        defensive_weight * danger * value_factor
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Crowding penalty precomputation
//
// The crowding penalty aims to penalize valuable pieces for occupying
// high-PST-value squares when the board is congested with enemy pieces
// that can contest those squares. This is NOT a flat density penalty —
// it's specific to each piece type's PST landscape.
//
// Approach:
//   For each (piece_type, color, square), precompute how "contested" that
//   square is: the number of distinct enemy piece types that have
//   theoretical attacks reaching the square, weighted by their mobility.
//   Call this the "contest score" for the square.
//
//   During evaluation: crowding_penalty(piece, square) =
//     crowding_weight
//     * (piece_intrinsic / max_intrinsic)^2    ← quadratic: queens hurt much more
//     * pst_percentile(piece, square)          ← only penalize genuinely good squares
//     * contest_score(square)                  ← enemy pressure on this square
//     * crowding_phase                         ← less penalty in endgame
//
//   pst_percentile = (base_pst - min_pst) / (max_pst - min_pst) for this piece type.
//   This means the penalty is zero on bad squares and max on the best squares.
//
// The contest_score table is computed once during EvalData initialization
// (O(board * piece_types * theoretical_moves)) and stored as a flat Vec<f32>.
// It only needs rebuilding when the board size or piece set changes.
// ─────────────────────────────────────────────────────────────────────────

/// Per-square enemy attack density, normalized to [0, 1].
/// `contest[flat]` = sum of normalized mobility for all enemy piece types
/// that can theoretically reach that square.
fn compute_contest_scores(
    rows: usize,
    cols: usize,
    num_pieces: usize,
    intrinsic_values: &[f32],
    max_intrinsic: f32,
    move_generator: &MoveGenerator,
    config_manager: &PieceConfigManager,
) -> Vec<f32> {
    let flat_size = rows * cols;
    let mut scores = vec![0.0f32; flat_size];

    if max_intrinsic <= 0.0 {
        return scores;
    }

    let board_size = (rows, cols);

    for piece_type in 0..num_pieces {
        // Skip royal pieces — they don't participate in normal attacks
        if config_manager
            .get_piece_by_index(piece_type)
            .map_or(false, |c| c.properties.is_royal || c.properties.is_royalty)
            {
                continue;
            }

            let piece_rel_value = intrinsic_values[piece_type] / max_intrinsic;

        // For each square, can this piece type attack it?
        // We use the reverse: from each square, generate theoretical moves
        // and mark destinations as reachable by this piece type.
        // This is O(squares * avg_mobility) per piece type.
        for r in 0..rows {
            for c in 0..cols {
                let moves = move_generator.generate_theoretical_moves_for_pst(
                    (r, c),
                                                                              piece_type,
                                                                              PieceColor::White, // color doesn't matter for coverage
                                                                              board_size,
                                                                              1, // treat as moved
                );
                for mv in &moves {
                    if mv.rule.can_land_enemy {
                        let dest = mv.destination;
                        if dest.0 < rows && dest.1 < cols {
                            scores[dest.0 * cols + dest.1] += piece_rel_value;
                        }
                    }
                }
            }
        }
    }

    // Normalize to [0, 1] by dividing by the max score.
    let max_score = scores.iter().cloned().fold(0.0f32, f32::max);
    if max_score > 0.0 {
        for s in scores.iter_mut() {
            *s /= max_score;
        }
    }

    scores
}

// ─────────────────────────────────────────────────────────────────────────
// PST percentile table
//
// For each (piece_type, color_idx), the percentile of each square's PST
// value within that piece type's range. Stored flat, same layout as FlatPst.
// pst_percentile[piece_type][color_idx][row][col] ∈ [0, 1].
// ─────────────────────────────────────────────────────────────────────────
fn compute_pst_percentiles(pst: &FlatPst, num_pieces: usize, rows: usize, cols: usize) -> Vec<f32> {
    // Same layout as FlatPst: piece_type * stride + color_idx * rows*cols + flat
    let stride = 2 * rows * cols;
    let mut out = vec![0.0f32; num_pieces * stride];

    for piece_type in 0..num_pieces {
        for color_idx in 0..2 {
            let mut min_v = f32::INFINITY;
            let mut max_v = f32::NEG_INFINITY;
            for row in 0..rows {
                for col in 0..cols {
                    let v = pst.get(piece_type, color_idx, row, col);
                    if v < min_v {
                        min_v = v;
                    }
                    if v > max_v {
                        max_v = v;
                    }
                }
            }
            let range = max_v - min_v;
            for row in 0..rows {
                for col in 0..cols {
                    let v = pst.get(piece_type, color_idx, row, col);
                    let pct = if range > 0.0 {
                        (v - min_v) / range
                    } else {
                        0.5
                    };
                    let idx = piece_type * stride + color_idx * rows * cols + row * cols + col;
                    if idx < out.len() {
                        out[idx] = pct;
                    }
                }
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Precomputed evaluation state
// ─────────────────────────────────────────────────────────────────────────

struct EvalData {
    board_size: (usize, usize),
    num_pieces: usize,

    midgame_pst: FlatPst,
    endgame_pst: FlatPst,

    midgame_pieces: f32,
    /// Average midgame PST value per piece type. This is the "intrinsic
    /// value" proxy used by proximity/crowding penalties. Historically
    /// this was the only intrinsic vector and it fed both phases — which
    /// is the bug we're fixing.
    intrinsic_values: Vec<f32>,
    /// Average endgame PST value per piece type. Interpolated with
    /// `intrinsic_values` using the current phase wherever a
    /// phase-accurate piece value is needed.
    endgame_intrinsic_values: Vec<f32>,
    /// Max of all intrinsic values across both phases. Using the global
    /// max (rather than interpolating per-phase maxes) keeps the
    /// normalization factor stable — a piece that is 0.9 of the max in
    /// midgame and 0.8 in endgame should smoothly interpolate between
    /// those fractions, not jump because the denominator changed too.
    max_intrinsic: f32,

    /// Current game phase in [0, 1]: 1 = full midgame, 0 = deep endgame.
    /// Updated at the top of evaluate_position (so every leaf node
    /// refreshes it) and at the top of best_move / analyze_position (so
    /// the first order_moves call has a fresh value). Read by
    /// get_piece_value_on_square, which has no access to the board and
    /// therefore cannot compute phase itself.
    ///
    /// Staleness: inside the search tree, order_moves at an internal
    /// node sees the phase from whichever leaf evaluate_position touched
    /// last. Phase varies by ~1/midgame_pieces per capture, so staleness
    /// of a few plies is a few percent — well within the noise of
    /// heuristic move ordering.
    current_phase: f32,

    swarm: SwarmTables,

    // ── Proximity tables ─────────────────────────────────────────────────
    proximity: PieceProximityTables,
    /// Minimum intrinsic value that qualifies a piece as "high value" for
    /// proximity purposes. Set to the 50th percentile of non-royal intrinsic
    /// values so that only the top half participates.
    hvp_value_threshold: f32,

    // ── Crowding penalty precomputed data ────────────────────────────────
    /// contest_scores[flat] = normalized enemy attack density for square.
    /// Shared across all piece types and colors.
    contest_scores: Vec<f32>,

    /// pst_percentiles: same layout as FlatPst, values ∈ [0,1].
    /// pst_percentile[piece_type][color_idx][row][col] = how good this
    /// square is for this piece relative to its own range.
    pst_percentiles: Vec<f32>,

    /// Stride for pst_percentiles (= 2 * rows * cols).
    pst_percentile_stride: usize,
}

impl EvalData {
    fn new(
        board: &Board,
        params: &EngineParameters,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let board_size = board.size();
        let (rows, cols) = board_size;
        let num_pieces = config_manager.piece_order.len();
        let flat_size = rows * cols;
        let total_sq = (rows * cols) as f64;

        let future_discount = params.get_or_default(PARAM_FUTURE_MOBILITY_DISCOUNT, 0.65);
        let mobility_weight = params.get_or_default(PARAM_MOBILITY_WEIGHT, 1.0);
        let mid_structural = params.get_or_default(PARAM_MIDGAME_STRUCTURAL_FACTOR, 0.8);
        let end_structural = params.get_or_default(PARAM_ENDGAME_STRUCTURAL_FACTOR, 1.0);
        let promo_dampener = params.get_or_default(PARAM_PROMOTION_DAMPENER, 0.55);

        let mut initial_pieces = 0.0_f64;
        for r in 0..rows {
            for c in 0..cols {
                if board.get_piece((r, c)).is_some() {
                    initial_pieces += 1.0;
                }
            }
        }

        let midgame_pieces = initial_pieces.max(8.0);
        let endgame_pieces = 4.0_f64.min(midgame_pieces - 2.0).max(2.0);

        let mid_density = (midgame_pieces / total_sq).min(0.9);
        let end_density = (endgame_pieces / total_sq).min(0.9);

        let mid_empty_prob = 1.0 - mid_density * mid_structural;
        let end_empty_prob = 1.0 - end_density * end_structural;

        let mut midgame_pst = FlatPst::new(num_pieces, rows, cols);
        let mut endgame_pst = FlatPst::new(num_pieces, rows, cols);
        let mut intrinsic_values = vec![0.0_f32; num_pieces];
        let mut endgame_intrinsic_values = vec![0.0_f32; num_pieces];

        for piece_type in 0..num_pieces {
            // Sum midgame and endgame values separately so each phase
            // gets its own intrinsic. The old code summed only midgame
            // values, leaving the endgame intrinsic undefined (and then
            // incorrectly adding the MIDGAME intrinsic to the endgame PST).
            let mut mid_white_sum = 0.0_f64;
            let mut mid_black_sum = 0.0_f64;
            let mut end_white_sum = 0.0_f64;
            let mut end_black_sum = 0.0_f64;

            for row in 0..rows {
                for col in 0..cols {
                    let pos = (row, col);

                    let m_wv = compute_square_value(
                        pos,
                        piece_type,
                        PieceColor::White,
                        board_size,
                        mid_empty_prob,
                        future_discount,
                        mobility_weight,
                        move_generator,
                    );
                    let m_bv = compute_square_value(
                        pos,
                        piece_type,
                        PieceColor::Black,
                        board_size,
                        mid_empty_prob,
                        future_discount,
                        mobility_weight,
                        move_generator,
                    );
                    midgame_pst.set(piece_type, 0, row, col, m_wv as f32);
                    midgame_pst.set(piece_type, 1, row, col, m_bv as f32);

                    let e_wv = compute_square_value(
                        pos,
                        piece_type,
                        PieceColor::White,
                        board_size,
                        end_empty_prob,
                        future_discount,
                        mobility_weight,
                        move_generator,
                    );
                    let e_bv = compute_square_value(
                        pos,
                        piece_type,
                        PieceColor::Black,
                        board_size,
                        end_empty_prob,
                        future_discount,
                        mobility_weight,
                        move_generator,
                    );
                    endgame_pst.set(piece_type, 0, row, col, e_wv as f32);
                    endgame_pst.set(piece_type, 1, row, col, e_bv as f32);

                    mid_white_sum += m_wv;
                    mid_black_sum += m_bv;
                    end_white_sum += e_wv;
                    end_black_sum += e_bv;
                }
            }

            let denom = 2.0 * flat_size as f64;
            intrinsic_values[piece_type] = ((mid_white_sum + mid_black_sum) / denom) as f32;
            endgame_intrinsic_values[piece_type] = ((end_white_sum + end_black_sum) / denom) as f32;
        }

        // Add the intrinsic-weight addend phase-appropriately. Previously
        // the midgame intrinsic was added to BOTH tables — the endgame
        // table was getting midgame piece values baked into every square.
        let intrinsic_weight = params.get_or_default(PARAM_PIECE_INTRINSIC_WEIGHT, 1.0) as f32;
        if intrinsic_weight != 0.0 {
            for piece_type in 0..num_pieces {
                let mid_addend = intrinsic_values[piece_type] * intrinsic_weight;
                let end_addend = endgame_intrinsic_values[piece_type] * intrinsic_weight;
                for color_idx in 0..2 {
                    for row in 0..rows {
                        for col in 0..cols {
                            midgame_pst.add(piece_type, color_idx, row, col, mid_addend);
                            endgame_pst.add(piece_type, color_idx, row, col, end_addend);
                        }
                    }
                }
            }
        }

        blend_promotion_into_pst(
            &mut midgame_pst,
            board_size,
            future_discount,
            promo_dampener,
            num_pieces,
            move_generator,
            config_manager,
        );
        blend_promotion_into_pst(
            &mut endgame_pst,
            board_size,
            future_discount,
            promo_dampener,
            num_pieces,
            move_generator,
            config_manager,
        );

        // ── Crowding precomputation ──────────────────────────────────────
        // Max across BOTH phases so the normalization denominator is
        // stable. See the field doc for why we don't interpolate the max.
        let max_mid = intrinsic_values.iter().cloned().fold(0.0f32, f32::max);
        let max_end = endgame_intrinsic_values
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
        let max_intrinsic = max_mid.max(max_end).max(0.001);

        // Contest scores use midgame intrinsics. This is acceptable:
        // contest measures attack-density geometry, which is piece-set
        // dependent but not strongly phase-dependent (a knight's attack
        // pattern doesn't change in the endgame). The phase-sensitivity
        // of crowding comes from the value-squared term, which IS
        // interpolated at evaluation time.
        let contest_scores = compute_contest_scores(
            rows,
            cols,
            num_pieces,
            &intrinsic_values,
            max_intrinsic,
            move_generator,
            config_manager,
        );

        // Percentiles are computed from the midgame PST. The percentile
        // is a within-piece-type ranking of squares, and the ranking
        // rarely changes between phases — a corner is bad for a bishop
        // in both. The absolute VALUE changes (handled by interpolation
        // in get_interpolated); the relative ranking doesn't.
        let pst_percentiles = compute_pst_percentiles(&midgame_pst, num_pieces, rows, cols);
        let pst_percentile_stride = 2 * rows * cols;

        // value_sq_normalized deliberately NOT precomputed. It's now
        // derived per-piece in evaluate_position from the interpolated
        // intrinsic, costing one divide + one multiply per occupied
        // square — negligible against the existing board-scan work.

        // ── HVP threshold ────────────────────────────────────────────────
        // Uses midgame intrinsics. The threshold is a crude selector
        // (top half of pieces) and midgame vs. endgame rarely changes
        // which half a piece falls in.
        let hvp_value_threshold = {
            let mut non_royal: Vec<f32> = intrinsic_values
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                !config_manager
                .get_piece_by_index(*idx)
                .map_or(true, |c| c.properties.is_royal || c.properties.is_royalty)
            })
            .map(|(_, &v)| v)
            .collect();
            if non_royal.is_empty() {
                f32::MAX
            } else {
                non_royal.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                non_royal[non_royal.len() / 2]
            }
        };

        let swarm = SwarmTables::new(flat_size);
        let proximity = PieceProximityTables::new(flat_size);

        Self {
            board_size,
            num_pieces,
            midgame_pst,
            endgame_pst,
            midgame_pieces: midgame_pieces as f32,
            intrinsic_values,
            endgame_intrinsic_values,
            max_intrinsic,
            current_phase: 1.0, // Start at full midgame; refreshed on first eval.
            swarm,
            proximity,
            hvp_value_threshold,
            contest_scores,
            pst_percentiles,
            pst_percentile_stride,
        }
    }

    #[inline(always)]
    fn get_interpolated(
        &self,
        piece_type: usize,
        color_idx: usize,
        row: usize,
        col: usize,
        phase: f32,
    ) -> f32 {
        let mid = self.midgame_pst.get(piece_type, color_idx, row, col);
        let end = self.endgame_pst.get(piece_type, color_idx, row, col);
        mid * phase + end * (1.0 - phase)
    }

    /// Phase-interpolated intrinsic value for a piece type. This is the
    /// value that proximity and crowding penalties should use — NOT the
    /// raw midgame intrinsic.
    #[inline(always)]
    fn intrinsic_interpolated(&self, piece_type: usize, phase: f32) -> f32 {
        // Bounds check is cheap and guards against config mismatches.
        if piece_type >= self.intrinsic_values.len() {
            return 0.0;
        }
        let mid = self.intrinsic_values[piece_type];
        let end = self.endgame_intrinsic_values[piece_type];
        mid * phase + end * (1.0 - phase)
    }

    /// Get the PST percentile for a piece at a square. Returns [0,1].
    #[inline(always)]
    fn get_pst_percentile(
        &self,
        piece_type: usize,
        color_idx: usize,
        row: usize,
        col: usize,
    ) -> f32 {
        let idx = piece_type * self.pst_percentile_stride
        + color_idx * (self.board_size.0 * self.board_size.1)
        + row * self.board_size.1
        + col;
        if idx < self.pst_percentiles.len() {
            unsafe { *self.pst_percentiles.get_unchecked(idx) }
        } else {
            0.0
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Move Contribution Helper
// ─────────────────────────────────────────────────────────────────────────

struct MoveContribution {
    destination: Position,
    diffusion_weight: f64,
    scoring_weight: f64,
}

fn calculate_move_contribution(
    mv: &MoveWithPath,
    empty_probability: f64,
    move_generator: &MoveGenerator,
) -> MoveContribution {
    let blocking_squares = move_generator.count_blocking_squares(mv);
    let blocking_factor = empty_probability.powi(blocking_squares as i32);

    let diffusion_weight = if mv.rule.can_land_enemy {
        blocking_factor * 1.0
    } else if mv.rule.can_land_empty {
        blocking_factor * empty_probability
    } else {
        0.0
    };

    let scoring_weight = if mv.rule.can_land_enemy {
        blocking_factor * 1.0
    } else {
        0.0
    };

    MoveContribution {
        destination: mv.destination,
        diffusion_weight,
        scoring_weight,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PST diffusion computation
// ─────────────────────────────────────────────────────────────────────────

fn compute_square_value(
    pos: Position,
    piece_type: usize,
    color: PieceColor,
    board_size: (usize, usize),
                        empty_prob: f64,
                        future_discount: f64,
                        mobility_weight: f64,
                        move_generator: &MoveGenerator,
) -> f64 {
    let mut total_value = 0.0_f64;
    let mut depth = 0usize;
    let mut current_positions: HashMap<Position, f64> = HashMap::new();
    current_positions.insert(pos, 1.0);

    loop {
        let mut next_positions: HashMap<Position, f64> = HashMap::new();
        let mut total_scoring = 0.0_f64;
        let mut total_diffusion = 0.0_f64;

        for (&cur_pos, &probability) in &current_positions {
            let theoretical_moves = move_generator.generate_theoretical_moves_for_pst(
                cur_pos,
                piece_type,
                color,
                board_size,
                if depth == 0 { 0 } else { 1 },
            );

            for mv in &theoretical_moves {
                let contrib = calculate_move_contribution(mv, empty_prob, move_generator);

                total_scoring += probability * contrib.scoring_weight;
                total_diffusion += probability * contrib.diffusion_weight;
                *next_positions.entry(contrib.destination).or_insert(0.0) +=
                probability * contrib.diffusion_weight;
            }
        }

        if total_scoring <= 0.0 || total_diffusion <= 0.0 {
            break;
        }

        let existence = if depth == 0 {
            1.0
        } else {
            future_discount.powi(depth as i32)
        };
        let tempo_pen = if depth == 0 {
            0.0
        } else {
            future_discount * depth as f64
        };
        let penalised = (total_diffusion - tempo_pen).max(0.0);
        let diffusion_ratio = if total_diffusion > 0.0 {
            penalised / total_diffusion
        } else {
            0.0
        };
        let depth_weight = if depth == 0 { mobility_weight } else { 1.0 };
        let contrib = total_scoring * existence * diffusion_ratio * depth_weight;

        if contrib <= 0.0 {
            break;
        }
        total_value += contrib;

        if depth >= 10 {
            break;
        }

        let sum: f64 = next_positions.values().sum();
        if sum > 0.0 {
            for v in next_positions.values_mut() {
                *v /= sum;
            }
        }
        current_positions = next_positions;
        depth += 1;
    }

    total_value
}

// ─────────────────────────────────────────────────────────────────────────
// Promotion bonus blending
// ─────────────────────────────────────────────────────────────────────────

fn blend_promotion_into_pst(
    pst: &mut FlatPst,
    board_size: (usize, usize),
                            future_discount: f64,
                            promo_dampener: f64,
                            num_pieces: usize,
                            _move_generator: &MoveGenerator,
                            config_manager: &PieceConfigManager,
) {
    let (rows, cols) = board_size;

    for piece_type in 0..num_pieces {
        let can_promote = config_manager
        .get_piece_by_index(piece_type)
        .map_or(false, |c| c.properties.can_promote);
        if !can_promote {
            continue;
        }

        let targets = PromotionManager::get_promotion_targets(piece_type, config_manager);
        if targets.is_empty() {
            continue;
        }

        for color_idx in 0..2usize {
            let mut best_avg = 0.0_f32;
            let mut best_target = targets[0];

            for &t in &targets {
                if t >= num_pieces {
                    continue;
                }
                let avg: f32 = (0..rows)
                .flat_map(|r| (0..cols).map(move |c| (r, c)))
                .map(|(r, c)| pst.get(t, color_idx, r, c))
                .sum::<f32>()
                / (rows * cols) as f32;
                if avg > best_avg {
                    best_avg = avg;
                    best_target = t;
                }
            }

            if best_target >= num_pieces {
                continue;
            }

            for row in 0..rows {
                for col in 0..cols {
                    let moves_to_promote = if color_idx == 0 { row } else { rows - 1 - row };
                    if moves_to_promote == 0 {
                        continue;
                    }

                    let promo_rank_col_val = if color_idx == 0 {
                        pst.get(best_target, color_idx, 0, col) as f64
                    } else {
                        pst.get(best_target, color_idx, rows - 1, col) as f64
                    };

                    let existence = future_discount.powi(moves_to_promote as i32);
                    let tempo = future_discount * moves_to_promote as f64;
                    let bonus = (promo_rank_col_val * promo_dampener * existence - tempo).max(0.0);

                    if bonus > 0.0 {
                        pst.add(piece_type, color_idx, row, col, bonus as f32);
                    }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Evaluator struct
// ─────────────────────────────────────────────────────────────────────────

struct PstEvaluator<'a> {
    engine: &'a PstEngine,
}

impl<'a> EvaluatorTrait for PstEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let score = self.engine.evaluate_position(state, config_manager);
        (score * 100.0) as i32
    }

    /// Called by Search::order_moves for MVV-LVA capture ordering.
    /// Uses the cached phase (see EvalData::current_phase docs) to return
    /// the interpolated PST value rather than the raw midgame value.
    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        let data = self.engine.eval_data.borrow();
        let Some(data) = data.as_ref() else {
            return 100;
        };
        let (rows, cols) = data.board_size;
        if pos.0 >= rows || pos.1 >= cols || piece.piece_type >= data.num_pieces {
            return 100;
        }
        let phase = data.current_phase;
        (data.get_interpolated(piece.piece_type, piece.color.index(), pos.0, pos.1, phase) * 100.0)
        as i32
    }

    fn delta_pruning_margin(&self) -> i32 {
        2000
    }
    fn aspiration_window(&self) -> i32 {
        500
    }

    fn contempt(&self) -> i32 { 250 }

    fn evaluate_split(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<(f32, f32)> {
        let (w, b) = self.engine.white_black_scores(state, config_manager);
        Some(match state.current_turn {
            PieceColor::White => (w, b),
             PieceColor::Black => (b, w),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The engine
// ─────────────────────────────────────────────────────────────────────────

pub struct PstEngine {
    eval_data: RefCell<Option<EvalData>>,
    transposition_table: HashMap<u64, crate::engine::search::TTEntry>,
    parameters: EngineParameters,
    needs_reinit: bool,
}

impl PstEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        let defs = combined_params(PST_PARAMETERS, &MERGED);
        Self {
            eval_data: RefCell::new(None),
            transposition_table: HashMap::new(),
            parameters: EngineParameters::from_defaults(defs),
            needs_reinit: true,
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    pub fn initialize_psts(
        &mut self,
        board: &Board,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let board_size = board.size();
        let needs_rebuild = {
            let data = self.eval_data.borrow();
            self.needs_reinit
            || data.is_none()
            || data.as_ref().map_or(true, |d| d.board_size != board_size)
        };

        if !needs_rebuild {
            return;
        }

        println!(
            "🔄 Building Tapered PSTs for {}x{} board, {} piece types…",
            board_size.0,
            board_size.1,
            config_manager.piece_order.len()
        );

        let data = EvalData::new(board, &self.parameters, move_generator, config_manager);
        *self.eval_data.borrow_mut() = Some(data);
        self.needs_reinit = false;
        println!("✅ PST tables ready.");
    }

    /// Refresh the cached game phase from a board's piece count. Called
    /// at the top of best_move and analyze_position so that
    /// get_piece_value_on_square and get_pst_value have an accurate
    /// phase before any search or analysis runs.
    fn update_phase(&self, board: &Board) {
        let (_, _, piece_count) = compute_center_of_mass_and_count(board);
        let mut data = self.eval_data.borrow_mut();
        if let Some(data) = data.as_mut() {
            let mid_p = data.midgame_pieces;
            let end_p = 4.0_f32;
            data.current_phase = if mid_p > end_p {
                ((piece_count - end_p) / (mid_p - end_p)).clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
    }

    pub fn evaluate_position(
        &self,
        state: &GameState,
        config_manager: &PieceConfigManager,
    ) -> f32 {
        let (white, black) = self.white_black_scores(state, config_manager);
        match state.current_turn {
            PieceColor::White => white - black,
            PieceColor::Black => black - white,
        }
    }

    pub fn white_black_scores(&self, state: &GameState, _config_manager: &PieceConfigManager) -> (f32, f32) {
        let offensive_weight = self.get_param(PARAM_PIECE_PROXIMITY_OFFENSIVE, 0.0) as f32;
        let defensive_weight = self.get_param(PARAM_PIECE_PROXIMITY_DEFENSIVE, 0.0) as f32;
        let crowding_weight = self.get_param(PARAM_PST_CROWDING_PENALTY, 0.0) as f32;
        let has_proximity = offensive_weight > 0.0 || defensive_weight > 0.0;
        let has_crowding = crowding_weight > 0.0;

        // Piece count & center-of-mass, computed once up front. We need
        // the count for the phase update inside the mutable borrow below.
        let (center_r, center_c, current_pieces) = compute_center_of_mass_and_count(&state.board);

        // ── Mutable section: update phase, swarm, proximity caches ────────
        {
            let white_royals = state.board.get_royal_positions(PieceColor::White);
            let black_royals = state.board.get_royal_positions(PieceColor::Black);

            let mut data_mut = self.eval_data.borrow_mut();
            let data = match data_mut.as_mut() {
                Some(d) => d,
                None => return (0.0, 0.0),
            };

            // Store the phase. This is what get_piece_value_on_square
            // will read on subsequent order_moves calls until the next
            // leaf evaluation updates it again.
            let mid_p = data.midgame_pieces;
            let end_p = 4.0_f32;
            data.current_phase = if mid_p > end_p {
                ((current_pieces - end_p) / (mid_p - end_p)).clamp(0.0, 1.0)
            } else {
                1.0
            };

            if !data.swarm.is_cache_valid(white_royals, black_royals) {
                let swarm_mult = self.get_param(PARAM_MULTIPLICATIVE_SWARM, 0.05) as f32;
                let swarm_add = self.get_param(PARAM_ADDITIVE_SWARM, 0.5) as f32;
                let huddle = self.get_param(PARAM_HUDDLE_BONUS, 0.0) as f32;
                let (rows, cols) = data.board_size;
                data.swarm.recompute(
                    white_royals,
                    black_royals,
                    rows,
                    cols,
                    swarm_mult,
                    swarm_add,
                    huddle,
                );
            }

            // ── Update proximity cache (fingerprint-based, cheap) ─────────
            if has_proximity {
                let threshold = data.hvp_value_threshold;
                let num_pieces = data.num_pieces;
                let intrinsic_values = &data.intrinsic_values;
                data.proximity.update_if_needed(
                    &state.board,
                    intrinsic_values,
                    threshold,
                    num_pieces,
                    offensive_weight,
                );
            }
        }

        let data_ref = self.eval_data.borrow();
        let data = match data_ref.as_ref() {
            Some(d) => d,
            None => return (0.0, 0.0),
        };

        let (rows, cols) = data.board_size;
        if (rows, cols) != state.board.size() {
            return (0.0, 0.0);
        }

        let gravity_bonus = self.get_param(PARAM_GRAVITY_BONUS, 0.0) as f32;
        let center_of_mass = if gravity_bonus > 0.0 {
            Some((center_r, center_c))
        } else {
            None
        };

        // Read phase back from the cache (we just wrote it above).
        let phase = data.current_phase;

        // Crowding phase factor: full penalty in midgame, reduced in endgame.
        let crowding_phase = if has_crowding { phase } else { 0.0 };

        // Hoist the crowding normalization factor out of the per-piece
        // loop. max_intrinsic is phase-invariant (see field docs).
        let inv_max_intrinsic = 1.0 / data.max_intrinsic;

        let mut white_total = 0.0_f32;
        let mut black_total = 0.0_f32;

        for row in 0..rows {
            for col in 0..cols {
                let Some(piece) = state.board.get_piece((row, col)) else {
                    continue;
                };
                if piece.piece_type >= data.num_pieces {
                    continue;
                }

                let color_idx = piece.color.index();
                let flat = row * cols + col;

                // ── Base tapered PST value ────────────────────────────────
                let base = data.get_interpolated(piece.piece_type, color_idx, row, col, phase);

                // ── Swarm adjustment ──────────────────────────────────────
                let sm = data.swarm.get_mult(color_idx, flat);
                let sa = data.swarm.get_add(color_idx, flat);
                let hm = data.swarm.get_huddle(color_idx, flat);
                let adjusted = base * (1.0 + sm + hm) + sa;

                // ── Proximity bonus/penalty ───────────────────────────────
                // Uses the INTERPOLATED intrinsic so a queen's endgame
                // fragility is weighted with its endgame value.
                let proximity_delta = if has_proximity {
                    let off = data.proximity.get_offensive(color_idx, flat);
                    let piece_intrinsic = data.intrinsic_interpolated(piece.piece_type, phase);
                    let def = data.proximity.defensive_penalty(
                        color_idx,
                        flat,
                        piece_intrinsic,
                        data.max_intrinsic,
                        defensive_weight,
                    );
                    off - def
                } else {
                    0.0
                };

                // ── Crowding penalty ──────────────────────────────────────
                // val_sq now computed inline from the interpolated
                // intrinsic. Replaces the precomputed value_sq_normalized
                // table, which was frozen at midgame values.
                let crowding_pen = if has_crowding && crowding_phase > 0.0 {
                    let intrinsic = data.intrinsic_interpolated(piece.piece_type, phase);
                    let norm = intrinsic * inv_max_intrinsic;
                    let val_sq = norm * norm;
                    let pst_pct = data.get_pst_percentile(piece.piece_type, color_idx, row, col);
                    let contest = unsafe { *data.contest_scores.get_unchecked(flat) };
                    crowding_weight * val_sq * pst_pct * contest * crowding_phase
                } else {
                    0.0
                };

                // ── Gravity ───────────────────────────────────────────────
                let gravity = if let Some(com) = center_of_mass {
                    let dr = row as f32 - com.0;
                    let dc = col as f32 - com.1;
                    let dist = (dr * dr + dc * dc).sqrt().max(0.01);
                    gravity_bonus / dist
                } else {
                    0.0
                };

                let square_score = adjusted + proximity_delta - crowding_pen + gravity;

                if piece.color == PieceColor::White {
                    white_total += square_score;
                } else {
                    black_total += square_score;
                }
            }
        }

        (white_total, black_total)
    }

    /// Public PST value query. Uses the cached phase so analysis displays
    /// reflect the current game stage rather than always showing midgame
    /// values. Call update_phase() first if the board has changed since
    /// the last evaluate_position call.
    pub fn get_pst_value(&self, piece: &Piece, pos: Position) -> Option<i32> {
        let data = self.eval_data.borrow();
        let data = data.as_ref()?;
        let (rows, cols) = data.board_size;
        if pos.0 >= rows || pos.1 >= cols || piece.piece_type >= data.num_pieces {
            return None;
        }
        let phase = data.current_phase;
        let v = data.get_interpolated(piece.piece_type, piece.color.index(), pos.0, pos.1, phase);
        Some((v * 100.0) as i32)
    }
}

#[inline]
fn compute_center_of_mass_and_count(board: &Board) -> (f32, f32, f32) {
    let (rows, cols) = board.size();
    let mut total_r = 0.0_f32;
    let mut total_c = 0.0_f32;
    let mut count = 0u32;

    for row in 0..rows {
        for col in 0..cols {
            if board.get_piece((row, col)).is_some() {
                total_r += row as f32;
                total_c += col as f32;
                count += 1;
            }
        }
    }

    if count > 0 {
        (total_r / count as f32, total_c / count as f32, count as f32)
    } else {
        ((rows - 1) as f32 / 2.0, (cols - 1) as f32 / 2.0, 0.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Analysis — keeping existing implementations, just updating EvalData fields
// ─────────────────────────────────────────────────────────────────────────

impl PstEngine {
    pub fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::PositionAnalysis {
        use crate::engine::analysis::*;

        self.initialize_psts(&state.board, move_generator, config_manager);

        // Ensure get_pst_value (called throughout analysis) uses the
        // phase for THIS position, not whatever stale value was left
        // from the last search.
        self.update_phase(&state.board);

        let material_analysis = self.analyze_material(state, config_manager);
        let pst_analysis = Some(self.analyze_pst_position(state, config_manager));
        let mut mobility_analysis = self.analyze_mobility(state, move_generator, config_manager);

        mobility_analysis.theoretical_mobility =
        self.analyze_theoretical_mobility(move_generator, config_manager, &state.board);
        mobility_analysis.threat_analysis =
        self.analyze_threats(state, move_generator, config_manager);

        let density_analysis = self.analyze_density(state, config_manager);
        let statistical_analysis =
        self.analyze_statistics(state, config_manager, &material_analysis);

        PositionAnalysis {
            material_values: material_analysis,
            pst_analysis,
            mobility_analysis,
            density_analysis,
            statistical_analysis,
        }
    }

    fn analyze_material(
        &self,
        state: &GameState,
        _config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::MaterialAnalysis {
        use crate::engine::analysis::MaterialAnalysis;

        let mut white_total = 0.0_f64;
        let mut black_total = 0.0_f64;
        let mut piece_counts: HashMap<usize, (u32, u32)> = HashMap::new();
        let mut piece_sums: HashMap<usize, f64> = HashMap::new();

        for row in 0..state.board.size().0 {
            for col in 0..state.board.size().1 {
                if let Some(piece) = state.board.get_piece((row, col)) {
                    let piece_value =
                    self.get_pst_value(&piece, (row, col)).unwrap_or(100) as f64 / 100.0;
                    let (wc, bc) = piece_counts.entry(piece.piece_type).or_insert((0, 0));
                    match piece.color {
                        PieceColor::White => {
                            white_total += piece_value;
                            *wc += 1;
                        }
                        PieceColor::Black => {
                            black_total += piece_value;
                            *bc += 1;
                        }
                    }
                    *piece_sums.entry(piece.piece_type).or_insert(0.0) += piece_value;
                }
            }
        }

        let piece_values: HashMap<usize, f64> = piece_sums
        .iter()
        .filter_map(|(&pt, &sum)| {
            let (wc, bc) = piece_counts.get(&pt)?;
            let total = (wc + bc) as f64;
            if total > 0.0 {
                Some((pt, sum / total))
            } else {
                None
            }
        })
        .collect();

        MaterialAnalysis {
            white_total,
            black_total,
            difference: white_total - black_total,
            piece_counts,
            piece_values,
        }
    }

    fn analyze_pst_position(
        &self,
        state: &GameState,
        _config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::PstAnalysis {
        use crate::engine::analysis::*;

        let data_ref = self.eval_data.borrow();
        let data = match data_ref.as_ref() {
            Some(d) => d,
            None => {
                return PstAnalysis {
                    white_pst_total: 0.0,
                    black_pst_total: 0.0,
                    pst_difference: 0.0,
                    piece_pst_stats: HashMap::new(),
                    variance_analysis: VarianceAnalysis {
                        positional_bias: PositionalBias {
                            forward_bias: 0.0,
                                backward_bias: 0.0,
                                center_bias: 0.0,
                                edge_bias: 0.0,
                                left_right_bias: 0.0,
                        },
                        value_distribution: ValueDistribution {
                            highest_value_squares: vec![],
                            lowest_value_squares: vec![],
                            value_range: 0.0,
                            value_variance: 0.0,
                        },
                    },
                    swarm_factors: SwarmAnalysis {
                        average_swarm_bonus: 0.0,
                        max_swarm_position: None,
                        swarm_effectiveness: 0.0,
                        huddle_factor: 0.0,
                    },
                };
            }
        };

        let board_size = data.board_size;
        let (rows, cols) = board_size;
        let mut white_pst_total = 0.0_f64;
        let mut black_pst_total = 0.0_f64;
        let mut piece_pst_stats: HashMap<usize, PiecePstStats> = HashMap::new();

        let (_, _, current_pieces) = compute_center_of_mass_and_count(&state.board);
        let phase = if data.midgame_pieces > 4.0 {
            ((current_pieces - 4.0) / (data.midgame_pieces - 4.0)).clamp(0.0, 1.0)
        } else {
            1.0
        };

        for piece_type in 0..data.num_pieces {
            let mut all_vals: Vec<f64> = Vec::with_capacity(2 * rows * cols);
            for color_idx in 0..2 {
                for row in 0..rows {
                    for col in 0..cols {
                        all_vals.push(
                            data.get_interpolated(piece_type, color_idx, row, col, phase) as f64,
                        );
                    }
                }
            }
            if all_vals.is_empty() {
                continue;
            }
            let min_v = all_vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_v = all_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg_v = all_vals.iter().sum::<f64>() / all_vals.len() as f64;
            let var =
            all_vals.iter().map(|v| (v - avg_v).powi(2)).sum::<f64>() / all_vals.len() as f64;
            piece_pst_stats.insert(
                piece_type,
                PiecePstStats {
                    piece_type,
                    min_value: min_v,
                    max_value: max_v,
                    average_value: avg_v,
                    variance: var,
                    standard_deviation: var.sqrt(),
                                   current_total: 0.0,
                                   current_count: 0,
                                   current_average: 0.0,
                },
            );
        }

        for row in 0..rows {
            for col in 0..cols {
                if let Some(piece) = state.board.get_piece((row, col)) {
                    let v = data.get_interpolated(
                        piece.piece_type,
                        piece.color.index(),
                                                  row,
                                                  col,
                                                  phase,
                    ) as f64;
                    match piece.color {
                        PieceColor::White => white_pst_total += v,
                        PieceColor::Black => black_pst_total += v,
                    }
                    if let Some(stats) = piece_pst_stats.get_mut(&piece.piece_type) {
                        stats.current_total += v;
                        stats.current_count += 1;
                        stats.current_average = stats.current_total / stats.current_count as f64;
                    }
                }
            }
        }

        let variance_analysis = self.analyze_variance_from_flat(data, board_size, phase);
        let swarm_analysis = self.analyze_swarm_from_tables(&data.swarm, board_size);

        PstAnalysis {
            white_pst_total,
            black_pst_total,
            pst_difference: white_pst_total - black_pst_total,
            piece_pst_stats,
            variance_analysis,
            swarm_factors: swarm_analysis,
        }
    }

    fn analyze_variance_from_flat(
        &self,
        data: &EvalData,
        board_size: (usize, usize),
                                  phase: f32,
    ) -> crate::engine::analysis::VarianceAnalysis {
        use crate::engine::analysis::*;

        let (rows, cols) = board_size;
        let mut forward_bias = 0.0_f64;
        let mut backward_bias = 0.0_f64;
        let mut center_bias = 0.0_f64;
        let mut edge_bias = 0.0_f64;
        let mut left_right_bias = 0.0_f64;
        let mut all_values: Vec<f64> = Vec::new();
        let mut value_positions: Vec<(Position, f64)> = Vec::new();

        for piece_type in 0..data.num_pieces {
            for color_idx in 0..2 {
                for row in 0..rows {
                    for col in 0..cols {
                        let v =
                        data.get_interpolated(piece_type, color_idx, row, col, phase) as f64;
                        all_values.push(v);
                        value_positions.push(((row, col), v));

                        let rf = row as f64 / (rows - 1).max(1) as f64;
                        let cf = col as f64 / (cols - 1).max(1) as f64;

                        if color_idx == 0 {
                            forward_bias += v * rf;
                            backward_bias += v * (1.0 - rf);
                        } else {
                            forward_bias += v * (1.0 - rf);
                            backward_bias += v * rf;
                        }

                        let cd = ((row as f64 - (rows - 1) as f64 / 2.0).powi(2)
                        + (col as f64 - (cols - 1) as f64 / 2.0).powi(2))
                        .sqrt();
                        let max_d = ((rows as f64).powi(2) + (cols as f64).powi(2)).sqrt() / 2.0;
                        center_bias += v * (1.0 - cd / max_d);

                        let is_edge = row == 0 || row == rows - 1 || col == 0 || col == cols - 1;
                        edge_bias += v * if is_edge { 1.0 } else { 0.0 };
                        left_right_bias += v * (cf - 0.5) * 2.0;
                    }
                }
            }
        }

        let total: f64 = all_values.iter().sum();
        if total != 0.0 {
            forward_bias /= total;
            backward_bias /= total;
            center_bias /= total;
            edge_bias /= total;
            left_right_bias /= total;
        }

        value_positions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let highest = value_positions.iter().take(5).cloned().collect();
        let lowest = value_positions.iter().rev().take(5).cloned().collect();

        let range = if !all_values.is_empty() {
            all_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - all_values.iter().cloned().fold(f64::INFINITY, f64::min)
        } else {
            0.0
        };
        let mean = all_values.iter().sum::<f64>() / all_values.len().max(1) as f64;
        let var = all_values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / all_values.len().max(1) as f64;

        VarianceAnalysis {
            positional_bias: PositionalBias {
                forward_bias,
                    backward_bias,
                    center_bias,
                    edge_bias,
                    left_right_bias,
            },
            value_distribution: ValueDistribution {
                highest_value_squares: highest,
                lowest_value_squares: lowest,
                value_range: range,
                value_variance: var,
            },
        }
    }

    fn analyze_swarm_from_tables(
        &self,
        swarm: &SwarmTables,
        board_size: (usize, usize),
    ) -> crate::engine::analysis::SwarmAnalysis {
        use crate::engine::analysis::SwarmAnalysis;

        let (rows, cols) = board_size;
        let mut total_swarm = 0.0_f64;
        let mut max_swarm = 0.0_f64;
        let mut max_pos: Option<(Position, f64)> = None;
        let mut total_huddle = 0.0_f64;
        let mut count = 0usize;

        for row in 0..rows {
            for col in 0..cols {
                let flat = row * cols + col;
                for color_idx in 0..2 {
                    let s =
                    (swarm.get_mult(color_idx, flat) + swarm.get_add(color_idx, flat)) as f64;
                    let h = swarm.get_huddle(color_idx, flat) as f64;
                    total_swarm += s;
                    total_huddle += h;
                    if s > max_swarm {
                        max_swarm = s;
                        max_pos = Some(((row, col), s));
                    }
                    count += 1;
                }
            }
        }

        let n = count.max(1) as f64;
        SwarmAnalysis {
            average_swarm_bonus: total_swarm / n,
            max_swarm_position: max_pos,
            swarm_effectiveness: max_swarm,
            huddle_factor: total_huddle / n,
        }
    }

    fn analyze_mobility(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::MobilityAnalysis {
        use crate::engine::analysis::*;

        let current_player = state.current_turn;
        let current_moves = state.get_legal_moves(move_generator, config_manager);

        let original_turn = state.current_turn;
        state.current_turn = state.current_turn.opposite();
        let opponent_moves = state.get_legal_moves(move_generator, config_manager);
        state.current_turn = original_turn;

        let (white_mobility, black_mobility) = match current_player {
            PieceColor::White => (current_moves.len() as u32, opponent_moves.len() as u32),
            PieceColor::Black => (opponent_moves.len() as u32, current_moves.len() as u32),
        };

        let all_moves: Vec<_> = current_moves
        .into_iter()
        .chain(opponent_moves.into_iter())
        .collect();

        let mut piece_counts_total: HashMap<usize, u32> = HashMap::new();
        for row in 0..state.board.size().0 {
            for col in 0..state.board.size().1 {
                if let Some(piece) = state.board.get_piece((row, col)) {
                    *piece_counts_total.entry(piece.piece_type).or_insert(0) += 1;
                }
            }
        }

        let mut piece_move_counts: HashMap<usize, u32> = HashMap::new();
        let mut piece_attacking: HashMap<usize, u32> = HashMap::new();
        let mut piece_non_attacking: HashMap<usize, u32> = HashMap::new();

        for mv in &all_moves {
            if let Some(piece) = state.board.get_piece(mv.from) {
                *piece_move_counts.entry(piece.piece_type).or_insert(0) += 1;
                if mv.captures.is_some() {
                    *piece_attacking.entry(piece.piece_type).or_insert(0) += 1;
                } else {
                    *piece_non_attacking.entry(piece.piece_type).or_insert(0) += 1;
                }
            }
        }

        let mut piece_mobility: HashMap<usize, MobilityStats> = HashMap::new();
        for (&pt, &total_moves) in &piece_move_counts {
            let pc = piece_counts_total.get(&pt).copied().unwrap_or(1);
            let avg = total_moves as f64 / pc as f64;
            piece_mobility.insert(
                pt,
                MobilityStats {
                    total_moves,
                    piece_count: pc,
                    average_mobility: avg,
                    mobility_variance: 0.0,
                    attacking_moves: piece_attacking.get(&pt).copied().unwrap_or(0),
                                  non_attacking_moves: piece_non_attacking.get(&pt).copied().unwrap_or(0),
                },
            );
        }

        let cur_mob = match current_player {
            PieceColor::White => white_mobility,
            PieceColor::Black => black_mobility,
        } as f64;
        let cur_val = self.calculate_player_value(state, config_manager, current_player);
        let vtm = if cur_mob > 0.0 {
            cur_val / cur_mob
        } else {
            0.0
        };

        MobilityAnalysis {
            white_mobility,
            black_mobility,
            mobility_difference: white_mobility as i32 - black_mobility as i32,
            piece_mobility,
            value_to_mobility_ratio: vtm,
            theoretical_mobility: HashMap::new(),
            threat_analysis: ThreatAnalysis {
                white_threats_value: 0.0,
                black_threats_value: 0.0,
                white_attackers_value: 0.0,
                black_attackers_value: 0.0,
                white_capture_mobility_percentage: 0.0,
                black_capture_mobility_percentage: 0.0,
                white_threat_balance: 0.0,
                black_threat_balance: 0.0,
            },
        }
    }

    fn calculate_player_value(
        &self,
        state: &GameState,
        _config_manager: &PieceConfigManager,
        color: PieceColor,
    ) -> f64 {
        let mut total = 0.0_f64;
        for row in 0..state.board.size().0 {
            for col in 0..state.board.size().1 {
                if let Some(piece) = state.board.get_piece((row, col)) {
                    if piece.color == color {
                        total +=
                        self.get_pst_value(&piece, (row, col)).unwrap_or(100) as f64 / 100.0;
                    }
                }
            }
        }
        total
    }

    fn analyze_theoretical_mobility(
        &self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        board: &Board,
    ) -> HashMap<usize, crate::engine::analysis::TheoreticalMobilityStats> {
        use crate::engine::analysis::TheoreticalMobilityStats;

        let board_size = board.size();
        let (rows, cols) = board_size;
        let total_sq = (rows * cols) as f64;

        let mut initial_pieces = 0.0_f64;
        for r in 0..rows {
            for c in 0..cols {
                if board.get_piece((r, c)).is_some() {
                    initial_pieces += 1.0;
                }
            }
        }

        let density = (initial_pieces / total_sq).min(0.9);
        let mid_structural = self
        .parameters
        .get_or_default(PARAM_MIDGAME_STRUCTURAL_FACTOR, 0.8);
        let empty_prob = 1.0 - density * mid_structural;

        let center_row = rows / 2;
        let center_col = cols / 2;

        let mut result = HashMap::new();

        for (piece_type, _) in config_manager.piece_order.iter().enumerate() {
            let mut center_vals = Vec::new();
            let mut corner_vals = Vec::new();
            let mut edge_vals = Vec::new();
            let mut min_mob = u32::MAX;
            let mut max_mob = 0u32;

            for row in 0..rows {
                for col in 0..cols {
                    let moves = move_generator.generate_theoretical_moves_for_pst(
                        (row, col),
                                                                                  piece_type,
                                                                                  PieceColor::White,
                                                                                  board_size,
                                                                                  0,
                    );
                    let mob = moves.len() as u32;
                    min_mob = min_mob.min(mob);
                    max_mob = max_mob.max(mob);

                    let blocked_mob = moves
                    .iter()
                    .map(|mv| {
                        let blocking_squares = move_generator.count_blocking_squares(mv);
                        let blocking = empty_prob.powi(blocking_squares as i32);
                        if mv.rule.can_land_enemy {
                            blocking * (1.0 - empty_prob)
                        } else if mv.rule.can_land_empty {
                            blocking * empty_prob
                        } else {
                            0.0
                        }
                    })
                    .sum::<f64>();

                    let is_corner = (row == 0 || row == rows - 1) && (col == 0 || col == cols - 1);
                    let is_edge =
                    !is_corner && (row == 0 || row == rows - 1 || col == 0 || col == cols - 1);
                    let is_center = (row as i32 - center_row as i32).abs() <= 1
                    && (col as i32 - center_col as i32).abs() <= 1
                    && !is_edge
                    && !is_corner;

                    if is_corner {
                        corner_vals.push(blocked_mob);
                    } else if is_center {
                        center_vals.push(blocked_mob);
                    } else if is_edge {
                        edge_vals.push(blocked_mob);
                    }
                }
            }

            let avg = |v: &[f64]| {
                if v.is_empty() {
                    0.0
                } else {
                    v.iter().sum::<f64>() / v.len() as f64
                }
            };

            result.insert(
                piece_type,
                TheoreticalMobilityStats {
                    piece_type,
                    center_mobility: avg(&center_vals),
                          corner_mobility: avg(&corner_vals),
                          edge_mobility: avg(&edge_vals),
                          mobility_variance: 0.0,
                          concentration_factor: 0.0,
                          max_mobility: max_mob,
                          min_mobility: if min_mob == u32::MAX { 0 } else { min_mob },
                },
            );
        }

        result
    }

    fn analyze_threats(
        &self,
        state: &GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::ThreatAnalysis {
        use crate::engine::analysis::ThreatAnalysis;

        let mut sc = state.clone();

        sc.current_turn = PieceColor::White;
        let white_moves = sc.get_legal_moves(move_generator, config_manager);
        sc.current_turn = PieceColor::Black;
        let black_moves = sc.get_legal_moves(move_generator, config_manager);

        let mut wt = 0.0_f64;
        let mut wat = 0.0_f64;
        let mut wcap = 0u32;
        for mv in &white_moves {
            if let Some(cap) = mv.captures {
                wcap += 1;
                wt += self
                .get_pst_value(&cap, mv.captures_position.unwrap_or(mv.to))
                .unwrap_or(100) as f64
                / 100.0;
                if let Some(att) = state.board.get_piece(mv.from) {
                    wat += self.get_pst_value(&att, mv.from).unwrap_or(100) as f64 / 100.0;
                }
            }
        }

        let mut bt = 0.0_f64;
        let mut bat = 0.0_f64;
        let mut bcap = 0u32;
        for mv in &black_moves {
            if let Some(cap) = mv.captures {
                bcap += 1;
                bt += self
                .get_pst_value(&cap, mv.captures_position.unwrap_or(mv.to))
                .unwrap_or(100) as f64
                / 100.0;
                if let Some(att) = state.board.get_piece(mv.from) {
                    bat += self.get_pst_value(&att, mv.from).unwrap_or(100) as f64 / 100.0;
                }
            }
        }

        let wt_pct = if !white_moves.is_empty() {
            wcap as f64 / white_moves.len() as f64 * 100.0
        } else {
            0.0
        };
        let bt_pct = if !black_moves.is_empty() {
            bcap as f64 / black_moves.len() as f64 * 100.0
        } else {
            0.0
        };

        ThreatAnalysis {
            white_threats_value: wt,
            black_threats_value: bt,
            white_attackers_value: wat,
            black_attackers_value: bat,
            white_capture_mobility_percentage: wt_pct,
            black_capture_mobility_percentage: bt_pct,
            white_threat_balance: wt - bt,
            black_threat_balance: bt - wt,
        }
    }

    fn analyze_density(
        &self,
        state: &GameState,
        config_manager: &PieceConfigManager,
    ) -> crate::engine::analysis::DensityAnalysis {
        use crate::engine::analysis::*;

        let board_size = state.board.size();
        let total_sq = (board_size.0 * board_size.1) as f64;
        let mut piece_count = 0usize;
        let mut positions = Vec::new();

        for row in 0..board_size.0 {
            for col in 0..board_size.1 {
                if state.board.get_piece((row, col)).is_some() {
                    piece_count += 1;
                    positions.push((row, col));
                }
            }
        }

        let density = piece_count as f64 / total_sq;
        let total_val = self.evaluate_position(state, config_manager).abs() as f64;
        let ratio = if density > 0.0 {
            total_val / density
        } else {
            0.0
        };
        let clustering = self.analyze_clustering(&positions, board_size);

        DensityAnalysis {
            board_density: density,
            piece_density_ratio: ratio,
            clustering,
        }
    }

    fn analyze_clustering(
        &self,
        positions: &[Position],
        board_size: (usize, usize),
    ) -> crate::engine::analysis::ClusteringAnalysis {
        use crate::engine::analysis::ClusteringAnalysis;

        if positions.is_empty() {
            return ClusteringAnalysis {
                average_piece_distance: 0.0,
                clustering_coefficient: 0.0,
                isolated_pieces: Vec::new(),
                dense_regions: Vec::new(),
            };
        }

        let mut total_dist = 0.0_f64;
        let mut pair_count = 0usize;
        for i in 0..positions.len() {
            for j in i + 1..positions.len() {
                let dr = positions[i].0 as f64 - positions[j].0 as f64;
                let dc = positions[i].1 as f64 - positions[j].1 as f64;
                total_dist += (dr * dr + dc * dc).sqrt();
                pair_count += 1;
            }
        }
        let avg_dist = if pair_count > 0 {
            total_dist / pair_count as f64
        } else {
            0.0
        };

        let isolated: Vec<Position> = positions
        .iter()
        .filter(|&&p| {
            !positions.iter().any(|&q| {
                q != p && {
                    let dr = p.0 as f64 - q.0 as f64;
                    let dc = p.1 as f64 - q.1 as f64;
                    (dr * dr + dc * dc).sqrt() <= 2.0
                }
            })
        })
        .cloned()
        .collect();

        let mut dense = Vec::new();
        for row in 0..board_size.0 {
            for col in 0..board_size.1 {
                let nearby = positions
                .iter()
                .filter(|&&p| {
                    let dr = p.0 as f64 - row as f64;
                    let dc = p.1 as f64 - col as f64;
                    (dr * dr + dc * dc).sqrt() <= 2.0
                })
                .count();
                if nearby >= 3 {
                    dense.push(((row, col), nearby as f64));
                }
            }
        }

        let with_neighbors = positions.len() - isolated.len();
        let coeff = if !positions.is_empty() {
            with_neighbors as f64 / positions.len() as f64
        } else {
            0.0
        };

        ClusteringAnalysis {
            average_piece_distance: avg_dist,
            clustering_coefficient: coeff,
            isolated_pieces: isolated,
            dense_regions: dense,
        }
    }

    fn analyze_statistics(
        &self,
        state: &GameState,
        _config_manager: &PieceConfigManager,
        material: &crate::engine::analysis::MaterialAnalysis,
    ) -> crate::engine::analysis::StatisticalAnalysis {
        use crate::engine::analysis::*;

        let weakest = material
        .piece_values
        .values()
        .filter(|&&v| v > 0.0)
        .cloned()
        .fold(f64::INFINITY, f64::min);
        let weakest = if weakest.is_finite() { weakest } else { 1.0 };

        let normalized: HashMap<usize, f64> = material
        .piece_values
        .iter()
        .map(|(&pt, &v)| (pt, v / weakest))
        .collect();

        let total_pieces: u32 = material.piece_counts.values().map(|(w, b)| w + b).sum();
        let total_val = material.white_total + material.black_total;
        let vpp = if total_pieces > 0 {
            total_val / total_pieces as f64
        } else {
            0.0
        };

        let mut var_sum = 0.0_f64;
        let mut var_count = 0usize;
        for row in 0..state.board.size().0 {
            for col in 0..state.board.size().1 {
                if let Some(piece) = state.board.get_piece((row, col)) {
                    let v = self.get_pst_value(&piece, (row, col)).unwrap_or(100) as f64 / 100.0;
                    var_sum += (v - vpp).powi(2);
                    var_count += 1;
                }
            }
        }
        let var = if var_count > 0 {
            var_sum / var_count as f64
        } else {
            0.0
        };

        let mut diversity = 0.0_f64;
        if total_pieces > 0 {
            for (wc, bc) in material.piece_counts.values() {
                let c = (wc + bc) as f64;
                if c > 0.0 {
                    let p = c / total_pieces as f64;
                    diversity -= p * p.log2();
                }
            }
        }

        let mut weighted_sum = 0.0_f64;
        let mut weighted_count = 0.0_f64;
        for (&pt, &avg) in &material.piece_values {
            let (wc, bc) = material.piece_counts.get(&pt).unwrap_or(&(0, 0));
            let c = (wc + bc) as f64;
            weighted_sum += avg * c;
            weighted_count += c;
        }
        let weighted_avg = if weighted_count > 0.0 {
            weighted_sum / weighted_count
        } else {
            0.0
        };

        StatisticalAnalysis {
            normalized_values: NormalizedValues {
                weakest_piece_value: weakest,
                white_total_normalized: material.white_total / weakest,
                black_total_normalized: material.black_total / weakest,
                piece_values_normalized: normalized,
            },
            statistics: PositionStatistics {
                total_pieces,
                value_per_piece: vpp,
                value_weighted_average: weighted_avg,
                value_variance: var,
                piece_type_diversity: diversity,
                position_complexity: (material.piece_counts.len() as f64 + var.sqrt()) / 2.0,
            },
        }
    }

    pub fn evaluate_position_with_pst(
        &self,
        state: &GameState,
        config_manager: &PieceConfigManager,
    ) -> f64 {
        self.evaluate_position(state, config_manager) as f64
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ChessEngine impl
// ─────────────────────────────────────────────────────────────────────────

impl ChessEngine for PstEngine {
    fn name(&self) -> &str {
        "Piece Square Table Engine"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
        *self.eval_data.borrow_mut() = None;
        self.needs_reinit = true;
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.initialize_psts(&params.state.board, params.move_generator, params.config_manager);
        self.update_phase(&params.state.board);

        let evaluator = PstEvaluator { engine: self };
        let mut search = if SearchConfig::use_new_search(&self.parameters) {
            Search::with_config(&evaluator, SearchConfig::from_params(&self.parameters))
        } else {
            Search::new(&evaluator)
        };
        search.set_transposition_table(self.transposition_table.clone());

        let depth = if params.depth > 0 { params.depth } else { 4 };
        let result = if let Some(time_limit) = params.time_limit {
            search.find_best_move_iterative(
                params.state, params.move_generator, params.config_manager, depth, time_limit,
            )
        } else {
            search.find_best_move_with_depth(
                params.state, params.move_generator, params.config_manager, depth,
            )
        };
        self.transposition_table = search.get_transposition_table();
        let (best_move, evaluation, depth_reached) = result?;
        let mate_in = if evaluation >= 999000 {
            Some(((999999 - evaluation) / 2) as i32)
        } else if evaluation <= -999000 {
            Some(-((-999999 - evaluation) / 2) as i32)
        } else { None };
        Some(SearchResult {
            best_move,
            evaluation: Evaluation { score: evaluation, mate_in },
            depth_reached,
        })
    }

    fn stop(&mut self) {}

    fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::analysis::PositionAnalysis> {
        Some(self.analyze_position(state, move_generator, config_manager))
    }

    fn supports_analysis(&self) -> bool {
        true
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(PST_PARAMETERS, &MERGED))
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    // PstEngine::set_parameters
    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
            self.needs_reinit = true;
            self.transposition_table.clear(); // avoid mixing eval scales across modes
        }
        changed
    }
}
