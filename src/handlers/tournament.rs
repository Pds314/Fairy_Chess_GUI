use crate::app::{ChessGui, TournamentWorker};
use crate::background::{GameSearchSettings, WorkerMsg};
use crate::engine::EngineType;
use crate::tournament::{GameOutcome, Termination};
use crate::clog;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

impl ChessGui {
    pub(crate) fn reset_board_for_tournament(&mut self) {
        if let Some(game_file) = self.current_game_file.clone() {
            if let Err(e) = self.load_game_from_file(&game_file) {
                clog!("⚠️ Variant reload failed: {}. Using default.", e);
                let board_config =
                    crate::handlers::loading::load_board_config(&self.asset_manager);
                self.game_state = crate::core::GameState::from_config(
                    board_config,
                    &self.piece_config,
                    Arc::make_mut(&mut self.move_generator),
                );
                self.game_controller.reset_engine_caches();
            }
        } else {
            let board_config = crate::handlers::loading::load_board_config(&self.asset_manager);
            self.game_state = crate::core::GameState::from_config(
                board_config,
                &self.piece_config,
                Arc::make_mut(&mut self.move_generator),
            );
            self.game_controller.reset_engine_caches();
        }
        self.game_controller.reset_timers();
        self.selected_square = None;
        self.last_move_highlight = None;
        self.promotion_dialog = None;
        self.board_cache.clear();
    }

    pub(crate) fn tournament_tick(&mut self) {
        let featured_before = self.tournament_workers.first().map(|w| w.plies);

        let mut i = 0;
        while i < self.tournament_workers.len() {
            let mut done: Option<(GameOutcome, Termination, usize, Duration, Duration)> = None;
            let mut disconnected = false;
            loop {
                match self.tournament_workers[i].rx.try_recv() {
                    Ok(WorkerMsg::Progress(snap)) => {
                        self.tournament_workers[i].plies = snap.plies;
                        self.tournament_workers[i].last_snapshot = Some(snap);
                    }
                    Ok(WorkerMsg::Done {
                        outcome,
                        termination,
                        plies,
                        white_time,
                        black_time,
                    }) => {
                        self.tournament_workers[i].plies = plies;
                        done = Some((outcome, termination, plies, white_time, black_time));
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if let Some((outcome, termination, plies, wt, bt)) = done {
                let worker = self.tournament_workers.remove(i);
                self.record_tournament_result(&worker.pairing, outcome, termination, plies, wt, bt);
            } else if disconnected {
                self.tournament_workers.remove(i);
            } else {
                i += 1;
            }
        }

        if self.tournament.is_active() {
            while self.tournament_workers.len() < self.tournament.parallelism {
                match self.tournament.take_next_pairing() {
                    Some(pairing) => self.spawn_tournament_worker(pairing),
                    None => break,
                }
            }
        }

        let featured_after = self.tournament_workers.first().map(|w| w.plies);
        if featured_before != featured_after {
            self.board_cache.clear();
        }

        if self.tournament.is_active()
            && self.tournament_workers.is_empty()
            && !self.tournament.has_more_pairings()
        {
            self.tournament.mark_complete();
            self.tournament_initial_state = None;
            self.tournament_graph_cache.clear();
            self.board_cache.clear();
            self.tournament.elo.print_results_matrix();
            self.tournament.elo.print_detailed_report();
        }
    }

    pub(crate) fn spawn_tournament_worker(&mut self, pairing: (EngineType, EngineType)) {
        let Some(template) = &self.tournament_initial_state else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = crate::background::spawn_tournament_game(
            pairing.0.clone(),
            pairing.1.clone(),
            template.clone_for_worker(),
            Arc::clone(&self.move_generator),
            Arc::clone(&self.piece_config),
            GameSearchSettings::from_controller(&self.game_controller),
            self.tournament.max_plies,
            Arc::clone(&cancel),
        );
        self.tournament_workers.push(TournamentWorker {
            pairing,
            rx,
            cancel,
            plies: 0,
            last_snapshot: None,
        });
    }

    fn record_tournament_result(
        &mut self,
        pairing: &(EngineType, EngineType),
        outcome: GameOutcome,
        termination: Termination,
        plies: usize,
        white_time: Duration,
        black_time: Duration,
    ) {
        let (white, black) = pairing;
        let (dw, db) = self.tournament.record_game(
            white.clone(),
            black.clone(),
            outcome,
            termination,
            plies,
            white_time,
            black_time,
        );
        let result_str = match outcome {
            GameOutcome::WhiteWins => format!("1-0 ({} wins)", white.name()),
            GameOutcome::BlackWins => format!("0-1 ({} wins)", black.name()),
            GameOutcome::Draw => "½-½".to_string(),
            GameOutcome::AdjudicatedDraw => "½-½ (adjudicated)".to_string(),
        };
        clog!(
            "🎮 Game {}/{}: {} vs {} → {} [{}] ({} plies, {:.1}s/{:.1}s)  ELO {:+.1}/{:+.1}",
            self.tournament.games_played(),
            self.tournament.total_games(),
            white.name(),
            black.name(),
            result_str,
            termination.label(),
            plies,
            white_time.as_secs_f32(),
            black_time.as_secs_f32(),
            dw,
            db,
        );
        self.tournament_graph_cache.clear();
    }
}
