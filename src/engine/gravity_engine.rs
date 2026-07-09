// src/engine/gravity_engine.rs
use crate::core::piece::Piece;
use crate::core::position::Position;
use crate::core::GameState;
use crate::engine::api::{ChessEngine, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{combined_params, run_search, TTEntry};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const P_PULL: &str = "gravity_pull";
pub const P_CENTER_SQUARE: &str = "gravity_center_square";

pub static GRAVITY_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        P_PULL,
        "Gravitational Pull",
        "Strength of the 1/(d+1) attraction toward the board's centre of mass.",
                      0.0, 50.0, 10.0, 0.5,
    ),
ParameterDef::new(
    P_CENTER_SQUARE,
    "Centre-Square Bonus",
    "Bonus for occupying the centre-of-mass square itself.",
    0.0, 50.0, 5.0, 0.5,
),
];

pub struct GravityEngine {
    parameters: EngineParameters,
    transposition_table: HashMap<u64, TTEntry>,
}

impl GravityEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            parameters: EngineParameters::from_defaults(combined_params(
                GRAVITY_PARAMETERS,
                &MERGED,
            )),
            transposition_table: HashMap::new(),
        }
    }
    #[inline]
    fn p(&self, id: &str, d: f64) -> f64 {
        self.parameters.get_or_default(id, d)
    }
}

impl Default for GravityEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct GravityEvaluator<'a> {
    engine: &'a GravityEngine,
}

impl EvaluatorTrait for GravityEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let b = &state.board;
        let (rows, cols) = b.size();
        let me = state.current_turn;

        let pull = self.engine.p(P_PULL, 10.0);
        let cbonus = self.engine.p(P_CENTER_SQUARE, 5.0);

        // Single pass for the centre of mass.
        let mut tr = 0.0f64;
        let mut tc = 0.0f64;
        let mut n = 0u32;
        for r in 0..rows {
            for c in 0..cols {
                if b.get_piece((r, c)).is_some() {
                    tr += r as f64;
                    tc += c as f64;
                    n += 1;
                }
            }
        }
        let com = if n > 0 {
            (tr / n as f64, tc / n as f64)
        } else {
            ((rows as f64 - 1.0) / 2.0, (cols as f64 - 1.0) / 2.0)
        };

        let mut mine = 0.0f64;
        let mut theirs = 0.0f64;
        for r in 0..rows {
            for c in 0..cols {
                let Some(p) = b.get_piece((r, c)) else { continue };
                let dr = r as f64 - com.0;
                let dc = c as f64 - com.1;
                let s = pull / ((dr * dr + dc * dc).sqrt() + 1.0);
                if p.color == me {
                    mine += s;
                } else {
                    theirs += s;
                }
            }
        }

        let cs: Position = (com.0.round() as usize, com.1.round() as usize);
        let mut bonus = 0.0;
        if cs.0 < rows && cs.1 < cols {
            if let Some(p) = b.get_piece(cs) {
                bonus = if p.color == me { cbonus } else { -cbonus };
            }
        }

        ((mine - theirs + bonus) * 100.0) as i32
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
        2000
    }
    fn aspiration_window(&self) -> i32 {
        500
    }
    fn contempt(&self) -> i32 {
        50
    }
}

impl ChessEngine for GravityEngine {
    fn name(&self) -> &str {
        "Gravity Engine (Center of Mass)"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let ev = GravityEvaluator { engine: &*self };
            run_search(&ev, params, &self.parameters, &mut tt, 3)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(GRAVITY_PARAMETERS, &MERGED))
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
