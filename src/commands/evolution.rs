use crate::app::ChessGui;
use crate::evolution::{self, EvolutionSettings};
use crate::clog;
impl ChessGui {
    pub(crate) fn cmd_evolve(&mut self, parts: &[&str]) {
        if parts.len() < 2 {
            self.print_evolve_help();
            return;
        }
        match parts[1] {
            "start" => self.cmd_evolve_start(&parts[2..]),
            "stop" => self.handle_evolution_stop(),
            "status" => self.evolution.print_status(),
            "best" => match self.evolution.best() {
                Some(best) => clog!(
                    "🏆 Best individual: #{} rating {:.0} (gen {}, {}g)",
                                    best.id, best.rating, best.generation, best.games_played
                ),
                None => clog!("No individuals yet — run `evolve start` first."),
            },
            "export" => self.cmd_evolve_export(&parts[2..]),
            "set" => self.cmd_evolve_set(&parts[2..]),
            "lock" => self.cmd_evolve_lock(&parts[2..], true),
            "unlock" => self.cmd_evolve_lock(&parts[2..], false),
            "locks" => self.cmd_evolve_list_locks(),
            _ => self.print_evolve_help(),
        }
    }
    fn print_evolve_help(&self) {
        clog!("🧬 EVOLUTION COMMANDS:");
        clog!("  evolve start <engine> [population] [play_bias] [repl_bias] [mutation_scale] [crossover:0|1] [max_plies] [parallelism] [repro_rate]");
        clog!("  evolve stop");
        clog!("  evolve status");
        clog!("  evolve best");
        clog!("  evolve export <id> <file>   - save an individual as a .personality file");
        clog!("  evolve lock <param_id>      - freeze a parameter at its default for every individual");
        clog!("  evolve unlock <param_id>    - allow a parameter to evolve again");
        clog!("  evolve locks                - list currently locked parameters");
        clog!("  evolve set <field> <value>  - fields: play_bias, repl_bias, mutation, crossover,");
        clog!("                                parallelism, max_plies, repro, autosave, autosave_path");
        clog!("  (file paths are resolved under assets/personalities, not the working directory)");
    }
    fn cmd_evolve_start(&mut self, args: &[&str]) {
        if args.is_empty() {
            clog!("❌ Usage: evolve start <engine> [population] [play_bias] [repl_bias] [mutation_scale] [crossover] [max_plies] [parallelism] [repro_rate]");
            return;
        }
        let Some(engine) = crate::engine::personality::parse_engine_name(args[0]) else {
            clog!("❌ Unknown engine: '{}'", args[0]);
            return;
        };
        if self.tournament.is_active() {
            clog!("⚠️ Cannot start evolution while a tournament is running.");
            return;
        }
        let mut settings = EvolutionSettings::default();
        if let Some(v) = args.get(1).and_then(|s| s.parse::<usize>().ok()) {
            settings.population = v.max(2);
        }
        if let Some(v) = args.get(2).and_then(|s| s.parse::<f64>().ok()) {
            settings.play_elo_bias = v.max(0.0);
        }
        if let Some(v) = args.get(3).and_then(|s| s.parse::<f64>().ok()) {
            settings.replication_elo_bias = v.max(0.0);
        }
        if let Some(v) = args.get(4).and_then(|s| s.parse::<f64>().ok()) {
            settings.mutation_scale = v.clamp(0.0, 1.0);
        }
        if let Some(v) = args.get(5) {
            settings.use_crossover = *v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Some(v) = args.get(6).and_then(|s| s.parse::<usize>().ok()) {
            settings.max_plies = v.max(10);
        }
        if let Some(v) = args.get(7).and_then(|s| s.parse::<usize>().ok()) {
            settings.parallelism = v.max(1);
        }
        settings.games_per_replication = settings.population.max(4);
        if let Some(v) = args.get(8).and_then(|s| s.parse::<usize>().ok()) {
            if v >= 1 {
                settings.games_per_replication = v;
            }
        }
        settings.locked_params = self.evolution_locked_params.clone();
        let raw_name = if self.evolution_autosave_path.trim().is_empty() {
            format!("{}_evolved.personality", evolution::slugify_engine_name(&engine))
        } else {
            self.evolution_autosave_path.clone()
        };
        settings.autosave_enabled = self.evolution_autosave;
        settings.autosave_path = self
        .asset_manager
        .resolve_save_path(&raw_name)
        .to_string_lossy()
        .to_string();
        self.evolution_base_engine = engine.clone();
        self.evolution_crossover = settings.use_crossover;
        self.evolution_population_input = settings.population.to_string();
        self.evolution_play_bias_input = settings.play_elo_bias.to_string();
        self.evolution_replication_bias_input = settings.replication_elo_bias.to_string();
        self.evolution_mutation_scale_input = settings.mutation_scale.to_string();
        self.evolution_max_plies_input = settings.max_plies.to_string();
        self.evolution_parallelism_input = settings.parallelism.to_string();
        self.evolution_repro_rate_input = settings.games_per_replication.to_string();
        self.evolution_autosave_path = settings.autosave_path.clone();
        let seed = self.game_state.current_hash() ^ 0xE107_1701;
        match self.evolution.start(engine.clone(), settings, seed) {
            Ok(()) => {
                clog!(
                    "🧬 Evolution started: {} individuals of {}",
                    self.evolution.population.len(),
                      engine.name()
                );
                self.reset_board_for_tournament();
                self.evolution_initial_state = Some(self.game_state.clone_for_worker());
                self.board_cache.clear();
            }
            Err(e) => clog!("❌ Cannot start evolution: {}", e),
        }
    }
    fn cmd_evolve_export(&mut self, args: &[&str]) {
        if args.len() < 2 {
            clog!("❌ Usage: evolve export <id> <file>");
            return;
        }
        let Ok(id) = args[0].parse::<u64>() else {
            clog!("❌ Invalid id '{}'", args[0]);
            return;
        };
        match self.evolution.export_personality(id) {
            Some(content) => {
                let path = self.asset_manager.resolve_save_path(args[1]);
                match std::fs::write(&path, content) {
                    Ok(()) => clog!("✅ Exported individual #{} to {}", id, path.display()),
                    Err(e) => clog!("❌ Failed to write '{}': {}", path.display(), e),
                }
            }
            None => clog!("❌ No individual with id {}", id),
        }
    }
    fn cmd_evolve_lock(&mut self, args: &[&str], lock: bool) {
        if args.is_empty() {
            clog!(
                "❌ Usage: evolve {} <param_id>",
                if lock { "lock" } else { "unlock" }
            );
            return;
        }
        let id = args[0].to_string();
        if lock {
            self.evolution_locked_params.insert(id.clone());
            self.evolution.settings.locked_params.insert(id.clone());
        } else {
            self.evolution_locked_params.remove(&id);
            self.evolution.settings.locked_params.remove(&id);
        }
        clog!(
            "✅ {} parameter '{}'",
            if lock { "Locked" } else { "Unlocked" },
                id
        );
    }
    fn cmd_evolve_list_locks(&self) {
        if self.evolution_locked_params.is_empty() {
            clog!("No parameters are locked.");
            return;
        }
        let mut ids: Vec<&str> = self.evolution_locked_params.iter().map(|s| s.as_str()).collect();
        ids.sort_unstable();
        clog!("🔒 Locked parameters:");
        for id in ids {
            clog!("   {}", id);
        }
    }
    fn cmd_evolve_set(&mut self, args: &[&str]) {
        if args.len() < 2 {
            clog!("❌ Usage: evolve set <field> <value>  (fields: play_bias, repl_bias, mutation, crossover, parallelism, max_plies, autosave, autosave_path)");
            return;
        }
        let field = args[0];
        let value = args[1..].join(" ");
        match field {
            "play_bias" => {
                if let Ok(v) = value.parse::<f64>() {
                    self.evolution.settings.play_elo_bias = v.max(0.0);
                    clog!("✅ play_bias = {:.2}", v);
                } else {
                    clog!("❌ Invalid value");
                }
            }
            "repl_bias" => {
                if let Ok(v) = value.parse::<f64>() {
                    self.evolution.settings.replication_elo_bias = v.max(0.0);
                    clog!("✅ repl_bias = {:.2}", v);
                } else {
                    clog!("❌ Invalid value");
                }
            }
            "mutation" => {
                if let Ok(v) = value.parse::<f64>() {
                    self.evolution.settings.mutation_scale = v.clamp(0.0, 1.0);
                    clog!("✅ mutation_scale = {:.2}", v);
                } else {
                    clog!("❌ Invalid value");
                }
            }
            "crossover" => {
                let b = value == "1" || value.eq_ignore_ascii_case("true");
                self.evolution.settings.use_crossover = b;
                clog!("✅ crossover = {}", b);
            }
            "parallelism" => {
                if let Ok(v) = value.parse::<usize>() {
                    self.evolution.settings.parallelism = v.max(1);
                    self.evolution_parallelism_input = v.to_string();
                    clog!("✅ parallelism = {}", v);
                } else {
                    clog!("❌ Invalid value");
                }
            }
            "max_plies" => {
                if let Ok(v) = value.parse::<usize>() {
                    let v = v.max(10);
                    self.evolution.settings.max_plies = v;
                    self.evolution_max_plies_input = v.to_string();
                    clog!("✅ max_plies = {}", v);
                } else {
                    clog!("❌ Invalid value");
                }
            }
            "autosave" => {
                let b = value == "1" || value.eq_ignore_ascii_case("on") || value.eq_ignore_ascii_case("true");
                self.evolution.settings.autosave_enabled = b;
                self.evolution_autosave = b;
                clog!("✅ autosave = {}", b);
            }
            "autosave_path" => {
                let resolved = self.asset_manager.resolve_save_path(&value);
                self.evolution.settings.autosave_path = resolved.to_string_lossy().to_string();
                self.evolution_autosave_path = self.evolution.settings.autosave_path.clone();
                clog!("✅ autosave_path = {}", self.evolution.settings.autosave_path);
            }
            "repro" => {
                if let Ok(v) = value.parse::<usize>() {
                    if v >= 1 {
                        self.evolution.settings.games_per_replication = v;
                        self.evolution_repro_rate_input = v.to_string();
                        clog!("✅ games_per_replication = {}", v);
                    } else {
                        clog!("❌ Value must be >= 1");
                    }
                } else {
                    clog!("❌ Invalid value");
                }
            }
            _ => clog!("❌ Unknown field '{}'", field),
        }
    }
}
