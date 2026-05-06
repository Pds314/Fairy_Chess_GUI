// src/engine/search.rs
use crate::core::GameState;
use crate::core::game_state::{ExpandedMove, GameResult, MateStatus, MoveGenerationResult};
use crate::engine::analysis::MoveEvaluation;
use crate::engine::evaluator::EvaluatorTrait;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const ALPHA_BETA_MIN: i32 = -1_000_000_000;
const ALPHA_BETA_MAX: i32 = 1_000_000_000;

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

pub struct Search<'a, E: EvaluatorTrait> {
    evaluator: &'a E,
    transposition_table: HashMap<u64, TTEntry>,
    killer_moves: Vec<Vec<(usize, usize)>>,
    history_table: HashMap<(usize, usize), i32>,
    nodes_searched: usize,
    time_limit: Option<Instant>,
    should_stop: bool,
    can_timeout: bool, // Shield to protect Depth 1
}

impl<'a, E: EvaluatorTrait> Search<'a, E> {
    pub fn new(evaluator: &'a E) -> Self {
        Search {
            evaluator,
            transposition_table: HashMap::with_capacity(1_000_000),
            killer_moves: vec![Vec::new(); 64],
            history_table: HashMap::new(),
            nodes_searched: 0,
            time_limit: None,
            should_stop: false,
            can_timeout: true,
        }
    }

    pub fn set_transposition_table(&mut self, table: HashMap<u64, TTEntry>) {
        self.transposition_table = table;
    }

    pub fn get_transposition_table(&self) -> HashMap<u64, TTEntry> {
        self.transposition_table.clone()
    }

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
        self.can_timeout = true; // No time limit here anyway

        let mut best_move = None;
        let mut best_score = ALPHA_BETA_MIN;
        let mut depth_reached = 0;

        for d in 1..=depth {
            if self.should_stop {
                break;
            }
            if let Some((mv, score)) = self.search_root(state, move_generator, config_manager, d) {
                best_move = Some(mv);
                best_score = score;
                depth_reached = d;
                if score > 900000 || score < -900000 {
                    break;
                }
            } else {
                break;
            }
        }

        println!("Nodes searched: {}", self.nodes_searched);
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

        // SHIELD ON: Do not allow timeouts during Depth 1
        self.can_timeout = false;

        let hard_limit = target_time.mul_f32(3.0);
        self.time_limit = Some(start_time + hard_limit);

        let mut best_move = None;
        let mut best_score = ALPHA_BETA_MIN;
        let mut depth_reached = 0;
        let mut last_depth_time = Duration::from_millis(1);
        let mut branch_factor = 3.5;

        for d in 1..=max_depth {
            // SHIELD OFF: The clock matters again after we have guaranteed a Depth 1 move
            if d > 1 {
                self.can_timeout = true;
            }

            let elapsed = start_time.elapsed();

            if d > 2 {
                let expected_next_depth_time = last_depth_time.mul_f64(branch_factor);
                let estimated_total_time = elapsed + expected_next_depth_time;
                let acceptable_max_time = target_time.mul_f64(2.0);
                if estimated_total_time > acceptable_max_time {
                    println!(
                        "⏱️ Stopping before depth {}. Expected to take {:.2}s (Soft Limit: {:.2}s)",
                        d,
                        estimated_total_time.as_secs_f32(),
                        acceptable_max_time.as_secs_f32()
                    );
                    break;
                }
            }

            let depth_start = Instant::now();

            if let Some((mv, score)) = self.search_root(state, move_generator, config_manager, d) {
                if !self.should_stop {
                    best_move = Some(mv);
                    best_score = score;
                    depth_reached = d;
                    let current_depth_time = depth_start.elapsed();
                    if d > 1 && last_depth_time.as_millis() > 0 {
                        let current_bf =
                            current_depth_time.as_secs_f64() / last_depth_time.as_secs_f64();
                        branch_factor = (branch_factor * 0.85) + (current_bf * 0.15);
                    }
                    last_depth_time = current_depth_time;
                    println!(
                        "Depth {} completed: score={}, time={:?}",
                        d,
                        score,
                        start_time.elapsed()
                    );
                    if score > 900000 || score < -900000 {
                        break;
                    }
                } else {
                    println!(
                        "Depth {} aborted due to hard time limit ({:.2}s). Keeping best move from depth {}.",
                        d,
                        hard_limit.as_secs_f32(),
                        d - 1
                    );
                }
            } else {
                break;
            }
        }

        println!(
            "Total nodes searched: {}, depth reached: {}, time: {:?}",
            self.nodes_searched,
            depth_reached,
            start_time.elapsed()
        );
        best_move.map(|mv| (mv, best_score, depth_reached))
    }

    #[inline(always)]
    fn check_time(&mut self) {
        if !self.can_timeout {
            return;
        } // Ignore the clock if shielded

        if let Some(deadline) = self.time_limit {
            if Instant::now() >= deadline {
                self.should_stop = true;
            }
        }
    }

    fn search_root(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
    ) -> Option<(ExpandedMove, i32)> {
        let mut moves = state.get_legal_moves(move_generator, config_manager);
        if moves.is_empty() {
            return None;
        }

        self.order_moves(state, &mut moves, 0, config_manager);

        let mut best_move = None; // No fallback move! If it fails, it returns None.
        let mut best_score = ALPHA_BETA_MIN;
        let mut alpha = ALPHA_BETA_MIN;

        for mv in &mut moves {
            state.execute_expanded_move(mv, move_generator, config_manager);

            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }

            let score = -self.alpha_beta(
                state,
                move_generator,
                config_manager,
                depth - 1,
                -ALPHA_BETA_MAX,
                -alpha,
                1,
            );
            state.undo_move(config_manager);

            if self.should_stop {
                break;
            }

            if score > best_score {
                best_score = score;
                best_move = Some(mv.clone());
                alpha = score;
            }
        }

        best_move.map(|mv| (mv, best_score))
    }

    fn alpha_beta(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        depth: u32,
        mut alpha: i32,
        beta: i32,
        ply: usize,
    ) -> i32 {
        self.nodes_searched += 1;
        if (self.nodes_searched & 2047) == 0 {
            self.check_time();
            if self.should_stop {
                return 0;
            }
        }
        if let Some(GameResult::Draw(_)) = state.game_result {
            return 0;
        }

        let hash = state.current_hash();
        if let Some(&entry) = self.transposition_table.get(&hash) {
            if entry.hash == hash && entry.depth >= depth {
                match entry.flag {
                    TTFlag::Exact => return entry.score,
                    TTFlag::LowerBound => {
                        if entry.score >= beta {
                            return entry.score;
                        } else {
                            alpha = alpha.max(entry.score);
                        }
                    }
                    TTFlag::UpperBound => {
                        if entry.score <= alpha {
                            return entry.score;
                        }
                    }
                }
            }
        }

        if depth == 0 {
            return self.quiescence(state, move_generator, config_manager, alpha, beta, ply);
        }

        let mut moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Checkmate { .. } => return 999999 - ply as i32,
            MoveGenerationResult::Moves(m) => m,
        };

        if moves.is_empty() {
            // No pseudo-legal moves at all. Either checkmate (in check),
            // extinction (no pieces — only reachable in non-royal variants),
            // or genuine stalemate (pieces exist but are all boxed in).
            if state.is_in_check_fast(move_generator) {
                return -999999 + ply as i32;
            }
            if !state.board.has_pieces(state.current_turn) {
                return -999999 + ply as i32;
            }
            return 0;
        }

        // Null Move Pruning
        if depth > 2 && !state.is_in_check_fast(move_generator) {
            state.current_turn = state.current_turn.opposite();
            let null_score = -self.alpha_beta(
                state,
                move_generator,
                config_manager,
                depth - 3,
                -beta,
                -beta + 1,
                ply + 1,
            );
            state.current_turn = state.current_turn.opposite();
            if self.should_stop {
                return 0;
            }
            if null_score >= beta {
                return beta;
            }
        }

        self.order_moves(state, &mut moves, ply, config_manager);
        let mut best_score = ALPHA_BETA_MIN;
        let mut best_move_key = None;
        let mut legal_moves_found: u32 = 0;
        let original_alpha = alpha;

        for mv in &mut moves {
            state.execute_expanded_move(mv, move_generator, config_manager);

            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }

            legal_moves_found += 1;

            let score = if legal_moves_found == 1 {
                -self.alpha_beta(
                    state,
                    move_generator,
                    config_manager,
                    depth - 1,
                    -beta,
                    -alpha,
                    ply + 1,
                )
            } else {
                let reduction = if depth > 3 && legal_moves_found > 3 && mv.captures.is_none() {
                    1
                } else {
                    0
                };
                let mut score = -self.alpha_beta(
                    state,
                    move_generator,
                    config_manager,
                    depth - 1 - reduction,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                );
                if !self.should_stop && score > alpha && score < beta {
                    score = -self.alpha_beta(
                        state,
                        move_generator,
                        config_manager,
                        depth - 1,
                        -beta,
                        -alpha,
                        ply + 1,
                    );
                }
                score
            };

            state.undo_move(config_manager);
            if self.should_stop {
                return 0;
            }

            if score > best_score {
                best_score = score;
                best_move_key = Some((mv.from.0 * 16 + mv.from.1, mv.to.0 * 16 + mv.to.1));
            }
            alpha = alpha.max(best_score);

            if alpha >= beta {
                if mv.captures.is_none() {
                    if ply < self.killer_moves.len() {
                        let k = (mv.from.0 * 16 + mv.from.1, mv.to.0 * 16 + mv.to.1);
                        if !self.killer_moves[ply].contains(&k) {
                            self.killer_moves[ply].insert(0, k);
                            self.killer_moves[ply].truncate(2);
                        }
                    }
                    let hk = (mv.from.0 * 16 + mv.from.1, mv.to.0 * 16 + mv.to.1);
                    *self.history_table.entry(hk).or_insert(0) += (depth * depth) as i32;
                }
                if !self.should_stop {
                    self.store_tt(hash, depth, beta, TTFlag::LowerBound, best_move_key);
                }
                return beta;
            }
        }

        if legal_moves_found == 0 {
            // Had pseudo-legal moves but none survived legality filtering.
            // Extinction can't fire here (we had moves, so we have pieces),
            // but the check is O(1) and keeps all terminal-node sites
            // uniform.
            if state.is_in_check_fast(move_generator) {
                return -999999 + ply as i32;
            }
            if !state.board.has_pieces(state.current_turn) {
                return -999999 + ply as i32;
            }
            return 0;
        }

        if !self.should_stop {
            let flag = if best_score <= original_alpha {
                TTFlag::UpperBound
            } else {
                TTFlag::Exact
            };
            self.store_tt(hash, depth, best_score, flag, best_move_key);
        }
        best_score
    }

    fn quiescence(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        mut alpha: i32,
        beta: i32,
        ply: usize,
    ) -> i32 {
        self.nodes_searched += 1;
        if (self.nodes_searched & 2047) == 0 {
            self.check_time();
            if self.should_stop {
                return alpha;
            }
        }

        let stand_pat = self
            .evaluator
            .evaluate(state, move_generator, config_manager);
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

        let captures: Vec<_> = moves.into_iter().filter(|m| m.captures.is_some()).collect();

        for mv in captures {
            state.execute_expanded_move(&mv, move_generator, config_manager);
            if state.mover_king_in_check(move_generator) {
                state.undo_move(config_manager);
                continue;
            }
            let score = -self.quiescence(
                state,
                move_generator,
                config_manager,
                -beta,
                -alpha,
                ply + 1,
            );
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

    fn order_moves(
        &self,
        state: &GameState,
        moves: &mut [ExpandedMove],
        ply: usize,
        config_manager: &PieceConfigManager,
    ) {
        let hash = state.current_hash();
        let tt_move = self
            .transposition_table
            .get(&hash)
            .and_then(|e| e.best_move);

        moves.sort_by_key(|m| {
            let from_idx = m.from.0 * 16 + m.from.1;
            let to_idx = m.to.0 * 16 + m.to.1;

            // 1. TT Move (Always search the proven best move first)
            if let Some((tt_from, tt_to)) = tt_move {
                if from_idx == tt_from && to_idx == tt_to {
                    return -2_000_000_000;
                }
            }

            // 2. Promotions (Highly forcing, search immediately after TT)
            if m.promotion_target.is_some() {
                return -1_900_000_000;
            }

            // 3. Captures (MVV-LVA logic safely tiered)
            if let Some(captured) = m.captures {
                if let Some(attacker) = state.board.get_piece(m.from) {
                    let victim_value =
                        self.evaluator
                            .get_piece_value_on_square(&captured, m.to, config_manager);
                    let attacker_value =
                        self.evaluator
                            .get_piece_value_on_square(&attacker, m.from, config_manager);

                    return -1_800_000_000 - (victim_value * 10) + attacker_value;
                }
            }

            // 4. Killer Moves (Good quiet moves that caused cutoffs in sibling nodes)
            if ply < self.killer_moves.len() {
                for (i, killer) in self.killer_moves[ply].iter().enumerate() {
                    if (from_idx, to_idx) == *killer {
                        // + i ensures the 1st killer is searched before the 2nd killer
                        return -1_700_000_000 + i as i32;
                    }
                }
            }

            // 5. History Heuristic (Moves that have historically been good across the tree)
            // (Value is typically small positive, so -history puts it slightly below 0)
            -self
                .history_table
                .get(&(from_idx, to_idx))
                .copied()
                .unwrap_or(0)
        });
    }
    fn store_tt(
        &mut self,
        hash: u64,
        depth: u32,
        score: i32,
        flag: TTFlag,
        best_move: Option<(usize, usize)>,
    ) {
        if let Some(entry) = self.transposition_table.get_mut(&hash) {
            if depth >= entry.depth {
                *entry = TTEntry {
                    hash,
                    depth,
                    score,
                    flag,
                    best_move,
                };
            }
        } else {
            self.transposition_table.insert(
                hash,
                TTEntry {
                    hash,
                    depth,
                    score,
                    flag,
                    best_move,
                },
            );
        }
    }

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
            let opponent_evaluation =
                self.evaluator
                    .evaluate(state, move_generator, config_manager);
            if opponent_evaluation > -999999 {
                evaluations.push(MoveEvaluation {
                    mv,
                    opponent_evaluation,
                });
            }
            state.undo_move(config_manager);
        }
        evaluations
    }
}

#[derive(Debug, Clone)]
pub struct MateInTwoResult {
    pub first_move: ExpandedMove,
    pub responses: Vec<(ExpandedMove, Vec<ExpandedMove>)>,
}
