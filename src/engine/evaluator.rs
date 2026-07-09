use crate::core::game_state::ExpandedMove;
use crate::core::game_state::MoveGenerationResult;
use crate::core::{GameState, Piece, Position};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;

pub trait EvaluatorTrait {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32;

    /// Absolute / multiplicative evaluation hook.
    ///
    /// Returns `(own, opp)` — two INDEPENDENT, non-negative "goodness"
    /// scores for the side to move and its opponent. The search converts
    /// these into a log-ratio leaf score so alpha-beta optimises `own/opp`
    /// multiplicatively. `None` (the default) means this evaluator has no
    /// meaningful per-side decomposition; the search then falls back to
    /// `evaluate()` and behaves exactly as before.
    fn evaluate_split(
        &self,
        _state: &mut GameState,
        _move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> Option<(f32, f32)> {
        None
    }

    fn get_piece_value_on_square(
        &self,
        _piece: &Piece,
        _pos: Position,
        _config_manager: &PieceConfigManager,
    ) -> i32 {
        100
    }

    /// Safety margin for delta pruning in quiescence search. See module docs.
    fn delta_pruning_margin(&self) -> i32 {
        200
    }

    /// Aspiration-window half-width in this evaluator's unit scale.
    ///
    /// After the first iteration of iterative deepening, alpha-beta is
    /// re-rooted with `alpha = prev_score - W` and `beta = prev_score + W`.
    /// Too small ⇒ frequent fail-high/low re-searches (slower). Too large
    /// ⇒ no pruning benefit (no slower than plain search, just no win).
    ///
    /// Default `50` is appropriate for engines that return centipawn-ish
    /// values where a pawn ≈ 100. Override for any engine using a
    /// different scale (mobility-only evaluators want ~5; engines that
    /// post-multiply by 100 want ~1500).
    ///
    /// Return `0` to disable aspiration windows entirely for this engine.
    fn aspiration_window(&self) -> i32 {
        50
    }

    /// Search contempt: how much the side to move should dislike a draw,
    /// in THIS evaluator's score units. The search scores repetition /
    /// forced draws as `-contempt` for the root side (a winning engine
    /// steers away from repetitions) and `+contempt` for the opponent.
    /// Default 0 = flat draw scoring (draws are still detected correctly;
    /// there is just no tie-break steering). Keep this small relative to a
    /// "pawn" in your scale, or shuffling positions will be mis-evaluated.
    fn contempt(&self) -> i32 {
        0
    }

    fn calculate_delta(
        &self,
        state: &mut GameState,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let score_before = self.evaluate(state, move_generator, config_manager);
        state.execute_expanded_move(mv, move_generator, config_manager);
        let score_after = self.evaluate(state, move_generator, config_manager);
        state.undo_move(config_manager);
        -score_after - score_before
    }
}

pub struct Evaluator;

impl Evaluator {
    pub fn new() -> Self { Evaluator }

    pub fn evaluate_position_for_player(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        let player_moves = match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(moves) => moves,
            MoveGenerationResult::Checkmate { .. } => return 999999,
        };
        if player_moves.is_empty() {
            if state.is_in_check(move_generator, config_manager) { return -999999; }
            if !state.board.has_pieces(state.current_turn) { return -999999; }
            return 0;
        }
        let player_mobility = player_moves.len() as i32;
        let mut min_opponent_mobility = i32::MAX;
        let mut valid_moves_tested = 0;
        for mv in player_moves {
            state.execute_expanded_move(&mv, move_generator, config_manager);
            match state.generate_pseudo_legal_moves(move_generator, config_manager) {
                MoveGenerationResult::Moves(opp_moves) => {
                    min_opponent_mobility = min_opponent_mobility.min(opp_moves.len() as i32);
                    valid_moves_tested += 1;
                }
                MoveGenerationResult::Checkmate { .. } => { valid_moves_tested += 1; }
            };
            state.undo_move(config_manager);
        }
        if valid_moves_tested == 0 { return -999999; }
        player_mobility - min_opponent_mobility
    }
}

impl EvaluatorTrait for Evaluator {
    fn evaluate(
        &self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> i32 {
        match state.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Checkmate { .. } => 999999,
            MoveGenerationResult::Moves(moves) => {
                if moves.is_empty() {
                    if state.is_in_check(move_generator, config_manager) { return -999999; }
                    if !state.board.has_pieces(state.current_turn) { return -999999; }
                    0
                } else {
                    self.evaluate_position_for_player(state, move_generator, config_manager)
                }
            }
        }
    }

    /// Mobility-scale: typical eval is single-digit to a few tens.
    fn delta_pruning_margin(&self) -> i32 { 5 }

    /// Mobility-scale: ±5 is "noticeable" here.
    fn aspiration_window(&self) -> i32 { 2 }

    fn contempt(&self) -> i32 { 1 }
}
