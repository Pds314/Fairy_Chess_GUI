// src/engine/pressure_engine.rs
//
// Zone-control engine.
//
// Three fixes over the original:
//   * `evaluate` was returning white-minus-black regardless of side to move.
//     In a negamax search that is simply wrong; it is now STM-relative.
//   * Zone attribution was O(pieces · moves · zones) because every generated
//     move was tested against every zone. A zone index is a division.
//   * It used `generate_moves_with_details` (the re-tracing path) rather than
//     `generate_moves_with_database`.

use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::core::GameState;
use crate::engine::api::{ChessEngine, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{combined_params, run_search, TTEntry};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const PARAM_ZONE_SIZE: &str = "zone_size";
pub const PARAM_CONTROL_WEIGHT: &str = "control_weight";
pub const PARAM_CONTESTED_BONUS: &str = "contested_bonus";
pub const PARAM_EDGE_PENALTY: &str = "edge_penalty";

pub static PRESSURE_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_ZONE_SIZE,
        "Zone Size",
        "Size of pressure zones (2-4). Smaller = more granular analysis. Read once per search.",
                      2.0, 4.0, 3.0, 1.0,
    ),
ParameterDef::new(
    PARAM_CONTROL_WEIGHT,
    "Control Weight",
    "How much zone control matters. Higher = dominance is more important.",
    0.5, 3.0, 1.0, 0.1,
),
ParameterDef::new(
    PARAM_CONTESTED_BONUS,
    "Contested Zone Bonus",
    "Bonus for zones where both sides have pieces.",
    0.0, 2.0, 0.5, 0.1,
),
ParameterDef::new(
    PARAM_EDGE_PENALTY,
    "Edge Zone Penalty",
    "Penalty factor for control of edge zones. Higher = prefers central control.",
    0.0, 1.0, 0.3, 0.1,
),
];

struct ZoneScratch {
    zs: usize,
    zrows: usize,
    zcols: usize,
    pieces: [Vec<u32>; 2],
    attacks: [Vec<u32>; 2],
}

impl ZoneScratch {
    fn new(rows: usize, cols: usize, zs: usize) -> Self {
        let zrows = rows.div_ceil(zs);
        let zcols = cols.div_ceil(zs);
        let nz = (zrows * zcols).max(1);
        Self {
            zs,
            zrows,
            zcols,
            pieces: [vec![0; nz], vec![0; nz]],
            attacks: [vec![0; nz], vec![0; nz]],
        }
    }
    #[inline(always)]
    fn idx(&self, r: usize, c: usize) -> usize {
        (r / self.zs) * self.zcols + c / self.zs
    }
    fn clear(&mut self) {
        for ci in 0..2 {
            for v in self.pieces[ci].iter_mut() {
                *v = 0;
            }
            for v in self.attacks[ci].iter_mut() {
                *v = 0;
            }
        }
    }
    #[inline]
    fn is_edge_zone(&self, zi: usize) -> bool {
        let zr = zi / self.zcols;
        let zc = zi % self.zcols;
        zr == 0 || zc == 0 || zr + 1 == self.zrows || zc + 1 == self.zcols
    }
}

pub struct PressureEngine {
    parameters: EngineParameters,
    transposition_table: HashMap<u64, TTEntry>,
}

impl PressureEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            parameters: EngineParameters::from_defaults(combined_params(
                PRESSURE_PARAMETERS,
                &MERGED,
            )),
            transposition_table: HashMap::new(),
        }
    }
    #[inline]
    fn p(&self, id: &str, d: f64) -> f64 {
        self.parameters.get_or_default(id, d)
    }
    fn zone_size(&self) -> usize {
        (self.p(PARAM_ZONE_SIZE, 3.0) as usize).clamp(2, 4)
    }
}

impl Default for PressureEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct PressureEvaluator<'a> {
    engine: &'a PressureEngine,
    z: RefCell<ZoneScratch>,
}

impl EvaluatorTrait for PressureEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let b = &state.board;
        let (rows, cols) = b.size();
        let mut z = self.z.borrow_mut();
        z.clear();

        for r in 0..rows {
            for c in 0..cols {
                let Some(p) = b.get_piece((r, c)) else { continue };
                let ci = p.color.index();
                let zi = z.idx(r, c);
                z.pieces[ci][zi] += 1;
                for mv in mg.generate_moves_with_database(b, (r, c), p.piece_type) {
                    let d = mv.destination;
                    let di = z.idx(d.0, d.1);
                    z.attacks[ci][di] += 1;
                }
            }
        }

        let cw = self.engine.p(PARAM_CONTROL_WEIGHT, 1.0);
        let cb = self.engine.p(PARAM_CONTESTED_BONUS, 0.5);
        let ep = self.engine.p(PARAM_EDGE_PENALTY, 0.3);

        let nz = z.pieces[0].len();
        let mut white_total = 0.0f64;
        for zi in 0..nz {
            let wc = z.pieces[0][zi] as f64 + z.attacks[0][zi] as f64 * 0.5;
            let bc = z.pieces[1][zi] as f64 + z.attacks[1][zi] as f64 * 0.5;
            let zone_score = (wc - bc) * cw;
            let contested = if z.pieces[0][zi] > 0 && z.pieces[1][zi] > 0 {
                cb
            } else {
                0.0
            };
            let edge_factor = if z.is_edge_zone(zi) { 1.0 - ep } else { 1.0 };
            white_total += (zone_score + contested) * edge_factor;
        }

        // Negamax demands a side-to-move perspective. The original returned
        // white-minus-black unconditionally.
        let stm = match state.current_turn {
            PieceColor::White => white_total,
            PieceColor::Black => -white_total,
        };
        (stm * 100.0) as i32
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
        300
    }
    fn contempt(&self) -> i32 {
        40
    }
}

impl ChessEngine for PressureEngine {
    fn name(&self) -> &str {
        "Pressure Engine (Zone Control)"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let (rows, cols) = params.state.board.size();
        let zs = self.zone_size();
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let ev = PressureEvaluator {
                engine: &*self,
                z: RefCell::new(ZoneScratch::new(rows, cols, zs)),
            };
            run_search(&ev, params, &self.parameters, &mut tt, 3)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(PRESSURE_PARAMETERS, &MERGED))
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
