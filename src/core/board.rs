// src/core/board.rs
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use crate::zobrist::{ZOBRIST_PIECES, get_zobrist_piece_index, piece_square_key};
use smallvec::SmallVec;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnPassantTarget {
    pub position: Position,
    pub capturable_by_all: bool,
    pub piece_position: Position,
}

#[derive(Clone)]
pub struct Board {
    // FLATTENED 1D ARRAY for maximum cache-hit performance
    squares: Vec<Option<Piece>>,
    size: (usize, usize),
    en_passant_targets: Vec<EnPassantTarget>,

    /// Positions of pieces with the 'R' (is_royal) flag. Each is
    /// individually protected by check rules. Typically 0–2 per side.
    royal_positions: [SmallVec<[Position; 2]>; 2],

    /// Positions of pieces with the 'r' (is_royalty) flag. Collectively
    /// protected: when this list has exactly one entry, that piece is
    /// treated as royal (see GameState::is_in_check_fast). Variants may
    /// have several per side, hence the larger inline capacity.
    royalty_positions: [SmallVec<[Position; 4]>; 2],

    /// Total piece count per color. Used for O(1) extinction detection
    /// in variants with no royal pieces, where losing all pieces is a
    /// loss condition (not stalemate).
    piece_counts: [u32; 2],

    piece_hash: u64,
}

impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Board")
            .field("size", &self.size)
            .field("piece_hash", &self.piece_hash)
            .field("piece_counts", &self.piece_counts)
            .finish()
    }
}

impl Board {
    pub fn empty(size: (usize, usize)) -> Self {
        let squares = vec![None; size.0 * size.1];
        Board {
            squares,
            size,
            en_passant_targets: Vec::new(),
            royal_positions: [SmallVec::new(), SmallVec::new()],
            royalty_positions: [SmallVec::new(), SmallVec::new()],
            piece_counts: [0, 0],
            piece_hash: 0,
        }
    }

    pub fn new(config_manager: &PieceConfigManager) -> Self {
        let mut board = Self::empty((8, 8));
        board.setup_standard_position(config_manager);
        board
    }

    #[inline(always)]
    pub fn piece_hash(&self) -> u64 {
        self.piece_hash
    }

    #[inline]
    pub fn set_piece_hash(&mut self, hash: u64) {
        self.piece_hash = hash;
    }

    #[inline(always)]
    fn xor_piece(&mut self, piece: Piece, pos: Position) {
        self.piece_hash ^= piece_square_key(piece.color, piece.piece_type, pos);
    }

    // ─── Royal ('R') position tracking ─────────────────────────────────

    #[inline]
    pub fn get_royal_positions(&self, color: PieceColor) -> &[Position] {
        &self.royal_positions[color.index()]
    }

    #[inline]
    fn royal_remove(&mut self, color: PieceColor, pos: Position) {
        self.royal_positions[color.index()].retain(|p| *p != pos);
    }

    #[inline]
    fn royal_add(&mut self, color: PieceColor, pos: Position) {
        let list = &mut self.royal_positions[color.index()];
        if !list.contains(&pos) {
            list.push(pos);
        }
    }

    #[inline]
    fn royal_update(&mut self, color: PieceColor, old: Position, new: Position) {
        let list = &mut self.royal_positions[color.index()];
        if let Some(slot) = list.iter_mut().find(|p| **p == old) {
            *slot = new;
        }
    }

    // ─── Royalty ('r') position tracking ───────────────────────────────
    // Mirrors the royal tracking. When royalty_positions[color].len() == 1,
    // that piece is dynamically treated as royal by GameState's check logic.

    #[inline]
    pub fn get_royalty_positions(&self, color: PieceColor) -> &[Position] {
        &self.royalty_positions[color.index()]
    }

    #[inline]
    pub fn royalty_count(&self, color: PieceColor) -> usize {
        self.royalty_positions[color.index()].len()
    }

    #[inline]
    fn royalty_remove(&mut self, color: PieceColor, pos: Position) {
        self.royalty_positions[color.index()].retain(|p| *p != pos);
    }

    #[inline]
    fn royalty_add(&mut self, color: PieceColor, pos: Position) {
        let list = &mut self.royalty_positions[color.index()];
        if !list.contains(&pos) {
            list.push(pos);
        }
    }

    #[inline]
    fn royalty_update(&mut self, color: PieceColor, old: Position, new: Position) {
        let list = &mut self.royalty_positions[color.index()];
        if let Some(slot) = list.iter_mut().find(|p| **p == old) {
            *slot = new;
        }
    }

    // ─── Piece-count tracking (for extinction detection) ────────────────

    /// Total number of pieces of the given color. O(1).
    #[inline]
    pub fn piece_count(&self, color: PieceColor) -> u32 {
        self.piece_counts[color.index()]
    }

    /// Does this color have at least one piece on the board? O(1).
    /// Used to distinguish extinction (loss) from stalemate (draw) in
    /// variants with no royal pieces.
    #[inline]
    pub fn has_pieces(&self, color: PieceColor) -> bool {
        self.piece_counts[color.index()] > 0
    }

    // ─── En passant ─────────────────────────────────────────────────────

    pub fn clear_en_passant_targets(&mut self) {
        self.en_passant_targets.clear();
    }

    pub fn add_en_passant_target(&mut self, target: EnPassantTarget) {
        self.en_passant_targets.push(target);
    }

    pub fn get_en_passant_target(&self, pos: Position) -> Option<&EnPassantTarget> {
        self.en_passant_targets.iter().find(|t| t.position == pos)
    }

    pub fn get_en_passant_targets(&self) -> &[EnPassantTarget] {
        &self.en_passant_targets
    }

    pub fn set_en_passant_targets(&mut self, targets: Vec<EnPassantTarget>) {
        self.en_passant_targets = targets;
    }

    // ─── Standard position setup ────────────────────────────────────────

    fn setup_standard_position(&mut self, config_manager: &PieceConfigManager) {
        if self.size != (8, 8) {
            return;
        }
        if let Some(piece_indices) = Self::get_standard_piece_indices(config_manager) {
            let (rook, knight, bishop, queen, king, pawn) = piece_indices;
            self.setup_back_rank(
                0,
                PieceColor::Black,
                rook,
                knight,
                bishop,
                queen,
                king,
                config_manager,
            );
            self.setup_pawn_rank(1, PieceColor::Black, pawn, config_manager);
            self.setup_back_rank(
                7,
                PieceColor::White,
                rook,
                knight,
                bishop,
                queen,
                king,
                config_manager,
            );
            self.setup_pawn_rank(6, PieceColor::White, pawn, config_manager);
        }
    }

    fn get_standard_piece_indices(
        config_manager: &PieceConfigManager,
    ) -> Option<(usize, usize, usize, usize, usize, usize)> {
        Some((
            config_manager.get_piece_index("rook")?,
            config_manager.get_piece_index("knight")?,
            config_manager.get_piece_index("bishop")?,
            config_manager.get_piece_index("queen")?,
            config_manager.get_piece_index("king")?,
            config_manager.get_piece_index("pawn")?,
        ))
    }

    /// Look up both royal flags for a piece type from the config.
    /// Returns (is_royal, is_royalty).
    fn piece_flags(piece_type: usize, config_manager: &PieceConfigManager) -> (bool, bool) {
        config_manager
            .get_piece_by_index(piece_type)
            .map_or((false, false), |cfg| {
                (cfg.properties.is_royal, cfg.properties.is_royalty)
            })
    }

    fn setup_back_rank(
        &mut self,
        row: usize,
        color: PieceColor,
        rook: usize,
        knight: usize,
        bishop: usize,
        queen: usize,
        king: usize,
        config_manager: &PieceConfigManager,
    ) {
        if row >= self.size.0 || self.size.1 < 8 {
            return;
        }
        let pieces = [rook, knight, bishop, queen, king, bishop, knight, rook];
        for (col, &pt) in pieces.iter().enumerate() {
            let (is_royal, is_royalty) = Self::piece_flags(pt, config_manager);
            self.set_piece(
                (row, col),
                Some(Piece::new_with_flags(color, pt, is_royal, is_royalty)),
            );
        }
    }

    fn setup_pawn_rank(
        &mut self,
        row: usize,
        color: PieceColor,
        pawn: usize,
        config_manager: &PieceConfigManager,
    ) {
        if row >= self.size.0 {
            return;
        }
        let (is_royal, is_royalty) = Self::piece_flags(pawn, config_manager);
        for col in 0..self.size.1.min(8) {
            self.set_piece(
                (row, col),
                Some(Piece::new_with_flags(color, pawn, is_royal, is_royalty)),
            );
        }
    }

    // ─── Geometry ───────────────────────────────────────────────────────

    pub fn size(&self) -> (usize, usize) {
        self.size
    }
    pub fn rows(&self) -> usize {
        self.size.0
    }
    pub fn cols(&self) -> usize {
        self.size.1
    }

    pub fn is_valid_position(&self, pos: Position) -> bool {
        pos.0 < self.size.0 && pos.1 < self.size.1
    }

    #[inline]
    pub fn get_piece(&self, pos: Position) -> Option<Piece> {
        if self.is_valid_position(pos) {
            self.squares[pos.0 * self.size.1 + pos.1]
        } else {
            None
        }
    }

    // ─── Mutation — the three methods that maintain all tracking ────────

    pub fn set_piece(&mut self, pos: Position, piece: Option<Piece>) {
        if !self.is_valid_position(pos) {
            return;
        }

        let index = pos.0 * self.size.1 + pos.1;

        // Remove old occupant from all tracking structures.
        if let Some(old) = self.squares[index] {
            if old.is_royal {
                self.royal_remove(old.color, pos);
            }
            if old.is_royalty {
                self.royalty_remove(old.color, pos);
            }
            self.piece_counts[old.color.index()] -= 1;
            self.xor_piece(old, pos);
        }

        // Add new occupant to all tracking structures.
        if let Some(new) = piece {
            if new.is_royal {
                self.royal_add(new.color, pos);
            }
            if new.is_royalty {
                self.royalty_add(new.color, pos);
            }
            self.piece_counts[new.color.index()] += 1;
            self.xor_piece(new, pos);
        }

        self.squares[index] = piece;
    }

    pub fn move_piece(&mut self, from: Position, to: Position) {
        if !self.is_valid_position(from) || !self.is_valid_position(to) {
            return;
        }

        let from_index = from.0 * self.size.1 + from.1;
        let to_index = to.0 * self.size.1 + to.1;

        if let Some(mut piece) = self.squares[from_index].take() {
            self.xor_piece(piece, from);
            piece.move_count += 1;

            // Captured piece: remove from all tracking.
            if let Some(victim) = self.squares[to_index] {
                if victim.is_royal {
                    self.royal_remove(victim.color, to);
                }
                if victim.is_royalty {
                    self.royalty_remove(victim.color, to);
                }
                self.piece_counts[victim.color.index()] -= 1;
                self.xor_piece(victim, to);
            }

            // Moving piece: update position in royal/royalty lists.
            // Piece count is unchanged (same piece, different square).
            if piece.is_royal {
                self.royal_update(piece.color, from, to);
            }
            if piece.is_royalty {
                self.royalty_update(piece.color, from, to);
            }

            self.xor_piece(piece, to);
            self.squares[to_index] = Some(piece);
        }
    }

    pub fn clear(&mut self) {
        for square in &mut self.squares {
            *square = None;
        }
        self.royal_positions[0].clear();
        self.royal_positions[1].clear();
        self.royalty_positions[0].clear();
        self.royalty_positions[1].clear();
        self.piece_counts = [0, 0];
        self.piece_hash = 0;
    }

    // ─── Serialization & queries ────────────────────────────────────────

    pub fn to_position_string(&self, config_manager: &PieceConfigManager) -> String {
        let mut fen_parts = Vec::new();
        for row in 0..self.size.0 {
            fen_parts.push(self.rank_to_fen(row, config_manager));
        }
        fen_parts.join("/")
    }

    fn rank_to_fen(&self, row: usize, config_manager: &PieceConfigManager) -> String {
        let mut rank_fen = String::new();
        let mut empty_count = 0;
        for col in 0..self.size.1 {
            match self.squares[row * self.size.1 + col] {
                Some(piece) => {
                    if empty_count > 0 {
                        rank_fen.push_str(&empty_count.to_string());
                        empty_count = 0;
                    }
                    rank_fen.push(piece.to_char(config_manager));
                }
                None => {
                    empty_count += 1;
                }
            }
        }
        if empty_count > 0 {
            rank_fen.push_str(&empty_count.to_string());
        }
        rank_fen
    }

    pub fn calculate_board_hash(&self, _config_manager: &PieceConfigManager) -> u64 {
        let mut hash: u64 = 0;
        let max_rows = self.size.0.min(ZOBRIST_PIECES[0].len());
        let max_cols = self.size.1.min(ZOBRIST_PIECES[0][0].len());
        for row in 0..max_rows {
            for col in 0..max_cols {
                if let Some(piece) = self.squares[row * self.size.1 + col] {
                    let piece_idx = get_zobrist_piece_index(piece.color, piece.piece_type);
                    hash ^= ZOBRIST_PIECES[piece_idx][row][col];
                }
            }
        }
        hash
    }

    pub fn get_pieces_by_color(&self, color: PieceColor) -> Vec<(Position, Piece)> {
        let mut pieces = Vec::new();
        for row in 0..self.size.0 {
            for col in 0..self.size.1 {
                if let Some(piece) = self.squares[row * self.size.1 + col] {
                    if piece.color == color {
                        pieces.push(((row, col), piece));
                    }
                }
            }
        }
        pieces
    }

    pub fn count_pieces(&self) -> usize {
        (self.piece_counts[0] + self.piece_counts[1]) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.count_pieces() == 0
    }
}
