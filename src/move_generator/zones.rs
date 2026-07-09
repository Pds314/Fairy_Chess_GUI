//! Zone bitmap resolution and O(1) membership lookup.

use crate::board_config::ZoneConfig;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::move_generator::types::CompiledMove;
use std::collections::HashMap;

/// Resolved zone bitmaps for O(1) membership tests.
#[derive(Debug, Clone)]
pub(crate) struct ZoneBitmaps {
    bitmaps: HashMap<String, [Vec<bool>; 2]>,
    board_cols: usize,
}

impl ZoneBitmaps {
    pub fn empty() -> Self {
        Self { bitmaps: HashMap::new(), board_cols: 0 }
    }

    pub fn resolve(zones: &ZoneConfig, board_size: (usize, usize)) -> Self {
        let mut bitmaps = HashMap::new();
        for (name, zone) in &zones.zones {
            bitmaps.insert(name.clone(), zone.resolve(board_size));
        }
        Self { bitmaps, board_cols: board_size.1 }
    }

    #[inline]
    pub fn in_zone(&self, name: &str, pos: Position, color: PieceColor) -> bool {
        let ci = color.index();
        match self.bitmaps.get(name) {
            Some(maps) => {
                let idx = pos.0 * self.board_cols + pos.1;
                maps[ci].get(idx).copied().unwrap_or(false)
            }
            None => false,
        }
    }

    #[inline]
    pub fn pattern_from_ok(&self, p: &CompiledMove, from: Position, color: PieceColor) -> bool {
        if !p.has_zones { return true; }
        match &p.from_zone {
            None => true,
            Some(z) => self.in_zone(z, from, color),
        }
    }

    #[inline]
    pub fn pattern_to_ok(&self, p: &CompiledMove, dest: Position, color: PieceColor) -> bool {
        if !p.has_zones { return true; }
        match &p.to_zone {
            None => true,
            Some(z) => self.in_zone(z, dest, color),
        }
    }
}
