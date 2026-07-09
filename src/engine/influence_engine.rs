use crate::core::GameState;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::analysis;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{Search, TTEntry, combined_params, SearchConfig};
use crate::move_generator::{MoveGenerator, MoveWithPath};
use crate::piece_config::PieceConfigManager;
use crate::promotion::PromotionManager;
use smallvec::SmallVec;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

// ─────────────────────────────────────────────────────────────────────────
// Tunable parameters
// ─────────────────────────────────────────────────────────────────────────
pub const PARAM_MATERIAL_WEIGHT: &str = "material_weight";
pub const PARAM_TERRITORY_WEIGHT: &str = "territory_weight";
pub const PARAM_THREAT_WEIGHT: &str = "threat_weight";
pub const PARAM_CONTROL_SHARPNESS: &str = "control_sharpness";
pub const PARAM_BASE_INFLUENCE: &str = "base_influence";
pub const PARAM_ROYAL_FACTOR: &str = "royal_value_factor";
pub const PARAM_TEMPO_BONUS: &str = "tempo_bonus";
pub const PARAM_PROMOTION_INFLUENCE: &str = "promotion_influence";
pub const PARAM_KING_ATTACK_WEIGHT: &str = "king_attack_weight";

pub static INFLUENCE_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MATERIAL_WEIGHT,
        "Material Weight",
        "Centipawns per pawn‑unit of raw material difference.",
        0.0,
        300.0,
        100.0,
        1.0,
    ),
ParameterDef::new(
    PARAM_TERRITORY_WEIGHT,
    "Territory Weight",
    "Centipawns per unit of net square control.",
    0.0,
    50.0,
    12.0,
    0.5,
),
ParameterDef::new(
    PARAM_THREAT_WEIGHT,
    "Threat Weight",
    "How much extra a controlled square is worth when an enemy piece stands on it \
(as a fraction of that piece's value).",
                  0.0,
                  2.0,
                  0.4,
                  0.05,
),
ParameterDef::new(
    PARAM_CONTROL_SHARPNESS,
    "Control Sharpness",
    "Steepness of the pressure→control squash. Higher = faster saturation \
(stronger diminishing returns on over‑defence).",
                  0.1,
                  5.0,
                  1.5,
                  0.05,
),
ParameterDef::new(
    PARAM_BASE_INFLUENCE,
    "Base Influence",
    "Per‑attacker influence floor, independent of the attacker's value. \
Raising this makes attacker *count* matter more relative to attacker *cheapness*.",
0.0,
1.0,
0.3,
0.02,
),
ParameterDef::new(
    PARAM_ROYAL_FACTOR,
    "Royal Trade‑Value Factor",
    "A royal piece's effective trade‑value (for influence purposes only) is the \
strongest ordinary piece × this. Higher = royals exert less square control.",
1.0,
10.0,
3.0,
0.1,
),
ParameterDef::new(
    PARAM_TEMPO_BONUS,
    "Tempo Bonus",
    "Flat centipawn bonus for the side to move.",
    0.0,
    50.0,
    8.0,
    1.0,
),
ParameterDef::new(
    PARAM_PROMOTION_INFLUENCE,
    "Promotion Influence",
    "Latent promotion value: a promotable piece is credited a fraction of the board-COVERAGE \
(material) it will gain on promotion, discounted by distance to the promotion rank. This \
feeds the material & threat economy (the influence model's notion of value), NOT a \
positional square map. 0 = disabled.",
0.0, 2.0, 0.0, 0.01,
),
ParameterDef::new(
    PARAM_KING_ATTACK_WEIGHT,
    "King Attack Weight",
    "Centipawns per unit of net enemy pressure on a royal piece's zone (its own square plus the \
squares it can actually reach). Drives both king attack and king safety, and is amplified when \
the king is under fire and has few safe flight squares (confinement → mating gradient). Uses the \
live pressure maps and the king's real move pattern, so it works for any king geometry, any \
number of royal/royalty pieces, and vanishes automatically in king-less / extinction variants. \
0 = disabled (legacy behaviour).",
                  0.0, 60.0, 8.0, 1.0,
),
];

#[derive(Clone, Copy)]
struct CachedParams {
    material_weight: f32,
    territory_weight: f32,
    threat_weight: f32,
    sharpness: f32,
    base_influence: f32,
    royal_factor: f32,
    tempo: f32,
    promotion_weight: f32,
    king_attack_weight: f32,
}

impl CachedParams {
    fn from(p: &EngineParameters) -> Self {
        Self {
            material_weight: p.get_or_default(PARAM_MATERIAL_WEIGHT, 100.0) as f32,
            territory_weight: p.get_or_default(PARAM_TERRITORY_WEIGHT, 12.0) as f32,
            threat_weight: p.get_or_default(PARAM_THREAT_WEIGHT, 0.4) as f32,
            sharpness: p.get_or_default(PARAM_CONTROL_SHARPNESS, 1.5) as f32,
            base_influence: p.get_or_default(PARAM_BASE_INFLUENCE, 0.3) as f32,
            royal_factor: p.get_or_default(PARAM_ROYAL_FACTOR, 3.0) as f32,
            tempo: p.get_or_default(PARAM_TEMPO_BONUS, 8.0) as f32,
            promotion_weight: p.get_or_default(PARAM_PROMOTION_INFLUENCE, 0.0) as f32,
            king_attack_weight: p.get_or_default(PARAM_KING_ATTACK_WEIGHT, 8.0) as f32,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BlockCheck {
    row: u8,
    col: u8,
    perm_bits: u8,
}

#[derive(Clone)]
struct AttackRay {
    dest_flat: u16,
    requires_unmoved: bool,
    blockers: SmallVec<[BlockCheck; 6]>,
}

fn extract_blockers(mwp: &MoveWithPath) -> SmallVec<[BlockCheck; 6]> {
    let mut out: SmallVec<[BlockCheck; 6]> = SmallVec::new();
    let pattern = &*mwp.rule;
    let idxs = &mwp.path.step_indices;
    let steps = &mwp.path.steps;
    let n = idxs.len();
    if n == 0 {
        return out;
    }
    let mut rep_count = [0u32; 32];
    for i in 0..n {
        let step_idx = idxs[i] as usize;
        if step_idx >= pattern.steps.len() || step_idx >= 32 {
            continue;
        }
        let step = &pattern.steps[step_idx];
        rep_count[step_idx] += 1;
        if i + 1 == n {
            break;
        }
        let pos = steps[i + 1];
        let next_idx = idxs[i + 1] as usize;
        let continuing = next_idx == step_idx;
        let at_rep_boundary =
        continuing && step.length > 0 && (rep_count[step_idx] as usize) % step.length == 0;
        let (pe, px, pf) = if at_rep_boundary {
            (
                step.repetition_permissions.can_pass_empty,
             step.repetition_permissions.can_pass_enemy,
             step.repetition_permissions.can_pass_friendly,
            )
        } else {
            (
                step.permissions.can_pass_empty,
             step.permissions.can_pass_enemy,
             step.permissions.can_pass_friendly,
            )
        };
        if !(pe && px && pf) {
            let bits = (pe as u8) | ((px as u8) << 1) | ((pf as u8) << 2);
            out.push(BlockCheck {
                row: pos.0,
                col: pos.1,
                perm_bits: bits,
            });
        }
    }
    out
}

#[inline(always)]
fn ray_is_clear(ray: &AttackRay, board: &Board, mover: PieceColor) -> bool {
    for bc in &ray.blockers {
        let bits = bc.perm_bits;
        let ok = match board.get_piece((bc.row as usize, bc.col as usize)) {
            None => (bits & 0b001) != 0,
            Some(p) if p.color != mover => (bits & 0b010) != 0,
            Some(_) => (bits & 0b100) != 0,
        };
        if !ok {
            return false;
        }
    }
    true
}

// ═══════════════════════════════════════════════════════════════════════
//  POSITION ANALYSIS — terminal visualisation of what the engine "sees".
//  Everything below is additive; the hot‑path evaluator is untouched.
// ═══════════════════════════════════════════════════════════════════════

/// Flip to `false` if your terminal can't render ANSI 256‑colour escapes.
const ANALYSIS_USE_ANSI: bool = true;
/// The raw W/B pressure grid is useful but verbose; flip off if it's noise.
const ANALYSIS_SHOW_PRESSURE_GRID: bool = true;

struct PieceFootprint {
    pos: Position,
    color: PieceColor,
    piece_type: usize,
    squares_attacked: u32,
    influence_projected: f32,
}

/// Everything `evaluate_position` computes, but kept instead of discarded.
struct EvalBreakdown {
    rows: usize,
    cols: usize,
    // per‑square (flat = r*cols+c)
    pressure_w: Vec<f32>,
    pressure_b: Vec<f32>,
    control: Vec<f32>,
    threat: Vec<f32>,
    // per‑type
    material_value: Vec<f32>,
    attack_mobility: Vec<f32>,
    influence: Vec<f32>,
    is_royal: Vec<bool>,
    max_material_value: f32,
    // per‑piece on board
    footprints: Vec<PieceFootprint>,
    // scalars
    material_w: f32,
    material_b: f32,
    territory_sum: f32,
    score_white_pov: f32,
    score_stm: i32,
    params: CachedParams,
    white_king_danger: f32,   // ← add
    black_king_danger: f32,   // ← add
}

// ── tiny rendering helpers ───────────────────────────────────────────────

fn file_label(col: usize) -> String {
    if col < 26 {
        ((b'a' + col as u8) as char).to_string()
    } else {
        col.to_string()
    }
}

fn square_name(r: usize, c: usize, rows: usize) -> String {
    format!("{}{}", file_label(c), rows - r)
}

/// Map control ∈ [−1,1] to an ANSI‑256 background colour:
/// greys near 0, greens for White control, reds for Black control.
fn control_bg_256(c: f32) -> u8 {
    let a = c.clamp(-1.0, 1.0).abs();
    let lv = if a < 0.08 {
        0
    } else if a < 0.25 {
        1
    } else if a < 0.45 {
        2
    } else if a < 0.70 {
        3
    } else {
        4
    };
    if c >= 0.0 {
        [237, 22, 28, 34, 40][lv]
    }
    // grey → green
    else {
        [237, 52, 88, 124, 160][lv]
    } // grey → red
}

fn paint(txt: &str, bg: u8) -> String {
    if ANALYSIS_USE_ANSI && bg != 0 {
        format!("\x1b[48;5;{bg}m\x1b[38;5;15m{txt}\x1b[0m")
    } else {
        txt.to_string()
    }
}

/// Generic bordered‑grid printer. `cell(r,c)` returns the cell text (should be
/// exactly `cell_w` display columns) and an ANSI‑256 bg colour (0 = none).
fn print_board_grid(
    rows: usize,
    cols: usize,
    cell_w: usize,
    mut cell: impl FnMut(usize, usize) -> (String, u8),
) {
    // column header
    print!("     ");
    for c in 0..cols {
        print!("{:^w$} ", file_label(c), w = cell_w);
    }
    println!();
    // top border
    print!("   ┌");
    for c in 0..cols {
        print!(
            "{}{}",
            "─".repeat(cell_w),
               if c + 1 < cols { "┬" } else { "┐" }
        );
    }
    println!();
    // body
    for r in 0..rows {
        print!("{:>3} │", rows - r);
        for c in 0..cols {
            let (txt, bg) = cell(r, c);
            print!("{}│", paint(&format!("{:^w$}", txt, w = cell_w), bg));
        }
        println!(" {}", rows - r);
        if r + 1 < rows {
            print!("   ├");
            for c in 0..cols {
                print!(
                    "{}{}",
                    "─".repeat(cell_w),
                       if c + 1 < cols { "┼" } else { "┤" }
                );
            }
            println!();
        }
    }
    // bottom border
    print!("   └");
    for c in 0..cols {
        print!(
            "{}{}",
            "─".repeat(cell_w),
               if c + 1 < cols { "┴" } else { "┘" }
        );
    }
    println!();
    // column footer
    print!("     ");
    for c in 0..cols {
        print!("{:^w$} ", file_label(c), w = cell_w);
    }
    println!();
}

struct EvalData {
    board_size: (usize, usize),
    cols: usize,
    flat_size: usize,
    num_pieces: usize,
    attack_mobility: Vec<f32>,
    material_value: Vec<f32>,
    is_royal: Vec<bool>,
    max_material_value: f32,
    attack_rays: Vec<Vec<AttackRay>>,
    pressure: [Vec<f32>; 2],
    influence: Vec<f32>,
    promo_gain: Vec<f32>,        // per pt: coverage(material) gained on promotion (>=0)
    promo_proximity: Vec<f32>,   // (pt*2+ci)*flat_size + flat : discount^dist in [0,1]
    promo_any: bool,
}

impl EvalData {
    fn build(
        board: &Board,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let board_size = board.size();
        let (rows, cols) = board_size;
        let flat_size = rows * cols;
        let num_pieces = config_manager.piece_order.len();

        let initial_pieces = board.count_pieces() as f32;
        let initial_density = if flat_size > 0 {
            initial_pieces / flat_size as f32
        } else {
            0.0
        };
        let empty_prob = (1.0 - 0.6 * initial_density).clamp(0.30, 0.95) as f64;

        let mut attack_mobility = vec![0.0_f32; num_pieces];
        let mut is_royal = vec![false; num_pieces];
        let mut attack_rays: Vec<Vec<AttackRay>> = (0..num_pieces * 2 * flat_size)
        .map(|_| Vec::new())
        .collect();

        for pt in 0..num_pieces {
            is_royal[pt] = config_manager
            .get_piece_by_index(pt)
            .map_or(false, |c| c.properties.is_royal);
            let mut mobility_sum = 0.0_f64;
            let mut mobility_cnt = 0u32;

            for color_idx in 0..2usize {
                let color = if color_idx == 0 {
                    PieceColor::White
                } else {
                    PieceColor::Black
                };
                for r in 0..rows {
                    for c in 0..cols {
                        let from = (r, c);
                        let from_flat = r * cols + c;

                        let theoretical = move_generator
                        .generate_theoretical_moves_for_pst(from, pt, color, board_size, 0);
                        let mut rays: Vec<AttackRay> = Vec::new();

                        for mwp in &theoretical {
                            if !mwp.rule.can_land_enemy {
                                continue;
                            }
                            let dest = mwp.destination;
                            if dest.0 >= rows || dest.1 >= cols {
                                continue;
                            }
                            let blockers = extract_blockers(mwp);
                            rays.push(AttackRay {
                                dest_flat: (dest.0 * cols + dest.1) as u16,
                                      requires_unmoved: mwp.rule.requires_unmoved,
                                      blockers,
                            });
                        }

                        rays.sort_by(|a, b| {
                            (a.dest_flat, a.requires_unmoved, a.blockers.len()).cmp(&(
                                b.dest_flat,
                                b.requires_unmoved,
                                b.blockers.len(),
                            ))
                        });
                        rays.dedup_by(|later, earlier| {
                            later.dest_flat == earlier.dest_flat
                            && later.requires_unmoved == earlier.requires_unmoved
                        });

                        let mut mob_here = 0.0_f64;
                        for ray in &rays {
                            if ray.requires_unmoved {
                                continue;
                            }
                            mob_here += empty_prob.powi(ray.blockers.len() as i32);
                        }
                        mobility_sum += mob_here;
                        mobility_cnt += 1;

                        let slot = (pt * 2 + color_idx) * flat_size + from_flat;
                        attack_rays[slot] = rays;
                    }
                }
            }
            attack_mobility[pt] = if mobility_cnt > 0 {
                (mobility_sum / mobility_cnt as f64) as f32
            } else {
                0.0
            };
        }

        let min_mob = attack_mobility
        .iter()
        .copied()
        .filter(|&v| v > 0.0)
        .fold(f32::INFINITY, f32::min);
        let min_mob = if min_mob.is_finite() {
            min_mob.max(0.001)
        } else {
            1.0
        };

        let material_value: Vec<f32> = attack_mobility.iter().map(|&m| m / min_mob).collect();

        let max_material_value = material_value
        .iter()
        .zip(is_royal.iter())
        .filter(|t| !*t.1)
        .map(|t| *t.0)
        .fold(0.0_f32, f32::max)
        .max(1.0);

        // --- Promotion Logic Added Here ---
        const PROMO_DISCOUNT: f32 = 0.6; // per-step coverage discount toward promotion rank
        let mut promo_gain = vec![0.0_f32; num_pieces];
        let mut promo_proximity = vec![0.0_f32; num_pieces * 2 * flat_size];
        let mut promo_any = false;

        for pt in 0..num_pieces {
            let can_promote = config_manager
            .get_piece_by_index(pt)
            .map_or(false, |c| c.properties.can_promote);

            if !can_promote { continue; }

            let targets = PromotionManager::get_promotion_targets(pt, config_manager);
            let mut best = 0.0_f32;
            for &t in &targets {
                if t < num_pieces { best = best.max(material_value[t]); }
            }

            let gain = (best - material_value[pt]).max(0.0);
            if gain <= 0.0 { continue; }

            promo_gain[pt] = gain;
            promo_any = true;

            // color 0 (White) promotes toward row 0; color 1 toward the last row.
            for ci in 0..2usize {
                for r in 0..rows {
                    let dist = if ci == 0 { r } else { rows - 1 - r };
                    let prox = PROMO_DISCOUNT.powi(dist as i32);
                    for c in 0..cols {
                        promo_proximity[(pt * 2 + ci) * flat_size + r * cols + c] = prox;
                    }
                }
            }
        }

        // Return Struct
        EvalData {
            board_size,
            cols,
            flat_size,
            num_pieces,
            attack_mobility,
            material_value,
            is_royal,
            max_material_value,
            attack_rays,
            pressure: [vec![0.0; flat_size], vec![0.0; flat_size]],
            influence: vec![0.0; num_pieces],
            promo_gain,
            promo_proximity,
            promo_any,
        }
    }

    #[inline(always)]
    fn rays_for(&self, pt: usize, color_idx: usize, from_flat: usize) -> &[AttackRay] {
        let slot = (pt * 2 + color_idx) * self.flat_size + from_flat;
        &self.attack_rays[slot]
    }

    /// Net enemy pressure on one royal piece's zone.
    ///
    /// Zone = the king's own square (check pressure) plus every square it
    /// can currently reach (its cleared attack rays). For each zone square
    /// we take `attacker_pressure - defender_pressure`, accumulating only
    /// the squares the attacker is winning. Squares the defender holds are
    /// counted as "safe flight squares".
    ///
    /// Confinement amplifier (Change B): when the king square itself is
    /// under fire and few flight squares remain, the danger is scaled up to
    /// 2× — this is the gradient that turns pressure into a mating attack.
    ///
    /// Generic over king geometry (rays encode the real move pattern) and
    /// board size (zone is the king's actual reach, not a fixed radius).
    fn royal_zone_danger(&self, board: &Board, pos: Position, color: PieceColor) -> f32 {
        let Some(piece) = board.get_piece(pos) else { return 0.0 };
        let pt = piece.piece_type;
        if pt >= self.num_pieces {
            return 0.0;
        }
        let ai = color.opposite().index(); // attacker pressure index
        let di = color.index(); // defender pressure index
        let from_flat = pos.0 * self.cols + pos.1;

        // Pressure landing on the king's own square ≈ "is in check / attacked".
        let check_pressure = unsafe { *self.pressure[ai].get_unchecked(from_flat) };
        let mut danger = check_pressure;

        let slot = (pt * 2 + di) * self.flat_size + from_flat;
        let mut ring = 0u32;
        let mut safe = 0u32;
        for ray in &self.attack_rays[slot] {
            if ray.requires_unmoved && piece.move_count > 0 {
                continue;
            }
            if !ray_is_clear(ray, board, color) {
                continue;
            }
            let z = ray.dest_flat as usize;
            ring += 1;
            let net = unsafe {
                *self.pressure[ai].get_unchecked(z) - *self.pressure[di].get_unchecked(z)
            };
            if net > 0.0 {
                danger += net;
            } else {
                safe += 1;
            }
        }

        if check_pressure > 0.0 && ring > 0 {
            // confine ∈ [0,1]: fraction of flight squares the attacker controls.
            let confine = (ring - safe) as f32 / ring as f32;
            danger *= 1.0 + confine;
        }
        danger
    }

    /// Total danger to `color`'s royalty. Pulls the protected-piece set from
    /// the board's live tracking (Change C): all 'R' royal pieces always, plus
    /// the single remaining 'r' royalty piece when exactly one is left —
    /// mirroring GameState::is_in_check_fast. Returns 0 when `color` has no
    /// protected pieces (king-less / extinction variants).
    fn royal_danger(&self, board: &Board, color: PieceColor) -> f32 {
        let mut total = 0.0_f32;
        for &p in board.get_royal_positions(color) {
            total += self.royal_zone_danger(board, p, color);
        }
        let royalty = board.get_royalty_positions(color);
        if royalty.len() == 1 {
            total += self.royal_zone_danger(board, royalty[0], color);
        }
        total
    }
}

pub struct InfluenceEngine {
    eval_data: RefCell<Option<EvalData>>,
    transposition_table: HashMap<u64, TTEntry>,
    parameters: EngineParameters,
    cached_params: CachedParams,
    needs_reinit: bool,
}

impl InfluenceEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        let defs = combined_params(INFLUENCE_PARAMETERS, &MERGED);
        let parameters = EngineParameters::from_defaults(defs);
        let cached_params = CachedParams::from(&parameters);
        Self {
            eval_data: RefCell::new(None),
            transposition_table: HashMap::new(),
            parameters,
            cached_params,
            needs_reinit: true,
        }
    }

    fn initialize(
        &mut self,
        board: &Board,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let needs_rebuild = {
            let d = self.eval_data.borrow();
            self.needs_reinit
            || d.is_none()
            || d.as_ref().map_or(true, |d| {
                d.board_size != board.size() || d.num_pieces != config_manager.piece_order.len()
            })
        };
        if !needs_rebuild {
            return;
        }
        println!(
            "🔄 InfluenceEngine: building attack‑ray table for {}×{} board, {} piece types…",
            board.size().0,
                 board.size().1,
                 config_manager.piece_order.len()
        );
        let data = EvalData::build(board, move_generator, config_manager);
        for pt in 0..data.num_pieces {
            if let Some(cfg) = config_manager.get_piece_by_index(pt) {
                println!(
                    "   • {:<16}  mobility≈{:>6.2}   value≈{:>5.2} pawns{}",
                    cfg.display_name,
                    data.attack_mobility[pt],
                    data.material_value[pt],
                    if data.is_royal[pt] { "   (royal)" } else { "" },
                );
            }
        }
        *self.eval_data.borrow_mut() = Some(data);
        self.needs_reinit = false;
        println!("✅ InfluenceEngine ready.");
    }

    fn evaluate_position(&self, state: &GameState) -> i32 {
        let cp = self.cached_params;
        let mut guard = self.eval_data.borrow_mut();
        let data = match guard.as_mut() {
            Some(d) => d,
            None => return 0,
        };
        if data.board_size != state.board.size() {
            return 0;
        }
        let rows = data.board_size.0;
        let cols = data.cols;

        let promo_on = cp.promotion_weight > 0.0 && data.promo_any;

        for pt in 0..data.num_pieces {
            let trade_value = if data.is_royal[pt] {
                data.max_material_value * cp.royal_factor
            } else {
                data.material_value[pt]
            }
            .max(0.001);
            data.influence[pt] = cp.base_influence + 1.0 / trade_value;
        }

        data.pressure[0].fill(0.0);
        data.pressure[1].fill(0.0);

        let mut material_w = 0.0_f32;
        let mut material_b = 0.0_f32;
        for r in 0..rows {
            for c in 0..cols {
                let Some(piece) = state.board.get_piece((r, c)) else {
                    continue;
                };
                let pt = piece.piece_type;
                if pt >= data.num_pieces {
                    continue;
                }
                let ci = piece.color.index();
                let from_flat = r * cols + c;

                let mut mv = data.material_value[pt];
                if promo_on {
                    let g = data.promo_gain[pt];
                    if g > 0.0 {
                        let prox = unsafe {
                            *data.promo_proximity
                            .get_unchecked((pt * 2 + ci) * data.flat_size + from_flat)
                        };
                        mv += cp.promotion_weight * prox * g;
                    }
                }
                if ci == 0 {
                    material_w += mv;
                } else {
                    material_b += mv;
                }

                let infl = data.influence[pt];

                // 1. Immutable borrow of ONLY the attack_rays field
                let slot = (pt * 2 + ci) * data.flat_size + from_flat;
                let rays = &data.attack_rays[slot];

                for ray in rays {
                    if ray.requires_unmoved && piece.move_count > 0 {
                        continue;
                    }
                    if ray_is_clear(ray, &state.board, piece.color) {
                        // 2. Mutable borrow of ONLY the pressure field.
                        // The compiler knows this doesn't overlap with attack_rays!
                        unsafe {
                            *data.pressure[ci].get_unchecked_mut(ray.dest_flat as usize) += infl;
                        }
                    }
                }
            }
        }

        let mut territory = 0.0_f32;
        let sharpness = cp.sharpness;
        let tw = cp.threat_weight;
        for r in 0..rows {
            let row_base = r * cols;
            for c in 0..cols {
                let flat = row_base + c;
                let wp = unsafe { *data.pressure[0].get_unchecked(flat) };
                let bp = unsafe { *data.pressure[1].get_unchecked(flat) };
                let raw = sharpness * (wp - bp);
                let control = raw / (1.0 + raw.abs());

                let threat = match state.board.get_piece((r, c)) {
                    Some(occ) if occ.piece_type < data.num_pieces => {
                        let v = data.material_value[occ.piece_type];
                        match occ.color {
                            PieceColor::Black => control.max(0.0) * tw * v,
                            PieceColor::White => control.min(0.0) * tw * v,
                        }
                    }
                    _ => 0.0,
                };
                territory += control + threat;
            }
        }

        let mut score_w =
        cp.material_weight * (material_w - material_b) + cp.territory_weight * territory;

        // King safety / aggression (net enemy pressure on royal zones).
        // White is rewarded for Black-king danger and penalised for its own.
        if cp.king_attack_weight != 0.0 {
            let white_danger = data.royal_danger(&state.board, PieceColor::White);
            let black_danger = data.royal_danger(&state.board, PieceColor::Black);
            score_w += cp.king_attack_weight * (black_danger - white_danger);
        }

        let stm_sign = match state.current_turn {
            PieceColor::White => 1.0_f32,
            PieceColor::Black => -1.0_f32,
        };

        (stm_sign * score_w + cp.tempo).round() as i32
    }

    fn evaluate_split_position(&self, state: &GameState) -> Option<(f32, f32)> {
        let cp = self.cached_params;
        let mut guard = self.eval_data.borrow_mut();
        let data = guard.as_mut()?;
        if data.board_size != state.board.size() { return None; }
        let rows = data.board_size.0;
        let cols = data.cols;
        let promo_on = cp.promotion_weight > 0.0 && data.promo_any;

        for pt in 0..data.num_pieces {
            let trade = if data.is_royal[pt] {
                data.max_material_value * cp.royal_factor
            } else { data.material_value[pt] }.max(0.001);
            data.influence[pt] = cp.base_influence + 1.0 / trade;
        }
        data.pressure[0].fill(0.0);
        data.pressure[1].fill(0.0);

        let mut material_w = 0.0_f32;
        let mut material_b = 0.0_f32;
        for r in 0..rows {
            for c in 0..cols {
                let Some(piece) = state.board.get_piece((r, c)) else { continue };
                let pt = piece.piece_type;
                if pt >= data.num_pieces { continue; }
                let ci = piece.color.index();
                let from_flat = r * cols + c;

                let mut mv = data.material_value[pt];
                if promo_on {
                    let g = data.promo_gain[pt];
                    if g > 0.0 {
                        let prox = unsafe {
                            *data.promo_proximity
                            .get_unchecked((pt * 2 + ci) * data.flat_size + from_flat)
                        };
                        mv += cp.promotion_weight * prox * g;
                    }
                }
                if ci == 0 { material_w += mv; } else { material_b += mv; }

                let infl = data.influence[pt];
                let slot = (pt * 2 + ci) * data.flat_size + from_flat;
                for ray in &data.attack_rays[slot] {
                    if ray.requires_unmoved && piece.move_count > 0 { continue; }
                    if ray_is_clear(ray, &state.board, piece.color) {
                        unsafe { *data.pressure[ci].get_unchecked_mut(ray.dest_flat as usize) += infl; }
                    }
                }
            }
        }

        let mut white_mass = 0.0_f32;
        let mut black_mass = 0.0_f32;
        let (sharpness, tw) = (cp.sharpness, cp.threat_weight);
        for r in 0..rows {
            let rb = r * cols;
            for c in 0..cols {
                let flat = rb + c;
                let wp = unsafe { *data.pressure[0].get_unchecked(flat) };
                let bp = unsafe { *data.pressure[1].get_unchecked(flat) };
                let raw = sharpness * (wp - bp);
                let control = raw / (1.0 + raw.abs());
                if control > 0.0 { white_mass += control; } else { black_mass += -control; }

                if let Some(occ) = state.board.get_piece((r, c)) {
                    if occ.piece_type < data.num_pieces {
                        let v = data.material_value[occ.piece_type];
                        match occ.color {
                            PieceColor::Black => white_mass += control.max(0.0) * tw * v,
                            PieceColor::White => black_mass += -(control.min(0.0) * tw * v),
                        }
                    }
                }
            }
        }

        let mut white_abs = cp.material_weight * material_w + cp.territory_weight * white_mass;
        let mut black_abs = cp.material_weight * material_b + cp.territory_weight * black_mass;

        if cp.king_attack_weight != 0.0 {
            // Each side's "goodness" gains from the danger it inflicts on the
            // enemy king — keeps the split consistent with evaluate_position.
            let white_danger = data.royal_danger(&state.board, PieceColor::White);
            let black_danger = data.royal_danger(&state.board, PieceColor::Black);
            white_abs += cp.king_attack_weight * black_danger;
            black_abs += cp.king_attack_weight * white_danger;
        }

        Some(match state.current_turn {
            PieceColor::White => (white_abs, black_abs),
             PieceColor::Black => (black_abs, white_abs),
        })
    }

    // ─────────────────────────────────────────────────────────────────────
    //  Analysis‑mode evaluation: identical maths to evaluate_position(), but
    //  every intermediate is kept so it can be printed / returned.
    // ─────────────────────────────────────────────────────────────────────
    fn evaluate_with_breakdown(&self, state: &GameState) -> Option<EvalBreakdown> {
        let cp = self.cached_params;
        let mut guard = self.eval_data.borrow_mut();
        let data = guard.as_mut()?;
        if data.board_size != state.board.size() {
            return None;
        }
        let rows = data.board_size.0;
        let cols = data.cols;
        let flat_size = data.flat_size;

        // 0. influence weights (param‑dependent, cheap)
        for pt in 0..data.num_pieces {
            let tv = if data.is_royal[pt] {
                data.max_material_value * cp.royal_factor
            } else {
                data.material_value[pt]
            }
            .max(0.001);
            data.influence[pt] = cp.base_influence + 1.0 / tv;
        }

        // 1. clear scratch
        data.pressure[0].fill(0.0);
        data.pressure[1].fill(0.0);

        // 2. project every piece's attack rays; also record per‑piece footprint
        let mut material_w = 0.0_f32;
        let mut material_b = 0.0_f32;
        let mut footprints: Vec<PieceFootprint> = Vec::new();

        for r in 0..rows {
            for c in 0..cols {
                let Some(piece) = state.board.get_piece((r, c)) else {
                    continue;
                };
                let pt = piece.piece_type;
                if pt >= data.num_pieces {
                    continue;
                }
                let ci = piece.color.index();
                let from_flat = r * cols + c;

                let mv = data.material_value[pt];
                if ci == 0 {
                    material_w += mv;
                } else {
                    material_b += mv;
                }

                let infl = data.influence[pt];
                let slot = (pt * 2 + ci) * data.flat_size + from_flat;

                let mut attacked = 0u32;
                for ray in &data.attack_rays[slot] {
                    if ray.requires_unmoved && piece.move_count > 0 {
                        continue;
                    }
                    if ray_is_clear(ray, &state.board, piece.color) {
                        data.pressure[ci][ray.dest_flat as usize] += infl;
                        attacked += 1;
                    }
                }
                footprints.push(PieceFootprint {
                    pos: (r, c),
                                color: piece.color,
                                piece_type: pt,
                                squares_attacked: attacked,
                                influence_projected: attacked as f32 * infl,
                });
            }
        }

        // 3. per‑square control + threat
        let mut control = vec![0.0_f32; flat_size];
        let mut threat = vec![0.0_f32; flat_size];
        let mut territory_sum = 0.0_f32;

        for r in 0..rows {
            for c in 0..cols {
                let flat = r * cols + c;
                let wp = data.pressure[0][flat];
                let bp = data.pressure[1][flat];
                let raw = cp.sharpness * (wp - bp);
                let ctl = raw / (1.0 + raw.abs());
                control[flat] = ctl;

                let th = match state.board.get_piece((r, c)) {
                    Some(occ) if occ.piece_type < data.num_pieces => {
                        let v = data.material_value[occ.piece_type];
                        match occ.color {
                            PieceColor::Black => ctl.max(0.0) * cp.threat_weight * v,
                            PieceColor::White => ctl.min(0.0) * cp.threat_weight * v,
                        }
                    }
                    _ => 0.0,
                };
                threat[flat] = th;
                territory_sum += ctl + th;
            }
        }

        let (white_king_danger, black_king_danger) = if cp.king_attack_weight != 0.0 {
            (
                data.royal_danger(&state.board, PieceColor::White),
             data.royal_danger(&state.board, PieceColor::Black),
            )
        } else {
            (0.0, 0.0)
        };
        let king_cp = cp.king_attack_weight * (black_king_danger - white_king_danger);

        let score_white_pov = cp.material_weight * (material_w - material_b)
        + cp.territory_weight * territory_sum
        + king_cp;

        let stm_sign = if state.current_turn == PieceColor::White {
            1.0
        } else {
            -1.0
        };
        let score_stm = (stm_sign * score_white_pov + cp.tempo).round() as i32;

        Some(EvalBreakdown {
            rows,
            cols,
            pressure_w: data.pressure[0].clone(),
             pressure_b: data.pressure[1].clone(),
             control,
             threat,
             material_value: data.material_value.clone(),
             attack_mobility: data.attack_mobility.clone(),
             influence: data.influence.clone(),
             is_royal: data.is_royal.clone(),
             max_material_value: data.max_material_value,
             footprints,
             material_w,
             material_b,
             territory_sum,
             score_white_pov,
             score_stm,
             params: cp,
             white_king_danger,   // ← add
             black_king_danger,   // ← add
        })
    }

    // ─────────────────────────────────────────────────────────────────────
    //  Terminal renderer
    // ─────────────────────────────────────────────────────────────────────
    fn print_terminal_analysis(
        &self,
        bd: &EvalBreakdown,
        state: &GameState,
        cm: &PieceConfigManager,
    ) {
        let cp = bd.params;
        let (rows, cols) = (bd.rows, bd.cols);
        let hr = "═".repeat(72);

        let stm = match state.current_turn {
            PieceColor::White => "White",
            PieceColor::Black => "Black",
        };
        let opp = match state.current_turn {
            PieceColor::White => "Black",
            PieceColor::Black => "White",
        };
        let verdict = if bd.score_stm > 15 {
            format!("{stm} is better")
        } else if bd.score_stm < -15 {
            format!("{opp} is better")
        } else {
            "roughly equal".to_string()
        };

        println!("\n{hr}");
        println!(" INFLUENCE ENGINE — POSITION ANALYSIS");
        println!("{hr}");
        println!(
            " Side to move: {stm:<6}   Static eval (STM POV): {:+} cp   ({verdict})",
                 bd.score_stm
        );

        // ── score breakdown ─────────────────────────────────────────────
        let mat_diff = bd.material_w - bd.material_b;
        let mat_cp = cp.material_weight * mat_diff;
        let terr_cp = cp.territory_weight * bd.territory_sum;

        println!("\n── Score breakdown (White's point of view) ───────────────────────────");
        println!(
            "  {:<14}{:>9}{:>9}{:>10}{:>11}{:>11}",
            "", "White", "Black", "net", "× weight", "= cp"
        );
        println!(
            "  {:<14}{:>9.2}{:>9.2}{:>+10.2}{:>11.1}{:>+11.0}",
            "Material", bd.material_w, bd.material_b, mat_diff, cp.material_weight, mat_cp
        );
        println!(
            "  {:<14}{:>9}{:>9}{:>+10.2}{:>11.1}{:>+11.0}",
            "Territory", "", "", bd.territory_sum, cp.territory_weight, terr_cp
        );
        if cp.king_attack_weight != 0.0 {
            let king_net = bd.black_king_danger - bd.white_king_danger;
            println!(
                "  {:<14}{:>9.2}{:>9.2}{:>+10.2}{:>11.1}{:>+11.0}",
                "King safety",
                bd.black_king_danger, // White's credit = danger to Black king
                bd.white_king_danger, // Black's credit = danger to White king
                king_net,
                cp.king_attack_weight,
                cp.king_attack_weight * king_net
            );
        }
        println!("  {:>64}", "──────────");
        println!(
            "  {:<53}{:>+11.0}",
            "Subtotal (White POV)", bd.score_white_pov
        );
        println!(
            "  {:<53}{:>+11.0}",
            format!("Tempo (to move: {stm})"),
                cp.tempo
        );
        println!("  {:>64}", "══════════");
        println!("  {:<53}{:>+11}", "TOTAL (STM POV)", bd.score_stm);

        // ── derived per‑type values ─────────────────────────────────────
        println!("\n── Derived piece values & influence weights ──────────────────────────");
        println!(
            "  {:<20}{:>10}{:>14}{:>16}",
            "type", "mobility", "value(pawns)", "influence/atk"
        );
        for pt in 0..bd.material_value.len() {
            let name = cm
            .get_piece_by_index(pt)
            .map(|c| c.display_name.clone())
            .unwrap_or_else(|| format!("#{pt}"));
            let royal = bd.is_royal[pt];
            let extra = if royal {
                format!(
                    "   [royal; trade‑val ≈ {:.1}]",
                    bd.max_material_value * cp.royal_factor
                )
            } else {
                String::new()
            };
            println!(
                "  {:<20}{:>10.2}{:>14.2}{:>16.3}{}",
                if royal {
                    format!("{name} (royal)")
                } else {
                    name
                },
                bd.attack_mobility[pt],
                bd.material_value[pt],
                bd.influence[pt],
                extra
            );
        }

        // ── control heat‑map ────────────────────────────────────────────
        println!("\n── Control map  (−1 … +1;  + = White controls, − = Black controls) ────");
        println!("   cell = [occupant] [control];  UPPER=White, lower=Black, · = empty");
        if ANALYSIS_USE_ANSI {
            print!("   shade:");
            for &v in &[-0.9_f32, -0.5, -0.2, 0.0, 0.2, 0.5, 0.9] {
                print!(" {}", paint(&format!(" {:+.1} ", v), control_bg_256(v)));
            }
            println!();
        }
        print_board_grid(rows, cols, 7, |r, c| {
            let flat = r * cols + c;
            let ctl = bd.control[flat];
            let ch = state
            .board
            .get_piece((r, c))
            .map(|p| p.to_char(cm))
            .unwrap_or('·');
            (format!("{} {:+.2}", ch, ctl), control_bg_256(ctl))
        });

        // ── raw pressure grid ───────────────────────────────────────────
        if ANALYSIS_SHOW_PRESSURE_GRID {
            println!("\n── Raw pressure per square  (White / Black, before squash) ────────────");
            print_board_grid(rows, cols, 9, |r, c| {
                let flat = r * cols + c;
                (
                    format!("{:>4.1}/{:<4.1}", bd.pressure_w[flat], bd.pressure_b[flat]),
                        0,
                )
            });
        }

        // ── territory summary ───────────────────────────────────────────
        let (mut w_sq, mut b_sq, mut n_sq) = (0u32, 0u32, 0u32);
        let (mut w_sum, mut b_sum) = (0.0_f32, 0.0_f32);
        for &ctl in &bd.control {
            if ctl > 0.10 {
                w_sq += 1;
                w_sum += ctl;
            } else if ctl < -0.10 {
                b_sq += 1;
                b_sum += ctl;
            } else {
                n_sq += 1;
            }
        }
        println!("\n── Territory summary ──────────────────────────────────────────────────");
        println!(
            "  White‑held (> +0.10):  {:>3} squares   Σcontrol = {:+7.2}   ≈ {:+6.0} cp",
                 w_sq,
                 w_sum,
                 w_sum * cp.territory_weight
        );
        println!(
            "  Black‑held (< −0.10):  {:>3} squares   Σcontrol = {:+7.2}   ≈ {:+6.0} cp",
                 b_sq,
                 b_sum,
                 b_sum * cp.territory_weight
        );
        println!("  Contested / neutral :  {:>3} squares", n_sq);

        // ── threats (occupied squares with a non‑zero threat term) ──────
        let mut threats: Vec<(usize, usize, f32, f32, Piece)> = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let flat = r * cols + c;
                if bd.threat[flat].abs() > 1e-4 {
                    if let Some(p) = state.board.get_piece((r, c)) {
                        threats.push((r, c, bd.control[flat], bd.threat[flat], p));
                    }
                }
            }
        }
        if !threats.is_empty() {
            threats.sort_by(|a, b| {
                b.3.abs()
                .partial_cmp(&a.3.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
            });
            println!("\n── Pieces under pressure (threat‑term contributors) ───────────────────");
            println!(
                "  {:<5}{:<18}{:>9}{:>9}{:>14}",
                "sq", "piece", "control", "threat", "territory cp"
            );
            for (r, c, ctl, th, p) in threats.iter().take(12) {
                let side = if p.color == PieceColor::White {
                    "W"
                } else {
                    "B"
                };
                let nm = cm
                .get_piece_by_index(p.piece_type)
                .map(|c| c.display_name.clone())
                .unwrap_or_default();
                println!(
                    "  {:<5}{:<18}{:>+9.2}{:>+9.2}{:>+14.1}",
                    square_name(*r, *c, rows),
                         format!("{nm} ({side})"),
                             ctl,
                         th,
                         th * cp.territory_weight
                );
            }
            if threats.len() > 12 {
                println!("  … ({} more)", threats.len() - 12);
            }
        }

        // ── per‑piece attack footprint ──────────────────────────────────
        let mut fps: Vec<&PieceFootprint> = bd.footprints.iter().collect();
        fps.sort_by(|a, b| {
            b.influence_projected
            .partial_cmp(&a.influence_projected)
            .unwrap_or(std::cmp::Ordering::Equal)
        });
        println!("\n── Per‑piece attack footprint (sorted by influence projected) ─────────");
        println!(
            "  {:<5}{:<18}{:>9}{:>12}{:>16}",
            "sq", "piece", "attacks", "infl‑each", "infl‑projected"
        );
        for fp in fps.iter().take(24) {
            let side = if fp.color == PieceColor::White {
                "W"
            } else {
                "B"
            };
            let nm = cm
            .get_piece_by_index(fp.piece_type)
            .map(|c| c.display_name.clone())
            .unwrap_or_default();
            println!(
                "  {:<5}{:<18}{:>9}{:>12.3}{:>16.2}",
                square_name(fp.pos.0, fp.pos.1, rows),
                     format!("{nm} ({side})"),
                         fp.squares_attacked,
                     bd.influence[fp.piece_type],
                     fp.influence_projected
            );
        }
        if fps.len() > 24 {
            println!("  … ({} more)", fps.len() - 24);
        }

        println!("{hr}\n");
    }

    // ─────────────────────────────────────────────────────────────────────
    //  Populate the shared PositionAnalysis struct so the GUI gets data too.
    //  Only the fields that map naturally onto this engine are filled in;
    //  the rest are zero/empty. pst_analysis is None.
    // ─────────────────────────────────────────────────────────────────────
    fn build_position_analysis(
        &self,
        bd: &EvalBreakdown,
        state: &mut GameState,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
    ) -> analysis::PositionAnalysis {
        use analysis::*;
        let (rows, cols) = (bd.rows, bd.cols);

        // ── material ────────────────────────────────────────────────────
        let mut piece_counts: HashMap<usize, (u32, u32)> = HashMap::new();
        for r in 0..rows {
            for c in 0..cols {
                if let Some(p) = state.board.get_piece((r, c)) {
                    let e = piece_counts.entry(p.piece_type).or_insert((0, 0));
                    if p.color == PieceColor::White {
                        e.0 += 1;
                    } else {
                        e.1 += 1;
                    }
                }
            }
        }
        let piece_values: HashMap<usize, f64> = bd
        .material_value
        .iter()
        .enumerate()
        .map(|(pt, &v)| (pt, v as f64))
        .collect();

        let material = MaterialAnalysis {
            white_total: bd.material_w as f64,
            black_total: bd.material_b as f64,
            difference: (bd.material_w - bd.material_b) as f64,
            piece_counts,
            piece_values,
        };

        // ── mobility: count legal moves for both sides ──────────────────
        let stm_moves = state.get_legal_moves(mg, cm).len() as u32;
        let orig = state.current_turn;
        state.current_turn = orig.opposite();
        let opp_moves = state.get_legal_moves(mg, cm).len() as u32;
        state.current_turn = orig;
        let (wm, bm) = match orig {
            PieceColor::White => (stm_moves, opp_moves),
            PieceColor::Black => (opp_moves, stm_moves),
        };

        // threat totals from our per‑square threat map
        let (mut w_th, mut b_th) = (0.0_f64, 0.0_f64);
        for &t in &bd.threat {
            if t > 0.0 {
                w_th += t as f64;
            } else {
                b_th += (-t) as f64;
            }
        }

        let mobility = MobilityAnalysis {
            white_mobility: wm,
            black_mobility: bm,
            mobility_difference: wm as i32 - bm as i32,
            piece_mobility: HashMap::new(),
            value_to_mobility_ratio: 0.0,
            theoretical_mobility: HashMap::new(),
            threat_analysis: ThreatAnalysis {
                white_threats_value: w_th,
                black_threats_value: b_th,
                white_attackers_value: 0.0,
                black_attackers_value: 0.0,
                white_capture_mobility_percentage: 0.0,
                black_capture_mobility_percentage: 0.0,
                white_threat_balance: w_th - b_th,
                black_threat_balance: b_th - w_th,
            },
        };

        // ── density (basic) ──────────────────────────────────────────────
        let total_sq = (rows * cols) as f64;
        let total_pc = state.board.count_pieces() as f64;
        let density = DensityAnalysis {
            board_density: if total_sq > 0.0 {
                total_pc / total_sq
            } else {
                0.0
            },
            piece_density_ratio: 0.0,
            clustering: ClusteringAnalysis {
                average_piece_distance: 0.0,
                clustering_coefficient: 0.0,
                isolated_pieces: Vec::new(),
                dense_regions: Vec::new(),
            },
        };

        // ── statistics (basic) ───────────────────────────────────────────
        let weakest = bd
        .material_value
        .iter()
        .copied()
        .filter(|&v| v > 0.0)
        .fold(f32::INFINITY, f32::min);
        let weakest = if weakest.is_finite() {
            weakest as f64
        } else {
            1.0
        };

        let stats = StatisticalAnalysis {
            normalized_values: NormalizedValues {
                weakest_piece_value: weakest,
                white_total_normalized: bd.material_w as f64 / weakest,
                black_total_normalized: bd.material_b as f64 / weakest,
                piece_values_normalized: bd
                .material_value
                .iter()
                .enumerate()
                .map(|(pt, &v)| (pt, v as f64 / weakest))
                .collect(),
            },
            statistics: PositionStatistics {
                total_pieces: total_pc as u32,
                value_per_piece: if total_pc > 0.0 {
                    (bd.material_w + bd.material_b) as f64 / total_pc
                } else {
                    0.0
                },
                value_weighted_average: 0.0,
                value_variance: 0.0,
                piece_type_diversity: 0.0,
                position_complexity: 0.0,
            },
        };

        PositionAnalysis {
            material_values: material,
            pst_analysis: None,
            mobility_analysis: mobility,
            density_analysis: density,
            statistical_analysis: stats,
        }
    }
}

struct InfluenceEvaluator<'a> {
    engine: &'a InfluenceEngine,
}

impl<'a> EvaluatorTrait for InfluenceEvaluator<'a> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        self.engine.evaluate_position(state)
    }

    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        _pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        let d = self.engine.eval_data.borrow();
        let Some(d) = d.as_ref() else {
            return 100;
        };
        if piece.piece_type >= d.num_pieces {
            return 100;
        }
        (d.material_value[piece.piece_type] * 100.0).round() as i32
    }

    fn contempt(&self) -> i32 { 20 }

    fn evaluate_split(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> Option<(f32, f32)> {
        self.engine.evaluate_split_position(state)
    }
}

impl ChessEngine for InfluenceEngine {
    fn name(&self) -> &str {
        "Influence Engine (Raycast Territory)"
    }
    fn reset_cache(&mut self) {
        self.transposition_table.clear();
        *self.eval_data.borrow_mut() = None;
        self.needs_reinit = true;
    }
    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.initialize(&params.state.board, params.move_generator, params.config_manager);
        let evaluator = InfluenceEvaluator { engine: self };
        let mut search = if SearchConfig::use_new_search(&self.parameters) {
            Search::with_config(&evaluator, SearchConfig::from_params(&self.parameters))
        } else {
            Search::new(&evaluator)
        };
        search.set_transposition_table(self.transposition_table.clone());
        let depth = if params.depth > 0 { params.depth } else { 4 };
        let result = if let Some(time_limit) = params.time_limit {
            search.find_best_move_iterative(params.state, params.move_generator, params.config_manager, depth, time_limit)
        } else {
            search.find_best_move_with_depth(params.state, params.move_generator, params.config_manager, depth)
        };
        self.transposition_table = search.get_transposition_table();
        let (best_move, evaluation, depth_reached) = result?;
        let mate_in = if evaluation >= 999_000 {
            Some((999_999 - evaluation) / 2)
        } else if evaluation <= -999_000 {
            Some(-((-999_999 - evaluation) / 2))
        } else { None };
        Some(SearchResult { best_move, evaluation: Evaluation { score: evaluation, mate_in }, depth_reached })
    }
    fn stop(&mut self) {}
    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(INFLUENCE_PARAMETERS, &MERGED))
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
            self.cached_params = CachedParams::from(&self.parameters);
            self.transposition_table.clear();
        }
        changed
    }
    fn supports_analysis(&self) -> bool {
        true
    }

    fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<analysis::PositionAnalysis> {
        // Make sure the ray table exists for this board / piece set.
        self.initialize(&state.board, move_generator, config_manager);

        let bd = self.evaluate_with_breakdown(state)?;
        self.print_terminal_analysis(&bd, state, config_manager);
        Some(self.build_position_analysis(&bd, state, move_generator, config_manager))
    }
}

impl Default for InfluenceEngine {
    fn default() -> Self {
        Self::new()
    }
}
