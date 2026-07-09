// src/engine/outpost_engine.rs
//
// Territory/outpost engine.
//
// Fixes: uses the cached `Piece::is_royal`/`is_royalty` flags instead of a
// `PieceConfigManager` lookup per piece per leaf; uses `Board::piece_count`
// instead of allocating two `Vec`s just to read `.len()`; persists a
// transposition table; routes through `run_search`.
//
// NOTE: `evaluate_outpost_quality` calls `MoveGenerator::get_attackers_to_square`
// once per non-royal piece, which walks the whole reverse database. This
// engine is the single best candidate in the tree for opting into
// `crate::attack_table` (see `sentinel_engine.rs` for the pattern).

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
use std::collections::HashMap;
use std::sync::OnceLock;

pub const PARAM_OUTPOST_VALUE: &str = "outpost_value";
pub const PARAM_SUPPORT_BONUS: &str = "support_bonus";
pub const PARAM_ENEMY_TERRITORY_MULT: &str = "enemy_territory_mult";
pub const PARAM_CENTRAL_BONUS: &str = "central_bonus";
pub const PARAM_MATERIAL_WEIGHT: &str = "outpost_material_weight";

pub static OUTPOST_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_OUTPOST_VALUE,
        "Outpost Base Value",
        "Base value for pieces in advanced positions. Higher = more aggressive.",
        0.0, 100.0, 20.0, 1.0,
    ),
ParameterDef::new(
    PARAM_SUPPORT_BONUS,
    "Support Bonus",
    "Bonus for outpost pieces protected by friendly pieces (capped at 2 supporters).",
                  0.0, 50.0, 15.0, 1.0,
),
ParameterDef::new(
    PARAM_ENEMY_TERRITORY_MULT,
    "Enemy Territory Multiplier",
    "Multiplier for pieces deep in enemy territory.",
    1.0, 3.0, 1.5, 0.1,
),
ParameterDef::new(
    PARAM_CENTRAL_BONUS,
    "Central Files Bonus",
    "Extra bonus for outposts in central files.",
    0.0, 30.0, 10.0, 1.0,
),
ParameterDef::new(
    PARAM_MATERIAL_WEIGHT,
    "Material Weight",
    "Small material component so the engine does not sacrifice everything.",
    0.0, 1.0, 0.1, 0.01,
),
];

pub struct OutpostEngine {
    parameters: EngineParameters,
    transposition_table: HashMap<u64, TTEntry>,
}

impl OutpostEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Self {
            parameters: EngineParameters::from_defaults(combined_params(
                OUTPOST_PARAMETERS,
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

impl Default for OutpostEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct OutpostEvaluator<'a> {
    engine: &'a OutpostEngine,
}

impl OutpostEvaluator<'_> {
    fn quality(
        &self,
        pos: Position,
        piece: &Piece,
        board: &Board,
        board_size: (usize, usize),
               mg: &MoveGenerator,
    ) -> f64 {
        let (rows, cols) = board_size;

        let advancement = match piece.color {
            PieceColor::White => (rows - 1 - pos.0) as f64 / rows as f64,
            PieceColor::Black => pos.0 as f64 / rows as f64,
        };
        if advancement < 0.5 {
            return 0.0;
        }

        let mut quality = self.engine.p(PARAM_OUTPOST_VALUE, 20.0) * (advancement - 0.5) * 2.0;

        // Friendly pieces that can reach this square = supporters.
        let supporters = mg.get_attackers_to_square(board, pos, piece.color);
        quality += self.engine.p(PARAM_SUPPORT_BONUS, 15.0) * (supporters.len() as f64).min(2.0);

        let deep = match piece.color {
            PieceColor::White => pos.0 <= 2,
            PieceColor::Black => pos.0 + 3 >= rows,
        };
        if deep {
            quality *= self.engine.p(PARAM_ENEMY_TERRITORY_MULT, 1.5);
        }

        let center_col = cols / 2;
        let col_distance = (pos.1 as i32 - center_col as i32).abs();
        if col_distance <= 1 {
            quality += self.engine.p(PARAM_CENTRAL_BONUS, 10.0) * (2.0 - col_distance as f64) / 2.0;
        }

        quality
    }
}

impl EvaluatorTrait for OutpostEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        let me = state.current_turn;
        let them = me.opposite();
        let b = &state.board;
        let size = b.size();
        let (rows, cols) = size;

        let mut mine = 0.0f64;
        let mut theirs = 0.0f64;

        for r in 0..rows {
            for c in 0..cols {
                let Some(p) = b.get_piece((r, c)) else { continue };
                // `Piece` caches both flags precisely so the hot path never
                // touches the config manager.
                if p.is_royal || p.is_royalty {
                    continue;
                }
                let q = self.quality((r, c), &p, b, size, mg);
                if p.color == me {
                    mine += q;
                } else {
                    theirs += q;
                }
            }
        }

        // Board maintains piece counts incrementally; the old code allocated
        // two Vecs per leaf just to call `.len()`.
        let mw = self.engine.p(PARAM_MATERIAL_WEIGHT, 0.1);
        let material_diff =
        (b.piece_count(me) as f64 - b.piece_count(them) as f64) * 100.0 * mw;

        ((mine - theirs + material_diff) * 10.0) as i32
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
        50
    }
}

impl ChessEngine for OutpostEngine {
    fn name(&self) -> &str {
        "Outpost Engine (Territory Control)"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let ev = OutpostEvaluator { engine: &*self };
            run_search(&ev, params, &self.parameters, &mut tt, 3)
        };
        self.transposition_table = tt;
        result
    }

    fn stop(&mut self) {}

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(OUTPOST_PARAMETERS, &MERGED))
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
