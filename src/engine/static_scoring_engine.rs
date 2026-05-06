// src/engine/static_scoring_engine.rs
//
// A variant-agnostic static scoring engine.
//
// Evaluates every legal move from the current position using purely static
// features and picks the move with the highest score.  No game-tree search,
// no make/unmake.
//
// Scoring categories:
//   1. Material  – value of captured piece.
//   2. Safety    – risk of losing the moving piece at its destination.
//   3. Activity  – threats created against enemy pieces, especially royalty.
//   4. Position  – centrality gain and piece development.
//   5. Promotion – value gained through promotion.
//
// Piece values are estimated once per board size from theoretical mobility,
// making the engine fully variant-agnostic.

use crate::core::game_state::{ExpandedMove, GameState};
use crate::core::piece::PieceColor;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::parameters::{EngineParameters, ParameterDef, ParameterizedEngine};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::{HashMap, HashSet};

// ─── Parameter identifiers ──────────────────────────────────
pub const PARAM_MATERIAL_WEIGHT: &str = "material_weight";
pub const PARAM_SAFETY_WEIGHT: &str = "safety_weight";
pub const PARAM_ACTIVITY_WEIGHT: &str = "activity_weight";
pub const PARAM_POSITIONAL_WEIGHT: &str = "positional_weight";
pub const PARAM_CHECK_BONUS: &str = "check_bonus";

pub static STATIC_SCORING_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        PARAM_MATERIAL_WEIGHT,
        "Material Weight",
        "Weight for capture value.  Higher prefers captures more strongly.",
        0.0,
        5.0,
        1.0,
        0.1,
    ),
    ParameterDef::new(
        PARAM_SAFETY_WEIGHT,
        "Safety Weight",
        "Weight for piece safety.  Higher avoids hanging pieces more.",
        0.0,
        5.0,
        1.0,
        0.1,
    ),
    ParameterDef::new(
        PARAM_ACTIVITY_WEIGHT,
        "Activity Weight",
        "Weight for threats and piece activity.  Higher attacks more.",
        0.0,
        5.0,
        0.5,
        0.1,
    ),
    ParameterDef::new(
        PARAM_POSITIONAL_WEIGHT,
        "Positional Weight",
        "Weight for centrality and development.  Higher plays more positionally.",
        0.0,
        5.0,
        0.3,
        0.1,
    ),
    ParameterDef::new(
        PARAM_CHECK_BONUS,
        "Check Bonus",
        "Flat bonus for moves that appear to give check.",
        0.0,
        1000.0,
        300.0,
        10.0,
    ),
];

// ─── Engine ─────────────────────────────────────────────────
pub struct StaticScoringEngine {
    parameters: EngineParameters,
    piece_values: HashMap<usize, f64>,
    board_size: (usize, usize),
}

impl StaticScoringEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(STATIC_SCORING_PARAMETERS),
            piece_values: HashMap::new(),
            board_size: (0, 0),
        }
    }

    fn get_param(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    /// Lazily initialise piece-value table when the board size changes.
    fn ensure_initialised(
        &mut self,
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        if self.board_size == board_size && !self.piece_values.is_empty() {
            return;
        }
        self.board_size = board_size;
        self.piece_values = Self::estimate_piece_values(move_generator, config_manager, board_size);
        println!("🎯 Static Scoring Engine – estimated piece values ({board_size:?} board):");
        for (idx, name) in config_manager.piece_order.iter().enumerate() {
            if let Some(&v) = self.piece_values.get(&idx) {
                println!("   {name}: {v:.0}");
            }
        }
    }

    /// Estimate piece values from theoretical mobility on the current board.
    ///
    /// Each piece type is placed at the board centre on a hypothetical empty
    /// board and its reachable squares are counted (unique destinations).
    /// `ln(mobility + 1)` gives a natural logarithmic scaling that
    /// diminishes marginal value for each additional square.  Royal pieces
    /// receive a fixed high value since losing them loses the game.
    ///
    /// After computation the weakest non-royal piece is normalised to 100.
    fn estimate_piece_values(
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        board_size: (usize, usize),
    ) -> HashMap<usize, f64> {
        let mut values = HashMap::new();
        let center = (board_size.0 / 2, board_size.1 / 2);
        for (idx, name) in config_manager.piece_order.iter().enumerate() {
            if let Some(cfg) = config_manager.pieces.get(name) {
                // Royal pieces get a fixed very high value.
                if cfg.properties.is_royal || cfg.properties.is_royalty {
                    values.insert(idx, 10_000.0);
                    continue;
                }
                let moves = move_generator.get_theoretical_moves_for_piece(
                    center,
                    idx,
                    PieceColor::White,
                    board_size,
                    false,
                );
                let unique_dests: HashSet<_> = moves.iter().map(|(pos, _, _)| *pos).collect();
                let mobility = unique_dests.len() as f64;
                values.insert(idx, (mobility + 1.0).ln() * 100.0);
            }
        }
        // Normalise so the weakest non-royal piece has value 100.
        let min_val = values
            .values()
            .filter(|&&v| v < 5_000.0 && v > 0.0)
            .fold(f64::INFINITY, |a, &b| a.min(b));
        if min_val > 0.0 && min_val.is_finite() {
            let scale = 100.0 / min_val;
            for v in values.values_mut() {
                if *v < 5_000.0 {
                    *v *= scale;
                }
            }
        }
        values
    }

    #[inline]
    fn piece_value(&self, piece_type: usize) -> f64 {
        self.piece_values.get(&piece_type).copied().unwrap_or(100.0)
    }

    /// Score a single legal move using static features only.
    fn score_move(
        &self,
        state: &GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> f64 {
        let board = &state.board;
        let our_color = state.current_turn;
        let enemy_color = our_color.opposite();

        let moving_piece = match board.get_piece(mv.from) {
            Some(p) => p,
            None => return f64::NEG_INFINITY,
        };
        let our_value = self.piece_value(moving_piece.piece_type);

        let mat_w = self.get_param(PARAM_MATERIAL_WEIGHT, 1.0);
        let saf_w = self.get_param(PARAM_SAFETY_WEIGHT, 1.0);
        let act_w = self.get_param(PARAM_ACTIVITY_WEIGHT, 0.5);
        let pos_w = self.get_param(PARAM_POSITIONAL_WEIGHT, 0.3);
        let chk_b = self.get_param(PARAM_CHECK_BONUS, 300.0);

        let mut score = 0.0;

        // ── 1. Material: value of captured piece ────────────
        if let Some(captured) = mv.captures {
            score += self.piece_value(captured.piece_type) * mat_w;
        }

        // ── 2. Safety: is the piece safe at its destination? ─
        //
        // We query the *current* board for attackers/defenders of the
        // destination square.  This is an approximation (the board will
        // change after the move) but catches the vast majority of hanging-
        // piece situations.
        let enemy_attackers = move_generator.get_attackers_to_square(board, mv.to, enemy_color);

        // The captured piece (if any) won't exist after the move.
        let real_attackers: Vec<_> = enemy_attackers
            .iter()
            .filter(|(pos, _)| mv.captures_position.map_or(true, |cp| *pos != cp))
            .collect();

        if !real_attackers.is_empty() {
            // Friendly pieces that can recapture, excluding the moving piece
            // (it IS the piece on the square, not a defender of it).
            let friendly_defenders =
                move_generator.get_attackers_to_square(board, mv.to, our_color);
            let defender_count = friendly_defenders
                .iter()
                .filter(|(pos, _)| *pos != mv.from)
                .count();

            if defender_count == 0 {
                // Completely undefended – the piece will be captured for free.
                score -= our_value * saf_w;
            } else {
                // Defended, but the cheapest attacker may still win.
                let cheapest_attacker = real_attackers
                    .iter()
                    .map(|(_, p)| self.piece_value(p.piece_type))
                    .fold(f64::INFINITY, f64::min);
                if cheapest_attacker < our_value {
                    score -= (our_value - cheapest_attacker) * saf_w * 0.7;
                }
            }
        }

        // Bonus for escaping an existing attack.
        if move_generator.is_square_attacked(board, mv.from, enemy_color)
            && real_attackers.is_empty()
        {
            score += our_value * saf_w * 0.3;
        }

        // ── 3. Activity: threats created from the destination ─
        //
        // We use theoretical moves (ignoring blocking) to see which enemy
        // pieces fall within the attack pattern of our piece from the new
        // square.  This over-counts for sliders but provides a useful
        // signal, and accurately captures leaper attacks.
        {
            let theoretical = move_generator.get_theoretical_moves_for_piece(
                mv.to,
                moving_piece.piece_type,
                our_color,
                board.size(),
                false,
            );

            let mut already_counted_check = false;

            for &(target_pos, can_capture, _) in &theoretical {
                if !can_capture {
                    continue;
                }
                // Don't re-count the piece we are already capturing.
                if mv.captures_position == Some(target_pos) {
                    continue;
                }
                if let Some(target) = board.get_piece(target_pos) {
                    if target.color != enemy_color {
                        continue;
                    }
                    let target_val = self.piece_value(target.piece_type);
                    let is_royal = target.is_royal
                        || config_manager
                            .get_piece_by_index(target.piece_type)
                            .map_or(false, |c| c.properties.is_royal || c.properties.is_royalty);

                    if is_royal && !already_counted_check {
                        score += chk_b;
                        already_counted_check = true;
                    } else if !is_royal {
                        let defended =
                            move_generator.is_square_attacked(board, target_pos, enemy_color);
                        if !defended {
                            score += target_val * act_w * 0.4;
                        } else if target_val > our_value {
                            score += (target_val - our_value) * act_w * 0.15;
                        }
                    }
                }
            }
        }

        // ── 4. Positional: centrality and development ────────
        {
            let (rows, cols) = board.size();
            let cr = (rows - 1) as f64 / 2.0;
            let cc = (cols - 1) as f64 / 2.0;

            let old_dist =
                ((mv.from.0 as f64 - cr).powi(2) + (mv.from.1 as f64 - cc).powi(2)).sqrt();
            let new_dist = ((mv.to.0 as f64 - cr).powi(2) + (mv.to.1 as f64 - cc).powi(2)).sqrt();

            let max_dist = (cr.powi(2) + cc.powi(2)).sqrt();
            if max_dist > 0.0 {
                score += (old_dist - new_dist) / max_dist * 50.0 * pos_w;
            }

            // Small bonus for developing an unmoved non-royal piece.
            if moving_piece.move_count == 0 && !moving_piece.is_royal {
                score += 20.0 * pos_w;
            }
        }

        // ── 5. Promotion bonus ───────────────────────────────
        if let Some(promo_type) = mv.promotion_target {
            let promo_val = self.piece_value(promo_type);
            score += (promo_val - our_value).max(0.0) * mat_w;
        }

        score
    }
}

// ─── ParameterizedEngine ────────────────────────────────────
impl ParameterizedEngine for StaticScoringEngine {
    fn parameter_definitions(&self) -> &'static [ParameterDef] {
        STATIC_SCORING_PARAMETERS
    }

    fn get_parameters(&self) -> &EngineParameters {
        &self.parameters
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        let changed = self.parameters != params;
        if changed {
            self.parameters = params;
        }
        changed
    }

    fn on_parameters_changed(&mut self) {}
}

// ─── ChessEngine ────────────────────────────────────────────
impl ChessEngine for StaticScoringEngine {
    fn name(&self) -> &str {
        "Static Scoring Engine (Static Move Scoring)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let board_size = params.state.board.size();
        self.ensure_initialised(board_size, params.move_generator, params.config_manager);

        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);

        if legal_moves.is_empty() {
            return None;
        }

        let mut best_move = &legal_moves[0];
        let mut best_score = f64::NEG_INFINITY;

        for mv in &legal_moves {
            let s = self.score_move(
                params.state,
                mv,
                params.move_generator,
                params.config_manager,
            );
            if s > best_score {
                best_score = s;
                best_move = mv;
            }
        }

        Some(SearchResult {
            best_move: best_move.clone(),
            evaluation: Evaluation {
                score: best_score as i32,
                mate_in: None,
            },
            depth_reached: 0, // No search performed.
        })
    }

    fn stop(&mut self) {
        // No long-running computation to stop.
    }

    fn reset_cache(&mut self) {
        self.piece_values.clear();
        self.board_size = (0, 0);
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(ParameterizedEngine::parameter_definitions(self))
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(ParameterizedEngine::get_parameters(self).clone())
    }

    fn set_parameters(&mut self, params: EngineParameters) -> bool {
        ParameterizedEngine::set_parameters(self, params)
    }
}
