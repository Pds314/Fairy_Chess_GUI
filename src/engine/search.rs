use crate::core::game_state::{ExpandedMove, GameResult, MoveGenerationResult};
use crate::core::{GameState, PieceColor};
use crate::engine::analysis::MoveEvaluation;
use crate::engine::api::{Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::move_generator::{resolve_landing_capture, MoveGenerator};
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const ALPHA_BETA_MIN: i32 = -1_000_000_000;
const ALPHA_BETA_MAX: i32 = 1_000_000_000;
const MATE_THRESHOLD: i32 = 900_000;
const QUIESCENCE_MAX_PLY: u32 = 40;

// ─────────────────────────────────────────────────────────────────────────
// Transposition table
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct TTEntry {
    pub hash: u64,
    pub depth: u32,
    pub score: i32,
    pub flag: TTFlag,
    pub best_move: Option<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TTFlag {
    Exact,
    LowerBound,
    UpperBound,
}

// ─────────────────────────────────────────────────────────────────────────
// Parameter definitions
// ─────────────────────────────────────────────────────────────────────────

pub const PARAM_USE_NEW_SEARCH: &str = "search_use_improved";
pub const PARAM_BEST_FIRST_ORDERING: &str = "search_best_first_ordering";
pub const PARAM_BEST_FIRST_SEARCH: &str = "search_best_first_search";
pub const PARAM_BFS_MAX_NODES: &str = "search_bfs_max_nodes";
pub const PARAM_BFS_MAX_DEPTH: &str = "search_bfs_max_depth";
pub const PARAM_BFS_COMPACT_MOVES: &str = "search_bfs_compact_moves";
pub const PARAM_DELTA_QUIESCENCE: &str = "search_delta_quiescence";
pub const PARAM_DELTA_QUIESCENCE_THRESHOLD: &str = "search_delta_q_threshold";
pub const PARAM_DEPTH_DECAY_FACTOR: &str = "search_depth_decay_factor";
pub const PARAM_USE_ASPIRATION: &str = "search_use_aspiration";
pub const PARAM_MULTIPLICATIVE_EVAL: &str = "search_multiplicative_eval";
pub const PARAM_RELATIVE_HISTORY: &str = "search_relative_history";

const LOG_RATIO_SCALE: f32 = 4096.0;
const LOG_RATIO_BASE: f32 = 1.0;

pub fn score_to_ratio(score: i32) -> f64 {
    ((score as f64) / (LOG_RATIO_SCALE as f64)).exp()
}

pub static SEARCH_PARAMETER_DEFS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_USE_NEW_SEARCH,
        "Use Improved Search",
        "Master switch. ON = improved search with countermove heuristic, ordered/pruned quiescence, aspiration windows, and access to advanced toggles. OFF = original simple alpha-beta.",
        0.0, 1.0, 1.0, 1.0,
    ),
ParameterDef::new(
    PARAM_BEST_FIRST_ORDERING,
    "Best-First Ordering",
    "Evaluate every move with the full evaluator before deciding order. Expensive per node, near-optimal ordering.",
    0.0, 1.0, 0.0, 1.0,
),
ParameterDef::new(
    PARAM_BEST_FIRST_SEARCH,
    "Best-First Search",
    "Use a memory-hungry best-first tree search instead of alpha-beta. Requires Best-First Ordering.",
    0.0, 1.0, 0.0, 1.0,
),
ParameterDef::new(
    PARAM_BFS_MAX_NODES,
    "BFS Max Nodes",
    "Maximum node expansions for best-first search. ~6 KB of memory per expansion.",
    1000.0, 5_000_000.0, 50_000.0, 1000.0,
),
ParameterDef::new(
    PARAM_BFS_MAX_DEPTH,
    "BFS Max Depth",
    "Maximum tree depth in best-first search.",
    4.0, 256.0, 64.0, 1.0,
),
ParameterDef::new(
    PARAM_BFS_COMPACT_MOVES,
    "BFS Compact Moves",
    "Don't store full move data in BFS tree; regenerate at replay time. Cuts memory ~80% at modest CPU cost.",
    0.0, 1.0, 0.0, 1.0,
),
ParameterDef::new(
    PARAM_DELTA_QUIESCENCE,
    "Delta Quiescence",
    "Quiescence searches by eval-gain threshold rather than capture detection.",
    0.0, 1.0, 0.0, 1.0,
),
ParameterDef::new(
    PARAM_DELTA_QUIESCENCE_THRESHOLD,
    "Delta Q Threshold",
    "Minimum eval-gain (in this engine's units) to search a move in delta quiescence mode.",
                  0.0, 5000.0, 50.0, 1.0,
),
ParameterDef::new(
    PARAM_DEPTH_DECAY_FACTOR,
    "Depth Decay Factor",
    "Per-ply leaf-eval multiplier. 1.0 = off. 0.995 = mild preference for quick wins. 0.98 = strong. Applied ONCE at leaves based on ply.",
    0.90, 1.0, 1.0, 0.001,
),
ParameterDef::new(
    PARAM_USE_ASPIRATION,
    "Aspiration Windows",
    "After depth >= 3, re-root alpha-beta with a narrow window around the previous iteration's score. Window width is queried from the evaluator.",
    0.0, 1.0, 1.0, 1.0,
),
ParameterDef::new(
    PARAM_MULTIPLICATIVE_EVAL,
    "Multiplicative Eval",
    "Absolute/ratio mode: score each side independently and optimise the log-ratio (own/opp) instead of the centipawn difference. Needs a split-capable evaluator.",
                  0.0, 1.0, 0.0, 1.0,
),
ParameterDef::new(
    PARAM_RELATIVE_HISTORY,
    "Relative History",
    "Butterfly/relative history: order quiet moves by cutoff-success RATIO (good/tried) using flat cache-friendly tables.",
                  0.0, 1.0, 0.0, 1.0,
),
];

pub fn combined_params(
    engine_params: &'static [ParameterDef],
    cache: &'static OnceLock<Vec<ParameterDef>>,
) -> &'static [ParameterDef] {
    cache
    .get_or_init(|| {
        let mut v = Vec::with_capacity(engine_params.len() + SEARCH_PARAMETER_DEFS.len());
        v.extend_from_slice(engine_params);
        v.extend_from_slice(SEARCH_PARAMETER_DEFS);
        v
    })
    .as_slice()
}

// ─────────────────────────────────────────────────────────────────────────
// SearchConfig
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SearchConfig {
    pub best_first_ordering: bool,
    pub best_first_search: bool,
    pub bfs_max_nodes: usize,
    pub bfs_max_depth: u32,
    pub bfs_compact_moves: bool,
    pub delta_quiescence: bool,
    pub delta_quiescence_threshold: i32,
    pub depth_decay_factor: f32,
    pub use_aspiration: bool,
    pub multiplicative_eval: bool,
    pub relative_history: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            best_first_ordering: false,
            best_first_search: false,
            bfs_max_nodes: 50_000,
            bfs_max_depth: 64,
            bfs_compact_moves: false,
            delta_quiescence: false,
            delta_quiescence_threshold: 50,
            depth_decay_factor: 1.0,
            use_aspiration: true,
            multiplicative_eval: false,
            relative_history: false,
        }
    }
}

impl SearchConfig {
    pub fn from_params(params: &EngineParameters) -> Self {
        Self {
            best_first_ordering: params.get_or_default(PARAM_BEST_FIRST_ORDERING, 0.0) > 0.5,
            best_first_search: params.get_or_default(PARAM_BEST_FIRST_SEARCH, 0.0) > 0.5,
            bfs_max_nodes: params.get_or_default(PARAM_BFS_MAX_NODES, 50_000.0) as usize,
            bfs_max_depth: params.get_or_default(PARAM_BFS_MAX_DEPTH, 64.0) as u32,
            bfs_compact_moves: params.get_or_default(PARAM_BFS_COMPACT_MOVES, 0.0) > 0.5,
            delta_quiescence: params.get_or_default(PARAM_DELTA_QUIESCENCE, 0.0) > 0.5,
            delta_quiescence_threshold: params
            .get_or_default(PARAM_DELTA_QUIESCENCE_THRESHOLD, 50.0) as i32,
            depth_decay_factor: params.get_or_default(PARAM_DEPTH_DECAY_FACTOR, 1.0) as f32,
            use_aspiration: params.get_or_default(PARAM_USE_ASPIRATION, 1.0) > 0.5,
            multiplicative_eval: params.get_or_default(PARAM_MULTIPLICATIVE_EVAL, 0.0) > 0.5,
            relative_history: params.get_or_default(PARAM_RELATIVE_HISTORY, 0.0) > 0.5,
        }
    }

    pub fn use_new_search(params: &EngineParameters) -> bool {
        params.get_or_default(PARAM_USE_NEW_SEARCH, 1.0) > 0.5
    }
}

// ─────────────────────────────────────────────────────────────────────────
// BFS internals
// ─────────────────────────────────────────────────────────────────────────

#[repr(C)]
struct BfsNode {
    score: i32,
    edges_start: u32,
    edges_len: u16,
    depth: u16,
    expanded: bool,
}

struct BfsEdgeFull {
    child_idx: u32,
    mv: ExpandedMove,
}

#[derive(Clone, Copy)]
struct BfsEdgeCompact {
    child_idx: u32,
    from_packed: u16,
    to_packed: u16,
    promotion: u16,
    castling_rook_from: u16,
}

enum BfsEdges {
    Full(Vec<BfsEdgeFull>),
    Compact(Vec<BfsEdgeCompact>),
}

impl BfsEdges {
    fn len(&self) -> usize {
        match self {
            BfsEdges::Full(v) => v.len(),
            BfsEdges::Compact(v) => v.len(),
        }
    }
    fn child_idx(&self, i: usize) -> u32 {
        match self {
            BfsEdges::Full(v) => v[i].child_idx,
            BfsEdges::Compact(v) => v[i].child_idx,
        }
    }
}

struct RelHistory {
    cols: usize,
    n: usize,
    good: Vec<i32>,
    tried: Vec<i32>,
}

impl RelHistory {
    fn new(rows: usize, cols: usize) -> Option<Self> {
        let n = rows.checked_mul(cols)?;
        if n == 0 || n > 256 {
            return None;
        }
        Some(Self { cols, n, good: vec![0; n * n], tried: vec![0; n * n] })
    }
    #[inline(always)]
    fn idx(&self, from: (usize, usize), to: (usize, usize)) -> usize {
        (from.0 * self.cols + from.1) * self.n + (to.0 * self.cols + to.1)
    }
    #[inline(always)]
    fn record_tried(&mut self, from: (usize, usize), to: (usize, usize)) {
        let i = self.idx(from, to);
        let v = unsafe { self.tried.get_unchecked_mut(i) };
        *v = v.saturating_add(1);
    }
    #[inline(always)]
    fn record_good(&mut self, from: (usize, usize), to: (usize, usize), depth: u32) {
        let i = self.idx(from, to);
        let v = unsafe { self.good.get_unchecked_mut(i) };
        *v = v.saturating_add((depth * depth) as i32);
    }
    #[inline(always)]
    fn score(&self, from: (usize, usize), to: (usize, usize)) -> i32 {
        let i = self.idx(from, to);
        let g = unsafe { *self.good.get_unchecked(i) } as i64;
        let t = unsafe { *self.tried.get_unchecked(i) } as i64;
        ((g * 1024) / (t + 1)) as i32
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The search engine
// ─────────────────────────────────────────────────────────────────────────

pub struct Search<'a, E: EvaluatorTrait> {
    evaluator: &'a E,
    config: SearchConfig,
    improvements: bool,
    transposition_table: HashMap<u64, TTEntry>,
    killer_moves: Vec<Vec<(usize, usize)>>,
    history_table: HashMap<(usize, usize), i32>,
    countermove_table: HashMap<(usize, usize), (usize, usize)>,
    nodes_searched: usize,
    time_limit: Option<Instant>,
    should_stop: bool,
    can_timeout: bool,
    soft_time_limit: Option<Instant>,
    rel_history: Option<RelHistory>,
    max_q_depth: usize,
    max_bfs_depth: u32,
    root_side: PieceColor,
    contempt: i32,
    node_drawish: bool,
}

impl<'a, E: EvaluatorTrait> Search<'a, E> {
    pub fn new(evaluator: &'a E) -> Self {
        let mut s = Self::with_config(evaluator, SearchConfig::default());
        s.improvements = false;
        s
    }

    pub fn with_config(evaluator: &'a E, config: SearchConfig) -> Self {
        Search {
            evaluator,
            config,
            improvements: true,
            transposition_table: HashMap::with_capacity(1_000_000),
            killer_moves: vec![Vec::new(); 64],
            history_table: HashMap::new(),
            countermove_table: HashMap::new(),
            nodes_searched: 0,
            time_limit: None,
            should_stop: false,
            can_timeout: true,
            soft_time_limit: None,
            rel_history: None,
            max_q_depth: 0,
            max_bfs_depth: 0,
            root_side: PieceColor::White,
            contempt: 0,
            node_drawish: false,
        }
    }

    pub fn set_transposition_table(&mut self, table: HashMap<u64, TTEntry>) {
        self.transposition_table = table;
    }

    pub fn get_transposition_table(&self) -> HashMap<u64, TTEntry> {
        self.transposition_table.clone()
    }

    pub fn take_transposition_table(&mut self) -> HashMap<u64, TTEntry> {
        std::mem::take(&mut self.transposition_table)
    }

    // ─── Entry points ───────────────────────────────────────────────────

    pub fn find_best_move_with_depth(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
    ) -> Option<(ExpandedMove, i32, u32)> {
        self.nodes_searched = 0;
        self.should_stop = false;
        self.time_limit = None;
        self.can_timeout = true;
        self.soft_time_limit = None;
        self.max_q_depth = 0;
        self.max_bfs_depth = 0;
        self.root_side = state.current_turn;
        self.contempt = self.evaluator.contempt();
        self.node_drawish = false;

        self.rel_history = if self.improvements && self.config.relative_history {
            let (rows, cols) = state.board.size();
            RelHistory::new(rows, cols)
        } else {
            None
        };

        if self.improvements && self.config.best_first_search && self.config.best_first_ordering {
            let r = self.best_first_search(state, move_generator, config_manager)?;
            println!("BFS nodes: {}, max BFS depth: {}", self.nodes_searched, self.max_bfs_depth);
            return Some((r.0, r.1, 0));
        }

        let mut best_move = None;
        let mut best_score = ALPHA_BETA_MIN;
        let mut depth_reached = 0;

        for d in 1..=depth {
            if self.should_stop {
                break;
            }
            self.max_q_depth = 0;

            if let Some((mv, score)) = self.search_root_window(
                state, move_generator, config_manager, d, ALPHA_BETA_MIN, ALPHA_BETA_MAX,
            ) {
                best_move = Some(mv);
                best_score = score;
                depth_reached = d;
                println!("Depth {} (q={}) score={} nodes={}", d, self.max_q_depth, score, self.nodes_searched);
                if score.abs() > MATE_THRESHOLD {
                    break;
                }
            } else {
                break;
            }
        }

        println!(
            "Total nodes: {}, AB depth: {}, max Q depth: {}",
            self.nodes_searched, depth_reached, self.max_q_depth
        );
        best_move.map(|mv| (mv, best_score, depth_reached))
    }

    pub fn find_best_move_iterative(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        max_depth: u32,
        target_time: Duration,
    ) -> Option<(ExpandedMove, i32, u32)> {
        let start_time = Instant::now();
        self.nodes_searched = 0;
        self.should_stop = false;
        self.can_timeout = false;

        let hard_limit = target_time.mul_f32(3.0);
        self.time_limit = Some(start_time + hard_limit);
        self.soft_time_limit = Some(start_time + target_time);
        self.max_q_depth = 0;
        self.max_bfs_depth = 0;
        self.root_side = state.current_turn;
        self.contempt = self.evaluator.contempt();
        self.node_drawish = false;

        self.rel_history = if self.improvements && self.config.relative_history {
            let (rows, cols) = state.board.size();
            RelHistory::new(rows, cols)
        } else {
            None
        };

        if self.improvements && self.config.best_first_search && self.config.best_first_ordering {
            self.can_timeout = true;
            let r = self.best_first_search(state, move_generator, config_manager)?;
            println!(
                "BFS nodes: {}, max BFS depth: {}, time: {:?}",
                self.nodes_searched, self.max_bfs_depth, start_time.elapsed()
            );
            return Some((r.0, r.1, 0));
        }

        let mut best_move = None;
        let mut best_score = 0i32;
        let mut depth_reached = 0;
        let mut last_depth_time = Duration::from_millis(1);
        let mut branch_factor = 3.5;

        let aspiration_window = if self.improvements && self.config.use_aspiration {
            if self.config.multiplicative_eval {
                (LOG_RATIO_SCALE as i32) / 8
            } else {
                self.evaluator.aspiration_window()
            }
            .max(0)
        } else {
            0
        };

        for d in 1..=max_depth {
            if d > 1 {
                self.can_timeout = true;
            }

            if d > 1 {
                if let Some(soft) = self.soft_time_limit {
                    if Instant::now() >= soft {
                        println!("⏱️ Soft limit reached; not starting depth {} (have {})", d, depth_reached);
                        break;
                    }
                }
            }

            let elapsed = start_time.elapsed();
            if d > 2 {
                let expected = last_depth_time.mul_f64(branch_factor);
                let est_total = elapsed + expected;
                let acceptable = target_time.mul_f64(2.0);
                if est_total > acceptable {
                    println!(
                        "⏱️ Branching-factor projection halts before depth {}. Expected {:.2}s (acceptable: {:.2}s)",
                             d, est_total.as_secs_f32(), acceptable.as_secs_f32()
                    );
                    break;
                }
            }

            self.max_q_depth = 0;
            let depth_start = Instant::now();

            let result = if d > 2 && aspiration_window > 0 && best_score.abs() < MATE_THRESHOLD {
                self.search_root_aspiration(state, move_generator, config_manager, d, best_score, aspiration_window)
            } else {
                self.search_root_window(state, move_generator, config_manager, d, ALPHA_BETA_MIN, ALPHA_BETA_MAX)
            };

            if let Some((mv, score)) = result {
                if !self.should_stop {
                    best_move = Some(mv);
                    best_score = score;
                    depth_reached = d;
                    let cdt = depth_start.elapsed();
                    if d > 1 && last_depth_time.as_millis() > 0 {
                        let bf = cdt.as_secs_f64() / last_depth_time.as_secs_f64();
                        branch_factor = (branch_factor * 0.85) + (bf * 0.15);
                    }
                    last_depth_time = cdt;
                    println!(
                        "Depth {} (q={}) completed: score={}, dt={:.2}s, total={:?}",
                             d, self.max_q_depth, score, cdt.as_secs_f32(), start_time.elapsed()
                    );
                    if score.abs() > MATE_THRESHOLD {
                        break;
                    }
                } else {
                    println!("Depth {} aborted (hard limit {:.2}s).", d, hard_limit.as_secs_f32());
                }
            } else {
                break;
            }
        }

        println!(
            "Total nodes: {}, AB depth: {}, max Q depth (last iter): {}, time: {:?}",
                 self.nodes_searched, depth_reached, self.max_q_depth, start_time.elapsed()
        );
        best_move.map(|mv| (mv, best_score, depth_reached))
    }

    // ─── Time / decay helpers ───────────────────────────────────────────

    #[inline(always)]
    fn check_time(&mut self) {
        if !self.can_timeout {
            return;
        }
        if let Some(deadline) = self.time_limit {
            if Instant::now() >= deadline {
                self.should_stop = true;
            }
        }
    }

    #[inline(always)]
    fn hard_deadline_hit(&self) -> bool {
        match self.time_limit {
            Some(deadline) => Instant::now() >= deadline,
            None => false,
        }
    }

    #[inline(always)]
    fn apply_leaf_decay(&self, score: i32, ply: usize) -> i32 {
        if !self.improvements || self.config.depth_decay_factor >= 1.0 {
            return score;
        }
        if score.abs() > MATE_THRESHOLD {
            return score;
        }
        let factor = (self.config.depth_decay_factor as f64).powi(ply as i32);
        (score as f64 * factor) as i32
    }

    #[inline(always)]
    fn draw_score(&self, state: &GameState) -> i32 {
        if self.contempt == 0 {
            return 0;
        }
        if state.current_turn == self.root_side {
            -self.contempt
        } else {
            self.contempt
        }
    }

    // ─── Root search ────────────────────────────────────────────────────

    fn search_root_aspiration(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
        expected: i32,
        initial_window: i32,
    ) -> Option<(ExpandedMove, i32)> {
        let mut alpha = expected.saturating_sub(initial_window).max(ALPHA_BETA_MIN);
        let mut beta = expected.saturating_add(initial_window).min(ALPHA_BETA_MAX);
        let mut delta = initial_window;
        let mut last_result: Option<(ExpandedMove, i32)> = None;

        for _ in 0..5 {
            if self.should_stop {
                return last_result;
            }
            let result = self.search_root_window(state, move_generator, config_manager, depth, alpha, beta);
            match result {
                Some((mv, score)) => {
                    last_result = Some((mv.clone(), score));
                    if score <= alpha {
                        delta = delta.saturating_mul(2);
                        alpha = alpha.saturating_sub(delta).max(ALPHA_BETA_MIN);
                    } else if score >= beta {
                        delta = delta.saturating_mul(2);
                        beta = beta.saturating_add(delta).min(ALPHA_BETA_MAX);
                    } else {
                        return Some((mv, score));
                    }
                }
                None => return None,
            }
        }

        self.search_root_window(state, move_generator, config_manager, depth, ALPHA_BETA_MIN, ALPHA_BETA_MAX)
        .or(last_result)
    }

    fn search_root_window(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
        alpha_init: i32,
        beta: i32,
    ) -> Option<(ExpandedMove, i32)> {
        let mut moves = state.get_legal_moves(move_generator, config_manager);
        if moves.is_empty() {
            return None;
        }

        self.order_moves(state, &mut moves, 0, config_manager, move_generator, None);

        let mut best_move = None;
        let mut best_score = ALPHA_BETA_MIN;
        let mut alpha = alpha_init;

        for mv in &mut moves {
            state.execute_expanded_move(mv, move_generator, config_manager);
            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }

            let mk = (mv.from.0 * 16 + mv.from.1, mv.to.0 * 16 + mv.to.1);
            let score = -self.alpha_beta(state, move_generator, config_manager, depth - 1, -beta, -alpha, 1, Some(mk));
            state.undo_move(config_manager);

            if self.should_stop {
                break;
            }

            if score > best_score {
                best_score = score;
                best_move = Some(mv.clone());
                if score > alpha {
                    alpha = score;
                }
            }
            if alpha >= beta {
                break;
            }
        }

        best_move.map(|mv| (mv, best_score))
    }

    // ─── Alpha-beta ─────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn alpha_beta(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        last_move: Option<(usize, usize)>,
    ) -> i32 {
        self.nodes_searched += 1;
        if (self.nodes_searched & 2047) == 0 {
            self.check_time();
            if self.should_stop {
                self.node_drawish = false;
                return 0;
            }
        }

        let hash = state.current_hash();

        if matches!(state.game_result, Some(GameResult::Draw(_))) {
            self.node_drawish = true;
            return self.draw_score(state);
        }
        if state.fifty_move_counter >= 4
            && state.position_history.get(&hash).copied().unwrap_or(0) >= 2
            {
                self.node_drawish = true;
                return self.draw_score(state);
            }

            if let Some(&entry) = self.transposition_table.get(&hash) {
                if entry.hash == hash && entry.depth >= depth {
                    match entry.flag {
                        TTFlag::Exact => {
                            self.node_drawish = false;
                            return entry.score;
                        }
                        TTFlag::LowerBound => {
                            if entry.score >= beta {
                                self.node_drawish = false;
                                return entry.score;
                            }
                            alpha = alpha.max(entry.score);
                        }
                        TTFlag::UpperBound => {
                            if entry.score <= alpha {
                                self.node_drawish = false;
                                return entry.score;
                            }
                        }
                    }
                }
            }

            if depth == 0 {
                if self.improvements && self.config.delta_quiescence {
                    return self.delta_quiescence_search(state, move_generator, config_manager, alpha, beta, ply, 0);
                }
                return self.quiescence(state, move_generator, config_manager, alpha, beta, ply, 0);
            }

            let mut moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
                MoveGenerationResult::Checkmate { .. } => {
                    self.node_drawish = false;
                    return 999999 - ply as i32;
                }
                MoveGenerationResult::Moves(m) => m,
            };

            if moves.is_empty() {
                self.node_drawish = false;
                if state.is_in_check_fast(move_generator) {
                    return -999999 + ply as i32;
                }
                if !state.board.has_pieces(state.current_turn) {
                    return -999999 + ply as i32;
                }
                return 0;
            }

            if depth > 2 && !state.is_in_check_fast(move_generator) {
                state.current_turn = state.current_turn.opposite();
                let null_score = -self.alpha_beta(state, move_generator, config_manager, depth - 3, -beta, -beta + 1, ply + 1, None);
                state.current_turn = state.current_turn.opposite();

                if self.should_stop {
                    self.node_drawish = false;
                    return 0;
                }
                if null_score >= beta {
                    self.node_drawish = false;
                    return beta;
                }
            }

            self.order_moves(state, &mut moves, ply, config_manager, move_generator, last_move);

            let mut best_score = ALPHA_BETA_MIN;
            let mut best_move_key = None;
            let mut best_is_drawish = false;
            let mut legal_count: u32 = 0;
            let original_alpha = alpha;

            for mv in &mut moves {
                state.execute_expanded_move(mv, move_generator, config_manager);
                if state.mover_king_in_check(move_generator) {
                    state.undo_move(config_manager);
                    continue;
                }
                legal_count += 1;

                let mk = (mv.from.0 * 16 + mv.from.1, mv.to.0 * 16 + mv.to.1);

                if self.improvements && mv.captures.is_none() {
                    if let Some(rh) = self.rel_history.as_mut() {
                        rh.record_tried(mv.from, mv.to);
                    }
                }

                let score = if legal_count == 1 {
                    -self.alpha_beta(state, move_generator, config_manager, depth - 1, -beta, -alpha, ply + 1, Some(mk))
                } else {
                    let reduction = if depth > 3 && legal_count > 3 && mv.captures.is_none() { 1 } else { 0 };
                    let mut s = -self.alpha_beta(state, move_generator, config_manager, depth - 1 - reduction, -alpha - 1, -alpha, ply + 1, Some(mk));
                    if !self.should_stop && s > alpha && s < beta {
                        s = -self.alpha_beta(state, move_generator, config_manager, depth - 1, -beta, -alpha, ply + 1, Some(mk));
                    }
                    s
                };

                let child_drawish = self.node_drawish;
                state.undo_move(config_manager);
                if self.should_stop {
                    self.node_drawish = false;
                    return 0;
                }

                if score > best_score {
                    best_score = score;
                    best_move_key = Some(mk);
                    best_is_drawish = child_drawish;
                }
                alpha = alpha.max(best_score);

                if alpha >= beta {
                    if mv.captures.is_none() {
                        if ply < self.killer_moves.len() && !self.killer_moves[ply].contains(&mk) {
                            self.killer_moves[ply].insert(0, mk);
                            self.killer_moves[ply].truncate(2);
                        }
                        if let Some(rh) = self.rel_history.as_mut() {
                            rh.record_good(mv.from, mv.to, depth);
                        } else {
                            *self.history_table.entry(mk).or_insert(0) += (depth * depth) as i32;
                        }
                    }
                    if self.improvements {
                        if let Some(prev) = last_move {
                            self.countermove_table.insert(prev, mk);
                        }
                    }
                    if !self.should_stop && !child_drawish {
                        self.store_tt(hash, depth, beta, TTFlag::LowerBound, best_move_key);
                    }
                    self.node_drawish = child_drawish;
                    return beta;
                }
            }

            if legal_count == 0 {
                self.node_drawish = false;
                if state.is_in_check_fast(move_generator) {
                    return -999999 + ply as i32;
                }
                if !state.board.has_pieces(state.current_turn) {
                    return -999999 + ply as i32;
                }
                return 0;
            }

            if !self.should_stop && !best_is_drawish {
                let flag = if best_score <= original_alpha { TTFlag::UpperBound } else { TTFlag::Exact };
                self.store_tt(hash, depth, best_score, flag, best_move_key);
            }
            self.node_drawish = best_is_drawish;
            best_score
    }

    // ─── Quiescence ─────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn quiescence(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        q_depth: u32,
    ) -> i32 {
        self.nodes_searched += 1;
        self.node_drawish = false;

        if q_depth as usize > self.max_q_depth {
            self.max_q_depth = q_depth as usize;
        }

        if (self.nodes_searched & 2047) == 0 {
            self.check_time();
            if self.should_stop {
                return alpha;
            }
        }

        let (raw, split_opp) = self.leaf_evaluate(state, move_generator, config_manager);
        let stand_pat = self.apply_leaf_decay(raw, ply);

        if stand_pat >= beta {
            return beta;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }

        if q_depth >= QUIESCENCE_MAX_PLY {
            return alpha;
        }
        if (self.nodes_searched & 255) == 0 && self.hard_deadline_hit() {
            return alpha;
        }

        let moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Checkmate { .. } => return 999999 - ply as i32,
            MoveGenerationResult::Moves(m) => m,
        };

        let mut captures: Vec<_> = moves.into_iter().filter(|m| m.captures.is_some()).collect();

        if self.improvements && !captures.is_empty() {
            captures.sort_by_cached_key(|m| {
                let victim = m.captures.unwrap();
                let victim_value = self.evaluator.get_piece_value_on_square(
                    &victim,
                    m.captures_position.unwrap_or(m.to),
                                                                            config_manager,
                );
                let attacker_value = state
                .board
                .get_piece(m.from)
                .map(|a| self.evaluator.get_piece_value_on_square(&a, m.from, config_manager))
                .unwrap_or(0);
                -(victim_value * 10 - attacker_value)
            });
        }

        let use_delta_pruning = self.improvements;
        let margin = if use_delta_pruning { self.evaluator.delta_pruning_margin() } else { 0 };

        for mv in &captures {
            if use_delta_pruning {
                let captured = mv.captures.unwrap();
                let mut capture_value = self.evaluator.get_piece_value_on_square(
                    &captured,
                    mv.captures_position.unwrap_or(mv.to),
                                                                                 config_manager,
                );
                if let Some(opp) = split_opp {
                    let new_opp = (opp - capture_value as f32).max(0.0);
                    let p_old = (opp + LOG_RATIO_BASE).ln();
                    let p_new = (new_opp + LOG_RATIO_BASE).ln();
                    capture_value = (LOG_RATIO_SCALE * (p_old - p_new)) as i32;
                }
                let projected_score = stand_pat.saturating_add(capture_value).saturating_add(margin);
                if projected_score < alpha {
                    continue;
                }
            }

            state.execute_expanded_move(mv, move_generator, config_manager);
            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }

            let score = -self.quiescence(state, move_generator, config_manager, -beta, -alpha, ply + 1, q_depth + 1);
            state.undo_move(config_manager);

            if self.should_stop {
                return alpha;
            }
            if score >= beta {
                return beta;
            }
            if score > alpha {
                alpha = score;
            }
        }

        alpha
    }

    #[allow(clippy::too_many_arguments)]
    fn delta_quiescence_search(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        q_depth: u32,
    ) -> i32 {
        self.nodes_searched += 1;
        self.node_drawish = false;

        if q_depth as usize > self.max_q_depth {
            self.max_q_depth = q_depth as usize;
        }

        if (self.nodes_searched & 2047) == 0 {
            self.check_time();
            if self.should_stop {
                return alpha;
            }
        }

        let (raw, _) = self.leaf_evaluate(state, move_generator, config_manager);
        let stand_pat = self.apply_leaf_decay(raw, ply);
        if stand_pat >= beta {
            return beta;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }

        let moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Checkmate { .. } => return 999999 - ply as i32,
            MoveGenerationResult::Moves(m) => m,
        };

        let threshold = self.config.delta_quiescence_threshold;
        let mut scored: Vec<(ExpandedMove, i32)> = Vec::with_capacity(moves.len() / 2);

        for mv in moves {
            if let Some(captured) = mv.captures {
                let cv = self.evaluator.get_piece_value_on_square(
                    &captured,
                    mv.captures_position.unwrap_or(mv.to),
                                                                  config_manager,
                );
                if cv >= threshold {
                    scored.push((mv, cv));
                    continue;
                }
            }
            let delta = self.evaluator.calculate_delta(state, &mv, move_generator, config_manager);
            if delta >= threshold {
                scored.push((mv, delta));
            }
        }

        scored.sort_by(|a, b| b.1.cmp(&a.1));

        for (mv, _) in &scored {
            state.execute_expanded_move(mv, move_generator, config_manager);
            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }
            let score = -self.delta_quiescence_search(state, move_generator, config_manager, -beta, -alpha, ply + 1, q_depth + 1);
            state.undo_move(config_manager);

            if self.should_stop {
                return alpha;
            }
            if score >= beta {
                return beta;
            }
            if score > alpha {
                alpha = score;
            }
        }

        alpha
    }

    // ─── Move ordering ──────────────────────────────────────────────────

    fn order_moves(
        &mut self,
        state: &mut GameState,
        moves: &mut [ExpandedMove],
        ply: usize,
        config_manager: &PieceConfigManager,
        move_generator: &MoveGenerator,
        last_move: Option<(usize, usize)>,
    ) {
        if self.improvements && self.config.best_first_ordering {
            self.order_moves_evaluated(state, moves, config_manager, move_generator);
            return;
        }

        let hash = state.current_hash();
        let tt_move = self.transposition_table.get(&hash).and_then(|e| e.best_move);
        let countermove = if self.improvements {
            last_move.and_then(|lm| self.countermove_table.get(&lm).copied())
        } else {
            None
        };

        let killers = &self.killer_moves;
        let history = &self.history_table;
        let evaluator = self.evaluator;
        let improvements = self.improvements;
        let rel_hist = self.rel_history.as_ref();

        moves.sort_by_cached_key(|m| {
            let f = m.from.0 * 16 + m.from.1;
            let t = m.to.0 * 16 + m.to.1;

            if let Some((tf, tt)) = tt_move {
                if f == tf && t == tt {
                    return -2_000_000_000i32;
                }
            }
            if m.promotion_target.is_some() {
                return -1_900_000_000;
            }

            if let Some(captured) = m.captures {
                if let Some(attacker) = state.board.get_piece(m.from) {
                    let v = evaluator.get_piece_value_on_square(&captured, m.captures_position.unwrap_or(m.to), config_manager);
                    let a = evaluator.get_piece_value_on_square(&attacker, m.from, config_manager);
                    return -1_800_000_000 - (v * 10) + a;
                }
            }

            if ply < killers.len() {
                for (i, k) in killers[ply].iter().enumerate() {
                    if (f, t) == *k {
                        return -1_700_000_000 + i as i32;
                    }
                }
            }

            if improvements {
                if let Some((cf, ct)) = countermove {
                    if f == cf && t == ct {
                        return -1_600_000_000;
                    }
                }
            }

            if let Some(rh) = rel_hist {
                -rh.score(m.from, m.to)
            } else {
                -history.get(&(f, t)).copied().unwrap_or(0)
            }
        });
    }

    fn order_moves_evaluated(
        &self,
        state: &mut GameState,
        moves: &mut [ExpandedMove],
        config_manager: &PieceConfigManager,
        move_generator: &MoveGenerator,
    ) {
        let hash = state.current_hash();
        let tt_move = self.transposition_table.get(&hash).and_then(|e| e.best_move);

        let mut scores: Vec<i32> = Vec::with_capacity(moves.len());
        for mv in moves.iter() {
            let f = mv.from.0 * 16 + mv.from.1;
            let t = mv.to.0 * 16 + mv.to.1;
            if let Some((tf, tt)) = tt_move {
                if f == tf && t == tt {
                    scores.push(i32::MAX);
                    continue;
                }
            }
            state.execute_expanded_move(mv, move_generator, config_manager);
            let s = -self.evaluator.evaluate(state, move_generator, config_manager);
            state.undo_move(config_manager);
            scores.push(s);
        }

        let mut idx: Vec<usize> = (0..moves.len()).collect();
        idx.sort_by(|&a, &b| scores[b].cmp(&scores[a]));
        let original: Vec<ExpandedMove> = moves.to_vec();
        for (dst, &src) in idx.iter().enumerate() {
            moves[dst] = original[src].clone();
        }
    }

    // ─── Best-first search ──────────────────────────────────────────────

    fn best_first_search(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<(ExpandedMove, i32)> {
        let max_nodes = self.config.bfs_max_nodes;
        let max_depth = self.config.bfs_max_depth.min(u16::MAX as u32) as u16;
        let compact = self.config.bfs_compact_moves;

        let mut nodes: Vec<BfsNode> = Vec::with_capacity(max_nodes);
        let mut edges: BfsEdges = if compact {
            BfsEdges::Compact(Vec::with_capacity(max_nodes * 20))
        } else {
            BfsEdges::Full(Vec::with_capacity(max_nodes * 20))
        };

        let (root_raw, _) = self.leaf_evaluate(state, move_generator, config_manager);
        let root_eval = self.apply_leaf_decay(root_raw, 0);
        nodes.push(BfsNode { score: root_eval, edges_start: 0, edges_len: 0, depth: 0, expanded: false });

        self.bfs_expand(state, move_generator, config_manager, &mut nodes, &mut edges, 0);
        if nodes[0].edges_len == 0 {
            return None;
        }

        let bfs_start = Instant::now();
        let min_expansions_before_soft_timeout: usize = 64;
        let mut expansions = 1usize;
        let mut path_buf: Vec<u32> = Vec::with_capacity(max_depth as usize + 2);

        while expansions < max_nodes {
            if self.should_stop {
                break;
            }
            if (expansions & 1023) == 0 {
                self.check_time();
                if self.should_stop {
                    break;
                }
                if expansions >= min_expansions_before_soft_timeout {
                    if let Some(soft) = self.soft_time_limit {
                        if Instant::now() >= soft {
                            println!("⏱️ BFS soft timeout at {} expansions ({:?} elapsed).", expansions, bfs_start.elapsed());
                            break;
                        }
                    }
                }
            }

            path_buf.clear();
            if !Self::find_pv_leaf(&nodes, &edges, max_depth, &mut path_buf) {
                break;
            }
            let leaf = *path_buf.last().unwrap();

            if nodes[leaf as usize].depth >= max_depth {
                nodes[leaf as usize].expanded = true;
                continue;
            }

            for w in path_buf.windows(2) {
                let parent = w[0] as usize;
                let child = w[1];
                let n_edges = nodes[parent].edges_len as usize;
                let start = nodes[parent].edges_start as usize;
                let mut found = false;
                for i in 0..n_edges {
                    match &edges {
                        BfsEdges::Full(v) => {
                            if v[start + i].child_idx == child {
                                state.execute_expanded_move(&v[start + i].mv, move_generator, config_manager);
                                found = true;
                                break;
                            }
                        }
                        BfsEdges::Compact(v) => {
                            if v[start + i].child_idx == child {
                                let ce = v[start + i];
                                self.replay_compact_move(state, move_generator, config_manager, ce);
                                found = true;
                                break;
                            }
                        }
                    }
                }
                debug_assert!(found);
            }

            self.bfs_expand(state, move_generator, config_manager, &mut nodes, &mut edges, leaf);
            expansions += 1;

            for _ in 1..path_buf.len() {
                state.undo_move(config_manager);
            }

            for &n in path_buf.iter().rev() {
                let n = n as usize;
                if nodes[n].edges_len == 0 {
                    continue;
                }
                let s = nodes[n].edges_start as usize;
                let l = nodes[n].edges_len as usize;
                let mut best = i32::MIN;
                for i in 0..l {
                    let ci = edges.child_idx(s + i) as usize;
                    let v = -nodes[ci].score;
                    if v > best {
                        best = v;
                    }
                }
                nodes[n].score = best;
            }
        }

        let s = nodes[0].edges_start as usize;
        let l = nodes[0].edges_len as usize;
        if l == 0 {
            return None;
        }

        let mut best_idx = 0usize;
        let mut best_score = i32::MAX;
        for i in 0..l {
            let ci = edges.child_idx(s + i) as usize;
            if nodes[ci].score < best_score {
                best_score = nodes[ci].score;
                best_idx = i;
            }
        }

        let mv = match &edges {
            BfsEdges::Full(v) => v[s + best_idx].mv.clone(),
            BfsEdges::Compact(v) => {
                let ce = v[s + best_idx];
                self.compact_to_expanded(state, move_generator, ce)?
            }
        };

        Some((mv, -best_score))
    }

    fn bfs_expand(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        nodes: &mut Vec<BfsNode>,
        edges: &mut BfsEdges,
        node_idx: u32,
    ) {
        let nidx = node_idx as usize;
        let parent_depth = nodes[nidx].depth;
        nodes[nidx].expanded = true;

        let child_ply = (parent_depth as usize) + 1;
        if (child_ply as u32) > self.max_bfs_depth {
            self.max_bfs_depth = child_ply as u32;
        }

        let moves = state.get_legal_moves(move_generator, config_manager);
        self.nodes_searched += moves.len();

        if moves.is_empty() {
            let score = if state.is_in_check_fast(move_generator) {
                -999999 + parent_depth as i32
            } else if !state.board.has_pieces(state.current_turn) {
                -999999 + parent_depth as i32
            } else {
                0
            };
            nodes[nidx].score = score;
            return;
        }

        let start = edges.len() as u32;
        let mut len = 0u16;

        for mv in moves {
            state.execute_expanded_move(&mv, move_generator, config_manager);
            let (raw, _) = self.leaf_evaluate(state, move_generator, config_manager);
            let eval = self.apply_leaf_decay(raw, child_ply);
            state.undo_move(config_manager);

            let child_idx = nodes.len() as u32;
            nodes.push(BfsNode { score: eval, edges_start: 0, edges_len: 0, depth: parent_depth + 1, expanded: false });

            match edges {
                BfsEdges::Full(v) => v.push(BfsEdgeFull { child_idx, mv }),
                BfsEdges::Compact(v) => v.push(Self::compact_edge(child_idx, &mv)),
            }
            len += 1;
        }

        nodes[nidx].edges_start = start;
        nodes[nidx].edges_len = len;

        let mut best = i32::MIN;
        for i in 0..(len as usize) {
            let ci = edges.child_idx(start as usize + i) as usize;
            let v = -nodes[ci].score;
            if v > best {
                best = v;
            }
        }
        nodes[nidx].score = best;
    }

    fn compact_edge(child_idx: u32, mv: &ExpandedMove) -> BfsEdgeCompact {
        BfsEdgeCompact {
            child_idx,
            from_packed: (mv.from.0 as u16) * 256 + mv.from.1 as u16,
            to_packed: (mv.to.0 as u16) * 256 + mv.to.1 as u16,
            promotion: mv.promotion_target.map(|p| p as u16 + 1).unwrap_or(0),
            castling_rook_from: mv
            .castling_option
            .as_ref()
            .map(|c| (c.rook_from.0 as u16) * 256 + c.rook_from.1 as u16 + 1)
            .unwrap_or(0),
        }
    }

    fn replay_compact_move(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        ce: BfsEdgeCompact,
    ) {
        if let Some(em) = self.compact_to_expanded(state, move_generator, ce) {
            state.execute_expanded_move(&em, move_generator, config_manager);
        }
    }

    fn compact_to_expanded(
        &self,
        state: &GameState,
        move_generator: &MoveGenerator,
        ce: BfsEdgeCompact,
    ) -> Option<ExpandedMove> {
        let from = ((ce.from_packed / 256) as usize, (ce.from_packed % 256) as usize);
        let to = ((ce.to_packed / 256) as usize, (ce.to_packed % 256) as usize);
        let piece = state.board.get_piece(from)?;
        let candidates = move_generator.generate_moves_with_database(&state.board, from, piece.piece_type);
        let want_promo = if ce.promotion == 0 { None } else { Some((ce.promotion - 1) as usize) };

        if ce.castling_rook_from != 0 {
            let rf = (((ce.castling_rook_from - 1) / 256) as usize, ((ce.castling_rook_from - 1) % 256) as usize);
            let cand = candidates.into_iter().find(|m| m.destination == to && m.rule.is_king_castle)?;
            let opts = move_generator.get_castling_options(&state.board, from, to, &cand);
            let opt = opts.into_iter().find(|o| o.rook_from == rf)?;
            let captures = state.board.get_piece(opt.rook_to);
            return Some(ExpandedMove {
                from,
                to,
                move_with_path: cand,
                castling_option: Some(opt.clone()),
                        promotion_target: None,
                        captures,
                        captures_position: if captures.is_some() { Some(opt.rook_to) } else { None },
            });
        }

        let cand = candidates.into_iter().find(|m| m.destination == to && !m.rule.is_king_castle)?;

        // Capture resolution goes through the one shared rule, exactly as
        // `MoveChain::build_move` and `generate_pseudo_legal_moves` do.
        let (captures, captures_position) = match resolve_landing_capture(&state.board, to, &cand.rule) {
            Some((victim, sq)) => (Some(victim), Some(sq)),
            None => (None, None),
        };

        Some(ExpandedMove {
            from,
            to,
            move_with_path: cand,
            castling_option: None,
            promotion_target: want_promo,
            captures,
            captures_position,
        })
    }

    fn find_pv_leaf(nodes: &[BfsNode], edges: &BfsEdges, max_depth: u16, path_buf: &mut Vec<u32>) -> bool {
        path_buf.push(0);
        let mut current = 0u32;

        loop {
            let n = &nodes[current as usize];
            if !n.expanded {
                return true;
            }
            if n.edges_len == 0 {
                return Self::find_alt_leaf(nodes, edges, max_depth, path_buf);
            }

            let s = n.edges_start as usize;
            let l = n.edges_len as usize;
            let mut best_ci = edges.child_idx(s);
            let mut best_score = nodes[best_ci as usize].score;
            for i in 1..l {
                let ci = edges.child_idx(s + i);
                let sc = nodes[ci as usize].score;
                if sc < best_score {
                    best_score = sc;
                    best_ci = ci;
                }
            }

            path_buf.push(best_ci);
            current = best_ci;

            if nodes[current as usize].depth >= max_depth && !nodes[current as usize].expanded {
                return true;
            }
        }
    }

    fn find_alt_leaf(nodes: &[BfsNode], edges: &BfsEdges, max_depth: u16, path_buf: &mut Vec<u32>) -> bool {
        while path_buf.len() > 1 {
            let last = *path_buf.last().unwrap();
            let parent = path_buf[path_buf.len() - 2];
            let pn = &nodes[parent as usize];
            let s = pn.edges_start as usize;
            let l = pn.edges_len as usize;

            let mut chosen: Option<u32> = None;
            let mut chosen_score = i32::MAX;
            for i in 0..l {
                let ci = edges.child_idx(s + i);
                if ci == last {
                    continue;
                }
                let sc = nodes[ci as usize].score;
                if sc < chosen_score && Self::has_unexpanded(nodes, edges, ci, max_depth) {
                    chosen_score = sc;
                    chosen = Some(ci);
                }
            }

            path_buf.pop();
            if let Some(c) = chosen {
                path_buf.push(c);
                let mut cur = c;
                loop {
                    let n = &nodes[cur as usize];
                    if !n.expanded || n.edges_len == 0 {
                        return true;
                    }
                    let s = n.edges_start as usize;
                    let l = n.edges_len as usize;
                    let mut bci = u32::MAX;
                    let mut bs = i32::MAX;
                    for i in 0..l {
                        let ci = edges.child_idx(s + i);
                        if Self::has_unexpanded(nodes, edges, ci, max_depth) {
                            let sc = nodes[ci as usize].score;
                            if sc < bs {
                                bs = sc;
                                bci = ci;
                            }
                        }
                    }
                    if bci == u32::MAX {
                        return true;
                    }
                    path_buf.push(bci);
                    cur = bci;
                }
            }
        }
        false
    }

    fn has_unexpanded(nodes: &[BfsNode], edges: &BfsEdges, root: u32, max_depth: u16) -> bool {
        let mut stack = vec![root];
        while let Some(idx) = stack.pop() {
            let n = &nodes[idx as usize];
            if !n.expanded && n.depth <= max_depth {
                return true;
            }
            let s = n.edges_start as usize;
            let l = n.edges_len as usize;
            for i in 0..l {
                stack.push(edges.child_idx(s + i));
            }
        }
        false
    }

    // ─── TT storage ─────────────────────────────────────────────────────

    fn store_tt(&mut self, hash: u64, depth: u32, score: i32, flag: TTFlag, best_move: Option<(usize, usize)>) {
        if let Some(entry) = self.transposition_table.get_mut(&hash) {
            if depth >= entry.depth {
                *entry = TTEntry { hash, depth, score, flag, best_move };
            }
        } else {
            self.transposition_table.insert(hash, TTEntry { hash, depth, score, flag, best_move });
        }
    }

    // ─── Public API ─────────────────────────────────────────────────────

    pub fn find_best_move(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<(ExpandedMove, i32)> {
        self.find_best_move_with_depth(state, move_generator, config_manager, 1)
        .map(|(mv, score, _)| (mv, score))
    }

    pub fn evaluate_all_moves(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Vec<MoveEvaluation> {
        let moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(moves) => moves,
            MoveGenerationResult::Checkmate { .. } => return Vec::new(),
        };

        let mut evaluations = Vec::new();
        for mv in moves {
            state.execute_expanded_move(&mv, move_generator, config_manager);
            let opp = self.evaluator.evaluate(state, move_generator, config_manager);
            if opp > -999999 {
                evaluations.push(MoveEvaluation { mv, opponent_evaluation: opp });
            }
            state.undo_move(config_manager);
        }
        evaluations
    }

    #[inline(always)]
    fn leaf_evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> (i32, Option<f32>) {
        if self.improvements && self.config.multiplicative_eval {
            if let Some((own, opp)) = self.evaluator.evaluate_split(state, move_generator, config_manager) {
                let o = (own.max(0.0) + LOG_RATIO_BASE).ln();
                let p = (opp.max(0.0) + LOG_RATIO_BASE).ln();
                return ((LOG_RATIO_SCALE * (o - p)) as i32, Some(opp.max(0.0)));
            }
        }
        (self.evaluator.evaluate(state, move_generator, config_manager), None)
    }
}

#[derive(Debug, Clone)]
pub struct MateInTwoResult {
    pub first_move: ExpandedMove,
    pub responses: Vec<(ExpandedMove, Vec<ExpandedMove>)>,
}

// ─────────────────────────────────────────────────────────────────────────
// Shared engine entry point
// ─────────────────────────────────────────────────────────────────────────

#[inline]
pub fn mate_in_from_score(score: i32) -> Option<i32> {
    if score >= 999_000 {
        Some((999_999 - score) / 2)
    } else if score <= -999_000 {
        Some(-((-999_999 - score) / 2))
    } else {
        None
    }
}

/// The canonical `best_move` body for every search-based engine.
pub fn run_search<E: EvaluatorTrait>(
    evaluator: &E,
    params: SearchParams<'_>,
    engine_params: &EngineParameters,
    tt: &mut HashMap<u64, TTEntry>,
    default_depth: u32,
) -> Option<SearchResult> {
    let mut search = if SearchConfig::use_new_search(engine_params) {
        Search::with_config(evaluator, SearchConfig::from_params(engine_params))
    } else {
        Search::new(evaluator)
    };
    search.set_transposition_table(std::mem::take(tt));

    let depth = if params.depth > 0 { params.depth } else { default_depth };
    let time_limit = params.time_limit;
    let move_generator = params.move_generator;
    let config_manager = params.config_manager;
    let state = params.state;

    let result = if let Some(limit) = time_limit {
        search.find_best_move_iterative(state, move_generator, config_manager, depth, limit)
    } else {
        search.find_best_move_with_depth(state, move_generator, config_manager, depth)
    };

    *tt = search.take_transposition_table();

    let (best_move, score, depth_reached) = result?;
    Some(SearchResult {
        best_move,
         evaluation: Evaluation { score, mate_in: mate_in_from_score(score) },
         depth_reached,
    })
}
