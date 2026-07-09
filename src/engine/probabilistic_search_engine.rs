// src/engine/probabilistic_search_engine.rs
use crate::core::game_state::{ExpandedMove, GameState};
use crate::core::piece::PieceColor;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::evaluator::EvaluatorTrait;
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::pst_engine::PstEngine;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

pub const PARAM_NODE_BUDGET: &str = "node_budget";
pub const PARAM_TEMPERATURE: &str = "temperature";
pub const PARAM_MIN_PROBABILITY: &str = "min_probability";
pub const PARAM_CAPTURE_BONUS: &str = "capture_bonus";
pub const PARAM_CHECK_BONUS: &str = "check_bonus";

pub static PROB_SEARCH_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_NODE_BUDGET,
        "Node Budget",
        "Maximum number of nodes to expand",
        100.0, 100000.0, 10000.0, 100.0,
    ),
ParameterDef::new(
    PARAM_TEMPERATURE,
    "Temperature",
    "Controls probability distribution (lower = more selective)",
                  0.1, 5.0, 1.0, 0.1,
),
ParameterDef::new(
    PARAM_MIN_PROBABILITY,
    "Minimum Probability",
    "Prune branches below this cumulative probability",
    0.0001, 0.1, 0.001, 0.0001,
),
ParameterDef::new(
    PARAM_CAPTURE_BONUS,
    "Capture Bonus",
    "Extra score for capture moves in probability calculation",
    0.0, 1000.0, 200.0, 50.0,
),
ParameterDef::new(
    PARAM_CHECK_BONUS,
    "Check Bonus",
    "Extra score for checking moves in probability calculation",
    0.0, 500.0, 100.0, 50.0,
),
];

#[derive(Clone)]
struct SearchNode {
    state: GameState,
    path_probability: f64,
    path: Vec<ExpandedMove>,
    depth: usize,
    evaluation: Option<i32>,
}

struct QueueEntry {
    node_index: usize,
    path_probability: f64,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path_probability == other.path_probability
    }
}
impl Eq for QueueEntry {}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.path_probability.partial_cmp(&other.path_probability)
    }
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

pub struct ProbabilisticSearchEngine {
    parameters: EngineParameters,
    pst_engine: PstEngine,
}

impl ProbabilisticSearchEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(PROB_SEARCH_PARAMETERS),
            pst_engine: PstEngine::new(),
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    fn evaluate_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        self.pst_engine
        .initialize_psts(&state.board, move_generator, config_manager);
        let evaluator = PstEvaluator {
            engine: &self.pst_engine,
        };
        evaluator.evaluate(state, move_generator, config_manager)
    }

    fn score_move_for_probability(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        base_eval: i32,
    ) -> f64 {
        let mut score = base_eval as f64;

        if mv.captures.is_some() {
            score += self.get_param(PARAM_CAPTURE_BONUS, 200.0);
            if let Some(captured) = mv.captures {
                // Cached flags on `Piece`, not a config lookup.
                if captured.is_royal || captured.is_royalty {
                    score += 1000.0;
                }
            }
        }

        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);
        if test_state.is_in_check(move_generator, config_manager) {
            score += self.get_param(PARAM_CHECK_BONUS, 100.0);
        }

        score
    }

    fn compute_move_probabilities(&self, scores: &[f64], temperature: f64) -> Vec<f64> {
        if scores.is_empty() {
            return Vec::new();
        }
        let max_score = scores.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        let exp_scores: Vec<f64> = scores
        .iter()
        .map(|&s| ((s - max_score) / temperature).exp())
        .collect();
        let sum: f64 = exp_scores.iter().sum();
        if sum > 0.0 && sum.is_finite() {
            exp_scores.into_iter().map(|e| e / sum).collect()
        } else {
            vec![1.0 / scores.len() as f64; scores.len()]
        }
    }

    fn search(
        &mut self,
        initial_state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        time_limit: Option<Duration>,
    ) -> Option<(ExpandedMove, i32)> {
        let start_time = Instant::now();
        let node_budget = self.get_param(PARAM_NODE_BUDGET, 10000.0) as usize;
        let temperature = self.get_param(PARAM_TEMPERATURE, 1.0);
        let min_probability = self.get_param(PARAM_MIN_PROBABILITY, 0.001);

        println!(
            "\n🌳 Probabilistic Search: Budget={} nodes, Temp={:.2}",
            node_budget, temperature
        );

        let mut nodes: Vec<SearchNode> = Vec::with_capacity(node_budget);
        let mut queue: BinaryHeap<QueueEntry> = BinaryHeap::with_capacity(node_budget / 10);

        let root = SearchNode {
            state: initial_state.clone(),
            path_probability: 1.0,
            path: Vec::new(),
            depth: 0,
            evaluation: None,
        };
        nodes.push(root);
        queue.push(QueueEntry {
            node_index: 0,
            path_probability: 1.0,
        });

        let mut nodes_expanded = 0usize;
        let mut max_depth_reached = 0usize;

        while let Some(entry) = queue.pop() {
            if nodes_expanded >= node_budget {
                break;
            }
            if (nodes_expanded & 1023) == 0 {
                if let Some(limit) = time_limit {
                    if start_time.elapsed() >= limit {
                        println!("⏱️ Time limit reached! Bailing out early...");
                        break;
                    }
                }
            }

            let node_index = entry.node_index;
            if entry.path_probability < min_probability {
                continue;
            }

            let mut node = nodes[node_index].clone();
            nodes_expanded += 1;
            max_depth_reached = max_depth_reached.max(node.depth);

            let legal_moves = node.state.get_legal_moves(move_generator, config_manager);
            if legal_moves.is_empty() {
                let mut eval_state = node.state.clone();
                let eval = self.evaluate_position(&mut eval_state, move_generator, config_manager);
                nodes[node_index].evaluation = Some(eval);
                continue;
            }

            let mut eval_state = node.state.clone();
            let base_eval = self.evaluate_position(&mut eval_state, move_generator, config_manager);

            let move_scores: Vec<f64> = legal_moves
            .iter()
            .map(|mv| {
                self.score_move_for_probability(
                    &node.state,
                    mv,
                    move_generator,
                    config_manager,
                    base_eval,
                )
            })
            .collect();

            let move_probabilities = self.compute_move_probabilities(&move_scores, temperature);

            for (i, mv) in legal_moves.iter().enumerate() {
                let move_prob = move_probabilities[i];
                let child_path_prob = node.path_probability * move_prob;
                if child_path_prob < min_probability {
                    continue;
                }

                let mut child_state = node.state.clone();
                child_state.execute_expanded_move(mv, move_generator, config_manager);

                let mut child_path = node.path.clone();
                child_path.push(mv.clone());

                let child_node = SearchNode {
                    state: child_state,
                    path_probability: child_path_prob,
                    path: child_path,
                    depth: node.depth + 1,
                    evaluation: None,
                };

                let child_index = nodes.len();
                nodes.push(child_node);
                queue.push(QueueEntry {
                    node_index: child_index,
                    path_probability: child_path_prob,
                });
            }
        }

        println!(
            "  Expanded {} nodes, max depth {}, time: {:?}",
            nodes_expanded,
            max_depth_reached,
            start_time.elapsed()
        );

        let initial_color = initial_state.current_turn;
        let mut best_leaf_index = None;
        let mut best_score = i32::MIN;

        for i in 0..nodes.len() {
            let (turn, is_leaf, cached) = {
                let n = &nodes[i];
                (n.state.current_turn, n.evaluation.is_none() && !n.path.is_empty(), n.evaluation)
            };

            let eval = if is_leaf {
                let mut eval_state = nodes[i].state.clone();
                self.evaluate_position(&mut eval_state, move_generator, config_manager)
            } else if let Some(e) = cached {
                e
            } else {
                continue;
            };

            let adjusted_score = if turn == initial_color { eval } else { -eval };
            if adjusted_score > best_score {
                best_score = adjusted_score;
                best_leaf_index = Some(i);
            }
        }

        if let Some(leaf_idx) = best_leaf_index {
            let (depth, prob, first) = {
                let n = &nodes[leaf_idx];
                (n.depth, n.path_probability, n.path.first().cloned())
            };
            if let Some(mv) = first {
                println!(
                    "  Best path: depth {}, probability {:.6}, score {}",
                    depth, prob, best_score
                );
                return Some((mv, best_score));
            }
        }

        let legal_moves = initial_state.get_legal_moves(move_generator, config_manager);
        if let Some(mv) = legal_moves.first().cloned() {
            let mut test_state = initial_state.clone();
            test_state.execute_expanded_move(&mv, move_generator, config_manager);
            let eval = self.evaluate_position(&mut test_state, move_generator, config_manager);
            return Some((mv, -eval));
        }
        None
    }
}

impl Default for ProbabilisticSearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct PstEvaluator<'a> {
    engine: &'a PstEngine,
}

impl EvaluatorTrait for PstEvaluator<'_> {
    fn evaluate(
        &self,
        state: &mut GameState,
        _move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        // BUG FIX: the original wrote
        //     self.engine.evaluate_position(state, config_manager) as i32 * 100
        // which truncates the f32 to i32 BEFORE scaling. PST values are
        // single-digit, so almost the entire evaluation signal was discarded.
        (self.engine.evaluate_position(state, config_manager) * 100.0) as i32
    }
}

impl ChessEngine for ProbabilisticSearchEngine {
    fn name(&self) -> &str {
        "Probabilistic Search Engine (Best-First with PST Eval)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.pst_engine.initialize_psts(
            &params.state.board,
            params.move_generator,
            params.config_manager,
        );

        let (best_move, score) = self.search(
            params.state,
            params.move_generator,
            params.config_manager,
            params.time_limit,
        )?;

        let _ = PieceColor::White; // keep the import meaningful for clarity

        println!(
            "🎯 Selected move: {} → {} with score {}",
            crate::notation::position_to_algebraic(best_move.from, params.state.board.size()),
                 crate::notation::position_to_algebraic(best_move.to, params.state.board.size()),
                 score
        );

        Some(SearchResult {
            best_move,
            evaluation: Evaluation {
                score,
                mate_in: None,
            },
            depth_reached: 0,
        })
    }

    fn stop(&mut self) {}

    fn reset_cache(&mut self) {
        self.pst_engine.reset_cache();
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(PROB_SEARCH_PARAMETERS)
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
}
