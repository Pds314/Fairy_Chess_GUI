use crate::app::ChessGui;
use crate::asset_manager::AssetManager;
use crate::board_config::BoardConfig;
use crate::core::GameState;
use crate::move_generator::MoveGenerator;
use crate::pgn::PgnExporter;
use crate::piece_config::PieceConfigManager;
use crate::texture_manager::TextureManager;
use crate::clog;
use std::path::Path;
use std::sync::Arc;

impl ChessGui {
    pub(crate) fn load_game_from_file(
        &mut self,
        game_file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        clog!("Loading game from file: {}", game_file_path);
        let board_config = BoardConfig::load_from_file(game_file_path)
            .map_err(|e| format!("Failed to load board config: {}", e))?;
        board_config
            .validate()
            .map_err(|e| format!("Invalid game configuration: {}", e))?;

        let piece_config = if !board_config.pieces_files.is_empty() {
            let mut paths = Vec::new();
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
                        format!("Pieces file does not exist: {}", pieces_path.display()).into()
                    );
                }
                paths.push(pieces_path);
            }
            PieceConfigManager::load_from_files(&paths)
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
        clog!("Successfully loaded game from: {}", game_file_path);
        Ok(())
    }

    pub(crate) fn print_pgn(&self) {
        clog!("\n=== GAME PGN ===");
        let mut mg = (*self.move_generator).clone();
        let initial_state = if let Some(game_file) = self.current_game_file.clone() {
            if let Ok(board_config) = BoardConfig::load_from_file(&game_file) {
                GameState::from_config(board_config, &self.piece_config, &mut mg)
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
        clog!("{}", pgn);
        clog!("=== END PGN ===\n");
        clog!("Total moves: {}", self.game_state.move_history.len());
        clog!("Current turn: {:?}", self.game_state.current_turn);
        if let Some(result) = &self.game_state.game_result {
            clog!("Game result: {:?}", result);
        }
    }
}

// ─── Free functions for config/setup ────────────────────────────────────

pub(crate) fn load_piece_config(asset_manager: &AssetManager) -> PieceConfigManager {
    if let Some(config_path) = asset_manager.get_pieces_config_path() {
        match PieceConfigManager::load_from_file(&config_path) {
            Ok(config) => {
                clog!("Loaded pieces config from: {}", config_path.display());
                return config;
            }
            Err(e) => clog!("Failed to load pieces config: {}. Using default.", e),
        }
    } else {
        clog!("No pieces config found. Using default configuration.");
    }
    create_default_piece_config()
}

pub(crate) fn load_board_config(asset_manager: &AssetManager) -> BoardConfig {
    if let Some(config_path) = asset_manager.get_game_config_path() {
        match BoardConfig::load_from_file(&config_path) {
            Ok(config) => {
                clog!("Loaded game config from: {}", config_path.display());
                return config;
            }
            Err(e) => clog!("Failed to load game config: {}. Using standard chess.", e),
        }
    }
    BoardConfig::default()
}

pub(crate) fn load_default_game_config(
    asset_manager: &AssetManager,
) -> (BoardConfig, Option<String>) {
    if let Some(fide_path) = asset_manager.get_asset_path("FIDE.game") {
        if fide_path.exists() {
            match BoardConfig::load_from_file(&fide_path) {
                Ok(config) => {
                    if let Err(e) = config.validate() {
                        clog!("Warning: FIDE.game validation failed: {}. Using default.", e);
                        return (BoardConfig::default(), None);
                    }
                    clog!("Loaded FIDE.game from: {}", fide_path.display());
                    return (config, Some(fide_path.to_string_lossy().to_string()));
                }
                Err(e) => clog!("Failed to load FIDE.game: {}. Using default.", e),
            }
        }
    }
    let config = load_board_config(asset_manager);
    (config, None)
}

pub(crate) fn load_pieces_from_config(
    asset_manager: &AssetManager,
    board_config: &BoardConfig,
) -> PieceConfigManager {
    if !board_config.pieces_files.is_empty() {
        let mut paths = Vec::new();
        let mut missing = Vec::new();
        for pieces_file in &board_config.pieces_files {
            if let Some(path) = asset_manager.get_pieces_file_path(pieces_file) {
                paths.push(path);
            } else {
                missing.push(pieces_file.clone());
            }
        }
        if !missing.is_empty() {
            clog!(
                "Warning: Could not find pieces files: {:?}. Using fallback.",
                missing
            );
        }
        if !paths.is_empty() {
            match PieceConfigManager::load_from_files(&paths) {
                Ok(config) => return config,
                Err(e) => clog!("Failed to load pieces from files: {}. Using fallback.", e),
            }
        }
    }
    load_piece_config(asset_manager)
}

pub(crate) fn create_move_generator(piece_config: &PieceConfigManager) -> MoveGenerator {
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
