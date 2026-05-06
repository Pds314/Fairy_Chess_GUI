// src/promotion.rs
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use rand::Rng; // FIX: Use Rng trait correctly
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum PromotionZone {
    Rank(usize),
    File(usize),
    Rectangle {
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
    },
    Squares(Vec<Position>),
}

#[derive(Debug, Clone)]
pub struct PromotionConfig {
    white_zones: Vec<PromotionZone>,
    black_zones: Vec<PromotionZone>,
}

impl PromotionConfig {
    pub fn new() -> Self {
        Self {
            white_zones: Vec::new(),
            black_zones: Vec::new(),
        }
    }

    /// Parse promotion zones from config string
    /// Format examples:
    /// - "white:rank:0,rank:1" - white promotes on ranks 0 and 1
    /// - "black:rank:7,file:0" - black promotes on rank 7 or file a
    /// - "white:rect:0:1:0:7" - white promotes in rectangle from (0,0) to (1,7)
    /// - "black:square:0:4,square:0:3" - black promotes on specific squares
    pub fn parse(config_str: &str) -> Result<Self, String> {
        let mut white_zones = Vec::new();
        let mut black_zones = Vec::new();

        for zone_def in config_str.split(';') {
            let zone_def = zone_def.trim();
            if zone_def.is_empty() {
                continue;
            }

            let parts: Vec<&str> = zone_def.split(':').collect();
            if parts.len() < 3 {
                return Err(format!("Invalid promotion zone format: {}", zone_def));
            }

            let zone_list = match parts[0] {
                "white" | "w" => &mut white_zones,
                "black" | "b" => &mut black_zones,
                _ => return Err(format!("Invalid color in promotion zone: {}", parts[0])),
            };

            // Parse individual zones separated by commas
            let zone_specs = parts[1..].join(":");
            for zone_spec in zone_specs.split(',') {
                let zone_parts: Vec<&str> = zone_spec.trim().split(':').collect();

                match zone_parts[0] {
                    "rank" => {
                        if zone_parts.len() != 2 {
                            return Err("Invalid rank specification".to_string());
                        }
                        let rank: usize =
                            zone_parts[1].parse().map_err(|_| "Invalid rank number")?;
                        zone_list.push(PromotionZone::Rank(rank));
                    }
                    "file" => {
                        if zone_parts.len() != 2 {
                            return Err("Invalid file specification".to_string());
                        }
                        let file: usize =
                            zone_parts[1].parse().map_err(|_| "Invalid file number")?;
                        zone_list.push(PromotionZone::File(file));
                    }
                    "rect" => {
                        if zone_parts.len() != 5 {
                            return Err("Invalid rectangle specification".to_string());
                        }
                        let top: usize =
                            zone_parts[1].parse().map_err(|_| "Invalid rectangle top")?;
                        let bottom: usize = zone_parts[2]
                            .parse()
                            .map_err(|_| "Invalid rectangle bottom")?;
                        let left: usize = zone_parts[3]
                            .parse()
                            .map_err(|_| "Invalid rectangle left")?;
                        let right: usize = zone_parts[4]
                            .parse()
                            .map_err(|_| "Invalid rectangle right")?;
                        zone_list.push(PromotionZone::Rectangle {
                            top,
                            bottom,
                            left,
                            right,
                        });
                    }
                    "square" => {
                        if zone_parts.len() != 3 {
                            return Err("Invalid square specification".to_string());
                        }
                        let row: usize = zone_parts[1].parse().map_err(|_| "Invalid square row")?;
                        let col: usize =
                            zone_parts[2].parse().map_err(|_| "Invalid square column")?;

                        // Add to existing Squares zone or create new one
                        if let Some(PromotionZone::Squares(squares)) = zone_list
                            .last_mut()
                            .filter(|z| matches!(z, PromotionZone::Squares(_)))
                        {
                            squares.push((row, col));
                        } else {
                            zone_list.push(PromotionZone::Squares(vec![(row, col)]));
                        }
                    }
                    _ => return Err(format!("Unknown zone type: {}", zone_parts[0])),
                }
            }
        }

        Ok(PromotionConfig {
            white_zones,
            black_zones,
        })
    }

    /// Check if a position is in a promotion zone for the given color
    pub fn is_promotion_zone(&self, pos: Position, color: PieceColor) -> bool {
        let zones = match color {
            PieceColor::White => &self.white_zones,
            PieceColor::Black => &self.black_zones,
        };

        zones.iter().any(|zone| self.position_in_zone(pos, zone))
    }

    fn position_in_zone(&self, pos: Position, zone: &PromotionZone) -> bool {
        match zone {
            PromotionZone::Rank(rank) => pos.0 == *rank,
            PromotionZone::File(file) => pos.1 == *file,
            PromotionZone::Rectangle {
                top,
                bottom,
                left,
                right,
            } => pos.0 >= *top && pos.0 <= *bottom && pos.1 >= *left && pos.1 <= *right,
            PromotionZone::Squares(squares) => squares.contains(&pos),
        }
    }
}

impl Default for PromotionConfig {
    fn default() -> Self {
        // Standard chess promotion zones
        PromotionConfig {
            white_zones: vec![PromotionZone::Rank(0)],
            black_zones: vec![PromotionZone::Rank(7)],
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromotionManager;

impl PromotionManager {
    /// Get all valid promotion targets for a piece
    pub fn get_promotion_targets(
        piece_type: usize,
        config_manager: &PieceConfigManager,
    ) -> Vec<usize> {
        let mut targets = Vec::new();

        for (idx, piece_name) in config_manager.piece_order.iter().enumerate() {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                if piece_config.properties.promotion_target {
                    targets.push(idx);
                }
            }
        }

        targets
    }

    /// Check if a piece can promote
    pub fn can_promote(piece_type: usize, config_manager: &PieceConfigManager) -> bool {
        if let Some(piece_config) = config_manager.get_piece_by_index(piece_type) {
            piece_config.properties.can_promote
        } else {
            false
        }
    }

    /// Select a promotion piece (random for now)
    pub fn select_promotion_piece(
        targets: &[usize],
        config_manager: &PieceConfigManager,
    ) -> Option<usize> {
        if targets.is_empty() {
            return None;
        }

        // FIX: Use new rand 0.9 syntax
        let mut rng = rand::rng();
        let index = rng.random_range(0..targets.len());
        let selected = targets.get(index).copied();

        // Print the promotion to console
        if let Some(piece_type) = selected {
            if let Some(piece_config) = config_manager.get_piece_by_index(piece_type) {
                println!("Promoted to: {}", piece_config.display_name);
            }
        }

        selected
    }
}

/// Interface for different promotion selection methods
pub trait PromotionSelector {
    fn select_promotion(
        &self,
        targets: &[usize],
        config_manager: &PieceConfigManager,
    ) -> Option<usize>;
}

/// Random promotion selector
pub struct RandomPromotionSelector;

impl PromotionSelector for RandomPromotionSelector {
    fn select_promotion(
        &self,
        targets: &[usize],
        config_manager: &PieceConfigManager,
    ) -> Option<usize> {
        PromotionManager::select_promotion_piece(targets, config_manager)
    }
}

/// Future: GUI promotion selector placeholder
pub struct GuiPromotionSelector;

impl PromotionSelector for GuiPromotionSelector {
    fn select_promotion(
        &self,
        targets: &[usize],
        _config_manager: &PieceConfigManager,
    ) -> Option<usize> {
        // Placeholder - would show dialog and wait for user input
        targets.first().copied()
    }
}
