use crate::app::{ChessGui, EvolutionWorker};
use crate::background::{spawn_param_game, GameSearchSettings, WorkerMsg};
use crate::engine::EngineType;
use crate::evolution::{self, EvolutionSettings};
use crate::tournament::GameOutcome;
use crate::clog;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
/// Default autosave filename for an engine, e.g. "piece_square_table_engine_evolved.personality".
/// This is just a FILENAME — path resolution (anchoring it under the
/// project's `assets/personalities` directory rather than the current
/// working directory) happens separately via `AssetManager::resolve_save_path`.
fn default_autosave_filename(engine: &EngineType) -> String {
    format!("{}_evolved.personality", evolution::slugify_engine_name(engine))
}
impl ChessGui {
    pub(crate) fn handle_evolution_start(&mut self) {
        if self.evolution.is_active() {
            clog!("⚠️ Evolution already running; stop it first.");
            return;
        }
        if self.tournament.is_active() {
            clog!("⚠️ Cannot start evolution while a tournament is running.");
            return;
        }
        let population = self
        .evolution_population_input
        .parse::<usize>()
        .unwrap_or(12)
        .max(2);
        let play_bias = self
        .evolution_play_bias_input
        .parse::<f64>()
        .unwrap_or(1.0)
        .max(0.0);
        let repl_bias = self
        .evolution_replication_bias_input
        .parse::<f64>()
        .unwrap_or(1.0)
        .max(0.0);
        let mutation_scale = self
        .evolution_mutation_scale_input
        .parse::<f64>()
        .unwrap_or(0.08)
        .clamp(0.0, 1.0);
        let repro_rate = self
        .evolution_repro_rate_input
        .parse::<usize>()
        .unwrap_or(0);
        let max_plies = self
        .evolution_max_plies_input
        .parse::<usize>()
        .unwrap_or(400)
        .max(10);
        let parallelism = self
        .evolution_parallelism_input
        .parse::<usize>()
        .unwrap_or(1)
        .max(1);
        let engine = self.evolution_base_engine.clone();
        // Anchor the autosave path at the project's assets/personalities
        // directory (same discovery logic everything else uses) rather
        // than wherever the process happens to have been launched from.
        let raw_name = if self.evolution_autosave_path.trim().is_empty() {
            default_autosave_filename(&engine)
        } else {
            self.evolution_autosave_path.clone()
        };
        let resolved_autosave = self
        .asset_manager
        .resolve_save_path(&raw_name)
        .to_string_lossy()
        .to_string();
        let mut settings = EvolutionSettings::default();
        settings.population = population;
        settings.play_elo_bias = play_bias;
        settings.replication_elo_bias = repl_bias;
        settings.mutation_scale = mutation_scale;
        settings.use_crossover = self.evolution_crossover;
        settings.locked_params = self.evolution_locked_params.clone();
        settings.games_per_replication = if repro_rate >= 1 {
            repro_rate
        } else {
            population.max(4)
        };
        settings.max_plies = max_plies;
        settings.parallelism = parallelism;
        settings.autosave_enabled = self.evolution_autosave;
        settings.autosave_path = resolved_autosave;
        // Reflect the resolved path back into the GUI field so the user
        // can see exactly where it's going rather than what they typed.
        self.evolution_autosave_path = settings.autosave_path.clone();
        let seed = self.game_state.current_hash() ^ 0xE107_1701;
        match self.evolution.start(engine.clone(), settings, seed) {
            Ok(()) => {
                clog!(
                    "🧬 Evolution started: {} individuals of {} (max {} plies, {} thread(s), {} locked param(s))",
                      self.evolution.population.len(),
                      engine.name(),
                      max_plies,
                      parallelism,
                      self.evolution_locked_params.len(),
                );
                self.reset_board_for_tournament();
                self.evolution_initial_state = Some(self.game_state.clone_for_worker());
                self.board_cache.clear();
            }
            Err(e) => clog!("❌ Cannot start evolution: {}", e),
        }
    }
    pub(crate) fn handle_evolution_stop(&mut self) {
        for w in &self.evolution_workers {
            w.cancel.store(true, Ordering::Relaxed);
        }
        self.evolution.stop();
        self.evolution_workers.clear();
        self.evolution_initial_state = None;
        clog!(
            "⏹️ Evolution stopped ({} games played)",
              self.evolution.games_played
        );
    }
    /// One-off export of the current best individual, independent of
    /// autosave. Writes a uniquely-named file (per engine + individual
    /// id) under the resolved assets/personalities directory so repeated
    /// clicks don't clobber each other.
    pub(crate) fn handle_evolution_export_best(&mut self) {
        let Some((best_id, best_rating)) = self.evolution.best().map(|b| (b.id, b.rating)) else {
            clog!("❌ No individuals to export yet.");
            return;
        };
        let Some(content) = self.evolution.export_personality(best_id) else {
            clog!("❌ Failed to build export for #{}", best_id);
            return;
        };
        let filename = evolution::export_filename_for(&self.evolution.base_engine, best_id);
        let path = self.asset_manager.resolve_save_path(&filename);
        match std::fs::write(&path, &content) {
            Ok(()) => clog!(
                "✅ Exported best individual #{} (rating {:.0}) to {}",
                            best_id,
                            best_rating,
                            path.display()
            ),
            Err(e) => clog!("❌ Failed to write '{}': {}", path.display(), e),
        }
    }
    pub(crate) fn evolution_tick(&mut self) {
        let featured_before = self.evolution_workers.first().map(|w| w.plies);
        let mut i = 0;
        while i < self.evolution_workers.len() {
            let mut done: Option<(GameOutcome, usize, usize, u64, u64)> = None;
            let mut disconnected = false;
            loop {
                match self.evolution_workers[i].rx.try_recv() {
                    Ok(WorkerMsg::Progress(snap)) => {
                        self.evolution_workers[i].plies = snap.plies;
                        self.evolution_workers[i].last_snapshot = Some(snap);
                    }
                    Ok(WorkerMsg::Done { outcome, plies, .. }) => {
                        let w = &self.evolution_workers[i];
                        done = Some((outcome, w.white_idx, w.black_idx, w.white_id, w.black_id));
                        let _ = plies;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if let Some((outcome, wi, bi, wid, bid)) = done {
                self.evolution_workers.remove(i);
                let still_valid = self.evolution.population.get(wi).map(|ind| ind.id) == Some(wid)
                && self.evolution.population.get(bi).map(|ind| ind.id) == Some(bid);
                if still_valid {
                    self.evolution.record_game(wi, bi, outcome.white_score());
                }
            } else if disconnected {
                self.evolution_workers.remove(i);
            } else {
                i += 1;
            }
        }
        if self.evolution.is_active() {
            while self.evolution_workers.len() < self.evolution.settings.parallelism.max(1) {
                if !self.spawn_evolution_worker() {
                    break;
                }
            }
        }
        let featured_after = self.evolution_workers.first().map(|w| w.plies);
        if featured_before != featured_after {
            self.board_cache.clear();
        }
    }
    fn spawn_evolution_worker(&mut self) -> bool {
        let Some((wi, bi)) = self.evolution.draw_pairing() else {
            return false;
        };
        let Some(template) = &self.evolution_initial_state else {
            return false;
        };
        let Some(white_ind) = self.evolution.population.get(wi).cloned() else {
            return false;
        };
        let Some(black_ind) = self.evolution.population.get(bi).cloned() else {
            return false;
        };
        let Some(white_engine) = self.evolution.build_engine(&white_ind) else {
            return false;
        };
        let Some(black_engine) = self.evolution.build_engine(&black_ind) else {
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_param_game(
            white_engine,
            black_engine,
            template.clone_for_worker(),
                                  Arc::clone(&self.move_generator),
                                  Arc::clone(&self.piece_config),
                                  GameSearchSettings::from_controller(&self.game_controller),
                                  self.evolution.settings.max_plies,
                                  Arc::clone(&cancel),
        );
        self.evolution_workers.push(EvolutionWorker {
            white_idx: wi,
            black_idx: bi,
            white_id: white_ind.id,
            black_id: black_ind.id,
            rx,
            cancel,
            plies: 0,
            last_snapshot: None,
        });
        true
    }
}
