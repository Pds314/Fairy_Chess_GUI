use crate::app::ChessGui;
use crate::core::PieceColor;
use crate::messages::Message;
use crate::clog;
use iced::Task;
impl ChessGui {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SquareClicked(clicked_pos) => {
                if self.tournament.is_active() || !self.tournament_workers.is_empty() {
                    return Task::none();
                }
                if !self
                    .game_controller
                    .is_engine_turn(self.game_state.current_turn)
                    && self.engine_job.is_none()
                    {
                        self.handle_square_click(clicked_pos);
                        self.check_for_engine_move();
                    }
            }
            Message::UndoMove => {
                self.handle_undo();
                self.check_for_engine_move();
            }
            Message::RedoMove => {
                if self
                    .game_state
                    .redo_move(&self.move_generator, &self.piece_config)
                    {
                        self.clear_selection();
                        if let Some(last) = self.game_state.move_history.last() {
                            self.last_move_highlight = Some((last.from, last.to));
                        }
                        self.check_for_engine_move();
                    }
            }
            Message::ResetBoard => {
                self.handle_reset();
                self.game_controller.reset_timers();
                self.check_for_engine_move();
            }
            Message::GenerateMoves => self.handle_generate_moves(),
            Message::EvaluatePosition => self.handle_evaluate_position(),
            Message::EvaluateMoves => self.handle_evaluate_moves(),
            Message::MakeBestMove => {
                if self.engine_job.is_none() && self.is_game_ongoing() {
                    self.start_engine_move();
                }
            }
            Message::WhiteEngineSelected(engine_type) => {
                self.game_controller.set_white_engine(engine_type);
                clog!(
                    "White engine: {}",
                    self.game_controller.get_white_engine_type().name()
                );
                self.check_for_engine_move();
            }
            Message::BlackEngineSelected(engine_type) => {
                self.game_controller.set_black_engine(engine_type);
                clog!(
                    "Black engine: {}",
                    self.game_controller.get_black_engine_type().name()
                );
                self.check_for_engine_move();
            }
            Message::EvalEngineSelected(engine_type) => {
                self.game_controller.set_eval_engine(engine_type);
                clog!(
                    "Evaluation engine: {}",
                    self.game_controller.get_eval_engine_type().name()
                );
            }
            Message::WhiteDepthSelected(d) => {
                self.game_controller.set_white_search_depth(d);
                clog!("White engine depth set to: {}", d);
            }
            Message::BlackDepthSelected(d) => {
                self.game_controller.set_black_search_depth(d);
                clog!("Black engine depth set to: {}", d);
            }
            Message::EvalDepthSelected(d) => {
                self.game_controller.set_eval_search_depth(d);
                clog!("Evaluation engine depth set to: {}", d);
            }
            Message::WhiteTimeInputChanged(input) => {
                self.white_time_input = input;
                self.apply_time_input(PieceColor::White);
            }
            Message::BlackTimeInputChanged(input) => {
                self.black_time_input = input;
                self.apply_time_input(PieceColor::Black);
            }
            Message::EvalTimeInputChanged(input) => {
                self.eval_time_input = input;
                if let Ok(s) = self.eval_time_input.parse::<f32>() {
                    if s > 0.0 {
                        self.game_controller.set_eval_time_limit(Some(s));
                        clog!("Evaluation time limit set to: {} seconds", s);
                    }
                } else if self.eval_time_input.is_empty() {
                    self.game_controller.set_eval_time_limit(None);
                    clog!("Evaluation time limit disabled");
                }
            }
            Message::WhiteTimeRespectChanged(input) => {
                self.white_time_respect_input = input;
                if let Ok(r) = self.white_time_respect_input.parse::<f32>() {
                    self.game_controller.set_white_time_respect(r);
                    clog!("White time respect set to: {:.2}", r);
                }
            }
            Message::BlackTimeRespectChanged(input) => {
                self.black_time_respect_input = input;
                if let Ok(r) = self.black_time_respect_input.parse::<f32>() {
                    self.game_controller.set_black_time_respect(r);
                    clog!("Black time respect set to: {:.2}", r);
                }
            }
            Message::UnlimitedDepthToggled(enabled) => {
                self.game_controller.set_unlimited_depth_with_time(enabled);
                clog!(
                    "Unlimited depth with time limit: {}",
                    if enabled { "enabled" } else { "disabled" }
                );
            }
            Message::AutoPlayToggled(enabled) => {
                if self.tournament.is_active() {
                    clog!("⚠️ Auto-play disabled during tournament");
                    return Task::none();
                }
                self.game_controller.set_auto_play(enabled);
                if enabled {
                    clog!("Auto-play enabled");
                    self.check_for_engine_move();
                } else {
                    clog!("Auto-play disabled");
                }
            }
            Message::Tick => {
                self.poll_engine_job();
                if self.pending_engine_move && self.engine_job.is_none() && self.is_game_ongoing() {
                    self.pending_engine_move = false;
                    self.start_engine_move();
                }
            }
            Message::PlayEngineMove => {
                if self.engine_job.is_none()
                    && self.is_game_ongoing()
                    && self
                    .game_controller
                    .is_engine_turn(self.game_state.current_turn)
                    {
                        self.start_engine_move();
                    }
            }
            Message::PromotionSelected(piece_type) => {
                if let Some(dialog) = self.promotion_dialog.take() {
                    let current_turn = self.game_state.current_turn;
                    if let Some(move_with_path) = self.move_generator.get_move_rule(
                        &self.game_state.board,
                        dialog.from,
                        dialog.to,
                        self.game_state
                        .board
                        .get_piece(dialog.from)
                        .unwrap()
                        .piece_type,
                    ) {
                        self.game_state.clear_redo();
                        self.game_state.make_move(
                            dialog.from,
                            dialog.to,
                            &move_with_path,
                            &self.piece_config,
                            Some(piece_type),
                        );
                        self.clear_selection();
                        self.game_controller.end_turn(current_turn);
                        self.check_for_engine_move();
                    }
                }
            }
            Message::GameFileSelected(item) => {
                if let Some(file_path) = self.asset_manager.handle_browser_selection(&item) {
                    self.current_game_file = Some(file_path.to_string_lossy().to_string());
                    self.selected_game_file = Some(item);
                } else {
                    self.game_file_items = self.asset_manager.list_game_files();
                }
            }
            Message::LoadGameFile => {
                if let Some(game_file) = self.current_game_file.clone() {
                    if let Err(e) = self.load_game_from_file(&game_file) {
                        clog!("Failed to load game file '{}': {}", game_file, e);
                    }
                }
            }
            Message::AnalyzePosition => self.handle_analyze_position(),
            Message::TerminalInputChanged(input) => self.terminal_input = input,
            Message::TerminalCommand => {
                let command = self.terminal_input.clone();
                self.handle_terminal_command(&command);
                self.terminal_input.clear();
            }
            Message::WhiteEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_white_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_white_engine_parameters(params) {
                        clog!("⚙️ White engine parameter '{}' set to {:.3}", param_id, value);
                    }
                }
            }
            Message::BlackEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_black_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_black_engine_parameters(params) {
                        clog!("⚙️ Black engine parameter '{}' set to {:.3}", param_id, value);
                    }
                }
            }
            Message::EvalEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_eval_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_eval_engine_parameters(params) {
                        clog!("⚙️ Eval engine parameter '{}' set to {:.3}", param_id, value);
                        self.position_analysis = None;
                    }
                }
            }
            Message::PrintPgn => self.print_pgn(),
            Message::LoadPgn => {
                clog!("📄 Paste PGN into terminal and use 'loadpgn' command");
            }
            Message::TournamentStart => self.handle_tournament_start(),
            Message::TournamentStop => self.handle_tournament_stop(),
            Message::TournamentClearHistory => {
                self.tournament.clear_history();
                self.tournament_graph_cache.clear();
                clog!("🗑️ Tournament history cleared");
            }
            Message::TournamentToggleParticipant(engine) => {
                let added = self.tournament.toggle_participant(engine.clone());
                clog!(
                    "{} {} {} tournament",
                    if added { "➕" } else { "➖" },
                        engine.name(),
                      if added { "added to" } else { "removed from" }
                );
            }
            Message::TournamentGamesInputChanged(s) => {
                self.tournament_games_input = s;
                if let Ok(n) = self.tournament_games_input.parse::<usize>() {
                    if n > 0 {
                        self.tournament.games_per_pairing = n;
                    }
                }
            }
            Message::TournamentMaxPliesInputChanged(s) => {
                self.tournament_max_plies_input = s;
                if let Ok(n) = self.tournament_max_plies_input.parse::<usize>() {
                    if n >= 10 {
                        self.tournament.max_plies = n;
                    }
                }
            }
            Message::TournamentParallelismInputChanged(s) => {
                self.tournament_parallelism_input = s;
                if let Ok(n) = self.tournament_parallelism_input.parse::<usize>() {
                    if n >= 1 {
                        self.tournament.parallelism = n;
                    }
                }
            }
            Message::TournamentTick => {
                self.poll_engine_job();
                self.tournament_tick();
            }
            Message::EvolutionBaseEngineSelected(engine) => {
                self.evolution_base_engine = engine;
            }
            Message::EvolutionPopulationChanged(s) => {
                self.evolution_population_input = s;
            }
            Message::EvolutionPlayBiasChanged(s) => {
                self.evolution_play_bias_input = s;
            }
            Message::EvolutionReplicationBiasChanged(s) => {
                self.evolution_replication_bias_input = s;
            }
            Message::EvolutionMutationScaleChanged(s) => {
                self.evolution_mutation_scale_input = s;
            }
            Message::EvolutionReproRateChanged(s) => {
                self.evolution_repro_rate_input = s.clone();
                if let Ok(v) = s.parse::<usize>() {
                    if v >= 1 {
                        // Live-adjustable while running.
                        self.evolution.settings.games_per_replication = v;
                    }
                }
            }
            Message::EvolutionCrossoverToggled(b) => {
                self.evolution_crossover = b;
            }
            Message::EvolutionMaxPliesInputChanged(s) => {
                self.evolution_max_plies_input = s.clone();
                if let Ok(v) = s.parse::<usize>() {
                    if v >= 10 {
                        self.evolution.settings.max_plies = v;
                    }
                }
            }
            Message::EvolutionParallelismInputChanged(s) => {
                self.evolution_parallelism_input = s.clone();
                if let Ok(v) = s.parse::<usize>() {
                    if v >= 1 {
                        self.evolution.settings.parallelism = v;
                    }
                }
            }
            Message::EvolutionAutosaveToggled(b) => {
                self.evolution_autosave = b;
                self.evolution.settings.autosave_enabled = b;
            }
            Message::EvolutionAutosavePathChanged(s) => {
                self.evolution_autosave_path = s.clone();
                let resolved = self.asset_manager.resolve_save_path(&s);
                self.evolution.settings.autosave_path = resolved.to_string_lossy().to_string();
            }
            Message::EvolutionParamLockToggled(id, locked) => {
                if locked {
                    self.evolution_locked_params.insert(id.clone());
                    self.evolution.settings.locked_params.insert(id);
                } else {
                    self.evolution_locked_params.remove(&id);
                    self.evolution.settings.locked_params.remove(&id);
                }
            }
            Message::ToggleEvolutionLockMenu => {
                self.show_lock_menu = !self.show_lock_menu;
            }
            Message::EvolutionExportBest => self.handle_evolution_export_best(),
            Message::EvolutionStart => self.handle_evolution_start(),
            Message::EvolutionStop => self.handle_evolution_stop(),
            Message::EvolutionTick => {
                self.poll_engine_job();
                self.evolution_tick();
            }
            Message::UiPanelSelected(panel) => {
                self.active_panel = panel;
            }
            Message::ToggleConsole => {
                self.show_console = !self.show_console;
            }
        }
        Task::none()
    }
    /// Helper to apply a white/black time input change, deduplicating the pattern.
    fn apply_time_input(&mut self, color: PieceColor) {
        let input = match color {
            PieceColor::White => &self.white_time_input,
            PieceColor::Black => &self.black_time_input,
        };
        let label = match color {
            PieceColor::White => "White",
            PieceColor::Black => "Black",
        };
        if let Ok(seconds) = input.parse::<f32>() {
            if seconds > 0.0 {
                match color {
                    PieceColor::White => self.game_controller.set_white_time_limit(Some(seconds)),
                    PieceColor::Black => self.game_controller.set_black_time_limit(Some(seconds)),
                }
                clog!("{} time limit set to: {} seconds", label, seconds);
            }
        } else if input.is_empty() {
            match color {
                PieceColor::White => self.game_controller.set_white_time_limit(None),
                PieceColor::Black => self.game_controller.set_black_time_limit(None),
            }
            clog!("{} time limit disabled", label);
        }
    }
    fn handle_tournament_start(&mut self) {
        let seed = self.game_state.current_hash();
        match self.tournament.start(seed) {
            Ok(()) => {
                clog!(
                    "🏆 Tournament started: {} engines, {} games total, {} in parallel",
                    self.tournament.participants.len(),
                      self.tournament.total_games(),
                      self.tournament.parallelism,
                );
                self.tournament_graph_cache.clear();
                self.game_controller.set_auto_play(false);
                self.pending_engine_move = false;
                self.reset_board_for_tournament();
                self.tournament_initial_state = Some(self.game_state.clone_for_worker());
                self.board_cache.clear();
            }
            Err(msg) => clog!("❌ Cannot start tournament: {}", msg),
        }
    }
    fn handle_tournament_stop(&mut self) {
        clog!(
            "⏹️ Tournament stopped ({}/{} games completed, {} in flight)",
              self.tournament.games_played(),
              self.tournament.total_games(),
              self.tournament_workers.len()
        );
        for w in &self.tournament_workers {
            w.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.tournament.stop();
        self.tournament_initial_state = None;
    }
}
