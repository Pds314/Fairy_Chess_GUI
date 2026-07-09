// src/engine/swarm_engine.rs
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
use std::collections::HashMap;
use std::sync::OnceLock;

pub const P_ATTACK: &str = "swarm_attack_weight";
pub const P_HUDDLE: &str = "swarm_huddle_weight";

pub static SWARM_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        P_ATTACK,
        "Attack Weight",
        "Inverse-distance pull of pieces toward enemy royal/royalty squares.",
        0.0, 5.0, 1.0, 0.05,
    ),
ParameterDef::new(
    P_HUDDLE,
    "Huddle Weight",
    "Inverse-distance pull of pieces toward their own royals (defensive clustering).",
                  0.0, 5.0, 0.0, 0.05,
),
];

/// Royal + royalty squares, read straight from `Board`'s incremental lists.
/// The old version scanned all 64 squares and did a `PieceConfigManager`
/// lookup per piece — twice per evaluated leaf.
#[inline]
fn protected(board: &Board, color: PieceColor) -> SmallVec<[Position; 4]> {
    let mut v: SmallVec<[Position; 4]> = SmallVec::new();
    v.extend_from_slice(board.get_royal_positions(color));
    v.extend_from_slice(board.get_royalty_positions(color));
    v
}

#[inline]
fn pull(pos: Position, targets: &[Position]) -> f64 {
    let mut s = 0.0;
    for t in targets {
        let dr = pos.0 as f64 - t.0 as f64;
        let dc = pos.1 as f64 - t.1 as f64;
        let d = (dr * dr + dc * dc).sqrt();
        if d > 0.0 {
            s += 1.0 / d;
        }
    }
    s
}

pub struct SwarmEngine {
    parameters: EngineParameters,
    transposition_table: HashMap<u64, TTEntry>,
}

impl SwarmEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            parameters: EngineParameters::from_defaults(combined_params(SWARM_PARAMETERS, &MERGED)),
            transposition_table: HashMap::new(),
        }
    }
    #[inline]
    fn p(&self, id: &str, d: f64) -> f64 {
        self.parameters.get_or_default(id, d)
    }
}

impl Default for SwarmEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct SwarmEvaluator<'a> {
    engine: &'a SwarmEngine,
}

impl EvaluatorTrait for SwarmEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let b = &state.board;
        let me = state.current_turn;
        let them = me.opposite();

        let my_royals = protected(b, me);
        let their_royals = protected(b, them);

        let aw = self.engine.p(P_ATTACK, 1.0);
        let hw = self.engine.p(P_HUDDLE, 0.0);

        let (rows, cols) = b.size();
        let mut mine = 0.0;
        let mut theirs = 0.0;

        for r in 0..rows {
            for c in 0..cols {
                let Some(p) = b.get_piece((r, c)) else { continue };
                if p.color == me {
                    mine += aw * pull((r, c), &their_royals) + hw * pull((r, c), &my_royals);
                } else {
                    theirs += aw * pull((r, c), &my_royals) + hw * pull((r, c), &their_royals);
                }
            }
        }

        ((mine - theirs) * 1000.0) as i32
    }

    /// Swarm has no material model; a flat value keeps MVV-LVA ordering and
    /// quiescence delta pruning sane rather than undefined.
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
        100
    }
}

impl ChessEngine for SwarmEngine {
    fn name(&self) -> &str {
        "Swarm Engine"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let ev = SwarmEvaluator { engine: &*self };
            run_search(&ev, params, &self.parameters, &mut tt, 3)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(SWARM_PARAMETERS, &MERGED))
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
