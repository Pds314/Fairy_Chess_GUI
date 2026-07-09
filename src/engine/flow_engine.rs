// src/engine/flow_engine.rs
//
// Connectivity / pattern engine.
//
// The old version computed "shared target squares" as
//   Σ_{i<j} |reach_i ∩ reach_j|
// by building a `HashSet` per piece and intersecting every pair. That is
// identically  Σ_squares C(k,2)  where k is the number of friendly pieces
// that can reach the square — so one reach-count array replaces all the
// pairwise set work. It also used `generate_moves_with_details`, the
// re-tracing path, instead of the precomputed database.

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

pub const P_COORD: &str = "flow_coordination";
pub const P_PROTECT: &str = "flow_protection";
pub const P_ADJACENT: &str = "flow_adjacency";
pub const P_CENTER: &str = "flow_center_reach";
pub const P_PATTERN: &str = "flow_pattern";

pub static FLOW_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        P_COORD,
        "Coordination",
        "Per pair of friendly pieces that can reach the same square.",
        0.0, 10.0, 0.5, 0.05,
    ),
ParameterDef::new(
    P_PROTECT,
    "Mutual Protection",
    "Per friendly piece that can reach a friendly piece's square.",
    0.0, 20.0, 2.0, 0.1,
),
ParameterDef::new(
    P_ADJACENT,
    "Adjacency",
    "Per adjacent friendly pair (chain formation).",
                  0.0, 10.0, 1.0, 0.1,
),
ParameterDef::new(
    P_CENTER,
    "Centre Reach",
    "Per friendly reach of a centre square.",
    0.0, 5.0, 0.3, 0.05,
),
ParameterDef::new(
    P_PATTERN,
    "Pattern Score",
    "Weight of the spread/alignment term.",
    0.0, 20.0, 5.0, 0.25,
),
];

pub struct FlowEngine {
    parameters: EngineParameters,
    transposition_table: HashMap<u64, TTEntry>,
}

impl FlowEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            parameters: EngineParameters::from_defaults(combined_params(FLOW_PARAMETERS, &MERGED)),
            transposition_table: HashMap::new(),
        }
    }
    #[inline]
    fn p(&self, id: &str, d: f64) -> f64 {
        self.parameters.get_or_default(id, d)
    }
}

impl Default for FlowEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct FlowEvaluator<'a> {
    engine: &'a FlowEngine,
    /// Reused across leaves; sized once per `best_move`.
    reach: RefCell<Vec<u16>>,
}

impl FlowEvaluator<'_> {
    fn side_score(&self, state: &GameState, mg: &MoveGenerator, color: PieceColor) -> f64 {
        let b = &state.board;
        let (rows, cols) = b.size();
        let e = self.engine;

        let mut reach = self.reach.borrow_mut();
        if reach.len() != rows * cols {
            reach.resize(rows * cols, 0);
        }
        for x in reach.iter_mut() {
            *x = 0;
        }

        let mut mine: SmallVec<[Position; 32]> = SmallVec::new();
        for r in 0..rows {
            for c in 0..cols {
                let Some(p) = b.get_piece((r, c)) else { continue };
                if p.color != color {
                    continue;
                }
                mine.push((r, c));
                for mv in mg.generate_moves_with_database(b, (r, c), p.piece_type) {
                    let d = mv.destination;
                    reach[d.0 * cols + d.1] = reach[d.0 * cols + d.1].saturating_add(1);
                }
            }
        }

        // Σ_{i<j} |reach_i ∩ reach_j|  ==  Σ_sq C(k,2)
        let mut coord = 0.0f64;
        for &k in reach.iter() {
            let k = k as f64;
            coord += k * (k - 1.0) * 0.5;
        }

        // Directed mutual protection == Σ over friendly squares of reach[sq]
        let mut protect = 0.0f64;
        for &(r, c) in &mine {
            protect += reach[r * cols + c] as f64;
        }

        // Adjacency chains (each unordered pair counted once).
        let mut adj = 0.0f64;
        for &(r, c) in &mine {
            for dr in -1i32..=1 {
                for dc in -1i32..=1 {
                    if dr == 0 && dc == 0 {
                        continue;
                    }
                    let nr = r as i32 + dr;
                    let nc = c as i32 + dc;
                    if nr < 0 || nc < 0 || nr as usize >= rows || nc as usize >= cols {
                        continue;
                    }
                    if b.get_piece((nr as usize, nc as usize))
                        .map_or(false, |p| p.color == color)
                        {
                            adj += 0.5;
                        }
                }
            }
        }

        // Centre reach.
        let cr = rows / 2;
        let cc = cols / 2;
        let mut centre = 0.0f64;
        for dr in 0..=1usize {
            for dc in 0..=1usize {
                let r = cr.saturating_sub(dr);
                let c = cc.saturating_sub(dc);
                centre += reach[r * cols + c] as f64;
            }
        }

        // Spread / alignment.
        let mut pattern = 0.0f64;
        if mine.len() >= 2 {
            let n = mine.len() as f64;
            let ar = mine.iter().map(|p| p.0 as f64).sum::<f64>() / n;
            let ac = mine.iter().map(|p| p.1 as f64).sum::<f64>() / n;
            let var: f64 = mine
            .iter()
            .map(|p| (p.0 as f64 - ar).powi(2) + (p.1 as f64 - ac).powi(2))
            .sum::<f64>()
            / n;
            let ideal = ((rows.min(cols) as f64) / 4.0).powi(2);
            if var > 0.0 && ideal > 0.0 {
                pattern += (if var < ideal { var / ideal } else { ideal / var }) * 10.0;
            }
            for i in 0..mine.len() {
                for j in i + 1..mine.len() {
                    let dr = (mine[i].0 as i32 - mine[j].0 as i32).abs();
                    let dc = (mine[i].1 as i32 - mine[j].1 as i32).abs();
                    if dr == dc && dr > 0 {
                        pattern += 1.5;
                    } else if (dr == 0) != (dc == 0) {
                        pattern += 1.0;
                    }
                }
            }
        }

        coord * e.p(P_COORD, 0.5)
        + protect * e.p(P_PROTECT, 2.0)
        + adj * e.p(P_ADJACENT, 1.0)
        + centre * e.p(P_CENTER, 0.3)
        + pattern * e.p(P_PATTERN, 5.0)
    }
}

impl EvaluatorTrait for FlowEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let me = state.current_turn;
        let mine = self.side_score(state, mg, me);
        let theirs = self.side_score(state, mg, me.opposite());
        ((mine - theirs) * 10.0) as i32
    }

    fn get_piece_value_on_square(
        &self,
        _p: &Piece,
        _s: Position,
        _cm: &PieceConfigManager,
    ) -> i32 {
        100
    }
    fn delta_pruning_margin(&self) -> i32 {
        1500
    }
    fn aspiration_window(&self) -> i32 {
        400
    }
    fn contempt(&self) -> i32 {
        50
    }
}

impl ChessEngine for FlowEngine {
    fn name(&self) -> &str {
        "Flow Engine (Connectivity & Patterns)"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let flat = params.state.board.rows() * params.state.board.cols();
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let ev = FlowEvaluator {
                engine: &*self,
                reach: RefCell::new(vec![0u16; flat]),
            };
            run_search(&ev, params, &self.parameters, &mut tt, 3)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(FLOW_PARAMETERS, &MERGED))
    }
    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }
    fn set_parameters(&mut self, p: EngineParameters) -> bool {
        let changed = self.parameters != p;
        if changed {
            self.parameters = p;
            self.transposition_table.clear();
        }
        changed
    }
}
