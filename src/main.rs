// src/main.rs
mod asset_manager;
mod background;
mod board_config;
mod constants;
mod core;
mod drawing;
mod engine;
mod insufficient_material;
mod move_generator;
mod notation;
mod pgn;
mod piece_config;
mod promotion;
mod promotion_dialog;
mod texture_manager;
mod tournament;
mod tournament_graph;
mod zobrist; // ← add with the other `mod` lines

use crate::asset_manager::{AssetManager, BrowserItem};
use crate::board_config::BoardConfig;
use crate::constants::{WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::core::game_state::MateStatus;
use crate::core::{
    DrawReason, GameResult, GameState, MoveAttemptResult, PendingMove, Piece, PieceColor, Position,
};
use crate::drawing::BoardDrawer;
use crate::engine::EngineType;
use crate::engine::GameController;
use crate::engine::analysis::PositionAnalysis;
use crate::move_generator::MoveGenerator;
use crate::notation::position_to_algebraic;
use crate::pgn::PgnExporter;
use crate::piece_config::PieceConfigManager;
use crate::promotion_dialog::PromotionDialog;
use crate::texture_manager::TextureManager;
use crate::tournament::{GameOutcome, Termination, Tournament, TournamentPhase};
use crate::tournament_graph::TournamentGraph;
// add to the `use` block:
use crate::background::{DisplaySnapshot, GameSearchSettings, WorkerMsg};

use std::collections::HashMap;

use iced::time;
use iced::widget::{button, canvas, checkbox, column, container, pick_list, row, text, text_input};
use iced::{Element, Length, Settings, Task, Theme};
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::time::Instant;

pub fn main() -> iced::Result {
    use iced::window;

    iced::application("Fairy Chess GUI", ChessGui::update, ChessGui::view)
        .theme(ChessGui::theme)
        .subscription(ChessGui::subscription)
        .window(window::Settings {
            size: iced::Size::new(WINDOW_WIDTH * 1.5, WINDOW_HEIGHT * 1.2),
            min_size: Some(iced::Size::new(800.0, 600.0)),
            resizable: true,
            decorations: true,
            ..Default::default()
        })
        .settings(Settings {
            antialiasing: true,
            ..Settings::default()
        })
        .run()
}

struct ChessGui {
    game_state: GameState,
    selected_square: Option<Position>,
    board_cache: canvas::Cache,
    texture_manager: TextureManager,

    // Shared read‑only context. Arc so worker threads can hold references
    // without cloning the (potentially large) precomputed move tables.
    // Deref coercion means existing `&self.move_generator` call sites that
    // expect `&MoveGenerator` still compile unchanged.
    piece_config: Arc<PieceConfigManager>,
    move_generator: Arc<MoveGenerator>,

    asset_manager: AssetManager,
    castling_highlights: Vec<(Position, Position)>,
    game_controller: GameController,
    pending_engine_move: bool,

    // In‑flight single‑game engine computation. At most one at a time.
    engine_job: Option<EngineJob>,
    // Bumped whenever the variant / piece set changes so a stale engine
    // returning from a worker knows to drop its caches.
    variant_generation: u64,

    promotion_dialog: Option<PromotionDialog>,
    current_game_file: Option<String>,
    game_file_items: Vec<BrowserItem>,
    selected_game_file: Option<BrowserItem>,
    position_analysis: Option<PositionAnalysis>,
    last_move_highlight: Option<(Position, Position)>,
    terminal_input: String,
    white_time_input: String,
    black_time_input: String,
    eval_time_input: String,
    white_time_respect_input: String,
    black_time_respect_input: String,
    startup_commands: Vec<String>,

    tournament: Tournament,
    tournament_graph_cache: canvas::Cache,
    tournament_games_input: String,
    tournament_max_plies_input: String,
    tournament_parallelism_input: String,

    // Parallel‑tournament runtime state.
    tournament_workers: Vec<TournamentWorker>,
    tournament_initial_state: Option<GameState>,
}

/// A single engine move being computed off the GUI thread.
struct EngineJob {
    slot: PieceColor,
    engine_type: EngineType,
    rx: mpsc::Receiver<crate::background::EngineMoveResult>,
    dispatched_hash: u64,
    dispatched_ply: usize,
    variant_generation: u64,
}

/// One tournament game running on its own thread.
struct TournamentWorker {
    pairing: (EngineType, EngineType),
    rx: mpsc::Receiver<WorkerMsg>,
    cancel: Arc<AtomicBool>,
    plies: usize,
    last_snapshot: Option<DisplaySnapshot>,
}

impl Default for ChessGui {
    fn default() -> Self {
        let mut asset_manager = AssetManager::new();
        if let Some(dir) = asset_manager.get_personalities_directory() {
            let n = crate::engine::personality::load_from_dir(&dir);
            if n > 0 {
                println!("👥 {} engine personalities available", n);
            }
        } else {
            let _ = crate::engine::personality::load_from_dir(std::path::Path::new("/nonexistent"));
        }

        let (board_config, current_game_file) = load_default_game_config(&asset_manager);
        let piece_config = load_pieces_from_config(&asset_manager, &board_config);
        let mut move_generator = create_move_generator(&piece_config);
        let texture_manager = TextureManager::new(&piece_config);
        let game_state = GameState::from_config(board_config, &piece_config, &mut move_generator);
        let game_controller = GameController::new();

        let game_file_items = asset_manager.list_game_files();
        let startup_commands = Self::collect_startup_commands();

        let default_parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        println!("🎮 Fairy Chess GUI Started!");
        println!("💻 Terminal commands available - type 'help' in the terminal input below");
        println!("🎯 Use 'board' to see the current position, 'move e2 e4' to make moves");
        println!("📋 Type 'help' for full command list, 'board' to display current position");
        if !startup_commands.is_empty() {
            println!(
                "📝 {} startup commands queued from command line/stdin",
                startup_commands.len()
            );
        }
        println!("═══════════════════════════════════════════════════════");

        let mut gui = ChessGui {
            game_state,
            selected_square: None,
            board_cache: canvas::Cache::new(),
            texture_manager,
            piece_config: Arc::new(piece_config),
            move_generator: Arc::new(move_generator),
            asset_manager,
            castling_highlights: Vec::new(),
            game_controller,
            pending_engine_move: false,
            engine_job: None,
            variant_generation: 0,
            promotion_dialog: None,
            current_game_file,
            game_file_items,
            selected_game_file: None,
            position_analysis: None,
            last_move_highlight: None,
            terminal_input: String::new(),
            white_time_input: String::new(),
            black_time_input: String::new(),
            eval_time_input: String::new(),
            white_time_respect_input: String::new(),
            black_time_respect_input: String::new(),
            startup_commands,
            tournament: Tournament::new(),
            tournament_graph_cache: canvas::Cache::new(),
            tournament_games_input: "10".to_string(),
            tournament_max_plies_input: "400".to_string(),
            tournament_parallelism_input: "1".to_string(),
            tournament_workers: Vec::new(),
            tournament_initial_state: None,
        };

        // Suggest the machine's core count but leave the default at 1 so
        // behaviour is unchanged unless the user opts in.
        let _ = default_parallelism; // (shown in the UI hint below)

        gui.execute_startup_commands();
        gui
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SquareClicked(Position),
    UndoMove,
    RedoMove,
    ResetBoard,
    GenerateMoves,
    EvaluatePosition,
    EvaluateMoves,
    MakeBestMove,
    WhiteEngineSelected(EngineType),
    BlackEngineSelected(EngineType),
    EvalEngineSelected(EngineType),
    WhiteDepthSelected(u32),
    BlackDepthSelected(u32),
    EvalDepthSelected(u32),
    WhiteTimeInputChanged(String),
    BlackTimeInputChanged(String),
    EvalTimeInputChanged(String),
    WhiteTimeRespectChanged(String),
    BlackTimeRespectChanged(String),
    UnlimitedDepthToggled(bool),
    AutoPlayToggled(bool),
    Tick,
    PlayEngineMove,
    PromotionSelected(usize),
    GameFileSelected(BrowserItem),
    LoadGameFile,
    AnalyzePosition,
    TerminalInputChanged(String),
    TerminalCommand,
    WhiteEngineParameterChanged(String, f64),
    BlackEngineParameterChanged(String, f64),
    EvalEngineParameterChanged(String, f64),
    PrintPgn,
    LoadPgn,
    TournamentStart,
    TournamentStop,
    TournamentClearHistory,
    TournamentToggleParticipant(EngineType),
    TournamentGamesInputChanged(String),
    TournamentMaxPliesInputChanged(String),
    TournamentParallelismInputChanged(String),
    TournamentTick,
}

const DEPTHS: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

impl ChessGui {
    fn collect_startup_commands() -> Vec<String> {
        let mut commands = Vec::new();
        let args: Vec<String> = std::env::args().collect();
        for i in 1..args.len() {
            if args[i] == "-c" || args[i] == "--command" {
                if i + 1 < args.len() {
                    commands.push(args[i + 1].clone());
                }
            } else if args[i] == "-f" || args[i] == "--file" {
                if i + 1 < args.len() {
                    if let Ok(contents) = std::fs::read_to_string(&args[i + 1]) {
                        for line in contents.lines() {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                                commands.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        }

        if !std::io::stdin().is_terminal() {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                if let Ok(line) = line {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        commands.push(trimmed.to_string());
                    }
                }
            }
        }

        commands
    }

    fn print_pgn(&self) {
        println!("\n=== GAME PGN ===");

        // Need `&mut MoveGenerator` for from_config(); clone the inner
        // generator once rather than the Arc.
        let mut mg = (*self.move_generator).clone();

        let initial_state = if let Some(game_file) = self.current_game_file.clone() {
            if let Ok(board_config) = BoardConfig::load_from_file(&game_file) {
                GameState::from_config(board_config.clone(), &self.piece_config, &mut mg)
            } else {
                GameState::from_config(BoardConfig::default(), &self.piece_config, &mut mg)
            }
        } else {
            GameState::from_config(BoardConfig::default(), &self.piece_config, &mut mg)
        };

        let pgn = PgnExporter::export_game(
            &initial_state,
            &self.game_state.move_history,
            &self.move_generator,
            &self.piece_config,
            self.current_game_file.as_deref(),
        );

        println!("{}", pgn);
        println!("=== END PGN ===\n");

        println!("Total moves: {}", self.game_state.move_history.len());
        println!("Current turn: {:?}", self.game_state.current_turn);
        if let Some(result) = &self.game_state.game_result {
            println!("Game result: {:?}", result);
        }
    }

    fn execute_startup_commands(&mut self) {
        let commands = std::mem::take(&mut self.startup_commands);
        for command in commands {
            println!("📝 Executing startup command: {}", command);
            self.handle_terminal_command(&command);
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
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
            Message::GenerateMoves => {
                self.handle_generate_moves();
            }
            Message::EvaluatePosition => {
                self.handle_evaluate_position();
            }
            Message::EvaluateMoves => {
                self.handle_evaluate_moves();
            }
            Message::MakeBestMove => {
                if self.engine_job.is_none() && self.is_game_ongoing() {
                    self.start_engine_move();
                }
            }
            Message::WhiteEngineSelected(engine_type) => {
                self.game_controller.set_white_engine(engine_type);
                println!(
                    "White engine: {}",
                    self.game_controller.get_white_engine_type().name()
                );
                self.check_for_engine_move();
            }
            Message::BlackEngineSelected(engine_type) => {
                self.game_controller.set_black_engine(engine_type);
                println!(
                    "Black engine: {}",
                    self.game_controller.get_black_engine_type().name()
                );
                self.check_for_engine_move();
            }
            Message::EvalEngineSelected(engine_type) => {
                self.game_controller.set_eval_engine(engine_type);
                println!(
                    "Evaluation engine: {}",
                    self.game_controller.get_eval_engine_type().name()
                );
            }
            Message::WhiteDepthSelected(depth) => {
                self.game_controller.set_white_search_depth(depth);
                println!("White engine depth set to: {}", depth);
            }
            Message::BlackDepthSelected(depth) => {
                self.game_controller.set_black_search_depth(depth);
                println!("Black engine depth set to: {}", depth);
            }
            Message::EvalDepthSelected(depth) => {
                self.game_controller.set_eval_search_depth(depth);
                println!("Evaluation engine depth set to: {}", depth);
            }
            Message::WhiteTimeInputChanged(input) => {
                self.white_time_input = input;
                if let Ok(seconds) = self.white_time_input.parse::<f32>() {
                    if seconds > 0.0 {
                        self.game_controller.set_white_time_limit(Some(seconds));
                        println!("White time limit set to: {} seconds", seconds);
                    }
                } else if self.white_time_input.is_empty() {
                    self.game_controller.set_white_time_limit(None);
                    println!("White time limit disabled");
                }
            }
            Message::BlackTimeInputChanged(input) => {
                self.black_time_input = input;
                if let Ok(seconds) = self.black_time_input.parse::<f32>() {
                    if seconds > 0.0 {
                        self.game_controller.set_black_time_limit(Some(seconds));
                        println!("Black time limit set to: {} seconds", seconds);
                    }
                } else if self.black_time_input.is_empty() {
                    self.game_controller.set_black_time_limit(None);
                    println!("Black time limit disabled");
                }
            }
            Message::EvalTimeInputChanged(input) => {
                self.eval_time_input = input;
                if let Ok(seconds) = self.eval_time_input.parse::<f32>() {
                    if seconds > 0.0 {
                        self.game_controller.set_eval_time_limit(Some(seconds));
                        println!("Evaluation time limit set to: {} seconds", seconds);
                    }
                } else if self.eval_time_input.is_empty() {
                    self.game_controller.set_eval_time_limit(None);
                    println!("Evaluation time limit disabled");
                }
            }
            Message::WhiteTimeRespectChanged(input) => {
                self.white_time_respect_input = input;
                if let Ok(respect) = self.white_time_respect_input.parse::<f32>() {
                    self.game_controller.set_white_time_respect(respect);
                    println!("White time respect set to: {:.2}", respect);
                }
            }
            Message::BlackTimeRespectChanged(input) => {
                self.black_time_respect_input = input;
                if let Ok(respect) = self.black_time_respect_input.parse::<f32>() {
                    self.game_controller.set_black_time_respect(respect);
                    println!("Black time respect set to: {:.2}", respect);
                }
            }
            Message::UnlimitedDepthToggled(enabled) => {
                self.game_controller.set_unlimited_depth_with_time(enabled);
                println!(
                    "Unlimited depth with time limit: {}",
                    if enabled { "enabled" } else { "disabled" }
                );
            }
            Message::AutoPlayToggled(enabled) => {
                if self.tournament.is_active() {
                    println!("⚠️ Auto-play disabled during tournament");
                    return Task::none();
                }
                self.game_controller.set_auto_play(enabled);
                if enabled {
                    println!("Auto-play enabled");
                    self.check_for_engine_move();
                } else {
                    println!("Auto-play disabled");
                }
            }
            Message::Tick => {
                // Drain any finished background engine move first…
                self.poll_engine_job();
                // …then, if one is pending and nothing is running, kick it off.
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
                        println!("Failed to load game file '{}': {}", game_file, e);
                    }
                }
            }
            Message::AnalyzePosition => {
                self.handle_analyze_position();
            }
            Message::TerminalInputChanged(input) => {
                self.terminal_input = input;
            }
            Message::TerminalCommand => {
                let command = self.terminal_input.clone();
                self.handle_terminal_command(&command);
                self.terminal_input.clear();
            }
            Message::WhiteEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_white_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_white_engine_parameters(params) {
                        println!(
                            "⚙️ White engine parameter '{}' set to {:.3}",
                            param_id, value
                        );
                    }
                }
            }
            Message::BlackEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_black_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_black_engine_parameters(params) {
                        println!(
                            "⚙️ Black engine parameter '{}' set to {:.3}",
                            param_id, value
                        );
                    }
                }
            }
            Message::EvalEngineParameterChanged(param_id, value) => {
                if let Some(mut params) = self.game_controller.get_eval_engine_parameters() {
                    params.set(&param_id, value);
                    if self.game_controller.set_eval_engine_parameters(params) {
                        println!(
                            "⚙️ Eval engine parameter '{}' set to {:.3}",
                            param_id, value
                        );
                        self.position_analysis = None;
                    }
                }
            }
            Message::PrintPgn => {
                self.print_pgn();
            }
            Message::LoadPgn => {
                println!(
                    "📄 Paste PGN into terminal and use 'loadpgn' command (not yet wired to GUI file dialog)"
                );
            }
            Message::TournamentStart => {
                let seed = self.game_state.current_hash();
                match self.tournament.start(seed) {
                    Ok(()) => {
                        println!(
                            "🏆 Tournament started: {} engines, {} games total, {} in parallel",
                            self.tournament.participants.len(),
                            self.tournament.total_games(),
                            self.tournament.parallelism,
                        );
                        self.tournament_graph_cache.clear();
                        self.game_controller.set_auto_play(false);
                        self.pending_engine_move = false;

                        // Build the starting position once; each worker clones it.
                        self.reset_board_for_tournament();
                        self.tournament_initial_state = Some(self.game_state.clone_for_worker());
                        self.board_cache.clear();
                    }
                    Err(msg) => {
                        println!("❌ Cannot start tournament: {}", msg);
                    }
                }
            }
            Message::TournamentStop => {
                println!(
                    "⏹️ Tournament stopped ({}/{} games completed, {} in flight)",
                    self.tournament.games_played(),
                    self.tournament.total_games(),
                    self.tournament_workers.len()
                );
                for w in &self.tournament_workers {
                    w.cancel.store(true, Ordering::Relaxed);
                }
                self.tournament.stop();
                self.tournament_initial_state = None;
                // Workers drain on subsequent TournamentTicks.
            }
            Message::TournamentClearHistory => {
                self.tournament.clear_history();
                self.tournament_graph_cache.clear();
                println!("🗑️ Tournament history cleared");
            }
            Message::TournamentToggleParticipant(engine) => {
                let added = self.tournament.toggle_participant(engine.clone());
                println!(
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
                // Also drain any stale single‑game engine job so its engine
                // is returned to the controller instead of being leaked.
                self.poll_engine_job();
                self.tournament_tick();
            }
        }
        Task::none()
    }

    fn reset_board_for_tournament(&mut self) {
        if let Some(game_file) = self.current_game_file.clone() {
            if let Err(e) = self.load_game_from_file(&game_file) {
                println!(
                    "⚠️ Variant reload failed during tournament: {}. Using default.",
                    e
                );
                let board_config = load_board_config(&self.asset_manager);
                self.game_state = GameState::from_config(
                    board_config,
                    &self.piece_config,
                    Arc::make_mut(&mut self.move_generator),
                );
                self.game_controller.reset_engine_caches();
            }
        } else {
            let board_config = load_board_config(&self.asset_manager);
            self.game_state = GameState::from_config(
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

    // ──────────────────────────────────────────────────────────────────
    // Parallel tournament driver
    // ──────────────────────────────────────────────────────────────────

    fn tournament_tick(&mut self) {
        // 1. Drain all worker channels.
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

        // 2. Top the pool back up.
        if self.tournament.is_active() {
            while self.tournament_workers.len() < self.tournament.parallelism {
                match self.tournament.take_next_pairing() {
                    Some(pairing) => self.spawn_tournament_worker(pairing),
                    None => break,
                }
            }
        }

        // 3. Refresh the board canvas if the featured game advanced or changed.
        let featured_after = self.tournament_workers.first().map(|w| w.plies);
        if featured_before != featured_after {
            self.board_cache.clear();
        }

        // 4. Completion check.
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

    fn spawn_tournament_worker(&mut self, pairing: (EngineType, EngineType)) {
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
        println!(
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

    // ──────────────────────────────────────────────────────────────────
    // Background single‑move engine execution
    // ──────────────────────────────────────────────────────────────────

    /// Dispatch the current player's engine to a worker thread. No‑op if a
    /// job is already in flight.
    fn start_engine_move(&mut self) {
        if self.engine_job.is_some() {
            return;
        }
        let color = self.game_state.current_turn;

        // Start the clock now so the GUI timer animates while the worker
        // thinks. `end_turn` is called when the result is applied.
        self.game_controller.start_turn(color);
        let (depth, time_limit) = self.game_controller.compute_search_budget(color);

        let engine = match self.game_controller.take_engine(color) {
            Some(e) => e,
            None => {
                // Same behaviour as the old synchronous path when no engine
                // is assigned to this colour.
                println!("🏳️ Engine failed to provide a move! Resigning.");
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

    /// Poll the in‑flight engine job. Applies the move if it's still valid
    /// for the current position; otherwise discards it. Always returns the
    /// engine to the controller (unless the slot's engine type changed).
    fn poll_engine_job(&mut self) {
        let Some(job) = &self.engine_job else {
            return;
        };

        let (mut engine, result) = match job.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                // Worker thread panicked. Treat as forfeit.
                let slot = job.slot;
                self.engine_job = None;
                println!("⚠️ Engine worker thread terminated unexpectedly");
                self.game_state.game_result = Some(GameResult::Winner(slot.opposite()));
                self.game_controller.stop_timing(slot);
                return;
            }
        };

        let job = self.engine_job.take().unwrap();
        let engine_name = engine.name().to_string();

        // If the variant changed under us, the engine's caches are for the
        // wrong zobrist space.
        let variant_changed = job.variant_generation != self.variant_generation;
        if variant_changed {
            engine.reset_cache();
        }

        // Return the engine — unless the user re‑picked a different engine
        // for this slot while we were thinking, in which case the new one
        // is already installed and the old one is just dropped.
        let current_type = match job.slot {
            PieceColor::White => self.game_controller.get_white_engine_type().clone(),
            PieceColor::Black => self.game_controller.get_black_engine_type().clone(),
        };
        if current_type == job.engine_type {
            self.game_controller.put_engine(job.slot, engine);
        }

        // Is the result still for the position on the board?
        let stale = variant_changed
            || job.dispatched_hash != self.game_state.current_hash()
            || job.dispatched_ply != self.game_state.move_history.len();

        if stale {
            // Position changed (undo / reset / load) while thinking.
            self.game_controller.stop_timing(job.slot);
            self.check_for_engine_move();
            return;
        }

        match result {
            Some(r) => {
                let (from, to) = (r.best_move.from, r.best_move.to);
                println!(
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
                        println!("  Mate in {} moves!", mate_in);
                    } else {
                        println!("  Getting mated in {} moves!", -mate_in);
                    }
                } else {
                    println!("  Evaluation: {}", r.evaluation.score);
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
                println!("🏳️ Engine failed to provide a move! Resigning.");
                self.game_state.game_result = Some(GameResult::Winner(job.slot.opposite()));
                self.game_controller.stop_timing(job.slot);
            }
        }
    }

    /// Which board to draw. During a tournament we show the first running
    /// worker's latest snapshot; otherwise the real game state.
    fn displayed_board(&self) -> (&crate::core::board::Board, Option<(Position, Position)>) {
        if let Some(w) = self.tournament_workers.first() {
            if let Some(snap) = &w.last_snapshot {
                return (&snap.board, snap.last_move);
            }
        }
        (&self.game_state.board, self.last_move_highlight)
    }

    fn create_tournament_section(&self) -> Element<'_, Message> {
        use iced::widget::{Column, Row, checkbox};
        let header = text("🏆 Tournament").size(16);

        let phase_controls: Element<_> = match self.tournament.phase {
            TournamentPhase::Inactive | TournamentPhase::Complete => {
                let engines: Vec<EngineType> = EngineType::all()
                    .into_iter()
                    .filter(|e| !e.is_human())
                    .collect();

                let mid = (engines.len() + 1) / 2;
                let mut col_a = Column::new().spacing(4);
                let mut col_b = Column::new().spacing(4);

                for (i, engine) in engines.iter().enumerate() {
                    let checked = self.tournament.is_participant(engine);
                    let e = engine.clone();
                    let label = {
                        let n = engine.name();
                        if n.len() > 24 {
                            format!("{}…", &n[..23])
                        } else {
                            n.to_string()
                        }
                    };
                    let cb = checkbox(label, checked)
                        .on_toggle(move |_| Message::TournamentToggleParticipant(e.clone()))
                        .size(14)
                        .text_size(11);
                    if i < mid {
                        col_a = col_a.push(cb);
                    } else {
                        col_b = col_b.push(cb);
                    }
                }
                let participant_grid = Row::new().push(col_a).push(col_b).spacing(15);
                let games_input = text_input("games", &self.tournament_games_input)
                    .on_input(Message::TournamentGamesInputChanged)
                    .width(Length::Fixed(50.0));

                let plies_input = text_input("max plies", &self.tournament_max_plies_input)
                    .on_input(Message::TournamentMaxPliesInputChanged)
                    .width(Length::Fixed(60.0));

                // --- NEW CODE INSERTED HERE ---
                let parallelism_input = text_input("threads", &self.tournament_parallelism_input)
                    .on_input(Message::TournamentParallelismInputChanged)
                    .width(Length::Fixed(50.0));

                let cpu_hint = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1);
                // ------------------------------

                let n_participants = self
                    .tournament
                    .participants
                    .iter()
                    .filter(|e| !e.is_human())
                    .count();
                let can_start = n_participants >= 2;

                let start_btn = button("Start Tournament").on_press_maybe(if can_start {
                    Some(Message::TournamentStart)
                } else {
                    None
                });

                let total_games_preview = if can_start {
                    let n = n_participants;
                    let g = self.tournament.games_per_pairing;
                    // Each engine plays g games → n*g credits → n*g/2 pairings
                    text(format!("→ {} games total", n * g / 2)).size(11)
                } else {
                    text("(select ≥ 2 engines)").size(11)
                };

                let clear_btn = button("Clear History").on_press_maybe(
                    if self.tournament.elo.game_log().is_empty() {
                        None
                    } else {
                        Some(Message::TournamentClearHistory)
                    },
                );

                column![
                    text("Participants:").size(12),
                    participant_grid,
                    // --- MODIFIED ROW HERE ---
                    row![
                        text("Games/engine:").size(12),
                        games_input,
                        text("Max plies:").size(12),
                        plies_input,
                        text("Parallel:").size(12),
                        parallelism_input,
                        text(format!("(cpu: {})", cpu_hint)).size(10),
                    ]
                    .spacing(8),
                    // -------------------------
                    row![start_btn, clear_btn, total_games_preview].spacing(10),
                ]
                .spacing(6)
                .into()
            }
            TournamentPhase::SettingUpGame | TournamentPhase::Playing => {
                let played = self.tournament.games_played();
                let total = self.tournament.total_games();
                let running = self.tournament_workers.len();

                let progress_text = text(format!(
                    "Completed {}/{} · {} running",
                    played, total, running
                ))
                .size(14);

                let mut workers_col = iced::widget::Column::new().spacing(2);
                for (idx, w) in self.tournament_workers.iter().enumerate() {
                    let marker = if idx == 0 { "▶ " } else { "  " };
                    let line = format!(
                        "{}{} vs {} — ply {}",
                        marker,
                        w.pairing.0.name(),
                        w.pairing.1.name(),
                        w.plies
                    );
                    workers_col = workers_col.push(text(line).size(10));
                }

                let stop_btn = button("Stop Tournament")
                    .on_press(Message::TournamentStop)
                    .style(|_, status| {
                        let base = iced::widget::button::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                180, 70, 70,
                            ))),
                            text_color: iced::Color::WHITE,
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        };
                        match status {
                            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                    200, 90, 90,
                                ))),
                                ..base
                            },
                            _ => base,
                        }
                    });

                column![progress_text, workers_col, stop_btn]
                    .spacing(6)
                    .into()
            }
        };

        container(column![header, phase_controls].spacing(8))
            .padding(10)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    252, 248, 240,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(220, 200, 160),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn create_tournament_graph(&self) -> Element<'_, Message> {
        let (rows, _cols) = self.game_state.board.size();
        let board_height = constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;

        canvas(TournamentGraph {
            elo: &self.tournament.elo,
            total_games: self
                .tournament
                .total_games()
                .max(self.tournament.elo.game_log().len()),
            cache: &self.tournament_graph_cache,
        })
        .width(Length::Fixed(400.0))
        .height(Length::Fixed(board_height))
        .into()
    }

    fn check_for_engine_move(&mut self) {
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

    fn handle_evaluate_position(&mut self) {
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

            println!(
                "\n=== Position Evaluation (using {}) ===",
                self.game_controller.get_eval_engine_type().name()
            );
            println!("Current player: {:?}", self.game_state.current_turn);
            println!(
                "Evaluation: {} (from current player's perspective)",
                result.evaluation.score
            );
            println!(
                "Best move: {}{} -> {}{}",
                (b'a' + from.1 as u8) as char,
                8 - from.0,
                (b'a' + to.1 as u8) as char,
                8 - to.0
            );

            if result.evaluation.score > 900000 {
                println!("Position is winning (mate coming)");
            } else if result.evaluation.score < -900000 {
                println!("Position is losing (getting mated)");
            } else if result.evaluation.score > 100 {
                println!("Current player has advantage");
            } else if result.evaluation.score < -100 {
                println!("Current player is worse");
            } else {
                println!("Position is roughly equal");
            }

            print_search_stats(&stats, duration);
        } else {
            println!("No evaluation engine selected");
        }
    }

    fn handle_evaluate_moves(&mut self) {
        let start = Instant::now();
        self.game_state.reset_performance_stats();
        let legal_moves = self
            .game_state
            .get_legal_moves(&self.move_generator, &self.piece_config);
        if legal_moves.is_empty() {
            println!("No legal moves available!");
            return;
        }

        println!(
            "\n=== Evaluating All Moves (using {}) ===",
            self.game_controller.get_eval_engine_type().name()
        );
        println!("Evaluating {} legal moves...", legal_moves.len());

        let mut move_evaluations = Vec::new();
        let board_size = self.game_state.board.size();

        for mv in legal_moves {
            self.game_state
                .execute_expanded_move(&mv, &self.move_generator, &self.piece_config);
            if let Some(result) = self.game_controller.evaluate_position(
                &mut self.game_state,
                &self.move_generator,
                &self.piece_config,
            ) {
                move_evaluations.push((mv.clone(), -result.evaluation.score));
            }
            self.game_state.undo_move(&self.piece_config);
        }

        let duration = start.elapsed();
        let stats = self.game_state.get_performance_stats();
        move_evaluations.sort_by_key(|(_, score)| -score);

        println!("\n=== Move Evaluations (best to worst) ===");
        for (i, (mv, score)) in move_evaluations.iter().enumerate() {
            let move_str = format_move(mv, &self.piece_config, board_size);
            print!("{}. {} - Score: {}", i + 1, move_str, score);
            if *score > 900000 {
                print!(" (Winning!)");
            } else if *score < -900000 {
                print!(" (Losing!)");
            } else if *score > 100 {
                print!(" (Good)");
            } else if *score < -100 {
                print!(" (Bad)");
            }
            if i == 0 {
                print!(" <- BEST");
            }
            println!();
        }
        print_search_stats(&stats, duration);
    }

    fn handle_analyze_position(&mut self) {
        let start = Instant::now();
        println!("\n=== Comprehensive Position Analysis ===");

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
            println!(
                "Analysis completed using {} in {:.2?}",
                engine_name, duration
            );

            self.print_comprehensive_analysis(&analysis);
            self.position_analysis = Some(analysis);
        } else {
            println!(
                "❌ Engine '{}' does not support detailed position analysis",
                engine_name
            );
            println!(
                "💡 Try using the 'Piece Square Table Engine' for comprehensive analysis features"
            );
        }
    }

    fn print_comprehensive_analysis(&self, analysis: &PositionAnalysis) {
        println!("\n🔍 MATERIAL ANALYSIS:");
        println!("  White total: {:.2}", analysis.material_values.white_total);
        println!("  Black total: {:.2}", analysis.material_values.black_total);
        println!(
            "  Material difference: {:.2} (+ = White advantage)",
            analysis.material_values.difference
        );

        println!("\n📊 PIECE STATISTICS:");
        for (&piece_type, &avg_value) in &analysis.material_values.piece_values {
            if let Some(piece_config) = self.piece_config.get_piece_by_index(piece_type) {
                let (white_count, black_count) = analysis
                    .material_values
                    .piece_counts
                    .get(&piece_type)
                    .unwrap_or(&(0, 0));
                println!(
                    "  {}: Avg value {:.2}, Count W:{} B:{}",
                    piece_config.display_name, avg_value, white_count, black_count
                );
            }
        }

        if let Some(ref pst) = analysis.pst_analysis {
            println!("\n🎯 PIECE SQUARE TABLE ANALYSIS:");
            println!("  White PST total: {:.2}", pst.white_pst_total);
            println!("  Black PST total: {:.2}", pst.black_pst_total);
            println!("  PST difference: {:.2}", pst.pst_difference);

            println!("\n📈 PST STATISTICS BY PIECE:");
            for (piece_type, stats) in &pst.piece_pst_stats {
                if let Some(piece_config) = self.piece_config.get_piece_by_index(*piece_type) {
                    println!("  {}:", piece_config.display_name);
                    println!(
                        "    Value range: {:.2} to {:.2} (σ={:.2})",
                        stats.min_value, stats.max_value, stats.standard_deviation
                    );
                    if stats.current_count > 0 {
                        println!(
                            "    Current: {} pieces, avg {:.2}, total {:.2}",
                            stats.current_count, stats.current_average, stats.current_total
                        );
                    }
                }
            }

            println!("\n🧭 POSITIONAL BIAS ANALYSIS:");
            let bias = &pst.variance_analysis.positional_bias;
            println!(
                "  Forward bias: {:.3} (positive = pieces prefer advancement)",
                bias.forward_bias
            );
            println!(
                "  Center bias: {:.3} (positive = pieces prefer center)",
                bias.center_bias
            );
            println!(
                "  Edge bias: {:.3} (positive = pieces prefer edges)",
                bias.edge_bias
            );
            println!(
                "  Left/right bias: {:.3} (positive = right, negative = left)",
                bias.left_right_bias
            );

            println!("\n🔥 VALUE DISTRIBUTION:");
            let dist = &pst.variance_analysis.value_distribution;
            println!("  Value range: {:.2}", dist.value_range);
            println!("  Value variance: {:.2}", dist.value_variance);
            println!("  Highest value squares:");
            for (pos, value) in dist.highest_value_squares.iter().take(3) {
                let algebraic = position_to_algebraic(*pos, self.game_state.board.size());
                println!("    {} = {:.2}", algebraic, value);
            }

            println!("\n⚡ SWARM & TACTICAL FACTORS:");
            let swarm = &pst.swarm_factors;
            println!("  Average swarm bonus: {:.3}", swarm.average_swarm_bonus);
            println!(
                "  Max swarm effectiveness: {:.3}",
                swarm.swarm_effectiveness
            );
            println!("  Huddle factor: {:.3}", swarm.huddle_factor);
            if let Some((pos, value)) = swarm.max_swarm_position {
                let algebraic = position_to_algebraic(pos, self.game_state.board.size());
                println!("  Best attack square: {} (bonus: {:.2})", algebraic, value);
            }
        }

        println!("\n🏃 MOBILITY ANALYSIS:");
        let mobility = &analysis.mobility_analysis;
        println!("  White mobility: {} moves", mobility.white_mobility);
        println!("  Black mobility: {} moves", mobility.black_mobility);
        println!(
            "  Mobility difference: {} (+ = White advantage)",
            mobility.mobility_difference
        );
        println!(
            "  Value to mobility ratio: {:.2}",
            mobility.value_to_mobility_ratio
        );

        println!("\n🎲 MOBILITY BY PIECE TYPE:");
        for (piece_type, stats) in &mobility.piece_mobility {
            if let Some(piece_config) = self.piece_config.get_piece_by_index(*piece_type) {
                println!(
                    "  {}: {} total moves, {:.1} avg per piece ({} attacking, {} non-attacking)",
                    piece_config.display_name,
                    stats.total_moves,
                    stats.average_mobility,
                    stats.attacking_moves,
                    stats.non_attacking_moves
                );
            }
        }

        println!("\n🔬 THEORETICAL MOBILITY (Partially-Blocked Board):");
        for (piece_type, stats) in &mobility.theoretical_mobility {
            if let Some(piece_config) = self.piece_config.get_piece_by_index(*piece_type) {
                println!("  {}:", piece_config.display_name);
                println!(
                    "    Center: {:.1}, Edge: {:.1}, Corner: {:.1} value",
                    stats.center_mobility, stats.edge_mobility, stats.corner_mobility
                );
                println!(
                    "    Empty board range: {} to {} moves",
                    stats.min_mobility, stats.max_mobility
                );
                println!(
                    "    Concentration: {:.3} (1.0=focused, 0.0=diffuse)",
                    stats.concentration_factor
                );
                println!(
                    "    Positional variance: {:.2} (spread after probabilistic move)",
                    stats.mobility_variance
                );
            }
        }

        println!("\n⚔️ THREAT ANALYSIS:");
        let threats = &mobility.threat_analysis;
        println!(
            "  White threatens: {:.1} value ({:.1}% of mobility is captures)",
            threats.white_threats_value, threats.white_capture_mobility_percentage
        );
        println!(
            "  Black threatens: {:.1} value ({:.1}% of mobility is captures)",
            threats.black_threats_value, threats.black_capture_mobility_percentage
        );
        println!(
            "  White attackers total value: {:.1}",
            threats.white_attackers_value
        );
        println!(
            "  Black attackers total value: {:.1}",
            threats.black_attackers_value
        );
        println!(
            "  Threat balance - White: {:.1}, Black: {:.1}",
            threats.white_threat_balance, threats.black_threat_balance
        );

        println!("\n🏗️ DENSITY & CLUSTERING:");
        let density = &analysis.density_analysis;
        println!("  Board density: {:.1}%", density.board_density * 100.0);
        println!("  Value per density: {:.2}", density.piece_density_ratio);
        println!(
            "  Average piece distance: {:.2}",
            density.clustering.average_piece_distance
        );
        println!(
            "  Clustering coefficient: {:.2}",
            density.clustering.clustering_coefficient
        );
        println!(
            "  Isolated pieces: {}",
            density.clustering.isolated_pieces.len()
        );
        println!(
            "  Dense regions: {}",
            density.clustering.dense_regions.len()
        );

        println!("\n📊 STATISTICAL SUMMARY:");
        let stats = &analysis.statistical_analysis;
        println!(
            "  Weakest piece value: {:.2}",
            stats.normalized_values.weakest_piece_value
        );
        println!("  Total pieces: {}", stats.statistics.total_pieces);
        println!(
            "  Simple average value per piece: {:.2}",
            stats.statistics.value_per_piece
        );
        println!(
            "  Value-weighted average: {:.2}",
            stats.statistics.value_weighted_average
        );
        println!(
            "  Piece type diversity: {:.2}",
            stats.statistics.piece_type_diversity
        );
        println!(
            "  Position complexity: {:.2}",
            stats.statistics.position_complexity
        );

        println!("\n💰 NORMALIZED VALUES (relative to weakest piece):");
        println!(
            "  White total: {:.1}x",
            stats.normalized_values.white_total_normalized
        );
        println!(
            "  Black total: {:.1}x",
            stats.normalized_values.black_total_normalized
        );
        for (piece_type, &normalized_value) in &stats.normalized_values.piece_values_normalized {
            if let Some(piece_config) = self.piece_config.get_piece_by_index(*piece_type) {
                println!("  {}: {:.1}x", piece_config.display_name, normalized_value);
            }
        }

        println!("\n=== Analysis Complete ===");
    }

    fn handle_terminal_command(&mut self, command: &str) {
        let command = command.trim();
        println!("\n> {}", command);

        if command.is_empty() {
            return;
        }

        let parts: Vec<&str> = command.split_whitespace().collect();
        let cmd = parts[0].to_lowercase();

        match cmd.as_str() {
            "help" | "h" => {
                self.print_terminal_help();
            }
            "board" | "b" => {
                self.print_board();
            }
            "move" | "m" => {
                if parts.len() >= 3 {
                    let from_str = parts[1];
                    let to_str = parts[2];
                    if let (Some(from), Some(to)) =
                        (self.parse_position(from_str), self.parse_position(to_str))
                    {
                        self.attempt_terminal_move(from, to);
                    } else {
                        println!(
                            "❌ Invalid position format. Use algebraic notation (e.g., 'move e2 e4')"
                        );
                    }
                } else {
                    println!("❌ Usage: move <from> <to> (e.g., 'move e2 e4')");
                }
            }
            "undo" | "u" => {
                println!("🔄 Undoing last move...");
                self.handle_undo();
            }
            "redo" => {
                if self
                    .game_state
                    .redo_move(&self.move_generator, &self.piece_config)
                {
                    println!("✅ Move redone!");
                    if let Some(last) = self.game_state.move_history.last() {
                        self.last_move_highlight = Some((last.from, last.to));
                    }
                    self.print_board();
                    self.check_for_engine_move();
                } else {
                    println!("❌ Nothing to redo");
                }
            }
            "loadpgn" => {
                if parts.len() < 2 {
                    println!("❌ Usage: loadpgn <pgn_text_in_quotes_or_file_path>");
                    println!("   Example: loadpgn \"1. e4 e5 2. Nf3 Nc6\"");
                } else {
                    let pgn_text = parts[1..].join(" ");
                    let pgn_text = pgn_text.trim_matches('"');

                    let pgn_content = if std::path::Path::new(pgn_text).exists() {
                        match std::fs::read_to_string(pgn_text) {
                            Ok(content) => content,
                            Err(e) => {
                                println!("❌ Failed to read file: {}", e);
                                return;
                            }
                        }
                    } else {
                        pgn_text.to_string()
                    };

                    self.handle_reset();

                    match crate::pgn::PgnImporter::load_pgn(
                        &pgn_content,
                        &mut self.game_state,
                        &self.move_generator,
                        &self.piece_config,
                    ) {
                        Ok(count) => {
                            println!("✅ Loaded {} moves from PGN", count);
                            if let Some(last) = self.game_state.move_history.last() {
                                self.last_move_highlight = Some((last.from, last.to));
                            }
                            self.print_board();
                        }
                        Err(e) => {
                            println!("❌ PGN loading failed: {}", e);
                            println!("   Moves applied before error were kept.");
                            self.print_board();
                        }
                    }
                }
            }
            "reset" | "r" => {
                println!("🔄 Resetting board...");
                self.handle_reset();
            }
            "analyze" | "a" => {
                println!("🔬 Analyzing position...");
                self.handle_analyze_position();
            }
            "moves" => {
                println!("📋 Generating all legal moves...");
                self.handle_generate_moves();
            }
            "eval" => {
                println!("⚖️ Evaluating position...");
                self.handle_evaluate_position();
            }
            "best" => {
                println!("🎯 Making best move...");
                if self.is_game_ongoing() {
                    self.start_engine_move();
                } else {
                    println!("❌ Game is already over.");
                }
            }
            "engine" => {
                if parts.len() >= 2 {
                    self.set_engine_from_terminal(&parts[1..]);
                } else {
                    self.print_engine_status();
                }
            }
            "turn" => {
                println!("Current turn: {:?}", self.game_state.current_turn);
            }
            "depth" => {
                if parts.len() >= 3 {
                    let target = parts[1];
                    if let Ok(depth) = parts[2].parse::<u32>() {
                        match target {
                            "w" | "white" => {
                                self.game_controller.set_white_search_depth(depth);
                                println!("✅ White search depth set to {}", depth);
                            }
                            "b" | "black" => {
                                self.game_controller.set_black_search_depth(depth);
                                println!("✅ Black search depth set to {}", depth);
                            }
                            "e" | "eval" => {
                                self.game_controller.set_eval_search_depth(depth);
                                println!("✅ Evaluation search depth set to {}", depth);
                            }
                            _ => {
                                println!(
                                    "❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"
                                );
                            }
                        }
                    } else {
                        println!("❌ Invalid depth. Must be a positive number");
                    }
                } else {
                    println!("❌ Usage: depth <w|b|e> <number>");
                }
            }
            "time" => {
                if parts.len() >= 3 {
                    let target = parts[1];
                    let time_str = parts[2];

                    let time_limit = if time_str == "off" || time_str == "none" {
                        None
                    } else if let Ok(seconds) = time_str.parse::<f32>() {
                        if seconds > 0.0 {
                            Some(seconds)
                        } else {
                            println!("❌ Time must be positive");
                            return;
                        }
                    } else {
                        println!("❌ Invalid time. Use a number in seconds or 'off' to disable");
                        return;
                    };

                    match target {
                        "w" | "white" => {
                            self.game_controller.set_white_time_limit(time_limit);
                            if let Some(t) = time_limit {
                                self.white_time_input = t.to_string();
                                println!("✅ White time limit set to {} seconds", t);
                            } else {
                                self.white_time_input.clear();
                                println!("✅ White time limit disabled");
                            }
                        }
                        "b" | "black" => {
                            self.game_controller.set_black_time_limit(time_limit);
                            if let Some(t) = time_limit {
                                self.black_time_input = t.to_string();
                                println!("✅ Black time limit set to {} seconds", t);
                            } else {
                                self.black_time_input.clear();
                                println!("✅ Black time limit disabled");
                            }
                        }
                        "e" | "eval" => {
                            self.game_controller.set_eval_time_limit(time_limit);
                            if let Some(t) = time_limit {
                                self.eval_time_input = t.to_string();
                                println!("✅ Evaluation time limit set to {} seconds", t);
                            } else {
                                self.eval_time_input.clear();
                                println!("✅ Evaluation time limit disabled");
                            }
                        }
                        _ => {
                            println!(
                                "❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"
                            );
                        }
                    }
                } else {
                    println!("❌ Usage: time <w|b|e> <seconds|off>");
                }
            }
            "respect" => {
                if parts.len() >= 3 {
                    let target = parts[1];
                    if let Ok(respect) = parts[2].parse::<f32>() {
                        if respect < 0.0 || respect > 1.0 {
                            println!("❌ Time respect must be between 0.0 and 1.0");
                            return;
                        }
                        match target {
                            "w" | "white" => {
                                self.game_controller.set_white_time_respect(respect);
                                self.white_time_respect_input = respect.to_string();
                                println!("✅ White time respect set to {:.2}", respect);
                            }
                            "b" | "black" => {
                                self.game_controller.set_black_time_respect(respect);
                                self.black_time_respect_input = respect.to_string();
                                println!("✅ Black time respect set to {:.2}", respect);
                            }
                            _ => {
                                println!("❌ Invalid target. Use 'w' (white) or 'b' (black)");
                            }
                        }
                    } else {
                        println!("❌ Invalid respect value. Must be a number between 0.0 and 1.0");
                    }
                } else {
                    println!("❌ Usage: respect <w|b> <0.0-1.0>");
                    println!("   Example: respect w 0.1  (White gets ±10% of time difference)");
                }
            }
            "unlimited" => {
                let current = self.game_controller.get_unlimited_depth_with_time();
                self.game_controller.set_unlimited_depth_with_time(!current);
                println!(
                    "✅ Unlimited depth with time limits: {}",
                    if !current { "enabled" } else { "disabled" }
                );
            }
            "status" => {
                self.print_game_status();
            }
            "tstats" | "treport" => {
                if self.tournament.elo.game_log().is_empty() {
                    println!("No tournament data yet.");
                } else if parts.len() >= 3 {
                    // Pairing drill‑down. Engine names may contain spaces,
                    // so accept a '/' separator: `tstats Swarm Engine / MCTS Engine`
                    let joined = parts[1..].join(" ");
                    let mut split = joined.splitn(2, '/');
                    match (split.next(), split.next()) {
                        (Some(a), Some(b)) => {
                            let a = a.trim();
                            let b = b.trim();
                            match (
                                crate::engine::personality::parse_engine_name(a),
                                crate::engine::personality::parse_engine_name(b),
                            ) {
                                (Some(ea), Some(eb)) => {
                                    self.tournament.elo.print_pairing_detail(&ea, &eb);
                                }
                                _ => println!(
                                    "❌ Unknown engine name. Use `tstats` alone for the full report."
                                ),
                            }
                        }
                        _ => println!("❌ Usage: tstats <engineA> / <engineB>"),
                    }
                } else {
                    self.tournament.elo.print_detailed_report();
                }
            }
            "pgn" | "p" => {
                println!("📄 Printing game PGN...");
                self.print_pgn();
            }
            "param" | "parameter" => {
                if parts.len() >= 4 {
                    let target = parts[1];
                    let param_id = parts[2];
                    if let Ok(value) = parts[3].parse::<f64>() {
                        match target {
                            "w" | "white" => {
                                if let Some(mut params) =
                                    self.game_controller.get_white_engine_parameters()
                                {
                                    params.set(param_id, value);
                                    if self.game_controller.set_white_engine_parameters(params) {
                                        println!(
                                            "✅ White engine parameter '{}' set to {:.3}",
                                            param_id, value
                                        );
                                    } else {
                                        println!("❌ Failed to set parameter");
                                    }
                                } else {
                                    println!("❌ White engine has no tunable parameters");
                                }
                            }
                            "b" | "black" => {
                                if let Some(mut params) =
                                    self.game_controller.get_black_engine_parameters()
                                {
                                    params.set(param_id, value);
                                    if self.game_controller.set_black_engine_parameters(params) {
                                        println!(
                                            "✅ Black engine parameter '{}' set to {:.3}",
                                            param_id, value
                                        );
                                    } else {
                                        println!("❌ Failed to set parameter");
                                    }
                                } else {
                                    println!("❌ Black engine has no tunable parameters");
                                }
                            }
                            "e" | "eval" => {
                                if let Some(mut params) =
                                    self.game_controller.get_eval_engine_parameters()
                                {
                                    params.set(param_id, value);
                                    if self.game_controller.set_eval_engine_parameters(params) {
                                        println!(
                                            "✅ Eval engine parameter '{}' set to {:.3}",
                                            param_id, value
                                        );
                                        self.position_analysis = None;
                                    } else {
                                        println!("❌ Failed to set parameter");
                                    }
                                } else {
                                    println!("❌ Eval engine has no tunable parameters");
                                }
                            }
                            _ => {
                                println!(
                                    "❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"
                                );
                            }
                        }
                    } else {
                        println!("❌ Invalid value. Must be a number");
                    }
                } else if parts.len() >= 2 {
                    let target = parts[1];
                    match target {
                        "w" | "white" => {
                            self.print_engine_parameters(
                                "White",
                                self.game_controller.get_white_engine_parameter_defs(),
                                self.game_controller.get_white_engine_parameters(),
                            );
                        }
                        "b" | "black" => {
                            self.print_engine_parameters(
                                "Black",
                                self.game_controller.get_black_engine_parameter_defs(),
                                self.game_controller.get_black_engine_parameters(),
                            );
                        }
                        "e" | "eval" => {
                            self.print_engine_parameters(
                                "Eval",
                                self.game_controller.get_eval_engine_parameter_defs(),
                                self.game_controller.get_eval_engine_parameters(),
                            );
                        }
                        _ => {
                            println!(
                                "❌ Invalid target. Use 'w' (white), 'b' (black), or 'e' (eval)"
                            );
                        }
                    }
                } else {
                    println!("❌ Usage: param <w|b|e> [param_id] [value]");
                    println!("   List parameters: param <w|b|e>");
                    println!("   Set parameter:   param <w|b|e> <param_id> <value>");
                }
            }
            _ => {
                println!(
                    "❌ Unknown command: '{}'. Type 'help' for available commands.",
                    cmd
                );
            }
        }
    }

    fn print_terminal_help(&self) {
        println!("📚 TERMINAL COMMANDS:");
        println!("  help, h                    - Show this help");
        println!("  board, b                   - Pretty print the current board");
        println!("  move <from> <to>           - Make a move (e.g., 'move e2 e4')");
        println!("  undo, u                    - Undo the last move");
        println!("  redo                       - Redo an undone move");
        println!("  reset, r                   - Reset board to starting position");
        println!("  analyze, a                 - Run comprehensive position analysis");
        println!("  moves                      - Generate and display all legal moves");
        println!("  eval                       - Evaluate current position");
        println!("  best                       - Make the best move according to engine");
        println!("  engine [type]              - Set engine or show engine status");
        println!("  depth <w|b|e> <n>          - Set search depth (w=white, b=black, e=eval)");
        println!("  time <w|b|e> <seconds>     - Set time limit in seconds (or 'off' to disable)");
        println!("  respect <w|b> <0.0-1.0>    - Set time respect factor");
        println!("  unlimited                  - Toggle unlimited depth with time limits");
        println!("  turn                       - Show whose turn it is");
        println!("  status                     - Show game status");
        println!("  tstats                     - Detailed tournament report");
        println!(
            "  tstats <A> / <B>           - Drill into one pairing (colour-split, lengths, endings)"
        );
        println!("  pgn, p                     - Print game in PGN format");
        println!("  loadpgn <text|file>        - Load game from PGN text or file");
        println!("  param <w|b|e>              - List engine parameters");
        println!("  param <w|b|e> <id> <value> - Set engine parameter");
        println!();
        println!("📍 Position format: Use algebraic notation (a1, b2, c3, etc.)");
        println!("⏱️ Time limits: When set, engines use iterative deepening");
        println!("🎮 All GUI functions are also available via these commands!");
    }

    fn print_engine_parameters(
        &self,
        engine_name: &str,
        defs: Option<&'static [crate::engine::parameters::ParameterDef]>,
        params: Option<crate::engine::parameters::EngineParameters>,
    ) {
        println!("⚙️ {} ENGINE PARAMETERS:", engine_name);

        let Some(defs) = defs else {
            println!("  No tunable parameters available");
            return;
        };

        if defs.is_empty() {
            println!("  No tunable parameters available");
            return;
        }

        let params = params.unwrap_or_default();

        for def in defs {
            let current = params.get_or_default(def.id, def.default);
            println!("  {} ({}):", def.display_name, def.id);
            println!(
                "    Current: {:.3}, Default: {:.3}, Range: [{:.2}, {:.2}]",
                current, def.default, def.min, def.max
            );
            println!("    {}", def.description);
        }
    }

    fn print_board(&self) {
        let (rows, cols) = self.game_state.board.size();

        println!("\n┌─────────────────────────────────┐");
        println!("│         CURRENT BOARD         │");
        println!("├─────────────────────────────────┤");

        print!("│   ");
        for col in 0..cols {
            print!(" {} ", (b'a' + col as u8) as char);
        }
        println!(" │");

        println!("├─────────────────────────────────┤");

        for row in 0..rows {
            print!("│ {} │", rows - row);
            for col in 0..cols {
                if let Some(piece) = self.game_state.board.get_piece((row, col)) {
                    let symbol = self.get_piece_symbol(&piece);

                    if let Some((from, to)) = self.last_move_highlight {
                        if (row, col) == from {
                            print!("[{}]", symbol);
                        } else if (row, col) == to {
                            print!("<{}>", symbol);
                        } else {
                            print!(" {} ", symbol);
                        }
                    } else {
                        print!(" {} ", symbol);
                    }
                } else {
                    print!(" · ");
                }
            }
            println!("│ {}", rows - row);
        }

        println!("├─────────────────────────────────┤");

        print!("│   ");
        for col in 0..cols {
            print!(" {} ", (b'a' + col as u8) as char);
        }
        println!(" │");

        println!("└─────────────────────────────────┘");

        println!("Turn: {:?}", self.game_state.current_turn);
        if let Some((from, to)) = self.last_move_highlight {
            let from_algebraic = self.position_to_algebraic(from);
            let to_algebraic = self.position_to_algebraic(to);
            println!(
                "Last move: {} → {} (shown as [piece] → <piece>)",
                from_algebraic, to_algebraic
            );
        }
        println!();
    }

    fn get_piece_symbol(&self, piece: &Piece) -> char {
        if let Some(piece_config) = self.piece_config.get_piece_by_index(piece.piece_type) {
            if let Some(symbol) = piece_config.characters.first() {
                let base_char = symbol.chars().next().unwrap_or('?');
                match piece.color {
                    PieceColor::White => base_char.to_ascii_uppercase(),
                    PieceColor::Black => base_char.to_ascii_lowercase(),
                }
            } else {
                '?'
            }
        } else {
            '?'
        }
    }

    fn parse_position(&self, pos_str: &str) -> Option<Position> {
        if pos_str.len() < 2 {
            return None;
        }

        let chars: Vec<char> = pos_str.chars().collect();
        let col_char = chars[0].to_ascii_lowercase();
        let row_str = &pos_str[1..];

        if let Ok(rank) = row_str.parse::<usize>() {
            let col = (col_char as u8).wrapping_sub(b'a') as usize;
            let (rows, cols) = self.game_state.board.size();

            if rank > 0 && rank <= rows && col < cols {
                let row = rows - rank;
                return Some((row, col));
            }
        }

        None
    }

    fn position_to_algebraic(&self, pos: Position) -> String {
        let (rows, _) = self.game_state.board.size();
        let col_char = (b'a' + pos.1 as u8) as char;
        let rank = rows - pos.0;
        format!("{}{}", col_char, rank)
    }

    fn attempt_terminal_move(&mut self, from: Position, to: Position) {
        let from_algebraic = self.position_to_algebraic(from);
        let to_algebraic = self.position_to_algebraic(to);

        if let Some(piece) = self.game_state.board.get_piece(from) {
            if piece.color != self.game_state.current_turn {
                println!(
                    "❌ Not your piece! It's {:?}'s turn.",
                    self.game_state.current_turn
                );
                return;
            }

            println!("🎯 Attempting move: {} → {}", from_algebraic, to_algebraic);

            match self
                .game_state
                .attempt_move(from, to, &self.move_generator, &self.piece_config)
            {
                MoveAttemptResult::Success => {
                    self.game_state.clear_redo();
                    self.last_move_highlight = Some((from, to));
                    self.position_analysis = None;
                    self.game_controller
                        .end_turn(self.game_state.current_turn.opposite());
                    println!("✅ Move successful!");
                    self.print_board();
                    self.check_for_engine_move();
                }
                MoveAttemptResult::Invalid => {
                    println!("❌ Invalid move!");
                }
                MoveAttemptResult::NeedsCastlingChoice => {
                    println!(
                        "🏰 Castling move detected - multiple options available. Use GUI for castling selection."
                    );
                }
                MoveAttemptResult::NeedsPromotion => {
                    println!("👑 Promotion required - use GUI for piece selection.");
                }
            }
        } else {
            println!("❌ No piece at {}", from_algebraic);
        }
    }

    fn set_engine_from_terminal(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.print_engine_status();
            return;
        }

        let engine_name = args.join(" ");
        if let Some(engine) = crate::engine::personality::parse_engine_name(&engine_name) {
            if engine.is_human() {
                println!("❌ Cannot use Human as evaluation engine");
                return;
            }
            self.game_controller.set_eval_engine(engine.clone());
            println!("🤖 Evaluation engine set to: {}", engine.name());
        } else {
            println!("❌ Unknown engine: '{}'. Available engines:", engine_name);
            for et in EngineType::all() {
                if !et.is_human() {
                    println!("  - {}", et.name());
                }
            }
        }
    }

    fn print_engine_status(&self) {
        println!("🤖 ENGINE STATUS:");
        println!(
            "  White: {} (depth: {}, time: {}, respect: {:.2})",
            self.game_controller.get_white_engine_type().name(),
            self.game_controller.get_white_search_depth(),
            self.game_controller
                .get_white_time_limit()
                .map(|t| format!("{}s", t))
                .unwrap_or_else(|| "none".to_string()),
            self.game_controller.get_white_time_respect()
        );
        println!(
            "  Black: {} (depth: {}, time: {}, respect: {:.2})",
            self.game_controller.get_black_engine_type().name(),
            self.game_controller.get_black_search_depth(),
            self.game_controller
                .get_black_time_limit()
                .map(|t| format!("{}s", t))
                .unwrap_or_else(|| "none".to_string()),
            self.game_controller.get_black_time_respect()
        );
        println!(
            "  Eval:  {} (depth: {}, time: {})",
            self.game_controller.get_eval_engine_type().name(),
            self.game_controller.get_eval_search_depth(),
            self.game_controller
                .get_eval_time_limit()
                .map(|t| format!("{}s", t))
                .unwrap_or_else(|| "none".to_string())
        );
        println!(
            "  Auto-play: {}",
            if self.game_controller.is_auto_play() {
                "ON"
            } else {
                "OFF"
            }
        );
        println!(
            "  Unlimited depth with time: {}",
            if self.game_controller.get_unlimited_depth_with_time() {
                "ON"
            } else {
                "OFF"
            }
        );

        let white_time = self.game_controller.get_white_time();
        let black_time = self.game_controller.get_black_time();
        if white_time != Duration::ZERO || black_time != Duration::ZERO {
            let diff = white_time.as_secs_f32() - black_time.as_secs_f32();
            println!(
                "  Time used: White {:.1}s, Black {:.1}s (diff: {:+.1}s)",
                white_time.as_secs_f32(),
                black_time.as_secs_f32(),
                diff
            );
        }
    }

    fn format_game_status(&self) -> String {
        if !self.tournament_workers.is_empty() {
            if let Some(w) = self.tournament_workers.first() {
                return format!(
                    "🏆 Tournament — showing: {} vs {} (ply {})",
                    w.pairing.0.name(),
                    w.pairing.1.name(),
                    w.plies
                );
            }
        }

        let base = match &self.game_state.game_result {
            Some(GameResult::Winner(color)) => format!("Game Over: {:?} Wins!", color),
            Some(GameResult::Draw(reason)) => format!(
                "Game Over: Draw by {}",
                match reason {
                    DrawReason::FiftyMoveRule => "fifty-move rule",
                    DrawReason::Repetition => "repetition",
                    DrawReason::Stalemate => "stalemate",
                    DrawReason::InsufficientMaterial => "insufficient material",
                }
            ),
            Some(GameResult::Ongoing) => format!(
                "Turn: {} | Fifty-move: {}",
                match self.game_state.current_turn {
                    PieceColor::White => "White",
                    PieceColor::Black => "Black",
                },
                self.game_state.fifty_move_counter
            ),
            None => "Game state unknown".to_string(),
        };

        if self.engine_job.is_some() {
            let who = match self.game_state.current_turn {
                PieceColor::White => "White",
                PieceColor::Black => "Black",
            };
            format!("{} | 🤔 {} is thinking…", base, who)
        } else {
            base
        }
    }

    fn print_game_status(&self) {
        println!("🎮 GAME STATUS:");
        println!("  Current turn: {:?}", self.game_state.current_turn);
        println!(
            "  Fifty-move counter: {}",
            self.game_state.fifty_move_counter
        );

        match &self.game_state.game_result {
            Some(GameResult::Winner(color)) => println!("  Result: {:?} Wins", color),
            Some(GameResult::Draw(reason)) => {
                println!(
                    "  Result: Draw by {}",
                    match reason {
                        DrawReason::FiftyMoveRule => "fifty-move rule",
                        DrawReason::Repetition => "repetition",
                        DrawReason::Stalemate => "stalemate",
                        DrawReason::InsufficientMaterial => "insufficient material",
                    }
                );
            }
            Some(GameResult::Ongoing) => println!("  Result: Game in progress"),
            None => println!("  Result: Unknown"),
        }

        if let Some(ref file) = self.current_game_file {
            println!(
                "  Game file: {}",
                Path::new(file)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
        }
    }

    fn handle_square_click(&mut self, clicked_pos: Position) {
        if self.promotion_dialog.is_some() {
            return;
        }
        if let Some(PendingMove::Castling {
            king_from,
            king_to,
            king_move,
            options,
        }) = &self.game_state.pending_move
        {
            for option in options.iter() {
                if clicked_pos == option.rook_from || clicked_pos == option.rook_to {
                    let (option, king_from, king_to, king_move) =
                        (option.clone(), *king_from, *king_to, king_move.clone());
                    self.game_state.pending_move = None;
                    self.game_state.execute_castling(
                        king_from,
                        king_to,
                        &king_move,
                        &option,
                        &self.piece_config,
                        &self.move_generator,
                    );
                    self.clear_selection();
                    self.castling_highlights.clear();
                    self.game_controller
                        .end_turn(self.game_state.current_turn.opposite());
                    self.check_for_engine_move();
                    return;
                }
            }
        }
        if !self.is_game_ongoing() {
            return;
        }
        if let Some(from_pos) = self.selected_square {
            self.try_make_move(from_pos, clicked_pos);
        } else {
            self.try_select_piece(clicked_pos);
        }
    }

    fn handle_generate_moves(&self) {
        let result = self
            .game_state
            .generate_pseudo_legal_moves(&self.move_generator, &self.piece_config);
        let board_size = self.game_state.board.size();
        print_move_generation_results(&result, &self.piece_config, board_size);
    }

    fn is_game_ongoing(&self) -> bool {
        matches!(self.game_state.game_result, Some(GameResult::Ongoing))
    }

    fn try_make_move(&mut self, from: Position, to: Position) {
        if from == to {
            self.clear_selection();
            return;
        }
        let current_turn = self.game_state.current_turn;

        if !self.game_controller.is_engine_turn(current_turn) {
            if let Some(piece) = self.game_state.board.get_piece(from) {
                if self
                    .move_generator
                    .get_move_rule(&self.game_state.board, from, to, piece.piece_type)
                    .is_some()
                {
                    if crate::promotion::PromotionManager::can_promote(
                        piece.piece_type,
                        &self.piece_config,
                    ) && self
                        .game_state
                        .promotion_config
                        .is_promotion_zone(to, piece.color)
                    {
                        let targets = crate::promotion::PromotionManager::get_promotion_targets(
                            piece.piece_type,
                            &self.piece_config,
                        );
                        if !targets.is_empty() {
                            self.promotion_dialog = Some(PromotionDialog::new(from, to, targets));
                            self.board_cache.clear();
                            return;
                        }
                    }
                }
            }
        }

        match self
            .game_state
            .attempt_move(from, to, &self.move_generator, &self.piece_config)
        {
            MoveAttemptResult::Success => {
                self.game_state.clear_redo();
                self.clear_selection();
                self.castling_highlights.clear();
                self.position_analysis = None;
                self.last_move_highlight = Some((from, to));
                self.game_controller.end_turn(current_turn);
                self.check_for_engine_move();
            }
            MoveAttemptResult::Invalid => self.try_select_piece(to),
            MoveAttemptResult::NeedsCastlingChoice => {
                if let Some(PendingMove::Castling { options, .. }) = &self.game_state.pending_move {
                    self.castling_highlights = options
                        .iter()
                        .map(|opt| (opt.rook_from, opt.rook_to))
                        .collect();
                    self.board_cache.clear();
                }
            }
            MoveAttemptResult::NeedsPromotion => {
                println!("Unexpected promotion state");
                self.clear_selection();
                self.game_controller.end_turn(current_turn);
                self.check_for_engine_move();
            }
        }
    }

    fn try_select_piece(&mut self, pos: Position) {
        if let Some(piece) = self.game_state.board.get_piece(pos) {
            if piece.color == self.game_state.current_turn {
                self.selected_square = Some(pos);
                self.board_cache.clear();
                return;
            }
        }
        self.clear_selection();
    }

    fn clear_selection(&mut self) {
        self.selected_square = None;
        self.promotion_dialog = None;
        self.board_cache.clear();
    }

    fn handle_undo(&mut self) {
        if self.game_state.undo_move_for_gui(&self.piece_config) {
            self.clear_selection();
            self.promotion_dialog = None;
        }
    }

    fn handle_reset(&mut self) {
        if let Some(game_file) = self.current_game_file.clone() {
            if let Err(e) = self.load_game_from_file(&game_file) {
                println!("Failed to reload game file '{}': {}", game_file, e);
                let board_config = load_board_config(&self.asset_manager);
                self.game_state = GameState::from_config(
                    board_config,
                    &self.piece_config,
                    Arc::make_mut(&mut self.move_generator),
                );
            }
        } else {
            let board_config = load_board_config(&self.asset_manager);
            self.game_state = GameState::from_config(
                board_config,
                &self.piece_config,
                Arc::make_mut(&mut self.move_generator),
            );
        }

        self.game_controller.reset_engine_caches();
        self.clear_selection();
        self.promotion_dialog = None;
        self.position_analysis = None;
        self.last_move_highlight = None;
    }

    fn load_game_from_file(
        &mut self,
        game_file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("Loading game from file: {}", game_file_path);

        let board_config = BoardConfig::load_from_file(game_file_path)
            .map_err(|e| format!("Failed to load board config: {}", e))?;

        board_config
            .validate()
            .map_err(|e| format!("Invalid game configuration: {}", e))?;

        println!("Game file validation passed");

        let piece_config = if !board_config.pieces_files.is_empty() {
            let mut pieces_file_paths = Vec::new();
            let game_file_dir = Path::new(game_file_path).parent();

            for pieces_file in &board_config.pieces_files {
                let pieces_path = if pieces_file.contains('/') || pieces_file.contains('\\') {
                    if let Some(game_dir) = game_file_dir {
                        game_dir.join(pieces_file)
                    } else {
                        self.asset_manager
                            .get_pieces_file_path(pieces_file)
                            .ok_or_else(|| format!("Could not find pieces file: {}", pieces_file))?
                    }
                } else {
                    self.asset_manager
                        .get_pieces_file_path(pieces_file)
                        .ok_or_else(|| format!("Could not find pieces file: {}", pieces_file))?
                };

                if !pieces_path.exists() {
                    return Err(
                        format!("Pieces file does not exist: {}", pieces_path.display()).into(),
                    );
                }

                pieces_file_paths.push(pieces_path);
            }

            PieceConfigManager::load_from_files(&pieces_file_paths)
                .map_err(|e| format!("Failed to load pieces: {}", e))?
        } else {
            load_piece_config(&self.asset_manager)
        };

        let mut move_generator = create_move_generator(&piece_config);
        let texture_manager = TextureManager::new(&piece_config);
        let game_state = GameState::from_config(board_config, &piece_config, &mut move_generator);

        self.piece_config = Arc::new(piece_config);
        self.move_generator = Arc::new(move_generator);
        self.variant_generation = self.variant_generation.wrapping_add(1);
        self.texture_manager = texture_manager;
        self.game_state = game_state;

        self.clear_selection();
        self.game_controller.reset_timers();
        self.game_controller.reset_engine_caches();

        println!("Successfully loaded game from: {}", game_file_path);
        Ok(())
    }

    fn view(&self) -> Element<'_, Message> {
        let canvas = self.create_board_canvas();
        let status = self.create_status_display();
        let timer = self.create_timer_display();
        let controls = self.create_control_panel();
        let controls_section = iced::widget::scrollable(controls)
            .height(Length::Shrink)
            .width(Length::Fill);

        let show_tournament_graph =
            self.tournament.is_active() || !self.tournament.elo.game_log().is_empty();

        let main_content = if show_tournament_graph {
            let graph = self.create_tournament_graph();
            let (rows, _cols) = self.game_state.board.size();
            let board_height =
                constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;

            let graph_section = iced::widget::scrollable(graph)
                .height(Length::Fixed(board_height))
                .width(Length::Fixed(400.0));

            column![
                status,
                timer,
                row![canvas, graph_section].spacing(10),
                controls_section,
            ]
            .spacing(10)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
        } else if let Some(ref analysis) = self.position_analysis {
            let analysis_display = self.create_analysis_display(analysis);
            let (rows, _cols) = self.game_state.board.size();
            let board_height =
                constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;

            let analysis_section = iced::widget::scrollable(analysis_display)
                .height(Length::Fixed(board_height))
                .width(Length::Fixed(400.0));

            column![
                status,
                timer,
                row![canvas, analysis_section].spacing(10),
                controls_section
            ]
            .spacing(10)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            column![status, timer, canvas, controls_section]
                .spacing(10)
                .padding(10)
                .width(Length::Fill)
                .height(Length::Fill)
        };

        if let Some(dialog) = &self.promotion_dialog {
            let dialog_view = dialog.view(
                &self.texture_manager,
                &self.piece_config,
                self.game_state.current_turn,
            );
            iced::widget::stack![main_content, dialog_view].into()
        } else {
            main_content.into()
        }
    }

    fn create_analysis_display(&self, analysis: &PositionAnalysis) -> Element<'_, Message> {
        let mut content = column![text("🔬 Position Analysis").size(16),].spacing(5);

        let material_text = format!(
            "Material: W:{:.1} B:{:.1} Diff:{:.1}",
            analysis.material_values.white_total,
            analysis.material_values.black_total,
            analysis.material_values.difference
        );
        content = content.push(text(material_text).size(12));

        if let Some(ref pst) = analysis.pst_analysis {
            let pst_text = format!(
                "PST Values: W:{:.1} B:{:.1} Diff:{:.1}",
                pst.white_pst_total, pst.black_pst_total, pst.pst_difference
            );
            content = content.push(text(pst_text).size(12));

            let bias = &pst.variance_analysis.positional_bias;
            let bias_text = format!(
                "Bias - Forward:{:.2} Center:{:.2} Edge:{:.2}",
                bias.forward_bias, bias.center_bias, bias.edge_bias
            );
            content = content.push(text(bias_text).size(11));

            let swarm_text = format!(
                "Swarm: Avg:{:.3} Max:{:.3} Huddle:{:.3}",
                pst.swarm_factors.average_swarm_bonus,
                pst.swarm_factors.swarm_effectiveness,
                pst.swarm_factors.huddle_factor
            );
            content = content.push(text(swarm_text).size(11));
        }

        let mobility_text = format!(
            "Mobility: W:{} B:{} Diff:{} Ratio:{:.2}",
            analysis.mobility_analysis.white_mobility,
            analysis.mobility_analysis.black_mobility,
            analysis.mobility_analysis.mobility_difference,
            analysis.mobility_analysis.value_to_mobility_ratio
        );
        content = content.push(text(mobility_text).size(12));

        let threats = &analysis.mobility_analysis.threat_analysis;
        let threat_text = format!(
            "Threats: W:{:.1}v ({:.0}% capt) B:{:.1}v ({:.0}% capt)",
            threats.white_threats_value,
            threats.white_capture_mobility_percentage,
            threats.black_threats_value,
            threats.black_capture_mobility_percentage
        );
        content = content.push(text(threat_text).size(11));

        let density_text = format!(
            "Density: {:.1}% Clustering:{:.2} Isolated:{}",
            analysis.density_analysis.board_density * 100.0,
            analysis.density_analysis.clustering.clustering_coefficient,
            analysis.density_analysis.clustering.isolated_pieces.len()
        );
        content = content.push(text(density_text).size(12));

        let stats_text = format!(
            "Stats: {} pieces, {:.1} avg, {:.1} weighted avg, {:.1}x complexity",
            analysis.statistical_analysis.statistics.total_pieces,
            analysis.statistical_analysis.statistics.value_per_piece,
            analysis
                .statistical_analysis
                .statistics
                .value_weighted_average,
            analysis.statistical_analysis.statistics.position_complexity
        );
        content = content.push(text(stats_text).size(12));

        let weakest_value = analysis
            .statistical_analysis
            .normalized_values
            .weakest_piece_value;
        let norm_text = format!(
            "Normalized (vs weakest {:.2}): W:{:.1}x B:{:.1}x",
            weakest_value,
            analysis
                .statistical_analysis
                .normalized_values
                .white_total_normalized,
            analysis
                .statistical_analysis
                .normalized_values
                .black_total_normalized
        );
        content = content.push(text(norm_text).size(11));

        content = content.push(text("📊 Piece Analysis:").size(12));

        for (&piece_type, &avg_value) in analysis.material_values.piece_values.iter().take(6) {
            if let Some(piece_config) = self.piece_config.get_piece_by_index(piece_type) {
                let (white_count, black_count) = analysis
                    .material_values
                    .piece_counts
                    .get(&piece_type)
                    .unwrap_or(&(0, 0));

                let mobility_info = if let Some(theoretical) = analysis
                    .mobility_analysis
                    .theoretical_mobility
                    .get(&piece_type)
                {
                    format!(
                        " [C:{:.1} E:{:.1} σ:{:.2}]",
                        theoretical.center_mobility,
                        theoretical.edge_mobility,
                        theoretical.concentration_factor
                    )
                } else {
                    String::new()
                };

                let piece_text = format!(
                    "  {}: {:.1}v (W:{} B:{}){}",
                    piece_config.display_name, avg_value, white_count, black_count, mobility_info
                );
                content = content.push(text(piece_text).size(10));
            }
        }

        container(content)
            .padding(10)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    248, 248, 255,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(200, 200, 220),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    fn create_board_canvas(&self) -> Element<'_, Message> {
        let (board, last_move) = self.displayed_board();
        let (rows, cols) = board.size();
        let board_width = constants::DEFAULT_SQUARE_SIZE * cols as f32 + constants::BOARD_PADDING;
        let board_height = constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;
        let canvas_size = board_width.max(board_height);

        canvas(BoardDrawer {
            board,
            selected_square: self.selected_square,
            cache: &self.board_cache,
            texture_manager: &self.texture_manager,
            piece_config: &self.piece_config,
            last_move_highlight: last_move,
        })
        .width(Length::Fixed(canvas_size))
        .height(Length::Fixed(canvas_size))
        .into()
    }

    fn create_status_display(&self) -> Element<'_, Message> {
        text(self.format_game_status()).size(16).into()
    }

    fn create_control_panel(&self) -> Element<'_, Message> {
        let file_browser_section = self.create_file_browser_section();
        let tournament_section = self.create_tournament_section();

        let white_selector = pick_list(
            EngineType::all(),
            Some(self.game_controller.get_white_engine_type().clone()),
            Message::WhiteEngineSelected,
        )
        .width(Length::Fill);
        let white_depth_selector = pick_list(
            DEPTHS.to_vec(),
            Some(self.game_controller.get_white_search_depth()),
            Message::WhiteDepthSelected,
        )
        .placeholder("Depth");
        let white_time_input = text_input("Time (s)", &self.white_time_input)
            .on_input(Message::WhiteTimeInputChanged)
            .width(Length::Fixed(70.0));
        let white_time_respect_input = text_input("Respect", &self.white_time_respect_input)
            .on_input(Message::WhiteTimeRespectChanged)
            .width(Length::Fixed(60.0));

        let black_selector = pick_list(
            EngineType::all(),
            Some(self.game_controller.get_black_engine_type().clone()),
            Message::BlackEngineSelected,
        )
        .width(Length::Fill);
        let black_depth_selector = pick_list(
            DEPTHS.to_vec(),
            Some(self.game_controller.get_black_search_depth()),
            Message::BlackDepthSelected,
        )
        .placeholder("Depth");
        let black_time_input = text_input("Time (s)", &self.black_time_input)
            .on_input(Message::BlackTimeInputChanged)
            .width(Length::Fixed(70.0));
        let black_time_respect_input = text_input("Respect", &self.black_time_respect_input)
            .on_input(Message::BlackTimeRespectChanged)
            .width(Length::Fixed(60.0));

        let eval_selector = pick_list(
            EngineType::all(),
            Some(self.game_controller.get_eval_engine_type().clone()),
            Message::EvalEngineSelected,
        )
        .width(Length::Fill);
        let eval_depth_selector = pick_list(
            DEPTHS.to_vec(),
            Some(self.game_controller.get_eval_search_depth()),
            Message::EvalDepthSelected,
        )
        .placeholder("Depth");
        let eval_time_input = text_input("Time (s)", &self.eval_time_input)
            .on_input(Message::EvalTimeInputChanged)
            .width(Length::Fixed(80.0));

        let auto_play_checkbox = checkbox("Auto-play", self.game_controller.is_auto_play())
            .on_toggle(Message::AutoPlayToggled);
        let unlimited_depth_checkbox = checkbox(
            "Unlimited depth with time",
            self.game_controller.get_unlimited_depth_with_time(),
        )
        .on_toggle(Message::UnlimitedDepthToggled);

        let game_actions = row![
            self.create_undo_button(),
            self.create_redo_button(),
            self.create_reset_button(),
            button("Generate Moves").on_press(Message::GenerateMoves),
            button("Print PGN").on_press(Message::PrintPgn),
        ]
        .spacing(10);

        let analysis_actions = column![
            row![
                button("Evaluate Position").on_press_maybe(if self.is_game_ongoing() {
                    Some(Message::EvaluatePosition)
                } else {
                    None
                }),
                button("Evaluate Moves").on_press_maybe(if self.is_game_ongoing() {
                    Some(Message::EvaluateMoves)
                } else {
                    None
                }),
                button("Make Best Move").on_press_maybe(if self.is_game_ongoing() {
                    Some(Message::MakeBestMove)
                } else {
                    None
                }),
            ]
            .spacing(10),
            row![
                button("🔬 Analyze Position")
                    .on_press_maybe(if self.is_game_ongoing() {
                        Some(Message::AnalyzePosition)
                    } else {
                        None
                    })
                    .style(|theme, status| {
                        let active = iced::widget::button::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                70, 130, 180,
                            ))),
                            text_color: iced::Color::WHITE,
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        };
                        match status {
                            iced::widget::button::Status::Active => active,
                            iced::widget::button::Status::Hovered => iced::widget::button::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                    100, 150, 200,
                                ))),
                                ..active
                            },
                            _ => active,
                        }
                    }),
                if self.game_controller.supports_analysis() {
                    text("✅ Advanced analysis available").size(12)
                } else {
                    text("⚠️ Use PST Engine for advanced analysis").size(12)
                }
            ]
            .spacing(10),
        ]
        .spacing(8);

        let white_params = self.create_parameter_controls(
            self.game_controller.get_white_engine_parameter_defs(),
            self.game_controller.get_white_engine_parameters(),
            Message::WhiteEngineParameterChanged,
        );

        let black_params = self.create_parameter_controls(
            self.game_controller.get_black_engine_parameter_defs(),
            self.game_controller.get_black_engine_parameters(),
            Message::BlackEngineParameterChanged,
        );

        let eval_params = self.create_parameter_controls(
            self.game_controller.get_eval_engine_parameter_defs(),
            self.game_controller.get_eval_engine_parameters(),
            Message::EvalEngineParameterChanged,
        );

        let engine_settings = row![
            column![
                text("White Player").size(16),
                row![
                    white_selector,
                    white_depth_selector,
                    white_time_input,
                    white_time_respect_input
                ]
                .spacing(5),
                white_params,
                text("Black Player").size(16),
                row![
                    black_selector,
                    black_depth_selector,
                    black_time_input,
                    black_time_respect_input
                ]
                .spacing(5),
                black_params,
            ]
            .spacing(8)
            .width(Length::FillPortion(2)),
            column![
                text("Analysis Engine").size(16),
                row![eval_selector, eval_depth_selector, eval_time_input].spacing(5),
                eval_params,
                row![
                    auto_play_checkbox.size(20),
                    unlimited_depth_checkbox.size(16)
                ]
                .spacing(10),
                text("Time Respect: 0.0-1.0 (adjusts time based on clock difference)").size(12),
            ]
            .spacing(8)
            .width(Length::FillPortion(2)),
        ]
        .spacing(20);

        let terminal_section = self.create_terminal_section();

        column![
            file_browser_section,
            tournament_section,
            engine_settings,
            game_actions,
            analysis_actions,
            terminal_section,
        ]
        .spacing(15)
        .width(Length::Fill)
        .into()
    }

    fn create_file_browser_section(&self) -> Element<'_, Message> {
        let current_file_display = if let Some(ref file) = self.current_game_file {
            text(format!(
                "Current: {}",
                Path::new(file)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ))
            .size(14)
        } else {
            text("Current: Default").size(14)
        };

        let file_selector = pick_list(
            self.game_file_items.clone(),
            self.selected_game_file.clone(),
            Message::GameFileSelected,
        )
        .placeholder("Browse for game files...")
        .width(Length::Fill);

        let load_button = button("Load Game").on_press_maybe(
            if self
                .selected_game_file
                .as_ref()
                .map_or(false, |item| matches!(item, BrowserItem::File(_)))
            {
                Some(Message::LoadGameFile)
            } else {
                None
            },
        );

        column![
            text("Game File").size(16),
            current_file_display,
            row![file_selector, load_button].spacing(10)
        ]
        .spacing(8)
        .into()
    }

    fn create_terminal_section(&self) -> Element<'_, Message> {
        let terminal_input = text_input("Enter command (try 'help')", &self.terminal_input)
            .on_input(Message::TerminalInputChanged)
            .on_submit(Message::TerminalCommand)
            .width(Length::Fill);

        column![
            text("💻 Terminal Commands").size(16),
            text("Type 'help' for available commands, 'board' to print board, 'move e2 e4' to make moves").size(12),
            terminal_input
        ].spacing(8).into()
    }

    fn create_undo_button(&self) -> Element<'_, Message> {
        button("Undo Move")
            .on_press_maybe(if self.game_state.can_undo() {
                Some(Message::UndoMove)
            } else {
                None
            })
            .into()
    }

    fn create_redo_button(&self) -> Element<'_, Message> {
        button("Redo Move")
            .on_press_maybe(if self.game_state.can_redo() {
                Some(Message::RedoMove)
            } else {
                None
            })
            .into()
    }

    fn create_reset_button(&self) -> Element<'_, Message> {
        button("Reset Board").on_press(Message::ResetBoard).into()
    }

    fn theme(&self) -> Theme {
        Theme::Light
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if self.tournament.is_active() || !self.tournament_workers.is_empty() {
            // Poll workers; also keeps the board/graph animating.
            time::every(Duration::from_millis(50)).map(|_| Message::TournamentTick)
        } else if self.engine_job.is_some() || self.pending_engine_move {
            // Fast tick while an engine is (about to be) thinking so the
            // clock animates and results are applied promptly.
            time::every(Duration::from_millis(33)).map(|_| Message::Tick)
        } else if self.is_game_ongoing() {
            time::every(Duration::from_millis(100)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
        }
    }

    fn create_timer_display(&self) -> Element<'_, Message> {
        let white_time = self.game_controller.get_white_time();
        let black_time = self.game_controller.get_black_time();
        let current_thinking = self
            .game_controller
            .get_current_thinking_time(self.game_state.current_turn);
        let white_total = if self.game_state.current_turn == PieceColor::White {
            white_time + current_thinking
        } else {
            white_time
        };
        let black_total = if self.game_state.current_turn == PieceColor::Black {
            black_time + current_thinking
        } else {
            black_time
        };
        let timer_text = format!(
            "⏱ White: {:02}:{:02}.{:01} | Black: {:02}:{:02}.{:01}",
            white_total.as_secs() / 60,
            white_total.as_secs() % 60,
            white_total.subsec_millis() / 100,
            black_total.as_secs() / 60,
            black_total.as_secs() % 60,
            black_total.subsec_millis() / 100
        );
        container(text(timer_text).size(14))
            .padding(5)
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    240, 240, 240,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(200, 200, 200),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    fn create_parameter_controls(
        &self,
        param_defs: Option<&'static [crate::engine::parameters::ParameterDef]>,
        current_params: Option<crate::engine::parameters::EngineParameters>,
        message_fn: fn(String, f64) -> Message,
    ) -> Element<'_, Message> {
        use iced::widget::{column, row, slider, text};

        let Some(defs) = param_defs else {
            return text("No tunable parameters").size(12).into();
        };

        if defs.is_empty() {
            return text("No tunable parameters").size(12).into();
        }

        let params = current_params.unwrap_or_default();

        let mut param_column = column![].spacing(8);

        for def in defs {
            let current_value = params.get_or_default(def.id, def.default);
            let param_id = def.id.to_string();

            let value_display = format!("{}: {:.2}", def.display_name, current_value);

            let param_slider = slider(
                def.min as f32..=def.max as f32,
                current_value as f32,
                move |v| message_fn(param_id.clone(), v as f64),
            )
            .step(if def.step > 0.0 {
                def.step as f32
            } else {
                0.01
            })
            .width(Length::Fixed(150.0));

            let param_row = row![
                text(value_display).size(11).width(Length::Fixed(180.0)),
                param_slider,
            ]
            .spacing(10);

            param_column = param_column.push(param_row);
        }

        param_column.into()
    }
}

// Helper functions
fn load_piece_config(asset_manager: &AssetManager) -> PieceConfigManager {
    if let Some(config_path) = asset_manager.get_pieces_config_path() {
        match PieceConfigManager::load_from_file(&config_path) {
            Ok(config) => {
                println!("Loaded pieces config from: {}", config_path.display());
                return config;
            }
            Err(e) => {
                println!("Failed to load pieces config: {}. Using default.", e);
            }
        }
    } else {
        println!("No pieces config found. Using default configuration.");
    }
    create_default_piece_config()
}

fn load_board_config(asset_manager: &AssetManager) -> BoardConfig {
    if let Some(config_path) = asset_manager.get_game_config_path() {
        match BoardConfig::load_from_file(&config_path) {
            Ok(config) => {
                println!("Loaded game config from: {}", config_path.display());
                return config;
            }
            Err(e) => {
                println!("Failed to load game config: {}. Using standard chess.", e);
            }
        }
    }
    BoardConfig::default()
}

fn load_default_game_config(asset_manager: &AssetManager) -> (BoardConfig, Option<String>) {
    if let Some(fide_path) = asset_manager.get_asset_path("FIDE.game") {
        if fide_path.exists() {
            match BoardConfig::load_from_file(&fide_path) {
                Ok(config) => {
                    if let Err(e) = config.validate() {
                        println!(
                            "Warning: FIDE.game validation failed: {}. Using default.",
                            e
                        );
                        return (BoardConfig::default(), None);
                    }
                    println!("Loaded FIDE.game from: {}", fide_path.display());
                    return (config, Some(fide_path.to_string_lossy().to_string()));
                }
                Err(e) => println!("Failed to load FIDE.game: {}. Using default.", e),
            }
        }
    }

    let config = load_board_config(asset_manager);
    (config, None)
}

fn load_pieces_from_config(
    asset_manager: &AssetManager,
    board_config: &BoardConfig,
) -> PieceConfigManager {
    if !board_config.pieces_files.is_empty() {
        let mut pieces_file_paths = Vec::new();
        let mut missing_files = Vec::new();

        for pieces_file in &board_config.pieces_files {
            if let Some(path) = asset_manager.get_pieces_file_path(pieces_file) {
                pieces_file_paths.push(path);
            } else {
                missing_files.push(pieces_file.clone());
            }
        }

        if !missing_files.is_empty() {
            println!(
                "Warning: Could not find pieces files: {:?}. Using fallback.",
                missing_files
            );
        }

        if !pieces_file_paths.is_empty() {
            match PieceConfigManager::load_from_files(&pieces_file_paths) {
                Ok(config) => return config,
                Err(e) => println!("Failed to load pieces from files: {}. Using fallback.", e),
            }
        }
    }

    load_piece_config(asset_manager)
}

fn create_move_generator(piece_config: &PieceConfigManager) -> MoveGenerator {
    MoveGenerator::new(piece_config).unwrap_or_else(|e| {
        panic!("Failed to create move generator: {}", e);
    })
}

fn create_default_piece_config() -> PieceConfigManager {
    let default_config = r#"
    Knight / knight, horse / N / +x / p;
    Rook / rook, castle, tower / R / +*,+*Ou / p;
    Bishop / bishop, elephant / B / x* / p;
    King / king, mann / K / +,x,<>+E_<>+E_ou / R;
    Queen / queen, lady / Q / +*,x* / p;
    Pawn / pawn, soldier / P / ^x!~i,^+_i,^+_^+e_ui / P;
    "#;
    PieceConfigManager::parse_config(default_config)
        .expect("Failed to parse default piece configuration")
}

fn format_move(
    mv: &core::game_state::ExpandedMove,
    config_manager: &PieceConfigManager,
    board_size: (usize, usize),
) -> String {
    let from_notation = position_to_algebraic(mv.from, board_size);
    let to_notation = position_to_algebraic(mv.to, board_size);
    let mut result = from_notation;
    if mv.captures.is_some() {
        result.push('x');
    } else {
        result.push('-');
    }
    result.push_str(&to_notation);
    if let Some(ref castling) = mv.castling_option {
        let rook_notation = position_to_algebraic(castling.rook_from, board_size);
        result.push_str(&format!(" (castle with {})", rook_notation));
    }
    if let Some(promo_type) = mv.promotion_target {
        if let Some(piece_config) = config_manager.get_piece_by_index(promo_type) {
            result.push_str(&format!("={}", piece_config.display_name));
        }
    }
    if mv.captures_position != mv.captures.map(|_| mv.to) {
        result.push_str(" e.p.");
    }
    result
}

fn print_move_generation_results(
    result: &core::game_state::MoveGenerationResult,
    config_manager: &PieceConfigManager,
    board_size: (usize, usize),
) {
    match result {
        core::game_state::MoveGenerationResult::Moves(moves) => {
            println!("\n=== Pseudo-legal Moves ({} total) ===", moves.len());
            let mut moves_by_from: std::collections::HashMap<
                Position,
                Vec<&core::game_state::ExpandedMove>,
            > = std::collections::HashMap::new();
            for mv in moves {
                moves_by_from
                    .entry(mv.from)
                    .or_insert_with(Vec::new)
                    .push(mv);
            }
            let mut positions: Vec<_> = moves_by_from.keys().cloned().collect();
            positions.sort_by_key(|&(r, c)| (r, c));
            for pos in positions {
                if let Some(moves) = moves_by_from.get(&pos) {
                    println!(
                        "\nFrom {}: ({} moves)",
                        position_to_algebraic(pos, board_size),
                        moves.len()
                    );
                    for mv in moves {
                        print!("  {}", format_move(mv, config_manager, board_size));
                        if let Some(captured) = mv.captures {
                            if let Some(cap_config) =
                                config_manager.get_piece_by_index(captured.piece_type)
                            {
                                print!(" (captures {})", cap_config.display_name);
                            }
                        }
                        println!();
                    }
                }
            }
        }
        core::game_state::MoveGenerationResult::Checkmate {
            move_that_captures_royal,
        } => {
            println!("\n=== CHECKMATE - Royal Can Be Captured! ===");
            println!(
                "Fatal move: {}",
                format_move(move_that_captures_royal, config_manager, board_size)
            );
            if let Some(captured) = move_that_captures_royal.captures {
                if let Some(royal_config) = config_manager.get_piece_by_index(captured.piece_type) {
                    println!(
                        "{} {} can be captured!",
                        if royal_config.properties.is_royal {
                            "Royal (R)"
                        } else {
                            "Royalty (r)"
                        },
                        royal_config.display_name
                    );
                }
            }
            println!(
                "\nThis position is illegal - the previous player left their royal piece in check!"
            );
        }
    }
}

fn print_search_stats(stats: &core::game_state::PerformanceTracker, duration: std::time::Duration) {
    println!("\n--- Search Statistics ---");
    println!("Time elapsed: {:.2?}", duration);
    println!("Moves generated: {}", stats.moves_generated);
    println!(
        "Moves made/undone: {}/{}",
        stats.moves_made, stats.moves_undone
    );
    println!(
        "Pseudo-legal generations: {}",
        stats.pseudo_legal_generations
    );
    println!("Legal move checks: {}", stats.legal_move_checks);
    println!("Check tests: {}", stats.check_tests);
    println!("Mate status checks: {}", stats.mate_status_checks);
    let total_operations = stats.moves_made + stats.pseudo_legal_generations + stats.check_tests;
    if duration.as_millis() > 0 {
        let ops_per_sec = (total_operations as f64 * 1000.0) / duration.as_millis() as f64;
        println!("Operations/second: {:.0}", ops_per_sec);
        let moves_per_sec = (stats.moves_generated as f64 * 1000.0) / duration.as_millis() as f64;
        println!("Moves generated/second: {:.0}", moves_per_sec);
    }
}
