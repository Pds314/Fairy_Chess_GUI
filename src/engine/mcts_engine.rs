// src/engine/mcts_engine.rs

use crate::core::game_state::{ExpandedMove, GameState};
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::pst_engine::PstEngine;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::time::{Duration, Instant};

// Parameters
pub const PARAM_TEMPERATURE: &str = "temperature";
pub const PARAM_MAX_NODES: &str = "max_nodes";
pub const PARAM_CPUCT: &str = "cpuct";

const MATE_SCORE: f64 = 1_000_000.0;

pub static MCTS_PARAMETERS: &[ParameterDef] = &[
    // T=200 correctly maps true millipawns to a healthy exploration curve
    ParameterDef::new(
        PARAM_TEMPERATURE,
        "Softmax Temperature",
        "Controls policy distribution sharpness",
        10.0,
        1000.0,
        300.0,
        10.0,
    ),
    ParameterDef::new(
        PARAM_MAX_NODES,
        "Maximum Nodes",
        "Search budget",
        100.0,
        5_000_000.0,
        50000.0,
        1000.0,
    ),
    // cpuct=2.5 perfectly scales the policy priors against the tanh(q / 1000.0) squashed bounds
    ParameterDef::new(
        PARAM_CPUCT,
        "Exploration Constant",
        "PUCT exploration factor",
        0.1,
        10.0,
        6.0,
        0.1,
    ),
];

#[derive(Clone)]
struct MctsNode {
    pub mv: Option<ExpandedMove>,
    pub visits: u32,
    pub q_value: f64,
    pub policy_prob: f64,
    pub state_eval: f64,
    pub children: Vec<usize>,
    pub expanded: bool,
}

pub struct MctsEngine {
    parameters: EngineParameters,
    pst_engine: PstEngine,
    nodes: Vec<MctsNode>,
}

impl MctsEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(MCTS_PARAMETERS),
            pst_engine: PstEngine::new(),
            nodes: Vec::new(),
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    fn reset_tree(&mut self) {
        self.nodes.clear();
    }

    fn evaluate_position(&mut self, state: &GameState, config_manager: &PieceConfigManager) -> f64 {
        self.pst_engine.evaluate_position(state, config_manager) as f64
    }

    fn compute_softmax(&self, scores: &[f64], temperature: f64) -> Vec<f64> {
        if scores.is_empty() {
            return Vec::new();
        }
        let max_score = scores.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        let exp_scores: Vec<f64> = scores
            .iter()
            .map(|&s| ((s - max_score) / temperature).exp())
            .collect();
        let sum_exp: f64 = exp_scores.iter().sum();

        if sum_exp > 0.0 && sum_exp.is_finite() {
            exp_scores.into_iter().map(|e| e / sum_exp).collect()
        } else {
            let mut fallback = vec![0.0; scores.len()];
            let best_idx = scores
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .unwrap()
                .0;
            fallback[best_idx] = 1.0;
            fallback
        }
    }

    fn search(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        time_limit: Option<Duration>,
    ) -> Option<(ExpandedMove, i32, usize)> {
        let start_time = Instant::now();
        let max_nodes = self.get_param(PARAM_MAX_NODES, 50000.0) as usize;
        let temperature = self.get_param(PARAM_TEMPERATURE, 0.20);
        let cpuct = self.get_param(PARAM_CPUCT, 2.5);

        self.reset_tree();
        self.pst_engine
            .initialize_psts(&state.board, move_generator, config_manager);

        let root_eval = self.evaluate_position(state, config_manager);

        self.nodes.push(MctsNode {
            mv: None,
            visits: 0,
            q_value: 0.0,
            policy_prob: 1.0,
            state_eval: root_eval,
            children: Vec::new(),
            expanded: false,
        });

        let mut nodes_expanded = 0;

        while nodes_expanded < max_nodes {
            if let Some(limit) = time_limit {
                if (nodes_expanded & 1023) == 0 && start_time.elapsed() >= limit * 9 / 10 {
                    break;
                }
            }

            let mut path = vec![0];
            let mut current_node = 0;

            // 1. SELECTION
            while self.nodes[current_node].expanded && !self.nodes[current_node].children.is_empty()
            {
                let mut best_puct = f64::NEG_INFINITY;
                let mut best_child = 0;
                let parent_visits = self.nodes[current_node].visits;

                for &child_idx in &self.nodes[current_node].children {
                    let child = &self.nodes[child_idx];

                    let q_a = if child.visits > 0 {
                        -child.q_value
                    } else {
                        -child.state_eval
                    };

                    let normalized_q = (q_a / 1000.0).tanh();

                    let u = cpuct * child.policy_prob * (parent_visits as f64).sqrt()
                        / (1.0 + child.visits as f64);
                    let puct_score = normalized_q + u;

                    if puct_score > best_puct {
                        best_puct = puct_score;
                        best_child = child_idx;
                    }
                }

                let mv = self.nodes[best_child].mv.as_ref().unwrap().clone();
                state.execute_expanded_move(&mv, move_generator, config_manager);
                path.push(best_child);
                current_node = best_child;
            }

            // 2. EXPANSION & EVALUATION
            // 2. EXPANSION & EVALUATION
            let leaf_eval = if !self.nodes[current_node].expanded {
                let moves = state.get_legal_moves(move_generator, config_manager);

                if moves.is_empty() {
                    // Terminal node. Checkmate, extinction, or stalemate.
                    let eval = if state.is_in_check(move_generator, config_manager) {
                        -MATE_SCORE
                    } else if !state.board.has_pieces(state.current_turn) {
                        -MATE_SCORE // extinction: no pieces = loss
                    } else {
                        0.0
                    };
                    self.nodes[current_node].expanded = true;
                    eval
                } else {
                    let mut child_evals_for_parent = Vec::with_capacity(moves.len());
                    let mut child_state_evals = Vec::with_capacity(moves.len());

                    for mv in &moves {
                        state.execute_expanded_move(mv, move_generator, config_manager);

                        let eval = self.evaluate_position(state, config_manager);

                        child_state_evals.push(eval);
                        child_evals_for_parent.push(-eval);

                        state.undo_move(config_manager);
                    }

                    let policy = self.compute_softmax(&child_evals_for_parent, temperature);

                    for (i, mv) in moves.into_iter().enumerate() {
                        let child_idx = self.nodes.len();
                        self.nodes.push(MctsNode {
                            mv: Some(mv),
                            visits: 0,
                            q_value: 0.0,
                            policy_prob: policy[i],
                            state_eval: child_state_evals[i],
                            children: Vec::new(),
                            expanded: false,
                        });
                        self.nodes[current_node].children.push(child_idx);
                    }
                    self.nodes[current_node].expanded = true;

                    child_evals_for_parent
                        .into_iter()
                        .fold(f64::NEG_INFINITY, |a, b| a.max(b))
                }
            } else {
                if state.is_in_check(move_generator, config_manager) {
                    -MATE_SCORE
                } else {
                    0.0
                }
            };

            nodes_expanded += 1;

            // 3. BACKPROPAGATION
            let mut current_avg = leaf_eval;
            for &node_idx in path.iter().rev() {
                let node = &mut self.nodes[node_idx];
                node.visits += 1;
                node.q_value += (current_avg - node.q_value) / (node.visits as f64);

                current_avg = -current_avg;

                if node_idx != 0 {
                    state.undo_move(config_manager);
                }
            }
        }

        // 4. CHOOSE BEST MOVE
        let root = &self.nodes[0];
        if root.children.is_empty() {
            return None;
        }

        let mut best_child = root.children[0];
        let mut max_visits = 0;

        for &child_idx in &root.children {
            let visits = self.nodes[child_idx].visits;
            if visits > max_visits {
                max_visits = visits;
                best_child = child_idx;
            }
        }

        let best_node = &self.nodes[best_child];

        let best_score = -best_node.q_value as i32;

        Some((best_node.mv.clone().unwrap(), best_score, nodes_expanded))
    }
}

impl ChessEngine for MctsEngine {
    fn name(&self) -> &str {
        "MCTS Engine (Temperature Tree Search)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let result = self.search(
            params.state,
            params.move_generator,
            params.config_manager,
            params.time_limit,
        )?;
        let (best_move, evaluation, nodes_searched) = result;
        Some(SearchResult {
            best_move,
            evaluation: Evaluation {
                score: evaluation,
                mate_in: None,
            },
            depth_reached: nodes_searched as u32,
        })
    }

    fn stop(&mut self) {}

    fn reset_cache(&mut self) {
        self.reset_tree();
        self.pst_engine.reset_cache();
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(MCTS_PARAMETERS)
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
        }
        changed
    }

    fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::analysis::PositionAnalysis> {
        let time_limit = Some(Duration::from_secs(5));
        self.search(state, move_generator, config_manager, time_limit);
        Some(
            self.pst_engine
                .analyze_position(state, move_generator, config_manager),
        )
    }

    fn supports_analysis(&self) -> bool {
        true
    }
}
