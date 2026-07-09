// In your app / gui module
use crate::app::{ChessGui, EngineJob};
use crate::core::{GameResult, PieceColor};
use crate::helpers::{format_move, print_search_stats};
use crate::clog;
use std::sync::{mpsc, Arc};
use std::time::Instant;

impl ChessGui {
    pub(crate) fn start_engine_move(&mut self) {
        if self.engine_job.is_some() {
            return;
        }
        let color = self.game_state.current_turn;
        self.game_controller.start_turn(color);
        let (depth, time_limit) = self.game_controller.compute_search_budget(color);

        let engine = match self.game_controller.take_engine(color) {
            Some(e) => e,
            None => {
                clog!("🏳️ Engine failed to provide a move! Resigning.");
                self.game_state.game_result = Some(GameResult::Winner(color.opposite()));
                self.game_controller.stop_timing(color);
                return;
            }
        };

        let engine_type = match color {
            PieceColor::White => self.game_controller.get_white_engine_type().clone(),
            PieceColor::Black => self.game_controller.get_black_engine_type().clone(),
        };

        let rx = crate::background::spawn_engine_move(
            engine,
            self.game_state.clone_for_worker(),
                                                      Arc::clone(&self.move_generator),
                                                      Arc::clone(&self.piece_config),
                                                      depth,
                                                      time_limit,
        );

        self.engine_job = Some(EngineJob {
            slot: color,
            engine_type,
            rx,
            dispatched_hash: self.game_state.current_hash(),
                               dispatched_ply: self.game_state.move_history.len(),
                               variant_generation: self.variant_generation,
        });
    }

    pub(crate) fn poll_engine_job(&mut self) {
        let Some(job) = &self.engine_job else {
            return;
        };
        let (mut engine, result) = match job.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                let slot = job.slot;
                self.engine_job = None;
                clog!("⚠️ Engine worker thread terminated unexpectedly");
                self.game_state.game_result = Some(GameResult::Winner(slot.opposite()));
                self.game_controller.stop_timing(slot);
                return;
            }
        };

        let job = self.engine_job.take().unwrap();
        let engine_name = engine.name().to_string();

        let variant_changed = job.variant_generation != self.variant_generation;
        if variant_changed {
            engine.reset_cache();
        }

        let current_type = match job.slot {
            PieceColor::White => self.game_controller.get_white_engine_type().clone(),
            PieceColor::Black => self.game_controller.get_black_engine_type().clone(),
        };
        if current_type == job.engine_type {
            self.game_controller.put_engine(job.slot, engine);
        }

        let stale = variant_changed
        || job.dispatched_hash != self.game_state.current_hash()
        || job.dispatched_ply != self.game_state.move_history.len();
        if stale {
            self.game_controller.stop_timing(job.slot);
            self.check_for_engine_move();
            return;
        }

        match result {
            Some(r) => {
                let (from, to) = (r.best_move.from, r.best_move.to);
                clog!(
                    "\n{} ({}) plays: {}{} -> {}{}",
                      match job.slot {
                          PieceColor::White => "White",
                          PieceColor::Black => "Black",
                      },
                      engine_name,
                      (b'a' + from.1 as u8) as char,
                      8 - from.0,
                      (b'a' + to.1 as u8) as char,
                      8 - to.0,
                );
                if let Some(mate_in) = r.evaluation.mate_in {
                    if mate_in > 0 {
                        clog!("  Mate in {} moves!", mate_in);
                    } else {
                        clog!("  Getting mated in {} moves!", -mate_in);
                    }
                } else {
                    clog!("  Evaluation: {}", r.evaluation.score);
                }
                self.game_state.clear_redo();
                self.game_state.execute_expanded_move(
                    &r.best_move,
                    &self.move_generator,
                    &self.piece_config,
                );
                self.clear_selection();
                self.last_move_highlight = Some((from, to));
                self.game_controller.end_turn(job.slot);
                self.check_for_engine_move();
            }
            None => {
                clog!("🏳️ Engine failed to provide a move! Resigning.");
                self.game_state.game_result = Some(GameResult::Winner(job.slot.opposite()));
                self.game_controller.stop_timing(job.slot);
            }
        }
    }

    pub(crate) fn check_for_engine_move(&mut self) {
        self.game_state
        .update_mate_status(&self.move_generator, &self.piece_config);
        if self.is_game_ongoing() {
            let current_turn = self.game_state.current_turn;
            if self.game_controller.is_engine_turn(current_turn) {
                if self.game_controller.is_auto_play() {
                    self.pending_engine_move = true;
                }
            } else if !self.game_controller.is_timing_active() {
                self.game_controller.start_turn(current_turn);
            }
        } else {
            self.game_controller
            .stop_timing(self.game_state.current_turn);
            self.pending_engine_move = false;
        }
    }

    pub(crate) fn handle_evaluate_position(&mut self) {
        let start = Instant::now();
        self.game_state.reset_performance_stats();
        if let Some(result) = self.game_controller.evaluate_position(
            &mut self.game_state,
            &self.move_generator,
            &self.piece_config,
        ) {
            let duration = start.elapsed();
            let stats = self.game_state.get_performance_stats();
            let (from, to) = (result.best_move.from, result.best_move.to);
            clog!(
                "\n=== Position Evaluation (using {}) ===",
                  self.game_controller.get_eval_engine_type().name()
            );
            clog!("Current player: {:?}", self.game_state.current_turn);
            clog!("Evaluation: {}", result.evaluation.score);
            clog!(
                "Best move: {}{} -> {}{}",
                (b'a' + from.1 as u8) as char,
                  8 - from.0,
                  (b'a' + to.1 as u8) as char,
                  8 - to.0
            );
            print_search_stats(&stats, duration);
        } else {
            clog!("No evaluation engine selected");
        }
    }

    pub(crate) fn handle_evaluate_moves(&mut self) {
        let start = Instant::now();
        self.game_state.reset_performance_stats();
        let legal_moves = self
        .game_state
        .get_legal_moves(&self.move_generator, &self.piece_config);
        if legal_moves.is_empty() {
            clog!("No legal moves available!");
            return;
        }
        clog!(
            "\n=== Evaluating All Moves (using {}) ===",
              self.game_controller.get_eval_engine_type().name()
        );
        clog!("Evaluating {} legal moves...", legal_moves.len());
        let mut evals = Vec::new();
        let board_size = self.game_state.board.size();
        for mv in legal_moves {
            self.game_state
            .execute_expanded_move(&mv, &self.move_generator, &self.piece_config);
            if let Some(result) = self.game_controller.evaluate_position(
                &mut self.game_state,
                &self.move_generator,
                &self.piece_config,
            ) {
                evals.push((mv.clone(), -result.evaluation.score));
            }
            self.game_state.undo_move(&self.piece_config);
        }
        let duration = start.elapsed();
        let stats = self.game_state.get_performance_stats();
        evals.sort_by_key(|(_, s)| -s);
        clog!("\n=== Move Evaluations (best to worst) ===");
        for (i, (mv, score)) in evals.iter().enumerate() {
            let move_str = format_move(mv, &self.piece_config, board_size);
            let mut line = format!("{}. {} - Score: {}", i + 1, move_str, score);
            if *score > 900000 {
                line.push_str(" (Winning!)");
            } else if *score < -900000 {
                line.push_str(" (Losing!)");
            } else if *score > 100 {
                line.push_str(" (Good)");
            } else if *score < -100 {
                line.push_str(" (Bad)");
            }
            if i == 0 {
                line.push_str(" <- BEST");
            }
            clog!("{}", line);
        }
        print_search_stats(&stats, duration);
    }

    pub(crate) fn handle_analyze_position(&mut self) {
        let start = Instant::now();
        clog!("\n=== Comprehensive Position Analysis ===");
        let engine_name = self
        .game_controller
        .get_eval_engine_type()
        .name()
        .to_string();
        if let Some(analysis) = self.game_controller.analyze_position(
            &mut self.game_state,
            &self.move_generator,
            &self.piece_config,
        ) {
            let duration = start.elapsed();
            clog!("Analysis completed using {} in {:.2?}", engine_name, duration);
            self.print_comprehensive_analysis(&analysis);
            self.position_analysis = Some(analysis);
        } else {
            clog!("❌ Engine '{}' does not support detailed analysis", engine_name);
            clog!("💡 Try using the 'Piece Square Table Engine' for analysis features");
        }
    }
}
