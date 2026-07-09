// src/engine/game_controller.rs
use crate::core::{GameState, PieceColor};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::engine::{ChessEngine, EngineType, SearchParams};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::time::{Duration, Instant};

// ─── Engine slot ────────────────────────────────────────────────────────

/// Configuration and instance for one engine role (White, Black, or Eval).
pub struct EngineSlot {
    engine_type: EngineType,
    engine: Option<Box<dyn ChessEngine>>,
    pub search_depth: u32,
    pub time_limit: Option<Duration>,
    pub time_respect: f32,
}

impl EngineSlot {
    fn new_player() -> Self {
        Self {
            engine_type: EngineType::Human,
            engine: None,
            search_depth: 3,
            time_limit: None,
            time_respect: 0.0,
        }
    }

    fn new_eval(et: EngineType) -> Self {
        Self {
            engine_type: et.clone(),
            engine: et.create(),
            search_depth: 4,
            time_limit: None,
            time_respect: 0.0,
        }
    }

    pub fn set_engine(&mut self, et: EngineType) {
        self.engine_type = et.clone();
        self.engine = et.create();
    }

    pub fn engine_type(&self) -> &EngineType { &self.engine_type }
    pub fn is_human(&self) -> bool { self.engine_type.is_human() }

    pub fn reset_cache(&mut self) {
        if let Some(e) = &mut self.engine { e.reset_cache(); }
    }

    pub fn take_engine(&mut self) -> Option<Box<dyn ChessEngine>> { self.engine.take() }
    pub fn put_engine(&mut self, engine: Box<dyn ChessEngine>) { self.engine = Some(engine); }

    pub fn parameters(&self) -> Option<EngineParameters> {
        self.engine.as_ref().and_then(|e| e.get_parameters())
    }
    pub fn parameter_defs(&self) -> Option<&'static [ParameterDef]> {
        self.engine.as_ref().and_then(|e| e.parameter_definitions())
    }
    pub fn set_parameters(&mut self, params: EngineParameters) -> bool {
        self.engine.as_mut().map_or(false, |e| e.set_parameters(params))
    }

    pub fn time_limit_secs(&self) -> Option<f32> { self.time_limit.map(|d| d.as_secs_f32()) }
    pub fn set_time_limit_secs(&mut self, s: Option<f32>) {
        self.time_limit = s.map(Duration::from_secs_f32);
    }
}

// ─── Game controller ────────────────────────────────────────────────────

pub struct GameController {
    white: EngineSlot,
    black: EngineSlot,
    eval: EngineSlot,
    auto_play: bool,
    thinking: bool,
    use_unlimited_depth_with_time: bool,
    white_time: Duration,
    black_time: Duration,
    current_turn_start: Option<Instant>,
}

impl GameController {
    pub fn new() -> Self {
        Self {
            white: EngineSlot::new_player(),
            black: EngineSlot::new_player(),
            eval: EngineSlot::new_eval(EngineType::Simple),
            auto_play: false,
            thinking: false,
            use_unlimited_depth_with_time: false,
            white_time: Duration::ZERO,
            black_time: Duration::ZERO,
            current_turn_start: None,
        }
    }

    // ─── Slot accessors ─────────────────────────────────────────────

    pub fn player_slot(&self, color: PieceColor) -> &EngineSlot {
        match color { PieceColor::White => &self.white, PieceColor::Black => &self.black }
    }
    pub fn player_slot_mut(&mut self, color: PieceColor) -> &mut EngineSlot {
        match color { PieceColor::White => &mut self.white, PieceColor::Black => &mut self.black }
    }
    pub fn eval_slot(&self) -> &EngineSlot { &self.eval }
    pub fn eval_slot_mut(&mut self) -> &mut EngineSlot { &mut self.eval }

    // ─── Convenience wrappers (keep old API names thin) ─────────────

    pub fn set_white_engine(&mut self, et: EngineType) { self.white.set_engine(et); }
    pub fn set_black_engine(&mut self, et: EngineType) { self.black.set_engine(et); }
    pub fn set_eval_engine(&mut self, et: EngineType) { self.eval.set_engine(et); }

    pub fn get_white_engine_type(&self) -> &EngineType { self.white.engine_type() }
    pub fn get_black_engine_type(&self) -> &EngineType { self.black.engine_type() }
    pub fn get_eval_engine_type(&self) -> &EngineType { self.eval.engine_type() }

    pub fn is_engine_turn(&self, color: PieceColor) -> bool {
        !self.player_slot(color).is_human()
    }

    pub fn reset_engine_caches(&mut self) {
        self.white.reset_cache();
        self.black.reset_cache();
        self.eval.reset_cache();
    }

    // ─── Auto-play / thinking ───────────────────────────────────────

    pub fn set_auto_play(&mut self, e: bool) { self.auto_play = e; }
    pub fn is_auto_play(&self) -> bool { self.auto_play }
    pub fn is_thinking(&self) -> bool { self.thinking }
    pub fn set_thinking(&mut self, t: bool) { self.thinking = t; }

    pub fn set_unlimited_depth_with_time(&mut self, e: bool) { self.use_unlimited_depth_with_time = e; }
    pub fn get_unlimited_depth_with_time(&self) -> bool { self.use_unlimited_depth_with_time }

    // ─── Timing ─────────────────────────────────────────────────────

    pub fn start_turn(&mut self, _color: PieceColor) {
        if self.current_turn_start.is_none() {
            self.current_turn_start = Some(Instant::now());
        }
    }

    pub fn end_turn(&mut self, color: PieceColor) {
        if let Some(start) = self.current_turn_start.take() {
            // Add the asterisk (*) to dereference the Duration reference inside the tuple
            *self.accumulated_time_mut(color).0 += start.elapsed();
        }
    }

    pub fn stop_timing(&mut self, color: PieceColor) { self.end_turn(color); }
    pub fn is_timing_active(&self) -> bool { self.current_turn_start.is_some() }
    pub fn get_white_time(&self) -> Duration { self.white_time }
    pub fn get_black_time(&self) -> Duration { self.black_time }

    pub fn get_current_thinking_time(&self, _color: PieceColor) -> Duration {
        self.current_turn_start.map_or(Duration::ZERO, |s| s.elapsed())
    }

    pub fn reset_timers(&mut self) {
        self.white_time = Duration::ZERO;
        self.black_time = Duration::ZERO;
        self.current_turn_start = None;
    }

    fn accumulated_time_mut(&mut self, color: PieceColor) -> (&mut Duration,) {
        match color {
            PieceColor::White => (&mut self.white_time,),
            PieceColor::Black => (&mut self.black_time,),
        }
    }

    fn time_pair(&self, color: PieceColor) -> (Duration, Duration) {
        match color {
            PieceColor::White => (self.white_time, self.black_time),
            PieceColor::Black => (self.black_time, self.white_time),
        }
    }

    // ─── Search budget ──────────────────────────────────────────────

    fn adjusted_time_limit(&self, color: PieceColor) -> Option<Duration> {
        let slot = self.player_slot(color);
        let base = slot.time_limit?;
        let respect = slot.time_respect;
        if respect == 0.0 { return Some(base); }

        let (my, opp) = self.time_pair(color);
        let diff = opp.as_secs_f32() - my.as_secs_f32();
        let adjusted = (base.as_secs_f32() + diff * respect).max(0.1);
        Some(Duration::from_secs_f32(adjusted))
    }

    pub fn compute_search_budget(&self, color: PieceColor) -> (u32, Option<Duration>) {
        let slot = self.player_slot(color);
        let adj = self.adjusted_time_limit(color);
        let depth = if adj.is_some() && self.use_unlimited_depth_with_time {
            99
        } else {
            slot.search_depth
        };
        (depth, adj)
    }

    // ─── Engine execution ───────────────────────────────────────────

    pub fn make_engine_move(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::api::SearchResult> {
        let color = state.current_turn;
        self.start_turn(color);

        let (depth, time_limit) = self.compute_search_budget(color);
        let engine = &mut self.player_slot_mut(color).engine;

        if let Some(engine) = engine {
            let multiplicative = engine.get_parameters().map_or(false, |p| {
                p.get_or_default(crate::engine::search::PARAM_MULTIPLICATIVE_EVAL, 0.0) > 0.5
            });

            let params = SearchParams {
                state, move_generator, config_manager, time_limit, depth,
            };

            if let Some(result) = engine.best_move(params) {
                println!(
                    "\n{:?} ({}) plays: {}{} -> {}{}",
                         color, engine.name(),
                         (b'a' + result.best_move.from.1 as u8) as char,
                         8u32.saturating_sub(result.best_move.from.0 as u32),
                         (b'a' + result.best_move.to.1 as u8) as char,
                         8u32.saturating_sub(result.best_move.to.0 as u32),
                );

                if let Some(mate_in) = result.evaluation.mate_in {
                    if mate_in > 0 {
                        println!("  Mate in {} moves!", mate_in);
                    } else {
                        println!("  Getting mated in {} moves!", -mate_in);
                    }
                } else if multiplicative {
                    let ratio = crate::engine::search::score_to_ratio(result.evaluation.score);
                    if ratio >= 1.0 {
                        println!("  {:?}'s position is {:.2}x better", color, ratio);
                    } else {
                        println!("  {:?}'s position is {:.2}x better", color.opposite(), 1.0 / ratio);
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
        let slot = &mut self.eval;
        if let Some(engine) = &mut slot.engine {
            let depth = if slot.time_limit.is_some() && self.use_unlimited_depth_with_time {
                99
            } else {
                slot.search_depth
            };
            let params = SearchParams {
                state, move_generator, config_manager,
                time_limit: slot.time_limit, depth,
            };
            engine.best_move(params)
        } else { None }
    }

    pub fn analyze_position(
        &mut self,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Option<crate::engine::analysis::PositionAnalysis> {
        self.eval.engine.as_mut()?.analyze_position(state, move_generator, config_manager)
    }

    pub fn supports_analysis(&self) -> bool {
        self.eval.engine.as_ref().map_or(false, |e| e.supports_analysis())
    }

    // ------------------ Legacy interface ---------------

    // --- Depth ---
    pub fn get_white_search_depth(&self) -> u32 { self.white.search_depth }
    pub fn set_white_search_depth(&mut self, d: u32) { self.white.search_depth = d; }
    pub fn get_black_search_depth(&self) -> u32 { self.black.search_depth }
    pub fn set_black_search_depth(&mut self, d: u32) { self.black.search_depth = d; }
    pub fn get_eval_search_depth(&self) -> u32 { self.eval.search_depth }
    pub fn set_eval_search_depth(&mut self, d: u32) { self.eval.search_depth = d; }

    // --- Time Limits ---
    pub fn get_white_time_limit(&self) -> Option<f32> { self.white.time_limit_secs() }
    pub fn set_white_time_limit(&mut self, s: Option<f32>) { self.white.set_time_limit_secs(s); }
    pub fn get_black_time_limit(&self) -> Option<f32> { self.black.time_limit_secs() }
    pub fn set_black_time_limit(&mut self, s: Option<f32>) { self.black.set_time_limit_secs(s); }
    pub fn get_eval_time_limit(&self) -> Option<f32> { self.eval.time_limit_secs() }
    pub fn set_eval_engine_limit(&mut self, s: Option<f32>) { self.eval.set_time_limit_secs(s); }

    pub fn set_eval_time_limit(&mut self, s: Option<f32>) {
        self.eval.set_time_limit_secs(s);
    }

    // --- Time Respect ---
    pub fn get_white_time_respect(&self) -> f32 { self.white.time_respect }
    pub fn set_white_time_respect(&mut self, r: f32) { self.white.time_respect = r; }
    pub fn get_black_time_respect(&self) -> f32 { self.black.time_respect }
    pub fn set_black_time_respect(&mut self, r: f32) { self.black.time_respect = r; }

    // --- Engine Parameters (Delegation) ---
    pub fn get_white_engine_parameters(&self) -> Option<crate::engine::parameters::EngineParameters> { self.white.parameters() }
    pub fn set_white_engine_parameters(&mut self, p: crate::engine::parameters::EngineParameters) -> bool { self.white.set_parameters(p) }
    pub fn get_white_engine_parameter_defs(&self) -> Option<&'static [crate::engine::parameters::ParameterDef]> { self.white.parameter_defs() }

    pub fn get_black_engine_parameters(&self) -> Option<crate::engine::parameters::EngineParameters> { self.black.parameters() }
    pub fn set_black_engine_parameters(&mut self, p: crate::engine::parameters::EngineParameters) -> bool { self.black.set_parameters(p) }
    pub fn get_black_engine_parameter_defs(&self) -> Option<&'static [crate::engine::parameters::ParameterDef]> { self.black.parameter_defs() }

    pub fn get_eval_engine_parameters(&self) -> Option<crate::engine::parameters::EngineParameters> { self.eval.parameters() }
    pub fn set_eval_engine_parameters(&mut self, p: crate::engine::parameters::EngineParameters) -> bool { self.eval.set_parameters(p) }
    pub fn get_eval_engine_parameter_defs(&self) -> Option<&'static [crate::engine::parameters::ParameterDef]> { self.eval.parameter_defs() }



    pub fn take_engine(&mut self, color: PieceColor) -> Option<Box<dyn crate::engine::ChessEngine>> {
        match color {
            PieceColor::White => self.white.take_engine(),
            PieceColor::Black => self.black.take_engine(),
        }
    }

    pub fn put_engine(&mut self, color: PieceColor, engine: Box<dyn crate::engine::ChessEngine>) {
        match color {
            PieceColor::White => self.white.put_engine(engine),
            PieceColor::Black => self.black.put_engine(engine),
        }
    }
}

impl Default for GameController {
    fn default() -> Self { Self::new() }
}
