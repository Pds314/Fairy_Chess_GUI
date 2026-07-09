// src/engine/diffusion_engine.rs

use crate::core::board::Board;
use crate::core::game_state::{ExpandedMove, GameState};
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;
use std::time::{Duration, Instant};

// Parameter definitions
pub const PARAM_ROYAL_REDUCTION_TARGET: &str = "royal_reduction_target";
pub const PARAM_MAX_DEPTH: &str = "max_depth";
pub const PARAM_CAPTURE_PROBABILITY: &str = "capture_probability";
pub const PARAM_BLOCKING_FACTOR: &str = "blocking_factor";
pub const PARAM_POLICY_TEMPERATURE: &str = "policy_temperature";
pub const PARAM_MIN_PROBABILITY_THRESHOLD: &str = "min_probability_threshold";
pub const PARAM_MASS_IMPORTANCE: &str = "mass_importance";
pub const PARAM_ROYAL_IMPORTANCE: &str = "royal_importance";

pub static DIFFUSION_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_ROYAL_REDUCTION_TARGET,
        "Royal Reduction Target",
        "Stop when royal pieces reduced by this fraction (0.1 = 10% reduction)",
        0.01,
        0.5,
        0.2,
        0.01,
    ),
    ParameterDef::new(
        PARAM_MAX_DEPTH,
        "Maximum Diffusion Depth",
        "Maximum number of diffusion steps to simulate",
        10.0,
        200.0,
        50.0,
        10.0,
    ),
    ParameterDef::new(
        PARAM_CAPTURE_PROBABILITY,
        "Base Capture Probability",
        "Probability of capture when pieces meet (per step)",
        0.01,
        1.0,
        0.8,
        0.01,
    ),
    ParameterDef::new(
        PARAM_BLOCKING_FACTOR,
        "Blocking Factor",
        "How much pieces block movement (0 = no blocking, 1 = full blocking)",
        0.0,
        1.0,
        0.8,
        0.1,
    ),
    ParameterDef::new(
        PARAM_POLICY_TEMPERATURE,
        "Policy Temperature",
        "Controls move probability distribution (lower = more selective)",
        0.1,
        3.0,
        1.0,
        0.1,
    ),
    ParameterDef::new(
        PARAM_MIN_PROBABILITY_THRESHOLD,
        "Minimum Probability Threshold",
        "Ignore probabilities below this threshold",
        0.0001,
        0.01,
        0.001,
        0.0001,
    ),
    ParameterDef::new(
        PARAM_MASS_IMPORTANCE,
        "Mass Importance",
        "Weight for total probability mass in scoring (0-1)",
        0.0,
        1.0,
        0.5,
        0.1,
    ),
    ParameterDef::new(
        PARAM_ROYAL_IMPORTANCE,
        "Royal Importance",
        "Weight for royal preservation in scoring (0-1)",
        0.0,
        1.0,
        0.5,
        0.1,
    ),
];

/// Represents a probability distribution of pieces across the board
#[derive(Clone, Debug)]
struct ProbabilityField {
    // piece_type -> color -> position -> probability
    distributions: HashMap<usize, [Vec<Vec<f64>>; 2]>,
    board_size: (usize, usize),
}

impl ProbabilityField {
    fn new(board_size: (usize, usize)) -> Self {
        Self {
            distributions: HashMap::new(),
            board_size,
        }
    }

    /// Initialize from actual board state
    fn from_board(board: &Board, _config_manager: &PieceConfigManager) -> Self {
        let board_size = board.size();
        let mut field = Self::new(board_size);

        // Place pieces with probability 1.0 at their actual positions
        for row in 0..board_size.0 {
            for col in 0..board_size.1 {
                if let Some(piece) = board.get_piece((row, col)) {
                    field.set_probability(piece.piece_type, piece.color, (row, col), 1.0);
                }
            }
        }

        field
    }

    /// Get probability of a specific piece type and color at a position
    fn get_probability(&self, piece_type: usize, color: PieceColor, pos: Position) -> f64 {
        let color_idx = match color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };

        self.distributions
            .get(&piece_type)
            .and_then(|color_dists| color_dists[color_idx].get(pos.0))
            .and_then(|row| row.get(pos.1))
            .copied()
            .unwrap_or(0.0)
    }

    /// Set probability of a specific piece type and color at a position
    fn set_probability(&mut self, piece_type: usize, color: PieceColor, pos: Position, prob: f64) {
        let color_idx = match color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };

        let color_dists = self.distributions.entry(piece_type).or_insert_with(|| {
            [
                vec![vec![0.0; self.board_size.1]; self.board_size.0],
                vec![vec![0.0; self.board_size.1]; self.board_size.0],
            ]
        });

        if pos.0 < self.board_size.0 && pos.1 < self.board_size.1 {
            color_dists[color_idx][pos.0][pos.1] = prob;
        }
    }

    /// Add probability (for accumulation during diffusion)
    fn add_probability(&mut self, piece_type: usize, color: PieceColor, pos: Position, prob: f64) {
        let current = self.get_probability(piece_type, color, pos);
        self.set_probability(piece_type, color, pos, current + prob);
    }

    /// Get total probability mass for a piece type and color
    fn get_total_probability(&self, piece_type: usize, color: PieceColor) -> f64 {
        let color_idx = match color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };

        self.distributions
            .get(&piece_type)
            .map(|color_dists| {
                color_dists[color_idx]
                    .iter()
                    .flat_map(|row| row.iter())
                    .sum()
            })
            .unwrap_or(0.0)
    }

    /// Get total probability mass for all pieces of a color
    fn get_total_mass(&self, color: PieceColor) -> f64 {
        let mut total = 0.0;
        for (&piece_type, _) in &self.distributions {
            total += self.get_total_probability(piece_type, color);
        }
        total
    }

    /// Get total probability of royal pieces for a color
    fn get_royal_probability(&self, color: PieceColor, config_manager: &PieceConfigManager) -> f64 {
        let mut total = 0.0;

        for (&piece_type, _) in &self.distributions {
            if let Some(piece_config) = config_manager.get_piece_by_index(piece_type) {
                if piece_config.properties.is_royal || piece_config.properties.is_royalty {
                    total += self.get_total_probability(piece_type, color);
                }
            }
        }

        total
    }

    /// Check if this color has any royal pieces
    fn has_royals(&self, color: PieceColor, config_manager: &PieceConfigManager) -> bool {
        self.get_royal_probability(color, config_manager) > 0.01
    }

    /// Clear small probabilities below threshold
    fn prune_small_probabilities(&mut self, threshold: f64) {
        for (_, color_dists) in &mut self.distributions {
            for color_idx in 0..2 {
                for row in 0..self.board_size.0 {
                    for col in 0..self.board_size.1 {
                        if color_dists[color_idx][row][col] < threshold {
                            color_dists[color_idx][row][col] = 0.0;
                        }
                    }
                }
            }
        }
    }

    /// Get density at a position (sum of all piece probabilities)
    fn get_density_at(&self, pos: Position) -> f64 {
        let mut density = 0.0;

        for (_, color_dists) in &self.distributions {
            for color_idx in 0..2 {
                if let Some(row) = color_dists[color_idx].get(pos.0) {
                    if let Some(&prob) = row.get(pos.1) {
                        density += prob;
                    }
                }
            }
        }

        density
    }

    /// Get enemy density at a position
    fn get_enemy_density_at(&self, pos: Position, my_color: PieceColor) -> f64 {
        let enemy_idx = match my_color {
            PieceColor::White => 1,
            PieceColor::Black => 0,
        };

        let mut density = 0.0;
        for (_, color_dists) in &self.distributions {
            if let Some(row) = color_dists[enemy_idx].get(pos.0) {
                if let Some(&prob) = row.get(pos.1) {
                    density += prob;
                }
            }
        }

        density
    }
}

/// Represents a move probability distribution
#[derive(Clone, Debug)]
struct MovePolicy {
    // From position -> (to position, probability)
    move_probabilities: HashMap<Position, Vec<(Position, f64)>>,
}

impl MovePolicy {
    /// Create move policy from current position
    fn from_position(
        state: &mut GameState, // FIX: Require mutable state
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        temperature: f64,
    ) -> Self {
        let mut policy = Self {
            move_probabilities: HashMap::new(),
        };

        // Get all legal moves
        let legal_moves = state.get_legal_moves(move_generator, config_manager);

        // Group moves by source position
        let mut moves_by_source: HashMap<Position, Vec<&ExpandedMove>> = HashMap::new();
        for mv in &legal_moves {
            moves_by_source.entry(mv.from).or_default().push(mv);
        }

        // Convert to probability distribution using softmax with temperature
        for (from, moves) in moves_by_source {
            let mut move_probs = Vec::new();

            // Score each move
            let scores: Vec<f64> = moves
                .iter()
                .map(|mv| {
                    let mut score = 1.0;

                    // Prefer captures
                    if mv.captures.is_some() {
                        score += 3.0;
                    }

                    // Prefer central moves
                    let center_row = state.board.rows() as f64 / 2.0;
                    let center_col = state.board.cols() as f64 / 2.0;
                    let dist_to_center = ((mv.to.0 as f64 - center_row).powi(2)
                        + (mv.to.1 as f64 - center_col).powi(2))
                    .sqrt();
                    score += 2.0 / (1.0 + dist_to_center);

                    // Slight preference for forward moves
                    if let Some(piece) = state.board.get_piece(from) {
                        let forward = match piece.color {
                            PieceColor::White => mv.to.0 < mv.from.0,
                            PieceColor::Black => mv.to.0 > mv.from.0,
                        };
                        if forward {
                            score += 0.5;
                        }
                    }

                    score
                })
                .collect();

            // Apply softmax with temperature
            let max_score = scores.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let exp_scores: Vec<f64> = scores
                .iter()
                .map(|&s| ((s - max_score) / temperature).exp())
                .collect();
            let sum_exp: f64 = exp_scores.iter().sum();

            for (i, mv) in moves.iter().enumerate() {
                let prob = exp_scores[i] / sum_exp;
                move_probs.push((mv.to, prob));
            }

            policy.move_probabilities.insert(from, move_probs);
        }

        policy
    }

    /// Get move probability distribution from a position
    fn get_move_distribution(&self, from: Position) -> Option<&Vec<(Position, f64)>> {
        self.move_probabilities.get(&from)
    }
}

/// Debug information for a diffusion step
#[derive(Clone, Debug)]
pub struct DiffusionStep {
    pub depth: usize,
    pub field: ProbabilityField,
    pub white_mass: f64,
    pub black_mass: f64,
    pub white_royal_probability: f64,
    pub black_royal_probability: f64,
}

/// Result of diffusion analysis for a move
#[derive(Clone, Debug)]
struct DiffusionResult {
    pub mv: ExpandedMove,
    pub final_score: f64,
    pub mass_score: f64,
    pub royal_score: f64,
    pub depth_reached: usize,
}

/// The main diffusion engine
pub struct DiffusionEngine {
    parameters: EngineParameters,
    debug_mode: bool,
}

impl DiffusionEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(DIFFUSION_PARAMETERS),
            debug_mode: false,
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    /// Perform diffusion analysis for a single move
    fn analyze_move(
        &self,
        state: &mut GameState, // FIX: Pass as mut to respect performance metrics
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> DiffusionResult {
        // Apply the move
        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);

        // Initialize probability field from the resulting position
        let mut field = ProbabilityField::from_board(&test_state.board, config_manager);

        // Get initial values
        let my_color = state.current_turn;
        let enemy_color = my_color.opposite();

        let initial_my_mass = field.get_total_mass(my_color);
        let initial_enemy_mass = field.get_total_mass(enemy_color);
        let initial_my_royal = field.get_royal_probability(my_color, config_manager);
        let initial_enemy_royal = field.get_royal_probability(enemy_color, config_manager);

        // Check if there are royals to track
        let track_my_royals = field.has_royals(my_color, config_manager);
        let track_enemy_royals = field.has_royals(enemy_color, config_manager);

        // Parameters
        let royal_reduction_target = self.get_param(PARAM_ROYAL_REDUCTION_TARGET, 0.2);
        let max_depth = self.get_param(PARAM_MAX_DEPTH, 50.0) as usize;
        let capture_probability = self.get_param(PARAM_CAPTURE_PROBABILITY, 0.05);
        let blocking_factor = self.get_param(PARAM_BLOCKING_FACTOR, 0.3);
        let policy_temperature = self.get_param(PARAM_POLICY_TEMPERATURE, 1.0);
        let min_threshold = self.get_param(PARAM_MIN_PROBABILITY_THRESHOLD, 0.001);

        let mut depth_reached = 0;

        // Diffusion loop
        for depth in 1..=max_depth {
            // Generate move policy for current player
            let current_player = if depth % 2 == 1 {
                test_state.current_turn
            } else {
                test_state.current_turn.opposite()
            };

            // Create theoretical move policy based on probability fields
            let move_policy = self.create_theoretical_move_policy(
                &field,
                current_player,
                move_generator,
                config_manager,
                policy_temperature,
            );

            // Apply diffusion step
            field = self.apply_diffusion_step(
                &field,
                &move_policy,
                current_player,
                capture_probability,
                blocking_factor,
                config_manager,
            );

            // Prune small probabilities
            field.prune_small_probabilities(min_threshold);

            depth_reached = depth;

            // Check termination conditions
            let current_my_royal = field.get_royal_probability(my_color, config_manager);
            let current_enemy_royal = field.get_royal_probability(enemy_color, config_manager);

            // Only check royal reduction if we're tracking royals
            if track_my_royals || track_enemy_royals {
                if track_my_royals && initial_my_royal > 0.0 {
                    let my_royal_reduction =
                        (initial_my_royal - current_my_royal) / initial_my_royal;
                    if my_royal_reduction >= royal_reduction_target {
                        break;
                    }
                }

                if track_enemy_royals && initial_enemy_royal > 0.0 {
                    let enemy_royal_reduction =
                        (initial_enemy_royal - current_enemy_royal) / initial_enemy_royal;
                    if enemy_royal_reduction >= royal_reduction_target {
                        break;
                    }
                }
            }

            // Also break if too much mass is lost overall
            let current_total_mass =
                field.get_total_mass(my_color) + field.get_total_mass(enemy_color);
            let initial_total_mass = initial_my_mass + initial_enemy_mass;
            if current_total_mass < initial_total_mass * 0.3 {
                break; // 70% of pieces have been captured
            }
        }

        // Calculate final scores
        let final_my_mass = field.get_total_mass(my_color);
        let final_enemy_mass = field.get_total_mass(enemy_color);
        let final_my_royal = field.get_royal_probability(my_color, config_manager);
        let final_enemy_royal = field.get_royal_probability(enemy_color, config_manager);

        // Mass score: ratio of my mass to enemy mass
        let mass_score = if final_enemy_mass > 0.0 {
            final_my_mass / final_enemy_mass
        } else if final_my_mass > 0.0 {
            10.0 // Enemy eliminated
        } else {
            0.1 // Both eliminated (bad)
        };

        // Royal score: depends on whether royals exist
        let royal_score = if track_my_royals && track_enemy_royals {
            // Both have royals: use ratio
            if final_enemy_royal > 0.0 {
                final_my_royal / final_enemy_royal
            } else if final_my_royal > 0.0 {
                10.0 // Enemy royals eliminated
            } else {
                0.1 // Both royals eliminated (bad)
            }
        } else if track_my_royals {
            // Only we have royals: preserve them
            final_my_royal / initial_my_royal.max(0.01)
        } else if track_enemy_royals {
            // Only enemy has royals: try to eliminate them
            if final_enemy_royal < initial_enemy_royal * 0.5 {
                2.0 // Good progress
            } else {
                1.0 // No progress
            }
        } else {
            // No royals: use mass only
            1.0
        };

        // Combine scores
        let mass_importance = self.get_param(PARAM_MASS_IMPORTANCE, 0.5);
        let royal_importance = self.get_param(PARAM_ROYAL_IMPORTANCE, 0.5);

        // AGGRESSION UPDATE: Reward reducing enemy material more than preserving ours
        let enemy_mass_reduction =
            (initial_enemy_mass - final_enemy_mass) / initial_enemy_mass.max(0.01);
        let my_mass_reduction = (initial_my_mass - final_my_mass) / initial_my_mass.max(0.01);

        // Positive if we reduced enemy more than they reduced us
        // Weight our losses less (0.5) to encourage aggressive trades
        let aggression_bonus = enemy_mass_reduction - my_mass_reduction * 0.5;

        let final_score = if track_my_royals || track_enemy_royals {
            // If royals exist, use weighted combination with aggression bonus
            let base_score = mass_score.powf(mass_importance) * royal_score.powf(royal_importance);
            base_score * (1.0 + aggression_bonus)
        } else {
            // No royals, just use mass with double weight on aggression
            mass_score * (1.0 + aggression_bonus * 2.0)
        };

        DiffusionResult {
            mv: mv.clone(),
            final_score,
            mass_score,
            royal_score,
            depth_reached,
        }
    }

    /// Create a theoretical move policy based on probability fields
    fn create_theoretical_move_policy(
        &self,
        field: &ProbabilityField,
        current_player: PieceColor,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        temperature: f64,
    ) -> MovePolicy {
        let mut policy = MovePolicy {
            move_probabilities: HashMap::new(),
        };

        // For each position with significant probability
        for (&piece_type, color_dists) in &field.distributions {
            let color_idx = match current_player {
                PieceColor::White => 0,
                PieceColor::Black => 1,
            };

            let piece_dists = &color_dists[color_idx];

            for row in 0..field.board_size.0 {
                for col in 0..field.board_size.1 {
                    let from_prob = piece_dists[row][col];
                    if from_prob < 0.01 {
                        continue; // Skip very low probability positions
                    }

                    let from = (row, col);

                    // Get theoretical moves for this piece type
                    let theoretical_moves = move_generator.get_theoretical_moves_for_piece(
                        from,
                        piece_type,
                        current_player,
                        field.board_size,
                        false, // Don't restrict to unmoved
                    );

                    if theoretical_moves.is_empty() {
                        continue;
                    }

                    // Score moves based on the field state
                    let mut move_scores = Vec::new();
                    for (to, can_capture, can_move_peacefully) in theoretical_moves {
                        let mut score = 1.0;

                        // Check enemy density at destination
                        let enemy_density = field.get_enemy_density_at(to, current_player);
                        let friendly_density = field.get_density_at(to) - enemy_density;

                        if can_capture && enemy_density > 0.0 {
                            // MUCH stronger preference for capturing
                            score += 10.0 + 20.0 * enemy_density;

                            // Even stronger bonus for attacking royals
                            for (&other_piece_type, _) in &field.distributions {
                                if let Some(piece_config) =
                                    config_manager.get_piece_by_index(other_piece_type)
                                {
                                    if piece_config.properties.is_royal
                                        || piece_config.properties.is_royalty
                                    {
                                        let royal_prob = field.get_probability(
                                            other_piece_type,
                                            current_player.opposite(),
                                            to,
                                        );
                                        if royal_prob > 0.0 {
                                            score += 100.0 * royal_prob;
                                        }
                                    }
                                }
                            }
                        }

                        // Penalize moving to squares with friendly pieces (avoid self-blocking)
                        score -= friendly_density * 2.0;

                        if can_move_peacefully {
                            // For non-captures, prefer squares that attack enemy pieces
                            // Check all adjacent squares for enemy pieces
                            for dr in -1..=1 {
                                for dc in -1..=1 {
                                    if dr == 0 && dc == 0 {
                                        continue;
                                    }
                                    let check_row = to.0 as i32 + dr;
                                    let check_col = to.1 as i32 + dc;

                                    if check_row >= 0
                                        && check_row < field.board_size.0 as i32
                                        && check_col >= 0
                                        && check_col < field.board_size.1 as i32
                                    {
                                        let check_pos = (check_row as usize, check_col as usize);
                                        let adjacent_enemy =
                                            field.get_enemy_density_at(check_pos, current_player);
                                        // Reward moves that put pressure on enemy pieces
                                        score += adjacent_enemy * 3.0;
                                    }
                                }
                            }

                            // Prefer moving to empty or low-density squares (still somewhat important)
                            let total_density = field.get_density_at(to);
                            score += (1.0 - total_density).max(0.0) * 0.5;
                        }

                        // Forward progress bonus (more aggressive)
                        let forward_progress = match current_player {
                            PieceColor::White => (from.0 as i32 - to.0 as i32).max(0) as f64,
                            PieceColor::Black => (to.0 as i32 - from.0 as i32).max(0) as f64,
                        };
                        score += forward_progress * 0.5;

                        // Central preference
                        let center_row = field.board_size.0 as f64 / 2.0;
                        let center_col = field.board_size.1 as f64 / 2.0;
                        let dist_to_center = ((to.0 as f64 - center_row).powi(2)
                            + (to.1 as f64 - center_col).powi(2))
                        .sqrt();
                        score += 1.0 / (1.0 + dist_to_center);

                        move_scores.push((to, score));
                    }

                    // Convert scores to probabilities with softmax
                    let max_score = move_scores
                        .iter()
                        .map(|(_, s)| *s)
                        .fold(f64::NEG_INFINITY, f64::max);

                    let exp_scores: Vec<f64> = move_scores
                        .iter()
                        .map(|(_, s)| ((s - max_score) / temperature).exp())
                        .collect();

                    let sum_exp: f64 = exp_scores.iter().sum();

                    if sum_exp > 0.0 {
                        let mut move_probs = Vec::new();
                        for (i, (to, _)) in move_scores.iter().enumerate() {
                            let prob = exp_scores[i] / sum_exp;
                            if prob > 0.01 {
                                // Only keep significant probabilities
                                move_probs.push((*to, prob));
                            }
                        }

                        if !move_probs.is_empty() {
                            // Renormalize after filtering
                            let total: f64 = move_probs.iter().map(|(_, p)| p).sum();
                            for (_, prob) in &mut move_probs {
                                *prob /= total;
                            }
                            policy.move_probabilities.insert(from, move_probs);
                        }
                    }
                }
            }
        }

        policy
    }

    /// Apply one step of diffusion
    fn apply_diffusion_step(
        &self,
        field: &ProbabilityField,
        move_policy: &MovePolicy,
        current_player: PieceColor,
        capture_probability: f64,
        blocking_factor: f64,
        config_manager: &PieceConfigManager,
    ) -> ProbabilityField {
        let mut new_field = ProbabilityField::new(field.board_size);

        // For each piece type and position
        for (&piece_type, color_dists) in &field.distributions {
            let color_idx = match current_player {
                PieceColor::White => 0,
                PieceColor::Black => 1,
            };

            // Process current player's pieces
            let piece_dists = &color_dists[color_idx];

            for row in 0..field.board_size.0 {
                for col in 0..field.board_size.1 {
                    let from = (row, col);
                    let from_prob = piece_dists[row][col];

                    if from_prob < 0.0001 {
                        continue;
                    }

                    // Get move distribution for this position
                    if let Some(moves) = move_policy.get_move_distribution(from) {
                        // Piece moves according to distribution
                        for &(to, move_prob) in moves {
                            // Check if move is blocked by density
                            let dest_density = field.get_density_at(to);
                            let blocking_prob = 1.0 - (blocking_factor * dest_density).min(0.9);

                            let effective_prob = from_prob * move_prob * blocking_prob;

                            if effective_prob > 0.0001 {
                                new_field.add_probability(
                                    piece_type,
                                    current_player,
                                    to,
                                    effective_prob,
                                );
                            }
                        }

                        // Some probability stays (piece doesn't move)
                        let total_move_prob: f64 = moves
                            .iter()
                            .map(|(to, p)| {
                                let density = field.get_density_at(*to);
                                let blocking = 1.0 - (blocking_factor * density).min(0.9);
                                p * blocking
                            })
                            .sum();
                        let stay_prob = 1.0 - total_move_prob.min(0.9); // At least 10% stays

                        if stay_prob > 0.0 {
                            new_field.add_probability(
                                piece_type,
                                current_player,
                                from,
                                from_prob * stay_prob,
                            );
                        }
                    } else {
                        // No moves available, piece stays
                        new_field.add_probability(piece_type, current_player, from, from_prob);
                    }
                }
            }

            // Opponent pieces stay in place (they don't move this turn)
            let opponent_idx = 1 - color_idx;
            let opponent_dists = &color_dists[opponent_idx];

            for row in 0..field.board_size.0 {
                for col in 0..field.board_size.1 {
                    let pos = (row, col);
                    let prob = opponent_dists[row][col];

                    if prob > 0.0001 {
                        new_field.add_probability(piece_type, current_player.opposite(), pos, prob);
                    }
                }
            }
        }

        // Apply capture interactions
        self.apply_captures(&mut new_field, capture_probability, config_manager);

        new_field
    }

    /// Apply probabilistic captures where pieces overlap
    fn apply_captures(
        &self,
        field: &mut ProbabilityField,
        capture_probability: f64,
        config_manager: &PieceConfigManager,
    ) {
        // For each square, check for overlapping pieces and apply capture
        for row in 0..field.board_size.0 {
            for col in 0..field.board_size.1 {
                let pos = (row, col);

                // Collect all pieces at this position
                let mut white_pieces = Vec::new();
                let mut black_pieces = Vec::new();

                for (&piece_type, color_dists) in &field.distributions {
                    let white_prob = color_dists[0][row][col];
                    let black_prob = color_dists[1][row][col];

                    if white_prob > 0.0001 {
                        white_pieces.push((piece_type, white_prob));
                    }
                    if black_prob > 0.0001 {
                        black_pieces.push((piece_type, black_prob));
                    }
                }

                // If pieces of opposite colors overlap, apply capture
                if !white_pieces.is_empty() && !black_pieces.is_empty() {
                    let white_density: f64 = white_pieces.iter().map(|(_, p)| p).sum();
                    let black_density: f64 = black_pieces.iter().map(|(_, p)| p).sum();

                    // White attacks Black
                    // Capture probability is proportional to enemy density AND attacker count
                    for (w_type, _) in &white_pieces {
                        let w_is_royal = config_manager
                            .get_piece_by_index(*w_type)
                            .map_or(false, |cfg| {
                                cfg.properties.is_royal || cfg.properties.is_royalty
                            });

                        // Non-royals are more aggressive attackers
                        let attack_modifier = if w_is_royal { 0.5 } else { 1.2 };

                        // Base capture rate + bonus for multiple attackers
                        let capture_rate = capture_probability * attack_modifier;
                        let effective_capture = (capture_rate
                            * black_density
                            * (1.0 + white_pieces.len() as f64 * 0.1))
                            .min(0.7);

                        // Update black pieces (being captured)
                        for (b_type, _) in &black_pieces {
                            let b_is_royal = config_manager
                                .get_piece_by_index(*b_type)
                                .map_or(false, |cfg| {
                                    cfg.properties.is_royal || cfg.properties.is_royalty
                                });

                            let defense_modifier = if b_is_royal { 0.7 } else { 1.0 };
                            let b_survives = 1.0 - (effective_capture * defense_modifier).min(0.9);

                            let current_prob =
                                field.get_probability(*b_type, PieceColor::Black, pos);
                            field.set_probability(
                                *b_type,
                                PieceColor::Black,
                                pos,
                                current_prob * b_survives,
                            );
                        }
                    }

                    // Black attacks White (Symmetric logic)
                    for (b_type, _) in &black_pieces {
                        let b_is_royal = config_manager
                            .get_piece_by_index(*b_type)
                            .map_or(false, |cfg| {
                                cfg.properties.is_royal || cfg.properties.is_royalty
                            });

                        let attack_modifier = if b_is_royal { 0.5 } else { 1.2 };
                        let capture_rate = capture_probability * attack_modifier;
                        let effective_capture = (capture_rate
                            * white_density
                            * (1.0 + black_pieces.len() as f64 * 0.1))
                            .min(0.7);

                        for (w_type, _) in &white_pieces {
                            let w_is_royal = config_manager
                                .get_piece_by_index(*w_type)
                                .map_or(false, |cfg| {
                                    cfg.properties.is_royal || cfg.properties.is_royalty
                                });

                            let defense_modifier = if w_is_royal { 0.7 } else { 1.0 };
                            let w_survives = 1.0 - (effective_capture * defense_modifier).min(0.9);

                            let current_prob =
                                field.get_probability(*w_type, PieceColor::White, pos);
                            field.set_probability(
                                *w_type,
                                PieceColor::White,
                                pos,
                                current_prob * w_survives,
                            );
                        }
                    }
                }
            }
        }
    }
}

impl ChessEngine for DiffusionEngine {
    fn name(&self) -> &str {
        "Diffusion Engine (Probabilistic Future Simulation)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let start_time = Instant::now();

        // Get all legal moves
        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);

        if legal_moves.is_empty() {
            return None;
        }

        println!(
            "\n🌊 Diffusion Engine analyzing {} moves...",
            legal_moves.len()
        );

        // Analyze each move
        let mut results = Vec::new();
        for (i, mv) in legal_moves.iter().enumerate() {
            let result = self.analyze_move(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
            );

            println!(
                "  Move {}/{}: {} → {}  |  Score: {:.3} (Mass: {:.3}, Royal: {:.3})  |  Depth: {}",
                i + 1,
                legal_moves.len(),
                crate::notation::position_to_algebraic(mv.from, params.state.board.size()),
                crate::notation::position_to_algebraic(mv.to, params.state.board.size()),
                result.final_score,
                result.mass_score,
                result.royal_score,
                result.depth_reached
            );

            results.push(result);

            // Check time limit
            if let Some(time_limit) = params.time_limit {
                if start_time.elapsed() >= time_limit * 9 / 10 {
                    println!("⏱️ Time limit approaching, stopping analysis");
                    break;
                }
            }
        }

        // Find best move (highest score)
        let best_result = results.iter().max_by(|a, b| {
            a.final_score
                .partial_cmp(&b.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        println!(
            "🎯 Best move: {} → {} with score {:.3}",
            crate::notation::position_to_algebraic(best_result.mv.from, params.state.board.size()),
            crate::notation::position_to_algebraic(best_result.mv.to, params.state.board.size()),
            best_result.final_score
        );

        // Convert score to integer evaluation
        // Use log scale for better integer resolution
        let score = if best_result.final_score > 10.0 {
            10000 // Overwhelming advantage
        } else if best_result.final_score < 0.1 {
            -10000 // Overwhelming disadvantage
        } else {
            // Log scale: score of 1.0 = 0, score of 2.0 = +693, score of 0.5 = -693
            (best_result.final_score.ln() * 1000.0) as i32
        };

        Some(SearchResult {
            best_move: best_result.mv.clone(),
            evaluation: Evaluation {
                score,
                mate_in: None,
            },
            depth_reached: best_result.depth_reached as u32,
        })
    }

    fn stop(&mut self) {
        // No async operations to stop
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(DIFFUSION_PARAMETERS)
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
