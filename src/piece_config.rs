// src/piece_config.rs
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PieceConfig {
    pub display_name: String,
    pub texture_names: Vec<String>,
    pub characters: Vec<String>,
    pub moveset: String,
    pub properties: PieceProperties,
}

#[derive(Debug, Clone, Default)]
pub struct PieceProperties {
    pub is_royal: bool,         // R - cannot move into check, checkmate ends game
    pub is_royalty: bool,       // r - capturing all instances ends game
    pub can_promote: bool,      // P - can promote
    pub promotion_target: bool, // p - can be promoted to
}

#[derive(Debug, Clone)]
pub struct PieceConfigManager {
    pub pieces: HashMap<String, PieceConfig>,
    pub piece_order: Vec<String>,
}

impl PieceConfigManager {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        Self::parse_config(&content)
    }

    /// Load from multiple files, merging the configurations
    pub fn load_from_files<P: AsRef<Path>>(
        paths: &[P],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut combined_pieces = HashMap::new();
        let mut combined_order = Vec::new();

        for path in paths {
            println!("Loading pieces from: {}", path.as_ref().display());
            let content = fs::read_to_string(path.as_ref())
                .map_err(|e| format!("Failed to read {}: {}", path.as_ref().display(), e))?;

            let config = Self::parse_config(&content)
                .map_err(|e| format!("Failed to parse {}: {}", path.as_ref().display(), e))?;

            // Merge pieces, but warn about duplicates
            for (key, piece) in config.pieces {
                if combined_pieces.contains_key(&key) {
                    println!(
                        "Warning: Piece '{}' redefined in {}, using new definition",
                        piece.display_name,
                        path.as_ref().display()
                    );
                    // Update the order position if it already exists
                    if let Some(pos) = combined_order.iter().position(|x| x == &key) {
                        combined_order.remove(pos);
                    }
                }
                combined_pieces.insert(key.clone(), piece);
                combined_order.push(key);
            }
        }

        if combined_pieces.is_empty() {
            return Err("No pieces loaded from any files".into());
        }

        println!(
            "Successfully loaded {} pieces from {} files",
            combined_pieces.len(),
            paths.len()
        );
        Ok(PieceConfigManager {
            pieces: combined_pieces,
            piece_order: combined_order,
        })
    }

    pub fn parse_config(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut pieces = HashMap::new();
        let mut piece_order = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(piece_config) = Self::parse_piece_line(line)? {
                let key = piece_config.display_name.to_lowercase();
                piece_order.push(key.clone());
                pieces.insert(key, piece_config);
            }
        }

        Ok(PieceConfigManager {
            pieces,
            piece_order,
        })
    }

    fn parse_piece_line(line: &str) -> Result<Option<PieceConfig>, Box<dyn std::error::Error>> {
        if !line.ends_with(';') {
            return Ok(None);
        }

        let line = &line[..line.len() - 1]; // Semicolon is separator, not syntax.
        let parts: Vec<&str> = line.split('/').map(|s| s.trim()).collect();

        if parts.len() != 5 {
            return Err(format!(
                "Invalid piece config line: expected 5 parts, got {}",
                parts.len()
            )
            .into());
        }

        let display_name = parts[0].to_string();
        let texture_names: Vec<String> = parts[1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let characters: Vec<String> = parts[2]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let moveset = parts[3].to_string();
        let properties = Self::parse_properties(parts[4])?;

        Ok(Some(PieceConfig {
            display_name,
            texture_names,
            characters,
            moveset,
            properties,
        }))
    }

    fn parse_properties(props_str: &str) -> Result<PieceProperties, Box<dyn std::error::Error>> {
        let mut properties = PieceProperties::default();

        for prop in props_str.chars() {
            match prop {
                'R' => properties.is_royal = true,
                'r' => properties.is_royalty = true,
                'P' => properties.can_promote = true,
                'p' => properties.promotion_target = true,
                ' ' | ',' => {}
                _ => {} // Properties will be ignored for now.
            }
        }

        Ok(properties)
    }

    pub fn get_piece_index(&self, piece_name: &str) -> Option<usize> {
        self.piece_order
            .iter()
            .position(|name| name == &piece_name.to_lowercase())
    }

    pub fn get_piece_by_index(&self, index: usize) -> Option<&PieceConfig> {
        self.piece_order
            .get(index)
            .and_then(|name| self.pieces.get(name))
    }
    /// Returns `(is_royal, is_royalty)` flags for a piece type.
    /// Single source of truth — replaces the duplicated free functions
    /// in game_state.rs and Board::piece_flags.
    #[inline]
    pub fn piece_flags(&self, piece_type: usize) -> (bool, bool) {
        self.get_piece_by_index(piece_type)
        .map_or((false, false), |c| {
            (c.properties.is_royal, c.properties.is_royalty)
        })
    }
}
