//! Move generator: precomputation, runtime generation, attack detection,
//! and castling support for arbitrary piece-movement DSLs.

mod compiler;
mod tracer;
mod types;
mod zones;

pub use types::*;

use crate::board_config::ZoneConfig;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use smallvec::smallvec;
use std::sync::Arc;
use tracer::{trace_path, GeometricHandler, LiveHandler, PrecomputeHandler};
use zones::ZoneBitmaps;

#[derive(Debug, Clone)]
pub struct MoveGenerator {
    compiled_pieces: Vec<CompiledPiece>,
    move_database: Vec<[Vec<Vec<PrecomputedMove>>; 2]>,
    reverse_move_database: Vec<[Vec<Vec<MoveToSquare>>; 2]>,
    zones: ZoneConfig,
    zone_bitmaps: ZoneBitmaps,
}

impl MoveGenerator {
    pub fn new(config_manager: &PieceConfigManager) -> Result<Self, String> {
        let num_pieces = config_manager.piece_order.len();
        let mut compiled_pieces = Vec::with_capacity(num_pieces);
        for piece_name in &config_manager.piece_order {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                let compiled = compiler::compile_moveset(&piece_config.moveset, config_manager)?;
                let arc_moves = compiled.into_iter().map(Arc::new).collect();
                compiled_pieces.push(CompiledPiece { moves: arc_moves, properties: piece_config.properties.clone() });
            }
        }
        Ok(MoveGenerator {
            compiled_pieces,
            move_database: Vec::new(),
           reverse_move_database: Vec::new(),
           zones: ZoneConfig::default(),
           zone_bitmaps: ZoneBitmaps::empty(),
        })
    }

    pub fn set_zones(&mut self, zones: ZoneConfig) {
        for cp in &self.compiled_pieces {
            for m in &cp.moves {
                for z in [m.from_zone.as_deref(), m.to_zone.as_deref()].into_iter().flatten() {
                    if !zones.has(z) {
                        println!("⚠️  move pattern references undefined zone '{}'; pattern will be inactive", z);
                    }
                }
            }
        }
        self.zones = zones;
        self.zone_bitmaps = ZoneBitmaps::empty();
    }

    pub fn num_piece_types(&self) -> usize {
        self.compiled_pieces.len()
    }

    // ─── Precomputation ─────────────────────────────────────────────

    pub fn precompute_moves_for_board(&mut self, board_size: (usize, usize)) {
        self.zone_bitmaps = ZoneBitmaps::resolve(&self.zones, board_size);
        let total_squares = board_size.0 * board_size.1;
        let num_pieces = self.compiled_pieces.len();

        self.move_database.clear();
        self.move_database.reserve(num_pieces);
        self.reverse_move_database.clear();
        self.reverse_move_database.reserve(num_pieces);

        for piece_type in 0..num_pieces {
            let compiled_piece = &self.compiled_pieces[piece_type];
            let mut white_moves = Vec::with_capacity(total_squares);
            let mut black_moves = Vec::with_capacity(total_squares);
            let mut white_reverse = vec![Vec::new(); total_squares];
            let mut black_reverse = vec![Vec::new(); total_squares];

            for row in 0..board_size.0 {
                for col in 0..board_size.1 {
                    let from = (row, col);
                    for (color, fwd, rev) in [
                        (PieceColor::White, &mut white_moves, &mut white_reverse),
                        (PieceColor::Black, &mut black_moves, &mut black_reverse),
                    ] {
                        let precomputed = self.precompute_from_square(board_size, from, color, &compiled_piece.moves);
                        for pm in &precomputed {
                            let to_idx = pm.destination.0 * board_size.1 + pm.destination.1;
                            rev[to_idx].push(MoveToSquare {
                                from,
                                pattern_index: pm.pattern_index,
                                is_blockable: pm.is_blockable,
                                path: pm.path.clone(),
                                             is_flight_threat: false,
                            });
                            if let Some(pattern) = compiled_piece.moves.get(pm.pattern_index) {
                                if pattern.has_flight_capture {
                                    for idx in flight_capture_indices(pattern, &pm.path) {
                                        let sq = pm.path.steps[idx + 1];
                                        let flight_idx = sq.0 as usize * board_size.1 + sq.1 as usize;
                                        if flight_idx < total_squares {
                                            rev[flight_idx].push(MoveToSquare {
                                                from,
                                                pattern_index: pm.pattern_index,
                                                is_blockable: true,
                                                path: pm.path.clone(),
                                                                 is_flight_threat: true,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        fwd.push(precomputed);
                    }
                }
            }
            self.move_database.push([white_moves, black_moves]);
            self.reverse_move_database.push([white_reverse, black_reverse]);
        }
    }

    fn precompute_from_square(
        &self,
        board_size: (usize, usize),
                              from: Position,
                              color: PieceColor,
                              patterns: &[Arc<CompiledMove>],
    ) -> Vec<PrecomputedMove> {
        let mut precomputed = Vec::new();
        let handler = PrecomputeHandler { size: board_size };
        for (pattern_idx, pattern) in patterns.iter().enumerate() {
            if !self.zone_bitmaps.pattern_from_ok(pattern, from, color) {
                continue;
            }
            let mut buf = Vec::new();
            trace_path(&handler, from, from, color, pattern, &pattern.steps, None,
                       smallvec![(from.0 as u8, from.1 as u8)], smallvec![], &mut buf);
            for mwp in buf {
                if !self.zone_bitmaps.pattern_to_ok(pattern, mwp.destination, color) {
                    continue;
                }
                let blockable = is_move_blockable(&mwp);
                precomputed.push(PrecomputedMove {
                    destination: mwp.destination,
                    pattern_index: pattern_idx,
                    is_blockable: blockable,
                    path: mwp.path,
                });
            }
        }
        precomputed
    }

    // ─── Theoretical / PST generation ───────────────────────────────

    pub fn generate_theoretical_moves_for_pst(
        &self,
        from: Position,
        piece_type: usize,
        color: PieceColor,
        board_size: (usize, usize),
                                              move_count: u32,
    ) -> Vec<MoveWithPath> {
        let mut final_moves = Vec::new();
        let Some(cp) = self.compiled_pieces.get(piece_type) else { return final_moves };
        let handler = GeometricHandler { size: board_size };
        for pattern in &cp.moves {
            if (pattern.requires_unmoved && move_count > 0) || pattern.is_king_castle || pattern.is_rook_castle {
                continue;
            }
            if !self.zone_bitmaps.pattern_from_ok(pattern, from, color) {
                continue;
            }
            let mut buf = Vec::new();
            trace_path(&handler, from, from, color, pattern, &pattern.steps, None,
                       smallvec![(from.0 as u8, from.1 as u8)], smallvec![], &mut buf);
            for m in buf {
                if self.zone_bitmaps.pattern_to_ok(pattern, m.destination, color) {
                    final_moves.push(m);
                }
            }
        }
        final_moves
    }

    pub fn get_theoretical_moves_for_piece(
        &self,
        from: Position,
        piece_type: usize,
        color: PieceColor,
        board_size: (usize, usize),
                                           only_unmoved: bool,
    ) -> Vec<(Position, bool, bool)> {
        let mut out = Vec::new();
        let Some(cp) = self.compiled_pieces.get(piece_type) else { return out };
        let handler = GeometricHandler { size: board_size };
        for pattern in &cp.moves {
            if only_unmoved && pattern.requires_unmoved { continue; }
            if pattern.is_king_castle || pattern.is_rook_castle { continue; }
            if !self.zone_bitmaps.pattern_from_ok(pattern, from, color) { continue; }
            let mut buf = Vec::new();
            trace_path(&handler, from, from, color, pattern, &pattern.steps, None,
                       smallvec![(from.0 as u8, from.1 as u8)], smallvec![], &mut buf);
            for m in buf {
                if self.zone_bitmaps.pattern_to_ok(pattern, m.destination, color) {
                    out.push((m.destination, pattern.can_land_enemy, pattern.can_land_empty));
                }
            }
        }
        out
    }

    pub fn count_blocking_squares(&self, mwp: &MoveWithPath) -> u32 {
        count_blocking_squares(mwp)
    }

    // ─── Database-backed generation ─────────────────────────────────
    //
    // `*_into` are the hot entry points: they append into a caller-owned
    // buffer that `generate_pseudo_legal_moves` reuses across all ~16 pieces.
    // The allocating wrappers remain for engines and diagnostics.

    /// Append `from`'s pseudo-legal moves to `out`. Does **not** clear `out`.
    pub fn generate_moves_into(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
        out: &mut Vec<MoveWithPath>,
    ) {
        let Some(piece) = board.get_piece(from) else { return };
        let color_idx = piece.color.index();

        let Some(piece_moves) = self.move_database.get(piece_type) else {
            self.generate_moves_details_into(board, from, piece_type, out);
            return;
        };
        let pos_index = from.0 * board.cols() + from.1;
        let Some(precomputed) = piece_moves[color_idx].get(pos_index) else { return };
        let Some(cp) = self.compiled_pieces.get(piece_type) else { return };

        for pm in precomputed {
            let Some(pattern) = cp.moves.get(pm.pattern_index) else { continue };
            if pattern.requires_unmoved && piece.move_count > 0 {
                continue;
            }
            let ok = if pm.is_blockable {
                self.is_path_valid(board, &pm.path, piece.color, pattern)
            } else {
                can_land_at(board, pm.destination, piece.color, pattern)
            };
            if ok {
                out.push(MoveWithPath {
                    destination: pm.destination,
                    rule: pattern.clone(),
                         path: pm.path.clone(),
                });
            }
        }
    }

    pub fn generate_moves_with_database(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
    ) -> Vec<MoveWithPath> {
        let mut v = Vec::new();
        self.generate_moves_into(board, from, piece_type, &mut v);
        v
    }

    /// Re-tracing (non-database) generation. Appends to `out`.
    pub fn generate_moves_details_into(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
        out: &mut Vec<MoveWithPath>,
    ) {
        let Some(moving_piece) = board.get_piece(from) else { return };
        let Some(cp) = self.compiled_pieces.get(piece_type) else { return };
        let handler = LiveHandler { board };

        for pattern in &cp.moves {
            if pattern.requires_unmoved && moving_piece.move_count > 0 { continue; }
            if !self.zone_bitmaps.pattern_from_ok(pattern, from, moving_piece.color) { continue; }
            let mut buf = Vec::new();
            trace_path(&handler, from, from, moving_piece.color, pattern, &pattern.steps, None,
                       smallvec![(from.0 as u8, from.1 as u8)], smallvec![], &mut buf);
            if pattern.to_zone.is_some() {
                buf.retain(|m| self.zone_bitmaps.pattern_to_ok(pattern, m.destination, moving_piece.color));
            }
            out.extend(buf);
        }
    }

    pub fn generate_moves_with_details(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
    ) -> Vec<MoveWithPath> {
        let mut v = Vec::new();
        self.generate_moves_details_into(board, from, piece_type, &mut v);
        v
    }

    pub fn get_move_rule(
        &self,
        board: &Board,
        from: Position,
        to: Position,
        piece_type: usize,
    ) -> Option<MoveWithPath> {
        let mut buf = Vec::new();
        self.generate_moves_into(board, from, piece_type, &mut buf);
        buf.into_iter().find(|m| m.destination == to)
    }

    // ─── Path validation ────────────────────────────────────────────

    fn intermediates_clear(&self, board: &Board, path: &MovePath, color: PieceColor, pattern: &CompiledMove) -> bool {
        let mut rep_count = [0u32; 32];
        let n = path.step_indices.len();
        for i in 0..n {
            let step_idx = path.step_indices[i] as usize;
            if step_idx >= pattern.steps.len() || step_idx >= 32 { continue; }
            let step = &pattern.steps[step_idx];
            rep_count[step_idx] += 1;
            if i + 1 == n { break; }

            let pos_u8 = path.steps[i + 1];
            let pos = (pos_u8.0 as usize, pos_u8.1 as usize);
            let same = path.step_indices[i + 1] as usize == step_idx;

            let ok = if same && step.length > 0 && rep_count[step_idx] as usize % step.length == 0 {
                match board.get_piece(pos) {
                    None => step.repetition_permissions.can_pass_empty,
                    Some(p) if p.color != color => step.repetition_permissions.can_pass_enemy,
                    Some(_) => step.repetition_permissions.can_pass_friendly,
                }
            } else {
                match board.get_piece(pos) {
                    None => step.permissions.can_pass_empty,
                    Some(p) if p.color != color => step.permissions.can_pass_enemy,
                    Some(_) => step.permissions.can_pass_friendly,
                }
            };
            if !ok { return false; }
        }
        true
    }

    fn is_path_valid(&self, board: &Board, path: &MovePath, color: PieceColor, pattern: &CompiledMove) -> bool {
        if !self.intermediates_clear(board, path, color, pattern) {
            return false;
        }
        let last = path.steps[path.steps.len() - 1];
        can_land_at(board, (last.0 as usize, last.1 as usize), color, pattern)
    }

    // ─── Attack detection ───────────────────────────────────────────

    pub fn is_square_attacked(&self, board: &Board, target: Position, attacking_color: PieceColor) -> bool {
        let color_idx = attacking_color.index();
        let target_index = target.0 * board.cols() + target.1;
        for (piece_type, reverse_moves) in self.reverse_move_database.iter().enumerate() {
            if let Some(moves_to_target) = reverse_moves[color_idx].get(target_index) {
                for mts in moves_to_target {
                    if let Some(piece) = board.get_piece(mts.from) {
                        if piece.color == attacking_color && piece.piece_type == piece_type {
                            if let Some(cp) = self.compiled_pieces.get(piece_type) {
                                if let Some(pattern) = cp.moves.get(mts.pattern_index) {
                                    if pattern.requires_unmoved && piece.move_count > 0 { continue; }
                                    if mts.is_flight_threat {
                                        match board.get_piece(target) {
                                            Some(p) if p.color != attacking_color => {}
                                            _ => continue,
                                        }
                                    }
                                    let ok = if mts.is_blockable {
                                        self.is_path_valid(board, &mts.path, attacking_color, pattern)
                                    } else {
                                        can_land_at(board, target, attacking_color, pattern)
                                    };
                                    if ok { return true; }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Could a piece of `attacking_color` **capture `victim`** if `victim`
    /// stood on `target`? The projection query for ghosts. Deliberately not
    /// `can_land_at`, which answers "can an enemy *move* here?" — and for an
    /// empty square a pawn's forward push qualifies.
    pub fn is_square_attacked_as(
        &self,
        board: &Board,
        target: Position,
        attacking_color: PieceColor,
        victim: Piece,
    ) -> bool {
        let color_idx = attacking_color.index();
        let target_index = target.0 * board.cols() + target.1;
        for (piece_type, reverse_moves) in self.reverse_move_database.iter().enumerate() {
            if let Some(moves_to_target) = reverse_moves[color_idx].get(target_index) {
                for mts in moves_to_target {
                    let Some(piece) = board.get_piece(mts.from) else { continue };
                    if piece.color != attacking_color || piece.piece_type != piece_type { continue; }
                    let Some(cp) = self.compiled_pieces.get(piece_type) else { continue };
                    let Some(pattern) = cp.moves.get(mts.pattern_index) else { continue };
                    if pattern.requires_unmoved && piece.move_count > 0 { continue; }
                    if !can_capture_piece(pattern, &victim) { continue; }
                    let ok = if mts.is_blockable {
                        self.intermediates_clear(board, &mts.path, attacking_color, pattern)
                    } else {
                        true
                    };
                    if ok { return true; }
                }
            }
        }
        false
    }

    pub fn get_attackers_to_square(
        &self,
        board: &Board,
        target: Position,
        attacking_color: PieceColor,
    ) -> Vec<(Position, Piece)> {
        let mut attackers = Vec::new();
        let color_idx = attacking_color.index();
        let target_index = target.0 * board.cols() + target.1;
        for (piece_type, reverse_moves) in self.reverse_move_database.iter().enumerate() {
            if let Some(moves_to_target) = reverse_moves[color_idx].get(target_index) {
                for mts in moves_to_target {
                    if mts.is_flight_threat { continue; }
                    if let Some(piece) = board.get_piece(mts.from) {
                        if piece.color == attacking_color && piece.piece_type == piece_type {
                            if let Some(cp) = self.compiled_pieces.get(piece_type) {
                                if let Some(pattern) = cp.moves.get(mts.pattern_index) {
                                    if pattern.requires_unmoved && piece.move_count > 0 { continue; }
                                    let ok = if mts.is_blockable {
                                        self.is_path_valid(board, &mts.path, attacking_color, pattern)
                                    } else {
                                        can_land_at(board, target, attacking_color, pattern)
                                    };
                                    if ok { attackers.push((mts.from, piece)); }
                                }
                            }
                        }
                    }
                }
            }
        }
        attackers
    }

    // ─── Castling ───────────────────────────────────────────────────

    pub fn get_castling_pieces(&self, board: &Board, color: PieceColor) -> Vec<(Position, usize)> {
        let mut out = Vec::new();
        let (rows, cols) = board.size();
        for r in 0..rows {
            for c in 0..cols {
                let Some(piece) = board.get_piece((r, c)) else { continue };
                if piece.color != color { continue; }
                if let Some(cp) = self.compiled_pieces.get(piece.piece_type) {
                    if cp.moves.iter().any(|m| m.is_rook_castle) {
                        out.push(((r, c), piece.piece_type));
                    }
                }
            }
        }
        out
    }

    /// Castling rights are derived, not stored.
    pub fn can_piece_castle(&self, board: &Board, pos: Position, piece_type: usize) -> bool {
        let Some(piece) = board.get_piece(pos) else { return false };
        if let Some(cp) = self.compiled_pieces.get(piece_type) {
            for pattern in &cp.moves {
                if pattern.is_rook_castle {
                    if pattern.requires_unmoved && piece.move_count > 0 { continue; }
                    return true;
                }
            }
        }
        false
    }

    /// Candidate rook landings come from the ghosts the king's move *will*
    /// leave behind: `CASTLE_TARGET` lives on the ghost, so a declared rook
    /// destination is necessarily also a transit assertion.
    pub fn get_castling_options(
        &self,
        board: &Board,
        king_from: Position,
        king_to: Position,
        king_move: &MoveWithPath,
    ) -> Vec<CastlingOption> {
        let mut options = Vec::new();
        if !king_move.rule.is_king_castle {
            return options;
        }
        let Some(king_piece) = board.get_piece(king_from) else { return options };

        let partners = self.get_castling_pieces(board, king_piece.color);
        let candidates = castle_target_squares(king_move, king_from, king_to);
        let mut buf: Vec<MoveWithPath> = Vec::with_capacity(16);

        for &sq in candidates.iter() {
            let occupant = if sq == king_from { None } else { board.get_piece(sq) };

            for &(rook_pos, pt) in &partners {
                if rook_pos == king_from { continue; }
                if !self.can_piece_castle(board, rook_pos, pt) { continue; }

                buf.clear();
                self.generate_moves_details_into(board, rook_pos, pt, &mut buf);
                for rm in buf.iter() {
                    if rm.destination == sq && rm.rule.is_rook_castle {
                        let ok = match occupant {
                            None => true,
                            Some(p) if p.color != king_piece.color => rm.rule.can_land_enemy,
                            Some(_) => false,
                        };
                        if ok {
                            options.push(CastlingOption {
                                king_to,
                                rook_from: rook_pos,
                                rook_to: sq,
                                rook_piece: board.get_piece(rook_pos).unwrap(),
                            });
                            break;
                        }
                    }
                }
            }
        }
        options
    }

    pub fn is_move_blockable(&self, mwp: &MoveWithPath) -> bool {
        is_move_blockable(mwp)
    }
}
