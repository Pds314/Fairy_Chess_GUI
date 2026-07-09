pub(crate) mod subscription;
pub(crate) mod update;
use crate::asset_manager::{AssetManager, BrowserItem};
use crate::background::{DisplaySnapshot, EngineMoveResult, WorkerMsg};
use crate::commands;
use crate::core::{GameResult, GameState, PieceColor, Position};
use crate::engine::analysis::PositionAnalysis;
use crate::engine::{EngineType, GameController};
use crate::evolution::EvolutionState;
use crate::handlers::loading;
use crate::messages::UiPanel;
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use crate::promotion_dialog::PromotionDialog;
use crate::texture_manager::TextureManager;
use crate::tournament::Tournament;
use crate::clog;
use iced::widget::canvas;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
pub struct ChessGui {
    pub(crate) game_state: GameState,
    pub(crate) selected_square: Option<Position>,
    pub(crate) board_cache: canvas::Cache,
    pub(crate) texture_manager: TextureManager,
    pub(crate) piece_config: Arc<PieceConfigManager>,
    pub(crate) move_generator: Arc<MoveGenerator>,
    pub(crate) asset_manager: AssetManager,
    pub(crate) castling_highlights: Vec<(Position, Position)>,
    pub(crate) game_controller: GameController,
    pub(crate) pending_engine_move: bool,
    pub(crate) engine_job: Option<EngineJob>,
    pub(crate) variant_generation: u64,
    pub(crate) promotion_dialog: Option<PromotionDialog>,
    pub(crate) current_game_file: Option<String>,
    pub(crate) game_file_items: Vec<BrowserItem>,
    pub(crate) selected_game_file: Option<BrowserItem>,
    pub(crate) position_analysis: Option<PositionAnalysis>,
    pub(crate) last_move_highlight: Option<(Position, Position)>,
    pub(crate) terminal_input: String,
    pub(crate) white_time_input: String,
    pub(crate) black_time_input: String,
    pub(crate) eval_time_input: String,
    pub(crate) white_time_respect_input: String,
    pub(crate) black_time_respect_input: String,
    pub(crate) startup_commands: Vec<String>,
    pub(crate) tournament: Tournament,
    pub(crate) tournament_graph_cache: canvas::Cache,
    pub(crate) tournament_games_input: String,
    pub(crate) tournament_max_plies_input: String,
    pub(crate) tournament_parallelism_input: String,
    pub(crate) tournament_workers: Vec<TournamentWorker>,
    pub(crate) tournament_initial_state: Option<GameState>,
    pub(crate) evolution: EvolutionState,
    pub(crate) evolution_workers: Vec<EvolutionWorker>,
    pub(crate) evolution_initial_state: Option<GameState>,
    pub(crate) evolution_base_engine: EngineType,
    pub(crate) evolution_population_input: String,
    pub(crate) evolution_play_bias_input: String,
    pub(crate) evolution_replication_bias_input: String,
    pub(crate) evolution_mutation_scale_input: String,
    pub(crate) evolution_repro_rate_input: String,
    pub(crate) evolution_crossover: bool,
    pub(crate) evolution_max_plies_input: String,
    pub(crate) evolution_parallelism_input: String,
    pub(crate) evolution_autosave: bool,
    pub(crate) evolution_autosave_path: String,
    pub(crate) evolution_locked_params: HashSet<String>,
    pub(crate) show_lock_menu: bool,
    pub(crate) active_panel: UiPanel,
    pub(crate) show_console: bool,
}
pub(crate) struct EngineJob {
    pub slot: PieceColor,
    pub engine_type: EngineType,
    pub rx: mpsc::Receiver<EngineMoveResult>,
    pub dispatched_hash: u64,
    pub dispatched_ply: usize,
    pub variant_generation: u64,
}
pub(crate) struct TournamentWorker {
    pub pairing: (EngineType, EngineType),
    pub rx: mpsc::Receiver<WorkerMsg>,
    pub cancel: Arc<AtomicBool>,
    pub plies: usize,
    pub last_snapshot: Option<DisplaySnapshot>,
}
/// A single in-flight evolution game. `white_idx`/`black_idx` are
/// population slot indices at dispatch time; `white_id`/`black_id` are
/// the individual IDs that occupied those slots, used to detect (and
/// discard) results whose slot got replaced by a replication event while
/// the game was still in flight.
pub(crate) struct EvolutionWorker {
    pub white_idx: usize,
    pub black_idx: usize,
    pub white_id: u64,
    pub black_id: u64,
    pub rx: mpsc::Receiver<WorkerMsg>,
    pub cancel: Arc<AtomicBool>,
    pub plies: usize,
    pub last_snapshot: Option<crate::background::DisplaySnapshot>,
}
impl ChessGui {
    pub(crate) fn is_game_ongoing(&self) -> bool {
        matches!(self.game_state.game_result, Some(GameResult::Ongoing))
    }
}
impl Default for ChessGui {
    fn default() -> Self {
        let mut asset_manager = AssetManager::new();
        if let Some(dir) = asset_manager.get_personalities_directory() {
            let n = crate::engine::personality::load_from_dir(&dir);
            if n > 0 {
                clog!("👥 {} engine personalities available", n);
            }
        } else {
            let _ =
            crate::engine::personality::load_from_dir(std::path::Path::new("/nonexistent"));
        }
        let (board_config, current_game_file) = loading::load_default_game_config(&asset_manager);
        let piece_config = loading::load_pieces_from_config(&asset_manager, &board_config);
        let mut move_generator = loading::create_move_generator(&piece_config);
        let texture_manager = TextureManager::new(&piece_config);
        let game_state =
        GameState::from_config(board_config, &piece_config, &mut move_generator);
        let game_controller = GameController::new();
        let game_file_items = asset_manager.list_game_files();
        let startup_commands = commands::collect_startup_commands();
        clog!("🎮 Fairy Chess GUI Started!");
        clog!("💻 Terminal commands available - type 'help' in the terminal input below");
        clog!("🎯 Use 'board' to see the current position, 'move e2 e4' to make moves");
        clog!("📋 Type 'help' for full command list, 'board' to display current position");
        if !startup_commands.is_empty() {
            clog!(
                "📝 {} startup commands queued from command line/stdin",
                startup_commands.len()
            );
        }
        clog!("═══════════════════════════════════════════════════════");
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
            evolution: EvolutionState::new(),
            evolution_workers: Vec::new(),
            evolution_initial_state: None,
            evolution_base_engine: EngineType::Simple,
            evolution_population_input: "12".to_string(),
            evolution_play_bias_input: "1.0".to_string(),
            evolution_replication_bias_input: "1.0".to_string(),
            evolution_mutation_scale_input: "0.08".to_string(),
            evolution_repro_rate_input: "0".to_string(), // 0 = auto (≈ population)
            evolution_crossover: true,
            evolution_max_plies_input: "400".to_string(),
            evolution_parallelism_input: "1".to_string(),
            evolution_autosave: false,
            evolution_autosave_path: String::new(),
            evolution_locked_params: HashSet::new(),
            show_lock_menu: false,
            active_panel: UiPanel::Game,
            show_console: true,
        };
        gui.execute_startup_commands();
        gui
    }
}
