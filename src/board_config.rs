// src/board_config.rs
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use crate::promotion::PromotionConfig;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ────────────────────────────────────────────────────────────────────────
// Named zones
//
// A zone is a colour‑indexed set of board regions. Move patterns reference
// zones *by name*; the same `.pieces` file can therefore be paired with
// different `.game` files that place the regions differently (or omit the
// zone, in which case the pattern is simply never active).
//
// Region grammar matches `promotion_zones` so the two can eventually share
// one parser:
//     rank:R         file:C
//     rect:R0:R1:C0:C1   (inclusive)
//     square:R:C
// ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Region {
    Rank(usize),
    File(usize),
    Rect {
        r0: usize,
        r1: usize,
        c0: usize,
        c1: usize,
    },
    Square(usize, usize),
}

impl Region {
    fn contains(&self, (r, c): Position) -> bool {
        match *self {
            Region::Rank(k) => r == k,
            Region::File(k) => c == k,
            Region::Rect { r0, r1, c0, c1 } => r >= r0 && r <= r1 && c >= c0 && c <= c1,
            Region::Square(rr, cc) => r == rr && c == cc,
        }
    }

    fn parse(spec: &str) -> Result<Self, String> {
        let p: Vec<&str> = spec.split(':').collect();
        match p.as_slice() {
            ["rank", r] => Ok(Region::Rank(r.parse().map_err(|_| spec.to_string())?)),
            ["file", c] => Ok(Region::File(c.parse().map_err(|_| spec.to_string())?)),
            ["square", r, c] => Ok(Region::Square(
                r.parse().map_err(|_| spec.to_string())?,
                c.parse().map_err(|_| spec.to_string())?,
            )),
            ["rect", r0, r1, c0, c1] => Ok(Region::Rect {
                r0: r0.parse().map_err(|_| spec.to_string())?,
                r1: r1.parse().map_err(|_| spec.to_string())?,
                c0: c0.parse().map_err(|_| spec.to_string())?,
                c1: c1.parse().map_err(|_| spec.to_string())?,
            }),
            _ => Err(format!("unrecognised region '{}'", spec)),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Zone {
    pub white: Vec<Region>,
    pub black: Vec<Region>,
}

impl Zone {
    pub fn contains(&self, pos: Position, color: PieceColor) -> bool {
        let side = match color {
            PieceColor::White => &self.white,
            PieceColor::Black => &self.black,
        };
        side.iter().any(|r| r.contains(pos))
    }

    /// Resolve this zone to two flat bitmaps (white, black), indexed by
    /// `row * cols + col`. O(rows*cols*regions) once per variant load; the
    /// resulting `Vec<bool>` gives O(1) membership tests thereafter.
    pub fn resolve(&self, size: (usize, usize)) -> [Vec<bool>; 2] {
        let total = size.0 * size.1;
        let mut w = vec![false; total];
        let mut b = vec![false; total];
        for r in 0..size.0 {
            for c in 0..size.1 {
                let idx = r * size.1 + c;
                if self.white.iter().any(|reg| reg.contains((r, c))) {
                    w[idx] = true;
                }
                if self.black.iter().any(|reg| reg.contains((r, c))) {
                    b[idx] = true;
                }
            }
        }
        [w, b]
    }
}

#[derive(Debug, Clone, Default)]
pub struct ZoneConfig {
    pub zones: HashMap<String, Zone>,
}

impl ZoneConfig {
    pub fn contains(&self, name: &str, pos: Position, color: PieceColor) -> bool {
        self.zones
            .get(name)
            .map_or(false, |z| z.contains(pos, color))
    }
    pub fn has(&self, name: &str) -> bool {
        self.zones.contains_key(name)
    }

    /// `white:rect:7:9:3:5; black:rect:0:2:3:5`
    /// `rect:3:6:3:6`                      (applies to both colours)
    fn parse_spec(spec: &str) -> Result<Zone, String> {
        let mut zone = Zone::default();
        for clause in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (colours, region_src) = if let Some(rest) = clause.strip_prefix("white:") {
                (&mut [Some(&mut zone.white), None][..], rest)
            } else if let Some(rest) = clause.strip_prefix("black:") {
                (&mut [None, Some(&mut zone.black)][..], rest)
            } else {
                (
                    &mut [Some(&mut zone.white), Some(&mut zone.black)][..],
                    clause,
                )
            };
            // region_src may itself be a comma list: rank:0,rank:1
            for r in region_src
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let region = Region::parse(r)?;
                for slot in colours.iter_mut().flatten() {
                    slot.push(region.clone());
                }
            }
        }
        Ok(zone)
    }
}

/// Configuration for board setup
#[derive(Debug, Clone)]
pub struct BoardConfig {
    pub board_size: (usize, usize), // (rows, cols) - now derived from position
    pub initial_position: String,   // FEN-like position string
    pub starting_player: PieceColor,
    pub fifty_move_counter: u32,
    pub promotion_config: PromotionConfig,
    pub pieces_files: Vec<String>, // List of pieces files to load
    pub insufficient_material: Vec<String>,
    pub zones: ZoneConfig,
}

impl BoardConfig {
    /// Load board configuration from a file
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        Self::parse_config(&content)
    }

    /// Try to load from file, or return default if not found
    pub fn load_or_default<P: AsRef<Path>>(path: Option<P>) -> Self {
        if let Some(p) = path {
            Self::load_from_file(p).unwrap_or_else(|e| {
                println!("Could not load board config: {}. Using standard chess.", e);
                Self::default()
            })
        } else {
            Self::default()
        }
    }

    /// Validate the game configuration
    pub fn validate(&self) -> Result<(), String> {
        // Check if position string is valid
        if self.initial_position.is_empty() {
            return Err("Initial position cannot be empty".to_string());
        }

        // Validate that we can derive board size from position
        match Self::derive_board_size(&self.initial_position) {
            Ok(size) => {
                if size.0 == 0 || size.1 == 0 {
                    return Err("Board size cannot be zero".to_string());
                }
                if size.0 > 16 || size.1 > 16 {
                    return Err("Board size cannot exceed 16x16".to_string());
                }
            }
            Err(e) => return Err(format!("Invalid position string: {}", e)),
        }

        // Validate fifty move counter
        if self.fifty_move_counter > 200 {
            return Err("Fifty move counter cannot exceed 200".to_string());
        }

        // Check if pieces_files list is not empty
        if self.pieces_files.is_empty() {
            return Err("At least one pieces file must be specified".to_string());
        }

        // Validate pieces files have correct extension
        for file in &self.pieces_files {
            if !file.ends_with(".pieces") {
                return Err(format!(
                    "Pieces file '{}' must have .pieces extension",
                    file
                ));
            }
        }

        Ok(())
    }

    /// Parse board configuration from text content
    fn parse_config(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut initial_position = String::new();
        let mut starting_player = PieceColor::White;
        let mut fifty_move_counter = 0;
        let mut promotion_config = PromotionConfig::default();
        let mut pieces_files = Vec::new();
        let mut insufficient_material: Vec<String> = Vec::new();
        let mut zones = ZoneConfig::default();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim();

                match key.as_str() {
                    "position" | "fen" | "setup" => {
                        initial_position = value.to_string();
                    }
                    "turn" | "player" => {
                        starting_player = Self::parse_player(value)?;
                    }
                    "fifty_move" | "halfmove" => {
                        fifty_move_counter = value.parse()?;
                    }
                    "promotion_zones" => {
                        promotion_config = PromotionConfig::parse(value)
                            .map_err(|e| format!("Invalid promotion zones: {}", e))?;
                    }
                    "zone" => {
                        // value is "name: spec…"
                        if let Some((name, spec)) = value.split_once(':') {
                            match ZoneConfig::parse_spec(spec.trim()) {
                                Ok(z) => {
                                    zones.zones.insert(name.trim().to_string(), z);
                                }
                                Err(e) => println!("⚠️  zone '{}': {}", name.trim(), e),
                            }
                        }
                    }
                    "pieces_files" | "pieces_file" => {
                        // Parse comma-separated list of pieces files
                        pieces_files = value
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    "insufficient_material" | "dead_position" | "material_draw" => {
                        // Accumulate. One line may hold several `;`-separated
                        // rules; the compiler splits those later.
                        if !value.is_empty() {
                            insufficient_material.push(value.to_string());
                        }
                    }
                    // Ignore board_size if present (for backwards compatibility)
                    "size" | "board_size" => {
                        println!(
                            "Warning: board_size in config is deprecated and will be ignored. Size is determined from position string."
                        );
                    }
                    _ => {}
                }
            }
        }

        // If no position specified, use standard chess
        if initial_position.is_empty() {
            initial_position = Self::standard_chess_position();
        }

        // Derive board size from position string
        let board_size = Self::derive_board_size(&initial_position)?;

        Ok(BoardConfig {
            board_size,
            initial_position,
            starting_player,
            fifty_move_counter,
            promotion_config,
            pieces_files,
            insufficient_material,
            zones,
        })
    }

    /// Derive board size from FEN-like position string
    fn derive_board_size(position: &str) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let ranks: Vec<&str> = position.split('/').collect();

        if ranks.is_empty() {
            return Err("Empty position string".into());
        }

        let num_rows = ranks.len();
        let mut max_cols = 0;

        // Calculate the number of columns from each rank
        for rank_str in &ranks {
            let col_count = Self::count_columns_in_rank(rank_str)?;
            max_cols = max_cols.max(col_count);
        }

        if max_cols == 0 {
            return Err("No columns found in position string".into());
        }

        // Verify all ranks have the same width
        for (i, rank_str) in ranks.iter().enumerate() {
            let col_count = Self::count_columns_in_rank(rank_str)?;

            if col_count != max_cols {
                println!(
                    "Warning: Rank {} has {} columns, expected {}. Board may be irregular.",
                    i + 1,
                    col_count,
                    max_cols
                );
            }
        }

        Ok((num_rows, max_cols))
    }

    /// Count columns in a rank, handling bracketed piece names
    fn count_columns_in_rank(rank_str: &str) -> Result<usize, Box<dyn std::error::Error>> {
        let mut col_count = 0;
        let mut chars = rank_str.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '[' {
                // Skip bracketed content
                let mut bracket_count = 1;
                while bracket_count > 0 && chars.peek().is_some() {
                    match chars.next() {
                        Some(']') => bracket_count -= 1,
                        Some('[') => bracket_count += 1,
                        _ => {}
                    }
                }
                col_count += 1; // A bracketed piece occupies one square
            } else if ch.is_ascii_digit() {
                // Empty squares
                if let Some(count) = ch.to_digit(10) {
                    col_count += count as usize;
                }
            } else if ch.is_ascii_alphabetic() {
                // Regular piece
                col_count += 1;
            } else if !ch.is_whitespace() {
                return Err(format!("Invalid character '{}' in position string", ch).into());
            }
        }

        Ok(col_count)
    }

    /// Parse player color from string
    fn parse_player(value: &str) -> Result<PieceColor, Box<dyn std::error::Error>> {
        match value.to_lowercase().as_str() {
            "w" | "white" => Ok(PieceColor::White),
            "b" | "black" => Ok(PieceColor::Black),
            _ => Err(format!("Invalid player color: {}", value).into()),
        }
    }

    /// Standard chess starting position
    fn standard_chess_position() -> String {
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR".to_string()
    }

    /// Create a board from this configuration
    pub fn create_board(&self, config_manager: &PieceConfigManager) -> Board {
        let mut board = Board::empty(self.board_size);

        if !self.initial_position.is_empty() {
            self.setup_position(&mut board, &self.initial_position, config_manager);
        }

        board
    }

    /// Set up board position from FEN-like string
    fn setup_position(
        &self,
        board: &mut Board,
        position: &str,
        config_manager: &PieceConfigManager,
    ) {
        let ranks: Vec<&str> = position.split('/').collect();

        for (row, rank_str) in ranks.iter().enumerate() {
            if row >= self.board_size.0 {
                break;
            }

            let mut col = 0;
            let mut chars = rank_str.chars().peekable();

            while let Some(ch) = chars.next() {
                if col >= self.board_size.1 {
                    break;
                }

                if ch == '[' {
                    // Parse bracketed piece name
                    let mut piece_name = String::new();
                    let mut bracket_count = 1;

                    while bracket_count > 0 && chars.peek().is_some() {
                        match chars.next() {
                            Some(']') => {
                                bracket_count -= 1;
                                if bracket_count == 0 {
                                    break;
                                }
                                piece_name.push(']');
                            }
                            Some('[') => {
                                bracket_count += 1;
                                piece_name.push('[');
                            }
                            Some(c) => piece_name.push(c),
                            None => break,
                        }
                    }

                    // Determine color from case of first letter
                    let color = if piece_name
                        .chars()
                        .next()
                        .map_or(false, |c| c.is_ascii_uppercase())
                    {
                        PieceColor::White
                    } else {
                        PieceColor::Black
                    };

                    // Find piece by name and place with both flags set.
                    if let Some(piece_type) =
                        self.find_piece_type_by_name(&piece_name, config_manager)
                    {
                        let (is_royal, is_royalty) = config_manager
                            .get_piece_by_index(piece_type)
                            .map_or((false, false), |c| {
                                (c.properties.is_royal, c.properties.is_royalty)
                            });
                        board.set_piece(
                            (row, col),
                            Some(Piece::new_with_flags(
                                color, piece_type, is_royal, is_royalty,
                            )),
                        );
                    }

                    col += 1;
                } else if ch.is_ascii_digit() {
                    // Empty squares
                    if let Some(count) = ch.to_digit(10) {
                        col += count as usize;
                    }
                } else if ch.is_ascii_alphabetic() {
                    // Regular piece character
                    let color = if ch.is_ascii_uppercase() {
                        PieceColor::White
                    } else {
                        PieceColor::Black
                    };

                    if let Some(piece_type) = self.find_piece_type(ch, config_manager) {
                        let (is_royal, is_royalty) = config_manager
                            .get_piece_by_index(piece_type)
                            .map_or((false, false), |c| {
                                (c.properties.is_royal, c.properties.is_royalty)
                            });
                        board.set_piece(
                            (row, col),
                            Some(Piece::new_with_flags(
                                color, piece_type, is_royal, is_royalty,
                            )),
                        );
                    }
                    col += 1;
                }
            }
        }
    }

    /// Find piece type index by character
    fn find_piece_type(&self, ch: char, config_manager: &PieceConfigManager) -> Option<usize> {
        let search_char_upper = ch.to_ascii_uppercase().to_string();

        // Normally we will always do this. Well-designed pieces.configs will include proper symbolic characters for those pieces.
        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                if piece_config
                    .characters
                    .iter()
                    .any(|c| c == &search_char_upper)
                {
                    return Some(idx);
                }
            }
        }

        // This is a fallback for malformed piece configs that do not contain a proper letter in the third position in a piece.
        let search_char_lower = ch.to_ascii_lowercase();
        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if piece_name.starts_with(search_char_lower) {
                return Some(idx);
            }
        }

        None
    }

    /// Find piece type by full name (case insensitive)
    fn find_piece_type_by_name(
        &self,
        name: &str,
        config_manager: &PieceConfigManager,
    ) -> Option<usize> {
        let search_name = name.to_lowercase();

        // Try exact match first
        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if piece_name == &search_name {
                return Some(idx);
            }
        }

        // Try matching display name
        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                if piece_config.display_name.to_lowercase() == search_name {
                    return Some(idx);
                }
            }
        }

        // Try matching any texture name
        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                for texture_name in &piece_config.texture_names {
                    if texture_name.to_lowercase() == search_name {
                        return Some(idx);
                    }
                }
            }
        }

        None
    }
}

impl Default for BoardConfig {
    fn default() -> Self {
        let initial_position = Self::standard_chess_position();
        let board_size = Self::derive_board_size(&initial_position).unwrap_or((8, 8));

        BoardConfig {
            board_size,
            initial_position,
            starting_player: PieceColor::White,
            fifty_move_counter: 0,
            promotion_config: PromotionConfig::default(),
            pieces_files: vec!["FIDE.pieces".to_string()],
            insufficient_material: Vec::new(),
            zones: ZoneConfig::default(),
        }
    }
}
