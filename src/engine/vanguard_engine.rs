// src/engine/vanguard_engine.rs

use crate::core::board::Board;
use crate::core::game_state::{ExpandedMove, GameState, MateStatus};
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Parameter Definitions
// ─────────────────────────────────────────────────────────────────────────────

const V_MATERIAL: &str = "vanguard_material";
const V_SAFETY: &str = "vanguard_safety";
const V_RESCUE: &str = "vanguard_rescue";
const V_OUTPOST: &str = "vanguard_outpost";
const V_THREATS: &str = "vanguard_threats";
const V_ADVANCEMENT: &str = "vanguard_advancement";
const V_CENTER: &str = "vanguard_center";
const V_KING_HUNT: &str = "vanguard_king_hunt";

pub static VANGUARD_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        V_MATERIAL,
        "Material Base",
        "Base weight for capturing material.",
        0.0,
        500.0,
        100.0,
        5.0,
    ),
    ParameterDef::new(
        V_SAFETY,
        "Destination Safety",
        "Penalty for moving to a square controlled by the enemy.",
        0.0,
        500.0,
        120.0,
        5.0,
    ),
    ParameterDef::new(
        V_RESCUE,
        "Rescue Bonus",
        "Reward for moving a piece that is currently under attack.",
        0.0,
        300.0,
        80.0,
        5.0,
    ),
    ParameterDef::new(
        V_OUTPOST,
        "Outpost Bonus",
        "Reward for moving to a square defended by a cheaper friendly piece.",
        0.0,
        100.0,
        25.0,
        1.0,
    ),
    ParameterDef::new(
        V_THREATS,
        "Threat Generation",
        "Reward for attacking enemy pieces from the new square.",
        0.0,
        200.0,
        15.0,
        1.0,
    ),
    ParameterDef::new(
        V_ADVANCEMENT,
        "Advancement",
        "Small positional reward for pushing pieces forward.",
        0.0,
        20.0,
        2.0,
        0.5,
    ),
    ParameterDef::new(
        V_CENTER,
        "Centralization",
        "Reward for moving closer to the center of the board.",
        0.0,
        50.0,
        8.0,
        1.0,
    ),
    ParameterDef::new(
        V_KING_HUNT,
        "King Hunt",
        "Reward for moving closer to the enemy royal (crucial for endgames).",
        0.0,
        50.0,
        10.0,
        1.0,
    ),
];

// ─────────────────────────────────────────────────────────────────────────────
// Engine Implementation
// ─────────────────────────────────────────────────────────────────────────────

pub struct VanguardEngine {
    parameters: EngineParameters,
    piece_values: HashMap<usize, f64>,
    board_size: (usize, usize),
}

impl VanguardEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(VANGUARD_PARAMETERS),
            piece_values: HashMap::new(),
            board_size: (0, 0),
        }
    }

    #[inline]
    fn p(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    fn ensure_context(
        &mut self,
        state: &GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let size = state.board.size();
        if self.board_size == size && !self.piece_values.is_empty() {
            return;
        }
        self.board_size = size;
        self.piece_values.clear();

        let center = (size.0 / 2, size.1 / 2);
        for (idx, name) in config_manager.piece_order.iter().enumerate() {
            if let Some(cfg) = config_manager.pieces.get(name) {
                if cfg.properties.is_royal || cfg.properties.is_royalty {
                    self.piece_values.insert(idx, 10_000.0);
                    continue;
                }

                let moves = move_generator.get_theoretical_moves_for_piece(
                    center,
                    idx,
                    PieceColor::White,
                    size,
                    false,
                );
                let mobility = moves.iter().filter(|(_, capture, _)| *capture).count() as f64;
                self.piece_values.insert(idx, (mobility + 1.0).ln() * 100.0);
            }
        }

        let min_val = self
            .piece_values
            .values()
            .filter(|&&v| v < 5000.0)
            .fold(f64::INFINITY, |a, &b| a.min(b));
        if min_val > 0.0 && min_val.is_finite() {
            let scale = 100.0 / min_val;
            for v in self.piece_values.values_mut() {
                if *v < 5000.0 {
                    *v *= scale;
                }
            }
        }
    }

    fn value_of(&self, piece_type: usize) -> f64 {
        self.piece_values.get(&piece_type).copied().unwrap_or(100.0)
    }

    fn evaluate_exchange(
        &self,
        board: &Board,
        square: Position,
        active_color: PieceColor,
        initial_victim_val: f64,
        ignore_square: Option<Position>,
        move_generator: &MoveGenerator,
    ) -> f64 {
        let mut get_attackers = |color: PieceColor| -> Vec<f64> {
            let mut vals: Vec<f64> = move_generator
                .get_attackers_to_square(board, square, color)
                .into_iter()
                .filter(|(pos, _)| Some(*pos) != ignore_square)
                .map(|(_, p)| self.value_of(p.piece_type))
                .collect();
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            vals
        };

        let mut attackers = [
            get_attackers(PieceColor::White),
            get_attackers(PieceColor::Black),
        ];

        if attackers[active_color.index()].is_empty() {
            return 0.0;
        }

        let mut gains = [0.0; 32];
        let mut d = 0;
        gains[0] = initial_victim_val;

        let mut on_square_val = attackers[active_color.index()].remove(0);
        let mut to_move = active_color.opposite();

        while d < 31 {
            d += 1;
            gains[d] = on_square_val - gains[d - 1];
            if gains[d].max(-gains[d - 1]) < 0.0 { /* Stand pat allowed */ }
            let idx = to_move.index();
            if attackers[idx].is_empty() {
                break;
            }
            on_square_val = attackers[idx].remove(0);
            to_move = to_move.opposite();
        }

        for i in (1..=d).rev() {
            gains[i - 1] = (-gains[i]).max(gains[i - 1]);
        }

        gains[0]
    }

    fn evaluate_move(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        enemy_royals: &[Position],
    ) -> f64 {
        // 0. Immediate Mate Check (Don't let the game slip away!)
        let mut test_state = state.clone();
        test_state.execute_expanded_move(mv, move_generator, config_manager);
        if test_state.get_mate_status(move_generator, config_manager) == MateStatus::Checkmate {
            return 999_999.0;
        }

        let board = &state.board;
        let us = state.current_turn;
        let them = us.opposite();

        let mover = board.get_piece(mv.from).unwrap();
        let mover_val = self.value_of(mover.piece_type);
        let captured_val = mv.captures.map_or(0.0, |p| self.value_of(p.piece_type));

        let mut score = 0.0;

        // 1. Material & Destination Safety
        let enemy_counter_see =
            self.evaluate_exchange(board, mv.to, them, mover_val, Some(mv.from), move_generator);
        let tactical_delta = captured_val - enemy_counter_see.max(0.0);
        score += tactical_delta * (self.p(V_MATERIAL, 100.0) / 100.0);
        if tactical_delta < 0.0 {
            score += tactical_delta * (self.p(V_SAFETY, 120.0) / 100.0);
        }

        // 2. Rescue Bonus
        let origin_see =
            self.evaluate_exchange(board, mv.from, them, mover_val, None, move_generator);
        if origin_see > 0.0 && tactical_delta >= 0.0 {
            score += origin_see * (self.p(V_RESCUE, 80.0) / 100.0);
        }

        // 3. Outposts
        let friendly_defenders = move_generator.get_attackers_to_square(board, mv.to, us);
        if !friendly_defenders.is_empty() {
            let cheapest_defender_val = friendly_defenders
                .iter()
                .filter(|(pos, _)| *pos != mv.from)
                .map(|(_, p)| self.value_of(p.piece_type))
                .fold(f64::INFINITY, f64::min);

            if cheapest_defender_val.is_finite() {
                let structural_bonus = if cheapest_defender_val < mover_val {
                    2.0
                } else {
                    1.0
                };
                score += structural_bonus * self.p(V_OUTPOST, 25.0);
            }
        }

        // 4. Threats
        let projected_moves = move_generator.get_theoretical_moves_for_piece(
            mv.to,
            mover.piece_type,
            us,
            board.size(),
            false,
        );
        let mut threat_score = 0.0;
        for &(target_pos, can_capture, _) in &projected_moves {
            if can_capture && target_pos != mv.from {
                if let Some(target) = board.get_piece(target_pos) {
                    if target.color == them && !enemy_royals.contains(&target_pos) {
                        let val = self.value_of(target.piece_type);
                        let is_defended =
                            move_generator.is_square_attacked(board, target_pos, them);
                        threat_score += val * if is_defended { 0.05 } else { 0.15 };
                    }
                }
            }
        }
        score += threat_score * (self.p(V_THREATS, 15.0) / 100.0);

        // 5. Centralization (Prevents spatial suffocation)
        let (rows, cols) = board.size();
        let cr = (rows as f64 - 1.0) / 2.0;
        let cc = (cols as f64 - 1.0) / 2.0;
        let old_c_dist = ((mv.from.0 as f64 - cr).powi(2) + (mv.from.1 as f64 - cc).powi(2)).sqrt();
        let new_c_dist = ((mv.to.0 as f64 - cr).powi(2) + (mv.to.1 as f64 - cc).powi(2)).sqrt();
        score += (old_c_dist - new_c_dist) * self.p(V_CENTER, 8.0);

        // 6. King Hunting (Prevents endgame wandering)
        if !enemy_royals.is_empty() {
            let nearest_from = enemy_royals
                .iter()
                .map(|&r| {
                    (mv.from.0 as f64 - r.0 as f64)
                        .abs()
                        .max((mv.from.1 as f64 - r.1 as f64).abs())
                })
                .fold(f64::INFINITY, f64::min);
            let nearest_to = enemy_royals
                .iter()
                .map(|&r| {
                    (mv.to.0 as f64 - r.0 as f64)
                        .abs()
                        .max((mv.to.1 as f64 - r.1 as f64).abs())
                })
                .fold(f64::INFINITY, f64::min);
            score += (nearest_from - nearest_to) * self.p(V_KING_HUNT, 10.0);
        }

        // 7. Advancement
        let advancement = match us {
            PieceColor::White => (mv.from.0 as f64) - (mv.to.0 as f64),
            PieceColor::Black => (mv.to.0 as f64) - (mv.from.0 as f64),
        };
        score += advancement * self.p(V_ADVANCEMENT, 2.0);

        // 8. Promotions & Castling
        if let Some(pt) = mv.promotion_target {
            score += self.value_of(pt) * (self.p(V_MATERIAL, 100.0) / 100.0);
        }
        if mv.castling_option.is_some() {
            score += 50.0;
        }

        score
    }
}

impl ChessEngine for VanguardEngine {
    fn name(&self) -> &str {
        "Vanguard Engine (Pure Geometric Policy)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        self.ensure_context(params.state, params.move_generator, params.config_manager);

        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);
        if legal_moves.is_empty() {
            return None;
        }

        let enemy_royals: Vec<Position> = params
            .state
            .board
            .get_royal_positions(params.state.current_turn.opposite())
            .to_vec();

        let mut best_move = None;
        let mut best_score = f64::NEG_INFINITY;

        for mv in &legal_moves {
            let score = self.evaluate_move(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
                &enemy_royals,
            );

            let hash_noise = (mv.to.0 * 17 + mv.to.1 * 31) as f64 * 0.001;
            let final_score = score + hash_noise;

            if final_score > best_score {
                best_score = final_score;
                best_move = Some(mv.clone());
            }
        }

        best_move.map(|mv| SearchResult {
            best_move: mv,
            evaluation: Evaluation {
                score: (best_score * 100.0) as i32,
                mate_in: None,
            },
            depth_reached: 0,
        })
    }

    fn stop(&mut self) {}
    fn reset_cache(&mut self) {
        self.piece_values.clear();
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(VANGUARD_PARAMETERS)
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
