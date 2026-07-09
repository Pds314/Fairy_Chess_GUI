// src/core/board.rs
use crate::core::ghost::Ghost;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use crate::zobrist::{get_zobrist_piece_index, piece_square_key, ZOBRIST_PIECES};
use smallvec::SmallVec;
use std::fmt;

#[derive(Clone)]
pub struct Board {
    // FLATTENED 1D ARRAY for maximum cache-hit performance
    squares: Vec<Option<Piece>>,
    size: (usize, usize),

    /// Append-only ghost stack. The **live** set is `ghosts[ghost_live_start..]`
    /// — the ghosts created by the most recent transaction. Undo is
    /// `truncate(ghost_live_start)` followed by restoring the previous
    /// `ghost_live_start` from the frame. No `born_ply`, no `lifetime`, no
    /// per-ply `Vec` clone.
    ///
    /// Everything *below* `ghost_live_start` is expired but retained, so that
    /// `GameState::ghosts_of(i)` can reconstruct any still-on-the-path move's
    /// ghosts purely from the frame chain.
    ghosts: Vec<Ghost>,
    ghost_live_start: u32,

    royal_positions: [SmallVec<[Position; 2]>; 2],
    royalty_positions: [SmallVec<[Position; 4]>; 2],
    piece_counts: [u32; 2],

    piece_hash: u64,
}

impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Board")
        .field("size", &self.size)
        .field("piece_hash", &self.piece_hash)
        .field("piece_counts", &self.piece_counts)
        .field("live_ghosts", &self.live_ghosts().len())
        .finish()
    }
}

impl Board {
    pub fn empty(size: (usize, usize)) -> Self {
        let squares = vec![None; size.0 * size.1];
        Board {
            squares,
            size,
            ghosts: Vec::new(),
            ghost_live_start: 0,
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

    // ─── Ghosts ─────────────────────────────────────────────────────────

    /// The ghosts created by the most recent transaction. Simultaneously the
    /// 0-ply set (castling transit assertions) and the 1-ply set (en-passant
    /// capture aliases). A multi-move-turn variant would look one extra frame
    /// back rather than introduce a counter.
    #[inline(always)]
    pub fn live_ghosts(&self) -> &[Ghost] {
        &self.ghosts[self.ghost_live_start as usize..]
    }

    /// The live ghost on `pos`, of *any* flavour — including a bare transit
    /// ghost with no capture alias.
    #[inline]
    pub fn ghost_at(&self, pos: Position) -> Option<&Ghost> {
        self.live_ghosts().iter().find(|g| g.square() == pos)
    }

    /// Every ghost still on the current search path, expired ones included.
    /// Diagnostic; `GameState::ghosts_of` slices it.
    #[inline]
    pub fn all_ghosts(&self) -> &[Ghost] {
        &self.ghosts
    }
    #[inline]
    pub fn ghost_len(&self) -> usize {
        self.ghosts.len()
    }
    /// Index of the first live ghost.
    #[inline]
    pub fn ghost_epoch(&self) -> u32 {
        self.ghost_live_start
    }

    /// Open a new ghost epoch. Returns the previous `ghost_live_start`, which
    /// the caller stores in the frame. Everything already on the stack
    /// silently expires.
    #[inline]
    pub(crate) fn begin_ghost_epoch(&mut self) -> u32 {
        let prev = self.ghost_live_start;
        self.ghost_live_start = self.ghosts.len() as u32;
        prev
    }

    #[inline]
    pub(crate) fn push_ghost(&mut self, g: Ghost) {
        self.ghosts.push(g);
    }

    /// Undo. Drops exactly the ghosts pushed since `begin_ghost_epoch`, then
    /// re-exposes the previous epoch.
    #[inline]
    pub(crate) fn rewind_ghosts(&mut self, prev_live_start: u32) {
        self.ghosts.truncate(self.ghost_live_start as usize);
        self.ghost_live_start = prev_live_start;
    }

    pub fn clear_ghosts(&mut self) {
        self.ghosts.clear();
        self.ghost_live_start = 0;
    }

    /// Seed a ghost into the initial position (e.g. a FEN en-passant square).
    pub fn seed_ghost(&mut self, g: Ghost) {
        debug_assert_eq!(self.ghost_live_start, 0, "seed ghosts before the first move");
        self.ghosts.push(g);
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

    // ─── Royalty ('r') position tracking ───────────────────────────────

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

    // ─── Piece-count tracking ───────────────────────────────────────────

    #[inline]
    pub fn piece_count(&self, color: PieceColor) -> u32 {
        self.piece_counts[color.index()]
    }
    #[inline]
    pub fn has_pieces(&self, color: PieceColor) -> bool {
        self.piece_counts[color.index()] > 0
    }

    // ─── Standard position setup ────────────────────────────────────────

    fn setup_standard_position(&mut self, config_manager: &PieceConfigManager) {
        if self.size != (8, 8) {
            return;
        }
        if let Some(pi) = Self::get_standard_piece_indices(config_manager) {
            let (rook, knight, bishop, queen, king, pawn) = pi;
            self.setup_back_rank(0, PieceColor::Black, rook, knight, bishop, queen, king, config_manager);
            self.setup_pawn_rank(1, PieceColor::Black, pawn, config_manager);
            self.setup_back_rank(7, PieceColor::White, rook, knight, bishop, queen, king, config_manager);
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

    fn piece_flags(piece_type: usize, config_manager: &PieceConfigManager) -> (bool, bool) {
        config_manager
        .get_piece_by_index(piece_type)
        .map_or((false, false), |cfg| {
            (cfg.properties.is_royal, cfg.properties.is_royalty)
        })
    }

    #[allow(clippy::too_many_arguments)]
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
            self.set_piece((row, col), Some(Piece::new_with_flags(color, pt, is_royal, is_royalty)));
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
            self.set_piece((row, col), Some(Piece::new_with_flags(color, pawn, is_royal, is_royalty)));
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

    // ─── Mutation — the ONE primitive ───────────────────────────────────
    //
    // `BoardEvent::{Lift, Drop}` are the only callers during play. There is
    // deliberately no `move_piece`: it bundled a `move_count` bump with a
    // two-square mutation and could not express Chess960's overlapping
    // king/rook squares.

    pub fn set_piece(&mut self, pos: Position, piece: Option<Piece>) {
        if !self.is_valid_position(pos) {
            return;
        }
        let index = pos.0 * self.size.1 + pos.1;

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
        self.clear_ghosts();
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
                None => empty_count += 1,
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
