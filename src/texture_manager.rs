// src/texture_manager.rs
use crate::asset_manager::AssetManager;
use crate::core::piece::Piece;
use crate::piece_config::PieceConfigManager;
use iced::widget::image;
use std::collections::HashMap;

#[derive(Debug)]
pub struct TextureManager {
    textures: HashMap<String, image::Handle>,
    asset_manager: AssetManager,
}

impl TextureManager {
    pub fn new(config_manager: &PieceConfigManager) -> Self {
        let asset_manager = AssetManager::new();
        let textures = Self::load_textures(&asset_manager, config_manager);

        Self {
            textures,
            asset_manager,
        }
    }

    /// Load all available textures for configured pieces
    fn load_textures(
        asset_manager: &AssetManager,
        config_manager: &PieceConfigManager,
    ) -> HashMap<String, image::Handle> {
        let mut textures = HashMap::new();

        if let Some(pieces_dir) = asset_manager.get_pieces_directory() {
            for piece_config in config_manager.pieces.values() {
                Self::load_piece_textures(&mut textures, piece_config, &pieces_dir, asset_manager);
            }
        } else {
            println!("Warning: Could not find pieces directory");
        }

        println!("Loaded {} piece textures", textures.len());
        textures
    }

    /// Load textures for a specific piece type
    fn load_piece_textures(
        textures: &mut HashMap<String, image::Handle>,
        piece_config: &crate::piece_config::PieceConfig,
        pieces_dir: &std::path::Path,
        asset_manager: &AssetManager,
    ) {
        for color in &["white", "black"] {
            for texture_name in &piece_config.texture_names {
                let filename = format!("{}_{}.png", color, texture_name);

                if asset_manager.piece_texture_exists(&filename) {
                    let piece_path = pieces_dir.join(&filename);

                    if let Some(path_str) = piece_path.to_str() {
                        let handle = image::Handle::from_path(path_str);
                        let key = format!("{}_{}", color, texture_name);
                        textures.insert(key.clone(), handle);

                        // Only load the first available texture for each color
                        break;
                    }
                }
            }
        }
    }

    /// Get the texture handle for a specific piece
    pub fn get_texture(
        &self,
        piece: &Piece,
        config_manager: &PieceConfigManager,
    ) -> Option<&image::Handle> {
        if let Some(config) = config_manager.get_piece_by_index(piece.piece_type) {
            let color_str = Self::color_to_string(piece.color);

            // Try each texture name in priority order
            for texture_name in &config.texture_names {
                let key = format!("{}_{}", color_str, texture_name);
                if let Some(handle) = self.textures.get(&key) {
                    return Some(handle);
                }
            }
        }
        None
    }

    /// Convert piece color to string
    fn color_to_string(color: crate::core::piece::PieceColor) -> &'static str {
        match color {
            crate::core::piece::PieceColor::White => "white",
            crate::core::piece::PieceColor::Black => "black",
        }
    }

    /// Check if textures were successfully loaded
    pub fn has_textures(&self) -> bool {
        !self.textures.is_empty()
    }

    /// Get the number of loaded textures
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// List all loaded texture keys (for debugging)
    pub fn list_loaded_textures(&self) -> Vec<String> {
        self.textures.keys().cloned().collect()
    }

    /// Check if the asset manager found assets
    pub fn has_assets(&self) -> bool {
        self.asset_manager.has_assets()
    }
}
