// src/engine/sentinel_engine.rs
//
// Technology demonstrator for the optional incremental coverage table.
//
// Everything the evaluator needs is expressed through `CoverageQuery`, so the
// engine runs unchanged on (a) the incremental `AttackTable` and (b) a
// from-scratch `CoverageMap` rebuilt at every leaf. `analyze_position`
// verifies the two agree, then benchmarks them against each other.
//
// Flip `sentinel_use_attack_table` to 0 to see the cost of the naive path.

use crate::attack_table::{CoverageMap, CoverageQuery, CoverageRays};
use crate::clog;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::core::GameState;
use crate::engine::analysis::{
    ClusteringAnalysis, DensityAnalysis, MaterialAnalysis, MobilityAnalysis, NormalizedValues,
    PositionAnalysis, PositionStatistics, StatisticalAnalysis, ThreatAnalysis,
};
use crate::engine::api::{ChessEngine, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::search::{combined_params, run_search, TTEntry};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use smallvec::SmallVec;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

pub const P_MATERIAL: &str = "sentinel_material";
pub const P_COVERAGE: &str = "sentinel_coverage";
pub const P_SATURATION: &str = "sentinel_saturation";
pub const P_CENTER: &str = "sentinel_center";
pub const P_OCCUPIED: &str = "sentinel_occupied_bonus";
pub const P_KING_ZONE: &str = "sentinel_king_zone";
pub const P_HANGING: &str = "sentinel_hanging";
pub const P_TEMPO: &str = "sentinel_tempo";
pub const P_CONTEMPT: &str = "sentinel_contempt";
pub const P_USE_TABLE: &str = "sentinel_use_attack_table";
pub const P_VERIFY: &str = "sentinel_verify_table";

pub static SENTINEL_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        P_MATERIAL,
        "Material Weight",
        "Centipawns per pawn-unit of derived material.",
        0.0, 300.0, 100.0, 1.0,
    ),
ParameterDef::new(
    P_COVERAGE,
    "Coverage Weight",
    "Centipawns per unit of saturated, importance-weighted square coverage.",
    0.0, 40.0, 5.0, 0.25,
),
ParameterDef::new(
    P_SATURATION,
    "Coverage Saturation",
    "Diminishing returns on stacking attackers: eff = n/(sat+n). Higher = breadth over depth.",
                  0.2, 6.0, 1.6, 0.05,
),
ParameterDef::new(
    P_CENTER,
    "Centrality Weight",
    "Extra importance for central squares.",
    0.0, 3.0, 0.35, 0.05,
),
ParameterDef::new(
    P_OCCUPIED,
    "Occupied Square Bonus",
    "Extra importance for covering an occupied square, scaled by sqrt(occupant value).",
                  0.0, 4.0, 0.8, 0.05,
),
ParameterDef::new(
    P_KING_ZONE,
    "King Zone Weight",
    "Extra importance within Chebyshev 3 of any protected (royal / last-royalty) piece.",
                  0.0, 5.0, 1.0, 0.05,
),
ParameterDef::new(
    P_HANGING,
    "Hanging Weight",
    "Centipawns per pawn-unit of material a static exchange says is currently loose.",
    0.0, 200.0, 30.0, 1.0,
),
ParameterDef::new(
    P_TEMPO,
    "Tempo",
    "Flat centipawn bonus for the side to move.",
    0.0, 60.0, 10.0, 1.0,
),
ParameterDef::new(
    P_CONTEMPT,
    "Contempt",
    "Draw dislike, in centipawns. Routed through EvaluatorTrait::contempt, never baked into the leaf score.",
    0.0, 80.0, 12.0, 1.0,
),
ParameterDef::new(
    P_USE_TABLE,
    "Use Incremental Attack Table",
    "1 = maintain the incremental coverage table and query it in O(1). 0 = recompute the whole coverage map from scratch at every leaf. Both paths go through the same CoverageQuery trait and must agree exactly.",
                  0.0, 1.0, 1.0, 1.0,
),
ParameterDef::new(
    P_VERIFY,
    "Verify Table",
    "1 = after every leaf sync, rebuild a reference map and assert equality (debug builds only). Ruinously slow; for correctness demos.",
                  0.0, 1.0, 0.0, 1.0,
),
];

// ─────────────────────────────────────────────────────────────────────────

struct SentinelData {
    board_size: (usize, usize),
    num_pieces: usize,
    /// Pawn-normalised intrinsic values, derived from average coverage.
    value: Vec<f32>,
    royal_value: f32,
    /// Per-square centrality in [0,1].
    center: Vec<f32>,
}

impl SentinelData {
    #[inline]
    fn value_of(&self, p: &Piece) -> f32 {
        if p.is_royal {
            self.royal_value
        } else {
            self.value.get(p.piece_type).copied().unwrap_or(1.0)
        }
    }
}

pub struct SentinelEngine {
    parameters: EngineParameters,
    data: Option<SentinelData>,
    rays: Option<Arc<CoverageRays>>,
    /// Reused so the "slow" path is measured fairly: it pays for
    /// recomputation, not for `malloc`.
    scratch: RefCell<Option<CoverageMap>>,
    transposition_table: HashMap<u64, TTEntry>,
    needs_reinit: bool,
}

impl SentinelEngine {
    pub fn new() -> Self {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        let defs = combined_params(SENTINEL_PARAMETERS, &MERGED);
        Self {
            parameters: EngineParameters::from_defaults(defs),
            data: None,
            rays: None,
            scratch: RefCell::new(None),
            transposition_table: HashMap::new(),
            needs_reinit: true,
        }
    }

    #[inline]
    fn p(&self, id: &str, d: f64) -> f32 {
        self.parameters.get_or_default(id, d) as f32
    }
    #[inline]
    fn use_table(&self) -> bool {
        self.p(P_USE_TABLE, 1.0) > 0.5
    }
    #[inline]
    fn verify_enabled(&self) -> bool {
        self.p(P_VERIFY, 0.0) > 0.5
    }

    fn initialize(&mut self, board: &Board, mg: &MoveGenerator, cm: &PieceConfigManager) {
        let board_size = board.size();
        let num_pieces = cm.piece_order.len();
        let fresh = self.needs_reinit
        || self.data.as_ref().map_or(true, |d| {
            d.board_size != board_size || d.num_pieces != num_pieces
        });
        if !fresh {
            return;
        }

        clog!(
            "🛡️  Sentinel: building coverage rays for {}x{}, {} piece types…",
            board_size.0,
            board_size.1,
            num_pieces
        );

        let rays = Arc::new(CoverageRays::build(mg, board_size, num_pieces));

        // Material from average coverage; weakest positive piece ≈ 1 pawn.
        let avg = rays.avg_coverage().to_vec();
        let min_pos = avg
        .iter()
        .copied()
        .filter(|&v| v > 0.01)
        .fold(f32::INFINITY, f32::min);
        let min_pos = if min_pos.is_finite() && min_pos > 0.0 { min_pos } else { 1.0 };

        let mut value = vec![1.0f32; num_pieces];
        let mut max_nonroyal = 1.0f32;
        for pt in 0..num_pieces {
            let is_royal = cm
            .get_piece_by_index(pt)
            .map_or(false, |c| c.properties.is_royal);
            let v = if avg[pt] > 0.01 { avg[pt] / min_pos } else { 0.5 };
            value[pt] = v;
            if !is_royal {
                max_nonroyal = max_nonroyal.max(v);
            }
        }

        let (rows, cols) = board_size;
        let cr = (rows as f32 - 1.0) * 0.5;
        let cc = (cols as f32 - 1.0) * 0.5;
        let maxd = (cr * cr + cc * cc).sqrt().max(1.0);
        let mut center = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                let dr = r as f32 - cr;
                let dc = c as f32 - cc;
                center.push(1.0 - (dr * dr + dc * dc).sqrt() / maxd);
            }
        }

        for (pt, name) in cm.piece_order.iter().enumerate() {
            clog!(
                "   {:<16} avg-coverage {:>6.2}   value {:>5.2}",
                name,
                avg[pt],
                value[pt]
            );
        }

        *self.scratch.borrow_mut() = Some(CoverageMap::new(board_size));
        self.data = Some(SentinelData {
            board_size,
            num_pieces,
            value,
            royal_value: max_nonroyal * 6.0,
            center,
        });
        self.rays = Some(rays);
        self.needs_reinit = false;
        clog!("✅ Sentinel ready.");
    }

    // ── Static exchange evaluation, O(1) attacker gathering ──────────────

    fn attacker_values(
        &self,
        q: &dyn CoverageQuery,
        board: &Board,
        sq: Position,
        c: PieceColor,
    ) -> SmallVec<[f32; 12]> {
        let d = self.data.as_ref().expect("initialize() not called");
        let mut v: SmallVec<[f32; 12]> = q
        .attackers(sq, c)
        .iter()
        .filter_map(|&f| board.get_piece(q.square(f)))
        .map(|p| d.value_of(&p))
        .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// Net material swing from `side`'s perspective for initiating the
    /// exchange on `sq` against a victim worth `victim`. Arithmetic only; the
    /// board is never touched. No X-ray resolution (documented limitation).
    fn see(
        &self,
        q: &dyn CoverageQuery,
        board: &Board,
        sq: Position,
        side: PieceColor,
        victim: f32,
    ) -> f32 {
        let att = [
            self.attacker_values(q, board, sq, PieceColor::White),
            self.attacker_values(q, board, sq, PieceColor::Black),
        ];
        let si = side.index();
        if att[si].is_empty() {
            return 0.0;
        }

        const MAX: usize = 32;
        let mut cur = [0usize, 0usize];
        let mut gains = [0.0f32; MAX];
        gains[0] = victim;

        let mut on_square = att[si][0];
        cur[si] = 1;
        let mut to_move = side.opposite();
        let mut d = 0usize;

        while d + 1 < MAX {
            d += 1;
            gains[d] = on_square - gains[d - 1];
            let i = to_move.index();
            if cur[i] >= att[i].len() {
                break;
            }
            on_square = att[i][cur[i]];
            cur[i] += 1;
            to_move = to_move.opposite();
        }

        for i in (1..=d).rev() {
            gains[i - 1] = (-gains[i]).max(gains[i - 1]);
        }
        gains[0]
    }

    /// Absolute, non-negative goodness for each side. Shared by `evaluate`
    /// (difference) and `evaluate_split` (log-ratio mode).
    fn score_sides(&self, q: &dyn CoverageQuery, board: &Board) -> (f32, f32) {
        let d = self.data.as_ref().expect("initialize() not called");
        let (rows, cols) = d.board_size;

        let sat = self.p(P_SATURATION, 1.6);
        let mat_w = self.p(P_MATERIAL, 100.0);
        let cov_w = self.p(P_COVERAGE, 5.0);
        let cen_w = self.p(P_CENTER, 0.35);
        let occ_w = self.p(P_OCCUPIED, 0.8);
        let kz_w = self.p(P_KING_ZONE, 1.0);
        let hang_w = self.p(P_HANGING, 30.0);

        // Protected pieces: all 'R', plus the last remaining 'r'
        // (mirrors GameState::is_in_check_fast). Allocation-free.
        let mut prot: SmallVec<[Position; 4]> = SmallVec::new();
        for ci in 0..2 {
            let c = if ci == 0 { PieceColor::White } else { PieceColor::Black };
            prot.extend_from_slice(board.get_royal_positions(c));
            let ry = board.get_royalty_positions(c);
            if ry.len() == 1 {
                prot.push(ry[0]);
            }
        }

        let mut mat = [0.0f32; 2];
        let mut cov = [0.0f32; 2];
        let mut hang = [0.0f32; 2];

        for r in 0..rows {
            for c in 0..cols {
                let sq = (r, c);
                let occ = board.get_piece(sq);
                let wc = q.attackers(sq, PieceColor::White).len() as f32;
                let bc = q.attackers(sq, PieceColor::Black).len() as f32;
                if wc == 0.0 && bc == 0.0 && occ.is_none() {
                    continue;
                }

                let mut imp = 1.0 + cen_w * d.center[r * cols + c];
                if let Some(p) = occ {
                    let v = d.value_of(&p);
                    mat[p.color.index()] += v;
                    imp += occ_w * v.sqrt();
                }
                if kz_w > 0.0 {
                    for &kp in &prot {
                        let dist = (r as i32 - kp.0 as i32)
                        .abs()
                        .max((c as i32 - kp.1 as i32).abs()) as f32;
                        if dist <= 3.0 {
                            imp += kz_w / (1.0 + dist);
                        }
                    }
                }

                cov[0] += (wc / (sat + wc)) * imp;
                cov[1] += (bc / (sat + bc)) * imp;

                if let Some(p) = occ {
                    let enemy = p.color.opposite();
                    if !q.attackers(sq, enemy).is_empty() {
                        let loss = self.see(q, board, sq, enemy, d.value_of(&p));
                        if loss > 0.0 {
                            hang[p.color.index()] += loss;
                        }
                    }
                }
            }
        }

        // Your opponent's loose material is *your* asset.
        let white = mat_w * mat[0] + cov_w * cov[0] + hang_w * hang[1];
        let black = mat_w * mat[1] + cov_w * cov[1] + hang_w * hang[0];
        (white.max(0.0), black.max(0.0))
    }

    /// Run `f` against whichever coverage source is configured, syncing the
    /// incremental table first (a no-op when nothing is dirty).
    ///
    /// `f` must not re-enter `self.scratch`; `score_sides` does not.
    fn with_coverage<R>(
        &self,
        state: &mut GameState,
        f: impl FnOnce(&dyn CoverageQuery, &Board) -> R,
    ) -> R {
        state.sync_attack_table();

        if state.has_attack_table() {
            if self.verify_enabled() {
                let t = state.attacks().expect("table present");
                debug_assert!(
                    t.verify(&state.board),
                              "incremental coverage table diverged from the reference map"
                );
            }
            let t = state.attacks().expect("table present");
            return f(t as &dyn CoverageQuery, &state.board);
        }

        let rays = self.rays.as_ref().expect("initialize() not called");
        let mut slot = self.scratch.borrow_mut();
        let map = slot.as_mut().expect("scratch not allocated");
        map.recompute(rays, &state.board);
        let q: &dyn CoverageQuery = &*map;
        f(q, &state.board)
    }
}

impl Default for SentinelEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────

struct SentinelEvaluator<'a> {
    engine: &'a SentinelEngine,
}

impl EvaluatorTrait for SentinelEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> i32 {
        if !state.board.has_pieces(state.current_turn) {
            return -999_999;
        }
        let engine = self.engine;
        let (w, b) = engine.with_coverage(state, |q, board| engine.score_sides(q, board));
        let stm = match state.current_turn {
            PieceColor::White => w - b,
            PieceColor::Black => b - w,
        };
        (stm + engine.p(P_TEMPO, 10.0)).round() as i32
    }

    fn evaluate_split(
        &self,
        state: &mut GameState,
        _mg: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> Option<(f32, f32)> {
        let engine = self.engine;
        let (w, b) = engine.with_coverage(state, |q, board| engine.score_sides(q, board));
        Some(match state.current_turn {
            PieceColor::White => (w, b),
             PieceColor::Black => (b, w),
        })
    }

    fn get_piece_value_on_square(
        &self,
        piece: &Piece,
        _pos: Position,
        _cm: &PieceConfigManager,
    ) -> i32 {
        match &self.engine.data {
            Some(d) => (d.value_of(piece) * 100.0) as i32,
            None => 100,
        }
    }

    fn delta_pruning_margin(&self) -> i32 {
        400
    }
    fn aspiration_window(&self) -> i32 {
        60
    }
    fn contempt(&self) -> i32 {
        self.engine.p(P_CONTEMPT, 12.0) as i32
    }
}

// ─────────────────────────────────────────────────────────────────────────

impl ChessEngine for SentinelEngine {
    fn name(&self) -> &str {
        "Sentinel Engine (Incremental Attack Table)"
    }

    fn reset_cache(&mut self) {
        self.transposition_table.clear();
        self.data = None;
        self.rays = None;
        *self.scratch.borrow_mut() = None;
        self.needs_reinit = true;
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let SearchParams {
            state,
            move_generator,
            config_manager,
            time_limit,
            depth,
        } = params;

        self.initialize(&state.board, move_generator, config_manager);

        if self.use_table() {
            if let Some(rays) = &self.rays {
                state.enable_attack_table(Arc::clone(rays));
            }
        }

        let mut tt = std::mem::take(&mut self.transposition_table);
        let result = {
            let evaluator = SentinelEvaluator { engine: &*self };
            run_search(
                &evaluator,
                SearchParams {
                    state: &mut *state,
                    move_generator,
                    config_manager,
                    time_limit,
                    depth,
                },
                &self.parameters,
                &mut tt,
                4,
            )
        };
        self.transposition_table = tt;

        if let Some(s) = state.attack_stats() {
            let avg = if s.syncs > 0 {
                (s.piece_recomputes as f64) / (s.syncs as f64)
            } else {
                0.0
            };
            clog!(
                "  🛡️  table: {} syncs, {} dirty marks, {} piece recomputes ({:.2}/sync), {} queries",
                  s.syncs, s.dirty_marks, s.piece_recomputes, avg, s.queries
            );
        }

        // Leave the shared GameState exactly as we found it, so no other
        // engine ever pays for this feature.
        state.disable_attack_table();
        result
    }

    fn stop(&mut self) {}

    fn supports_analysis(&self) -> bool {
        true
    }

    fn analyze_position(
        &mut self,
        state: &mut GameState,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
    ) -> Option<PositionAnalysis> {
        self.initialize(&state.board, mg, cm);
        let rays = Arc::clone(self.rays.as_ref()?);
        let (rows, cols) = state.board.size();

        state.enable_attack_table(Arc::clone(&rays));
        state.sync_attack_table();

        let hr = "=".repeat(70);
        clog!("\n{}", hr);
        clog!(" SENTINEL — INCREMENTAL COVERAGE ANALYSIS");
        clog!("{}", hr);

        // ── correctness ──────────────────────────────────────────────────
        {
            let t = state.attacks().expect("table present");
            let consistent = t.verify(&state.board);
            clog!(
                " Table vs. from-scratch reference: {}",
                if consistent { "✅ identical" } else { "❌ DIVERGED" }
            );
        }

        // ── coverage grid ────────────────────────────────────────────────
        clog!("\n-- Coverage (piece, whiteAttackers/blackAttackers) --");
        {
            let t = state.attacks().expect("table present");
            for r in 0..rows {
                let mut line = format!("{:>3} |", rows - r);
                for c in 0..cols {
                    let sq = (r, c);
                    let w = t.attackers(sq, PieceColor::White).len();
                    let b = t.attackers(sq, PieceColor::Black).len();
                    let ch = state
                    .board
                    .get_piece(sq)
                    .map(|p| p.to_char(cm))
                    .unwrap_or('.');
                    line.push_str(&format!(" {}{}/{} ", ch, w, b));
                }
                clog!("{}", line);
            }
            let mut files = String::from("    ");
            for c in 0..cols {
                files.push_str(&format!("  {}  ", (b'a' + c as u8) as char));
            }
            clog!("{}", files);
        }

        // ── loose material ───────────────────────────────────────────────
        {
            let t = state.attacks().expect("table present");
            let q: &dyn CoverageQuery = t;
            let d = self.data.as_ref().expect("initialized");
            let mut loose: Vec<(Position, usize, f32)> = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    let sq = (r, c);
                    if let Some(p) = state.board.get_piece(sq) {
                        let e = p.color.opposite();
                        if !q.attackers(sq, e).is_empty() {
                            let s = self.see(q, &state.board, sq, e, d.value_of(&p));
                            if s > 0.0 {
                                loose.push((sq, p.piece_type, s));
                            }
                        }
                    }
                }
            }
            loose.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
            if loose.is_empty() {
                clog!("\n-- Loose material: none --");
            } else {
                clog!("\n-- Loose material (static exchange > 0) --");
                for (sq, pt, s) in loose.iter().take(10) {
                    let nm = cm
                    .get_piece_by_index(*pt)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_default();
                    clog!(
                        "   {}{}  {:<16} loses {:.2} pawn-units",
                        (b'a' + sq.1 as u8) as char,
                          rows - sq.0,
                          nm,
                          s
                    );
                }
            }
        }

        // ── benchmark: incremental vs full rebuild ───────────────────────
        let legal = state.get_legal_moves(mg, cm);
        state.sync_attack_table();
        state.reset_attack_stats();

        let t0 = Instant::now();
        for mv in &legal {
            state.execute_expanded_move(mv, mg, cm);
            state.sync_attack_table();
            state.undo_move(cm);
            state.sync_attack_table();
        }
        let incremental = t0.elapsed();
        let stats = state.attack_stats().expect("table present");

        state.disable_attack_table();

        let mut map = CoverageMap::new((rows, cols));
        let t1 = Instant::now();
        for mv in &legal {
            state.execute_expanded_move(mv, mg, cm);
            map.recompute(&rays, &state.board);
            state.undo_move(cm);
            map.recompute(&rays, &state.board);
        }
        let full = t1.elapsed();

        let speedup = full.as_secs_f64() / incremental.as_secs_f64().max(1e-9);
        clog!(
            "\n-- Benchmark ({} legal moves, make+sync+undo+sync each) --",
              legal.len()
        );
        clog!(
            "   incremental : {:>9.3?}   ({} syncs, {} piece recomputes, {} scanned)",
              incremental, stats.syncs, stats.piece_recomputes, stats.scanned_pieces
        );
        clog!("   full rebuild: {:>9.3?}", full);
        clog!("   speedup     : {:.2}x", speedup);
        clog!(
            "   recomputes/sync: {:.2}   (a full rebuild touches {} pieces)",
              stats.piece_recomputes as f64 / stats.syncs.max(1) as f64,
              state.board.count_pieces()
        );
        clog!("{}\n", hr);

        // ── PositionAnalysis for the GUI ─────────────────────────────────
        let mut piece_counts: HashMap<usize, (u32, u32)> = HashMap::new();
        let mut mw = 0.0f64;
        let mut mb = 0.0f64;
        {
            let d = self.data.as_ref().expect("initialized");
            for r in 0..rows {
                for c in 0..cols {
                    if let Some(p) = state.board.get_piece((r, c)) {
                        let e = piece_counts.entry(p.piece_type).or_insert((0, 0));
                        if p.color == PieceColor::White {
                            e.0 += 1;
                            mw += d.value_of(&p) as f64;
                        } else {
                            e.1 += 1;
                            mb += d.value_of(&p) as f64;
                        }
                    }
                }
            }
        }

        let piece_values: HashMap<usize, f64> = self
        .data
        .as_ref()
        .expect("initialized")
        .value
        .iter()
        .enumerate()
        .map(|(i, &v)| (i, v as f64))
        .collect();

        let stm_moves = legal.len() as u32;
        let orig = state.current_turn;
        state.current_turn = orig.opposite();
        let opp_moves = state.get_legal_moves(mg, cm).len() as u32;
        state.current_turn = orig;
        let (wm, bm) = match orig {
            PieceColor::White => (stm_moves, opp_moves),
            PieceColor::Black => (opp_moves, stm_moves),
        };

        let total_sq = (rows * cols) as f64;
        let total_pc = state.board.count_pieces() as f64;

        Some(PositionAnalysis {
            material_values: MaterialAnalysis {
                white_total: mw,
                black_total: mb,
                difference: mw - mb,
                piece_counts,
                piece_values: piece_values.clone(),
            },
            pst_analysis: None,
            mobility_analysis: MobilityAnalysis {
                white_mobility: wm,
                black_mobility: bm,
                mobility_difference: wm as i32 - bm as i32,
                piece_mobility: HashMap::new(),
             value_to_mobility_ratio: 0.0,
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
            },
            density_analysis: DensityAnalysis {
                board_density: if total_sq > 0.0 { total_pc / total_sq } else { 0.0 },
                piece_density_ratio: 0.0,
                clustering: ClusteringAnalysis {
                    average_piece_distance: 0.0,
                    clustering_coefficient: 0.0,
                    isolated_pieces: Vec::new(),
             dense_regions: Vec::new(),
                },
            },
            statistical_analysis: StatisticalAnalysis {
                normalized_values: NormalizedValues {
                    weakest_piece_value: 1.0,
                    white_total_normalized: mw,
                    black_total_normalized: mb,
                    piece_values_normalized: piece_values,
                },
                statistics: PositionStatistics {
                    total_pieces: total_pc as u32,
                    value_per_piece: if total_pc > 0.0 { (mw + mb) / total_pc } else { 0.0 },
             value_weighted_average: 0.0,
             value_variance: 0.0,
             piece_type_diversity: 0.0,
             position_complexity: 0.0,
                },
            },
        })
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        static MERGED: OnceLock<Vec<ParameterDef>> = OnceLock::new();
        Some(combined_params(SENTINEL_PARAMETERS, &MERGED))
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    fn set_parameters(&mut self, p: EngineParameters) -> bool {
        let changed = self.parameters != p;
        if changed {
            self.parameters = p;
            self.transposition_table.clear(); // eval scale may have moved
        }
        changed
    }
}
