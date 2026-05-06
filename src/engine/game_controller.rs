// src/engine/game_controller.rs
use crate::core::{GameState, PieceColor};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::{ChessEngine, EngineType, SearchParams};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::time::{Duration, Instant};

pub struct GameController {
    white_engine_type: EngineType,
    black_engine_type: EngineType,
    eval_engine_type: EngineType,
    white_engine: Option<Box<dyn ChessEngine>>,
    black_engine: Option<Box<dyn ChessEngine>>,
    eval_engine: Option<Box<dyn ChessEngine>>,
    auto_play: bool,
    thinking: bool,
    white_search_depth: u32,
    black_search_depth: u32,
    eval_search_depth: u32,
    time_limit: Option<Duration>,
    white_time_limit: Option<Duration>,
    black_time_limit: Option<Duration>,
    eval_time_limit: Option<Duration>,
    use_unlimited_depth_with_time: bool,
    white_time_respect: f32,
    black_time_respect: f32,
    white_time: Duration,
    black_time: Duration,
    current_turn_start: Option<Instant>,
}

impl GameController {
    pub fn new() -> Self {
        let eval_engine_type = EngineType::Simple;
        Self {
            white_engine_type: EngineType::Human,
            black_engine_type: EngineType::Human,
            eval_engine_type: eval_engine_type.clone(),
            white_engine: None,
            black_engine: None,
            eval_engine: eval_engine_type.create(),
            auto_play: false,
            thinking: false,
            white_search_depth: 3,
            black_search_depth: 3,
            eval_search_depth: 4,
            time_limit: None,
            white_time_limit: None,
            black_time_limit: None,
            eval_time_limit: None,
            use_unlimited_depth_with_time: false,
            white_time_respect: 0.0,
            black_time_respect: 0.0,
            white_time: Duration::ZERO,
            black_time: Duration::ZERO,
            current_turn_start: None,
        }
    }

    pub fn start_turn(&mut self, _color: PieceColor) {
        if self.current_turn_start.is_none() {
            self.current_turn_start = Some(Instant::now());
        }
    }

    pub fn end_turn(&mut self, color: PieceColor) {
        if let Some(start) = self.current_turn_start.take() {
            let elapsed = start.elapsed();
            match color {
                PieceColor::White => self.white_time += elapsed,
                PieceColor::Black => self.black_time += elapsed,
            }
        }
    }

    pub fn stop_timing(&mut self, current_turn: PieceColor) {
        if let Some(start) = self.current_turn_start.take() {
            let elapsed = start.elapsed();
            match current_turn {
                PieceColor::White => self.white_time += elapsed,
                PieceColor::Black => self.black_time += elapsed,
            }
        }
    }

    pub fn is_timing_active(&self) -> bool {
        self.current_turn_start.is_some()
    }

    pub fn get_white_time(&self) -> Duration {
        self.white_time
    }

    pub fn get_black_time(&self) -> Duration {
        self.black_time
    }

    pub fn get_current_thinking_time(&self, _color: PieceColor) -> Duration {
        if let Some(start) = self.current_turn_start {
            start.elapsed()
        } else {
            Duration::ZERO
        }
    }

    pub fn reset_timers(&mut self) {
        self.white_time = Duration::ZERO;
        self.black_time = Duration::ZERO;
        self.current_turn_start = None;
    }

    pub fn reset_engine_caches(&mut self) {
        if let Some(engine) = &mut self.white_engine {
            engine.reset_cache();
        }
        if let Some(engine) = &mut self.black_engine {
            engine.reset_cache();
        }
        if let Some(engine) = &mut self.eval_engine {
            engine.reset_cache();
        }
    }

    // ─── Threaded‑play support ──────────────────────────────────────────
    //
    // The GUI thread takes the engine out, ships it to a worker, and puts
    // it back when the result arrives so that transposition tables and
    // other per‑engine caches persist across moves.

    /// Take ownership of the engine for `color`. Returns `None` if the
    /// slot is Human or the engine has already been taken.
    pub fn take_engine(&mut self, color: PieceColor) -> Option<Box<dyn ChessEngine>> {
        match color {
            PieceColor::White => self.white_engine.take(),
            PieceColor::Black => self.black_engine.take(),
        }
    }

    /// Return a previously‑taken engine to its slot.
    pub fn put_engine(&mut self, color: PieceColor, engine: Box<dyn ChessEngine>) {
        match color {
            PieceColor::White => self.white_engine = Some(engine),
            PieceColor::Black => self.black_engine = Some(engine),
        }
    }

    /// Compute the (depth, time limit) to use for `color`'s next move,
    /// applying unlimited‑depth‑with‑time and time‑respect adjustments.
    /// This is the budget half of `make_engine_move`, extracted so the
    /// GUI can compute it before dispatching the engine to a worker.
    pub fn compute_search_budget(&self, color: PieceColor) -> (u32, Option<Duration>) {
        let (depth, base_time) = match color {
            PieceColor::White => (self.white_search_depth, self.white_time_limit),
            PieceColor::Black => (self.black_search_depth, self.black_time_limit),
        };
        let adjusted = self.calculate_adjusted_time_limit(base_time, color);

        if let (Some(b), Some(a)) = (base_time, adjusted) {
            let diff = a.as_secs_f32() - b.as_secs_f32();
            if diff.abs() > 0.01 {
                println!(
                    "⏱️ Time adjusted: {:.1}s → {:.1}s ({:+.1}s due to time respect)",
                    b.as_secs_f32(),
                    a.as_secs_f32(),
                    diff
                );
            }
        }

        let actual_depth = if adjusted.is_some() && self.use_unlimited_depth_with_time {
            99
        } else {
            depth
        };
        (actual_depth, adjusted)
    }

    pub fn set_white_engine(&mut self, engine_type: EngineType) {
        self.white_engine_type = engine_type.clone();
        self.white_engine = engine_type.create();
    }

    pub fn set_black_engine(&mut self, engine_type: EngineType) {
        self.black_engine_type = engine_type.clone();
        self.black_engine = engine_type.create();
    }

    pub fn set_eval_engine(&mut self, engine_type: EngineType) {
        self.eval_engine_type = engine_type.clone();
        self.eval_engine = engine_type.create();
    }

    pub fn get_white_engine_type(&self) -> &EngineType {
        &self.white_engine_type
    }

    pub fn get_black_engine_type(&self) -> &EngineType {
        &self.black_engine_type
    }

    pub fn get_eval_engine_type(&self) -> &EngineType {
        &self.eval_engine_type
    }

    pub fn is_engine_turn(&self, current_turn: PieceColor) -> bool {
        match current_turn {
            PieceColor::White => !self.white_engine_type.is_human(),
            PieceColor::Black => !self.black_engine_type.is_human(),
        }
    }

    pub fn set_auto_play(&mut self, enabled: bool) {
        self.auto_play = enabled;
    }

    pub fn is_auto_play(&self) -> bool {
        self.auto_play
    }

    pub fn is_thinking(&self) -> bool {
        self.thinking
    }

    pub fn set_thinking(&mut self, thinking: bool) {
        self.thinking = thinking;
    }

    pub fn set_white_search_depth(&mut self, depth: u32) {
        self.white_search_depth = depth;
    }

    pub fn set_black_search_depth(&mut self, depth: u32) {
        self.black_search_depth = depth;
    }

    pub fn set_eval_search_depth(&mut self, depth: u32) {
        self.eval_search_depth = depth;
    }

    pub fn get_white_search_depth(&self) -> u32 {
        self.white_search_depth
    }

    pub fn get_black_search_depth(&self) -> u32 {
        self.black_search_depth
    }

    pub fn get_eval_search_depth(&self) -> u32 {
        self.eval_search_depth
    }

    pub fn set_white_time_limit(&mut self, seconds: Option<f32>) {
        self.white_time_limit = seconds.map(|s| Duration::from_secs_f32(s));
    }

    pub fn set_black_time_limit(&mut self, seconds: Option<f32>) {
        self.black_time_limit = seconds.map(|s| Duration::from_secs_f32(s));
    }

    pub fn set_eval_time_limit(&mut self, seconds: Option<f32>) {
        self.eval_time_limit = seconds.map(|s| Duration::from_secs_f32(s));
    }

    pub fn get_white_time_limit(&self) -> Option<f32> {
        self.white_time_limit.map(|d| d.as_secs_f32())
    }

    pub fn get_black_time_limit(&self) -> Option<f32> {
        self.black_time_limit.map(|d| d.as_secs_f32())
    }

    pub fn get_eval_time_limit(&self) -> Option<f32> {
        self.eval_time_limit.map(|d| d.as_secs_f32())
    }

    pub fn set_unlimited_depth_with_time(&mut self, enabled: bool) {
        self.use_unlimited_depth_with_time = enabled;
    }

    pub fn get_unlimited_depth_with_time(&self) -> bool {
        self.use_unlimited_depth_with_time
    }

    pub fn set_white_time_respect(&mut self, respect: f32) {
        self.white_time_respect = respect.clamp(0.0, 1.0);
    }

    pub fn set_black_time_respect(&mut self, respect: f32) {
        self.black_time_respect = respect.clamp(0.0, 1.0);
    }

    pub fn get_white_time_respect(&self) -> f32 {
        self.white_time_respect
    }

    pub fn get_black_time_respect(&self) -> f32 {
        self.black_time_respect
    }

    fn calculate_adjusted_time_limit(
        &self,
        base_time: Option<Duration>,
        color: PieceColor,
    ) -> Option<Duration> {
        let base_time = base_time?;

        let (my_time, opponent_time, time_respect) = match color {
            PieceColor::White => (self.white_time, self.black_time, self.white_time_respect),
            PieceColor::Black => (self.black_time, self.white_time, self.black_time_respect),
        };

        if time_respect == 0.0 {
            return Some(base_time);
        }

        let time_diff = opponent_time.as_secs_f32() - my_time.as_secs_f32();
        let adjustment = time_diff * time_respect;
        let adjusted_seconds = (base_time.as_secs_f32() + adjustment).max(0.1);

        Some(Duration::from_secs_f32(adjusted_seconds))
    }

    pub fn get_white_engine_parameters(&self) -> Option<EngineParameters> {
        self.white_engine.as_ref().and_then(|e| e.get_parameters())
    }

    pub fn get_black_engine_parameters(&self) -> Option<EngineParameters> {
        self.black_engine.as_ref().and_then(|e| e.get_parameters())
    }

    pub fn get_eval_engine_parameters(&self) -> Option<EngineParameters> {
        self.eval_engine.as_ref().and_then(|e| e.get_parameters())
    }

    pub fn get_white_engine_parameter_defs(&self) -> Option<&'static [ParameterDef]> {
        self.white_engine
            .as_ref()
            .and_then(|e| e.parameter_definitions())
    }

    pub fn get_black_engine_parameter_defs(&self) -> Option<&'static [ParameterDef]> {
        self.black_engine
            .as_ref()
            .and_then(|e| e.parameter_definitions())
    }

    pub fn get_eval_engine_parameter_defs(&self) -> Option<&'static [ParameterDef]> {
        self.eval_engine
            .as_ref()
            .and_then(|e| e.parameter_definitions())
    }

    pub fn set_white_engine_parameters(&mut self, params: EngineParameters) -> bool {
        if let Some(engine) = &mut self.white_engine {
            engine.set_parameters(params)
        } else {
            false
        }
    }

    pub fn set_black_engine_parameters(&mut self, params: EngineParameters) -> bool {
        if let Some(engine) = &mut self.black_engine {
            engine.set_parameters(params)
        } else {
            false
        }
    }

    pub fn set_eval_engine_parameters(&mut self, params: EngineParameters) -> bool {
        if let Some(engine) = &mut self.eval_engine {
            engine.set_parameters(params)
        } else {
            false
        }
    }

    pub fn make_engine_move(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::api::SearchResult> {
        let current_color = state.current_turn;
        self.start_turn(current_color);

        let (depth, base_time_limit) = match current_color {
            PieceColor::White => (self.white_search_depth, self.white_time_limit),
            PieceColor::Black => (self.black_search_depth, self.black_time_limit),
        };

        let adjusted_time_limit =
            self.calculate_adjusted_time_limit(base_time_limit, current_color);

        if let (Some(base), Some(adjusted)) = (base_time_limit, adjusted_time_limit) {
            let diff = adjusted.as_secs_f32() - base.as_secs_f32();
            if diff.abs() > 0.01 {
                println!(
                    "⏱️ Time adjusted: {:.1}s → {:.1}s ({:+.1}s due to time respect)",
                    base.as_secs_f32(),
                    adjusted.as_secs_f32(),
                    diff
                );
            }
        }

        let engine = match current_color {
            PieceColor::White => &mut self.white_engine,
            PieceColor::Black => &mut self.black_engine,
        };

        if let Some(engine) = engine {
            let actual_depth =
                if adjusted_time_limit.is_some() && self.use_unlimited_depth_with_time {
                    99
                } else {
                    depth
                };

            let params = SearchParams {
                state,
                move_generator,
                config_manager,
                time_limit: adjusted_time_limit,
                depth: actual_depth,
            };

            if let Some(result) = engine.best_move(params) {
                println!(
                    "\n{} ({}) plays: {}{} -> {}{}",
                    match current_color {
                        PieceColor::White => "White",
                        PieceColor::Black => "Black",
                    },
                    engine.name(),
                    (b'a' + result.best_move.from.1 as u8) as char,
                    8 - result.best_move.from.0,
                    (b'a' + result.best_move.to.1 as u8) as char,
                    8 - result.best_move.to.0
                );

                if let Some(mate_in) = result.evaluation.mate_in {
                    if mate_in > 0 {
                        println!("  Mate in {} moves!", mate_in);
                    } else {
                        println!("  Getting mated in {} moves!", -mate_in);
                    }
                } else {
                    println!("  Evaluation: {}", result.evaluation.score);
                }

                return Some(result);
            }
        }
        None
    }

    pub fn evaluate_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::api::SearchResult> {
        if let Some(engine) = &mut self.eval_engine {
            let actual_depth =
                if self.eval_time_limit.is_some() && self.use_unlimited_depth_with_time {
                    99
                } else {
                    self.eval_search_depth
                };

            let params = SearchParams {
                state,
                move_generator,
                config_manager,
                time_limit: self.eval_time_limit,
                depth: actual_depth,
            };

            if let Some(result) = engine.best_move(params) {
                return Some(result);
            }
        }
        None
    }

    pub fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::analysis::PositionAnalysis> {
        if let Some(engine) = &mut self.eval_engine {
            return engine.analyze_position(state, move_generator, config_manager);
        }
        None
    }

    pub fn supports_analysis(&self) -> bool {
        if let Some(engine) = &self.eval_engine {
            return engine.supports_analysis();
        }
        false
    }
}

impl Default for GameController {
    fn default() -> Self {
        Self::new()
    }
}
