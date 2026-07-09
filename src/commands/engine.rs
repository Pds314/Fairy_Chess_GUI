use crate::app::ChessGui;
use crate::engine::EngineType;
use crate::clog;
use std::time::Duration;

impl ChessGui {
    pub(crate) fn cmd_engine(&mut self, parts: &[&str]) {
        if parts.len() >= 2 {
            self.set_engine_from_terminal(&parts[1..]);
        } else {
            self.print_engine_status();
        }
    }

    pub(crate) fn cmd_best(&mut self) {
        clog!("🎯 Making best move...");
        if self.is_game_ongoing() {
            self.start_engine_move();
        } else {
            clog!("❌ Game is already over.");
        }
    }

    pub(crate) fn cmd_depth(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            clog!("❌ Usage: depth <w|b|e> <number>");
            return;
        }
        let Ok(depth) = parts[2].parse::<u32>() else {
            clog!("❌ Invalid depth. Must be a positive number");
            return;
        };
        match parts[1] {
            "w" | "white" => {
                self.game_controller.set_white_search_depth(depth);
                clog!("✅ White search depth set to {}", depth);
            }
            "b" | "black" => {
                self.game_controller.set_black_search_depth(depth);
                clog!("✅ Black search depth set to {}", depth);
            }
            "e" | "eval" => {
                self.game_controller.set_eval_search_depth(depth);
                clog!("✅ Evaluation search depth set to {}", depth);
            }
            _ => clog!("❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"),
        }
    }

    pub(crate) fn cmd_time(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            clog!("❌ Usage: time <w|b|e> <seconds|off>");
            return;
        }
        let time_limit = if parts[2] == "off" || parts[2] == "none" {
            None
        } else if let Ok(s) = parts[2].parse::<f32>() {
            if s > 0.0 {
                Some(s)
            } else {
                clog!("❌ Time must be positive");
                return;
            }
        } else {
            clog!("❌ Invalid time. Use a number in seconds or 'off'");
            return;
        };
        match parts[1] {
            "w" | "white" => {
                self.game_controller.set_white_time_limit(time_limit);
                self.white_time_input = time_limit.map(|t| t.to_string()).unwrap_or_default();
                clog!("✅ White time limit: {}", fmt_time(time_limit));
            }
            "b" | "black" => {
                self.game_controller.set_black_time_limit(time_limit);
                self.black_time_input = time_limit.map(|t| t.to_string()).unwrap_or_default();
                clog!("✅ Black time limit: {}", fmt_time(time_limit));
            }
            "e" | "eval" => {
                self.game_controller.set_eval_time_limit(time_limit);
                self.eval_time_input = time_limit.map(|t| t.to_string()).unwrap_or_default();
                clog!("✅ Evaluation time limit: {}", fmt_time(time_limit));
            }
            _ => clog!("❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"),
        }
    }

    pub(crate) fn cmd_respect(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            clog!("❌ Usage: respect <w|b> <0.0-1.0>");
            return;
        }
        let Ok(respect) = parts[2].parse::<f32>() else {
            clog!("❌ Invalid value. Must be between 0.0 and 1.0");
            return;
        };
        if !(0.0..=1.0).contains(&respect) {
            clog!("❌ Time respect must be between 0.0 and 1.0");
            return;
        }
        match parts[1] {
            "w" | "white" => {
                self.game_controller.set_white_time_respect(respect);
                self.white_time_respect_input = respect.to_string();
                clog!("✅ White time respect set to {:.2}", respect);
            }
            "b" | "black" => {
                self.game_controller.set_black_time_respect(respect);
                self.black_time_respect_input = respect.to_string();
                clog!("✅ Black time respect set to {:.2}", respect);
            }
            _ => clog!("❌ Invalid target. Use 'w' (white) or 'b' (black)"),
        }
    }

    pub(crate) fn cmd_unlimited(&mut self) {
        let current = self.game_controller.get_unlimited_depth_with_time();
        self.game_controller.set_unlimited_depth_with_time(!current);
        clog!(
            "✅ Unlimited depth with time limits: {}",
            if !current { "enabled" } else { "disabled" }
        );
    }

    pub(crate) fn cmd_param(&mut self, parts: &[&str]) {
        if parts.len() >= 4 {
            let target = parts[1];
            let param_id = parts[2];
            let Ok(value) = parts[3].parse::<f64>() else {
                clog!("❌ Invalid value. Must be a number");
                return;
            };
            match target {
                "w" | "white" => self.set_param_for(
                    "White",
                    param_id,
                    value,
                    |gc| gc.get_white_engine_parameters(),
                    |gc, p| gc.set_white_engine_parameters(p),
                ),
                "b" | "black" => self.set_param_for(
                    "Black",
                    param_id,
                    value,
                    |gc| gc.get_black_engine_parameters(),
                    |gc, p| gc.set_black_engine_parameters(p),
                ),
                "e" | "eval" => {
                    self.set_param_for(
                        "Eval",
                        param_id,
                        value,
                        |gc| gc.get_eval_engine_parameters(),
                        |gc, p| gc.set_eval_engine_parameters(p),
                    );
                    self.position_analysis = None;
                }
                _ => clog!("❌ Invalid target. Use 'w', 'b', or 'e'"),
            }
        } else if parts.len() >= 2 {
            match parts[1] {
                "w" | "white" => self.print_engine_parameters(
                    "White",
                    self.game_controller.get_white_engine_parameter_defs(),
                    self.game_controller.get_white_engine_parameters(),
                ),
                "b" | "black" => self.print_engine_parameters(
                    "Black",
                    self.game_controller.get_black_engine_parameter_defs(),
                    self.game_controller.get_black_engine_parameters(),
                ),
                "e" | "eval" => self.print_engine_parameters(
                    "Eval",
                    self.game_controller.get_eval_engine_parameter_defs(),
                    self.game_controller.get_eval_engine_parameters(),
                ),
                _ => clog!("❌ Invalid target. Use 'w', 'b', or 'e'"),
            }
        } else {
            clog!("❌ Usage: param <w|b|e> [param_id] [value]");
        }
    }

    /// Helper to set a parameter, reducing copy-paste across white/black/eval.
    fn set_param_for(
        &mut self,
        label: &str,
        param_id: &str,
        value: f64,
        getter: fn(
            &crate::engine::GameController,
        )
            -> Option<crate::engine::parameters::EngineParameters>,
        setter: fn(
            &mut crate::engine::GameController,
            crate::engine::parameters::EngineParameters,
        ) -> bool,
    ) {
        if let Some(mut params) = getter(&self.game_controller) {
            params.set(param_id, value);
            if setter(&mut self.game_controller, params) {
                clog!(
                    "✅ {} engine parameter '{}' set to {:.3}",
                    label,
                    param_id,
                    value
                );
            } else {
                clog!("❌ Failed to set parameter");
            }
        } else {
            clog!("❌ {} engine has no tunable parameters", label);
        }
    }

    pub(crate) fn set_engine_from_terminal(&mut self, args: &[&str]) {
        let engine_name = args.join(" ");
        if let Some(engine) = crate::engine::personality::parse_engine_name(&engine_name) {
            if engine.is_human() {
                clog!("❌ Cannot use Human as evaluation engine");
                return;
            }
            self.game_controller.set_eval_engine(engine.clone());
            clog!("🤖 Evaluation engine set to: {}", engine.name());
        } else {
            clog!("❌ Unknown engine: '{}'. Available engines:", engine_name);
            for et in EngineType::all() {
                if !et.is_human() {
                    clog!("  - {}", et.name());
                }
            }
        }
    }

    pub(crate) fn print_engine_status(&self) {
        clog!("🤖 ENGINE STATUS:");
        clog!(
            "  White: {} (depth: {}, time: {}, respect: {:.2})",
            self.game_controller.get_white_engine_type().name(),
            self.game_controller.get_white_search_depth(),
            fmt_time(self.game_controller.get_white_time_limit()),
            self.game_controller.get_white_time_respect()
        );
        clog!(
            "  Black: {} (depth: {}, time: {}, respect: {:.2})",
            self.game_controller.get_black_engine_type().name(),
            self.game_controller.get_black_search_depth(),
            fmt_time(self.game_controller.get_black_time_limit()),
            self.game_controller.get_black_time_respect()
        );
        clog!(
            "  Eval:  {} (depth: {}, time: {})",
            self.game_controller.get_eval_engine_type().name(),
            self.game_controller.get_eval_search_depth(),
            fmt_time(self.game_controller.get_eval_time_limit())
        );
        clog!(
            "  Auto-play: {}",
            if self.game_controller.is_auto_play() { "ON" } else { "OFF" }
        );
        clog!(
            "  Unlimited depth with time: {}",
            if self.game_controller.get_unlimited_depth_with_time() { "ON" } else { "OFF" }
        );
        let wt = self.game_controller.get_white_time();
        let bt = self.game_controller.get_black_time();
        if wt != Duration::ZERO || bt != Duration::ZERO {
            clog!(
                "  Time used: White {:.1}s, Black {:.1}s (diff: {:+.1}s)",
                wt.as_secs_f32(),
                bt.as_secs_f32(),
                wt.as_secs_f32() - bt.as_secs_f32()
            );
        }
    }

    pub(crate) fn print_engine_parameters(
        &self,
        engine_name: &str,
        defs: Option<&'static [crate::engine::parameters::ParameterDef]>,
        params: Option<crate::engine::parameters::EngineParameters>,
    ) {
        clog!("⚙️ {} ENGINE PARAMETERS:", engine_name);
        let Some(defs) = defs else {
            clog!("  No tunable parameters available");
            return;
        };
        if defs.is_empty() {
            clog!("  No tunable parameters available");
            return;
        }
        let params = params.unwrap_or_default();
        for def in defs {
            let current = params.get_or_default(def.id, def.default);
            clog!("  {} ({}):", def.display_name, def.id);
            clog!(
                "    Current: {:.3}, Default: {:.3}, Range: [{:.2}, {:.2}]",
                current,
                def.default,
                def.min,
                def.max
            );
            clog!("    {}", def.description);
        }
    }
}

fn fmt_time(t: Option<f32>) -> String {
    t.map(|s| format!("{}s", s))
        .unwrap_or_else(|| "none".to_string())
}
