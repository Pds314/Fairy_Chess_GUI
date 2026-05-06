// src/background.rs
//
// Off‑GUI‑thread engine execution.
//
// Two entry points:
//   * spawn_engine_move   — run a single best_move() call on a worker
//                           thread and hand the engine + result back.
//   * spawn_tournament_game — play an entire game between two engine
//                           types on a worker thread, streaming progress
//                           snapshots and a final outcome.
//
// Everything here is plain std::thread + std::sync::mpsc. The GUI polls
// the receivers from its subscription tick; no async runtime coupling.

use crate::core::DrawReason;
use crate::core::board::Board;
use crate::core::game_state::MateStatus;
use crate::core::{GameResult, GameState, PieceColor, Position};
use crate::engine::api::SearchResult;
use crate::engine::{ChessEngine, EngineType, GameController, SearchParams};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use crate::tournament::{GameOutcome, Termination};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

// ─────────────────────────────────────────────────────────────────────
// Single‑move engine job
// ─────────────────────────────────────────────────────────────────────

/// What a single‑move worker sends back: the engine (so its transposition
/// table / caches survive across moves) plus the search result.
pub type EngineMoveResult = (Box<dyn ChessEngine>, Option<SearchResult>);

pub fn spawn_engine_move(
    mut engine: Box<dyn ChessEngine>,
    mut state: GameState,
    move_generator: Arc<MoveGenerator>,
    piece_config: Arc<PieceConfigManager>,
    depth: u32,
    time_limit: Option<Duration>,
) -> mpsc::Receiver<EngineMoveResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = engine.best_move(SearchParams {
            state: &mut state,
            move_generator: &move_generator,
            config_manager: &piece_config,
            depth,
            time_limit,
        });
        // If the GUI dropped the receiver (e.g. user reset mid‑think) this
        // send fails and the engine is simply dropped with the thread.
        let _ = tx.send((engine, result));
    });
    rx
}

// ─────────────────────────────────────────────────────────────────────
// Tournament game worker
// ─────────────────────────────────────────────────────────────────────

/// Search settings snapshot, captured from the GameController at
/// tournament start so every worker plays under identical conditions.
/// Mirrors the budget logic in GameController::make_engine_move.
#[derive(Clone)]
pub struct GameSearchSettings {
    pub white_depth: u32,
    pub black_depth: u32,
    pub white_time_limit: Option<Duration>,
    pub black_time_limit: Option<Duration>,
    pub white_time_respect: f32,
    pub black_time_respect: f32,
    pub unlimited_depth_with_time: bool,
}

impl GameSearchSettings {
    pub fn from_controller(c: &GameController) -> Self {
        Self {
            white_depth: c.get_white_search_depth(),
            black_depth: c.get_black_search_depth(),
            white_time_limit: c.get_white_time_limit().map(Duration::from_secs_f32),
            black_time_limit: c.get_black_time_limit().map(Duration::from_secs_f32),
            white_time_respect: c.get_white_time_respect(),
            black_time_respect: c.get_black_time_respect(),
            unlimited_depth_with_time: c.get_unlimited_depth_with_time(),
        }
    }

    /// Depth + time limit for `color`, applying time‑respect against the
    /// per‑game clocks. Same formula as GameController.
    fn budget_for(
        &self,
        color: PieceColor,
        my_clock: Duration,
        opp_clock: Duration,
    ) -> (u32, Option<Duration>) {
        let (depth, base, respect) = match color {
            PieceColor::White => (
                self.white_depth,
                self.white_time_limit,
                self.white_time_respect,
            ),
            PieceColor::Black => (
                self.black_depth,
                self.black_time_limit,
                self.black_time_respect,
            ),
        };
        let adjusted = base.map(|b| {
            if respect == 0.0 {
                return b;
            }
            let diff = opp_clock.as_secs_f32() - my_clock.as_secs_f32();
            Duration::from_secs_f32((b.as_secs_f32() + diff * respect).max(0.1))
        });
        let d = if adjusted.is_some() && self.unlimited_depth_with_time {
            99
        } else {
            depth
        };
        (d, adjusted)
    }
}

/// Enough state for the GUI to redraw the board mid‑game.
pub struct DisplaySnapshot {
    pub board: Board,
    pub current_turn: PieceColor,
    pub last_move: Option<(Position, Position)>,
    pub plies: usize,
}

pub enum WorkerMsg {
    Progress(DisplaySnapshot),
    Done {
        outcome: GameOutcome,
        termination: Termination,
        plies: usize,
        white_time: Duration,
        black_time: Duration,
    },
}

pub fn spawn_tournament_game(
    white_type: EngineType,
    black_type: EngineType,
    initial_state: GameState,
    move_generator: Arc<MoveGenerator>,
    piece_config: Arc<PieceConfigManager>,
    settings: GameSearchSettings,
    max_plies: usize,
    cancel: Arc<AtomicBool>,
) -> mpsc::Receiver<WorkerMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        run_tournament_game(
            white_type,
            black_type,
            initial_state,
            &move_generator,
            &piece_config,
            &settings,
            max_plies,
            &cancel,
            &tx,
        );
    });
    rx
}

fn run_tournament_game(
    white_type: EngineType,
    black_type: EngineType,
    mut state: GameState,
    mg: &MoveGenerator,
    pc: &PieceConfigManager,
    settings: &GameSearchSettings,
    max_plies: usize,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<WorkerMsg>,
) {
    let mut white = match white_type.create() {
        Some(e) => e,
        None => {
            let _ = tx.send(WorkerMsg::Done {
                outcome: GameOutcome::BlackWins,
                termination: Termination::Forfeit,
                plies: 0,
                white_time: Duration::ZERO,
                black_time: Duration::ZERO,
            });
            return;
        }
    };
    let mut black = match black_type.create() {
        Some(e) => e,
        None => {
            let _ = tx.send(WorkerMsg::Done {
                outcome: GameOutcome::WhiteWins,
                termination: Termination::Forfeit,
                plies: 0,
                white_time: Duration::ZERO,
                black_time: Duration::ZERO,
            });
            return;
        }
    };

    let mut white_clock = Duration::ZERO;
    let mut black_clock = Duration::ZERO;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return; // rx sees Disconnected → "cancelled, don't record"
        }

        if let Some((outcome, termination)) = check_game_over(&mut state, mg, pc, max_plies) {
            let _ = tx.send(WorkerMsg::Done {
                outcome,
                termination,
                plies: state.move_history.len(),
                white_time: white_clock,
                black_time: black_clock,
            });
            return;
        }

        let color = state.current_turn;
        let (engine, my_clock, opp_clock) = match color {
            PieceColor::White => (&mut white, &mut white_clock, black_clock),
            PieceColor::Black => (&mut black, &mut black_clock, white_clock),
        };
        let (depth, time_limit) = settings.budget_for(color, *my_clock, opp_clock);

        let t0 = Instant::now();
        let result = engine.best_move(SearchParams {
            state: &mut state,
            move_generator: mg,
            config_manager: pc,
            depth,
            time_limit,
        });
        *my_clock += t0.elapsed();

        match result {
            Some(r) => {
                state.clear_redo();
                state.execute_expanded_move(&r.best_move, mg, pc);
                if tx
                    .send(WorkerMsg::Progress(DisplaySnapshot {
                        board: state.board.clone(),
                        current_turn: state.current_turn,
                        last_move: Some((r.best_move.from, r.best_move.to)),
                        plies: state.move_history.len(),
                    }))
                    .is_err()
                {
                    return;
                }
            }
            None => {
                let outcome = match color {
                    PieceColor::White => GameOutcome::BlackWins,
                    PieceColor::Black => GameOutcome::WhiteWins,
                };
                let _ = tx.send(WorkerMsg::Done {
                    outcome,
                    termination: Termination::Forfeit,
                    plies: state.move_history.len(),
                    white_time: white_clock,
                    black_time: black_clock,
                });
                return;
            }
        }
    }
}

/// Classify the position. Returns both the ELO‑relevant outcome and the
/// diagnostic termination reason.
fn check_game_over(
    state: &mut GameState,
    mg: &MoveGenerator,
    pc: &PieceConfigManager,
    max_plies: usize,
) -> Option<(GameOutcome, Termination)> {
    if state.move_history.len() >= max_plies {
        return Some((GameOutcome::AdjudicatedDraw, Termination::PlyLimit));
    }

    // make_move → check_draw_conditions may already have flagged a
    // rule‑based draw. Stalemate is *not* set there (that's mate‑status
    // territory), but handle it anyway for robustness.
    if let Some(GameResult::Draw(reason)) = &state.game_result {
        let t = match reason {
            DrawReason::FiftyMoveRule => Termination::FiftyMoveRule,
            DrawReason::Repetition => Termination::Repetition,
            DrawReason::InsufficientMaterial => Termination::InsufficientMaterial,
            DrawReason::Stalemate => Termination::Stalemate,
        };
        return Some((GameOutcome::Draw, t));
    }

    let stm = state.current_turn;
    match state.get_mate_status(mg, pc) {
        MateStatus::Checkmate => {
            // get_mate_status folds extinction (no pieces left, so no
            // moves, and trivially not in check) into Checkmate because
            // the score is the same. Split it back out here so the
            // report can tell a mating attack from a wipe‑out.
            let term = if state.board.has_pieces(stm) {
                Termination::Checkmate
            } else {
                Termination::Extinction
            };
            Some((
                match stm {
                    PieceColor::White => GameOutcome::BlackWins,
                    PieceColor::Black => GameOutcome::WhiteWins,
                },
                term,
            ))
        }
        MateStatus::OpponentLostByCheck => Some((
            match stm {
                PieceColor::White => GameOutcome::WhiteWins,
                PieceColor::Black => GameOutcome::BlackWins,
            },
            Termination::OpponentLeftInCheck,
        )),
        MateStatus::Stalemate => Some((GameOutcome::Draw, Termination::Stalemate)),
        MateStatus::Ongoing => None,
    }
}
