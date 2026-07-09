// src/core/perft.rs
//! Perft, divide, invariant verification, and micro-benchmarks.
//!
//! Perft is the only honest measurement of a move system. It is also, because
//! the event chain makes `undo` a pure inverse, a *complete* proof: if
//! `perft_verify` round-trips the Zobrist hash, the tape length, the ghost
//! epoch, the royal/royalty lists and the piece counts at every one of
//! millions of nodes, then make/unmake is a bijection.

use crate::core::game_state::GameState;
use crate::core::game_types::MoveGenerationResult;
use crate::core::piece::PieceColor;
use crate::move_generator::MoveGenerator;
use crate::notation::position_to_algebraic;
use crate::piece_config::PieceConfigManager;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default)]
pub struct PerftStats {
    pub nodes: u64,
    pub captures: u64,
    pub en_passant: u64,
    pub castles: u64,
    pub promotions: u64,
    pub flight_captures: u64,
}

/// Everything that must round-trip across a make/unmake pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Invariants {
    hash: u64,
    tape_len: usize,
    ghost_len: usize,
    ghost_epoch: u32,
    history_len: usize,
    fifty: u32,
    pieces_w: u32,
    pieces_b: u32,
    royal_w: usize,
    royal_b: usize,
    royalty_w: usize,
    royalty_b: usize,
}

impl Invariants {
    fn capture(s: &GameState) -> Self {
        let b = &s.board;
        Invariants {
            hash: b.piece_hash(),
            tape_len: s.tape_len(),
            ghost_len: b.ghost_len(),
            ghost_epoch: b.ghost_epoch(),
            history_len: s.move_history.len(),
            fifty: s.fifty_move_counter,
            pieces_w: b.piece_count(PieceColor::White),
            pieces_b: b.piece_count(PieceColor::Black),
            royal_w: b.get_royal_positions(PieceColor::White).len(),
            royal_b: b.get_royal_positions(PieceColor::Black).len(),
            royalty_w: b.royalty_count(PieceColor::White),
            royalty_b: b.royalty_count(PieceColor::Black),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchResult {
    pub pseudo_legal_per_sec: f64,
    pub legal_per_sec: f64,
    pub make_unmake_per_sec: f64,
    pub check_tests_per_sec: f64,
    pub perft_depth: u32,
    pub perft_nodes: u64,
    pub perft_time: Duration,
}

impl BenchResult {
    pub fn nps(&self) -> f64 {
        self.perft_nodes as f64 / self.perft_time.as_secs_f64().max(1e-9)
    }
}

#[inline]
fn rate(count: u64, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64().max(1e-9)
}

impl GameState {
    // ─── Perft ──────────────────────────────────────────────────────

    /// Leaf-node count at `depth`. Bulk-counts the final ply.
    pub fn perft(&mut self, mg: &MoveGenerator, cm: &PieceConfigManager, depth: u32) -> u64 {
        if depth == 0 {
            return 1;
        }
        if depth == 1 {
            return self.count_legal_moves(mg, cm);
        }

        let moves = match self.generate_pseudo_legal_moves(mg, cm) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return 0,
        };

        let mut nodes = 0u64;
        for mv in &moves {
            self.execute_expanded_move(mv, mg, cm);
            if !self.mover_king_in_check(mg) {
                nodes += self.perft(mg, cm, depth - 1);
            }
            self.undo_move(cm);
        }
        nodes
    }

    /// Per-root-move breakdown. The standard way to bisect a perft mismatch.
    pub fn perft_divide(
        &mut self,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
        depth: u32,
    ) -> (Vec<(String, u64)>, u64) {
        let size = self.board.size();
        let mut out = Vec::new();
        let mut total = 0u64;

        if depth == 0 {
            return (out, 1);
        }

        let moves = match self.generate_pseudo_legal_moves(mg, cm) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return (out, 0),
        };

        for mv in &moves {
            self.execute_expanded_move(mv, mg, cm);
            if !self.mover_king_in_check(mg) {
                let n = self.perft(mg, cm, depth - 1);
                let mut label = format!(
                    "{}{}",
                    position_to_algebraic(mv.from, size),
                    position_to_algebraic(mv.to, size)
                );
                if let Some(pt) = mv.promotion_target {
                    if let Some(c) = cm.get_piece_by_index(pt) {
                        label.push('=');
                        label.push(
                            c.characters
                                .first()
                                .and_then(|s| s.chars().next())
                                .unwrap_or('?'),
                        );
                    }
                }
                if mv.castling_option.is_some() {
                    label.push_str("(O)");
                }
                out.push((label, n));
                total += n;
            }
            self.undo_move(cm);
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        (out, total)
    }

    /// Classify every leaf. Slower than `perft` (no bulk counting).
    pub fn perft_detailed(
        &mut self,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
        depth: u32,
    ) -> PerftStats {
        let mut s = PerftStats::default();
        if depth == 0 {
            s.nodes = 1;
            return s;
        }

        let moves = match self.generate_pseudo_legal_moves(mg, cm) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return s,
        };

        for mv in &moves {
            self.execute_expanded_move(mv, mg, cm);
            if !self.mover_king_in_check(mg) {
                if depth == 1 {
                    let gm = *self.move_history.last().unwrap();
                    s.nodes += 1;
                    if gm.is_capture() {
                        s.captures += 1;
                    }
                    if gm.is_en_passant_capture() {
                        s.en_passant += 1;
                    }
                    if gm.is_castling() {
                        s.castles += 1;
                    }
                    if gm.promoted_to().is_some() {
                        s.promotions += 1;
                    }
                    s.flight_captures += gm.flight_capture_count() as u64;
                } else {
                    let sub = self.perft_detailed(mg, cm, depth - 1);
                    s.nodes += sub.nodes;
                    s.captures += sub.captures;
                    s.en_passant += sub.en_passant;
                    s.castles += sub.castles;
                    s.promotions += sub.promotions;
                    s.flight_captures += sub.flight_captures;
                }
            }
            self.undo_move(cm);
        }
        s
    }

    /// Perft with a full invariant check at every node. Slow, and the only
    /// test that actually proves the event chain is a bijection.
    pub fn perft_verify(
        &mut self,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
        depth: u32,
    ) -> Result<u64, String> {
        // The incrementally-maintained hash must equal a from-scratch one.
        let recomputed = self.board.calculate_board_hash(cm);
        if recomputed != self.board.piece_hash() {
            return Err(format!(
                "incremental piece_hash 0x{:016x} != recomputed 0x{:016x}",
                self.board.piece_hash(),
                recomputed
            ));
        }
        if depth == 0 {
            return Ok(1);
        }

        let size = self.board.size();
        let moves = match self.generate_pseudo_legal_moves(mg, cm) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return Ok(0),
        };

        let mut nodes = 0u64;
        for mv in &moves {
            let before = Invariants::capture(self);

            self.execute_expanded_move(mv, mg, cm);
            let legal = !self.mover_king_in_check(mg);
            if legal {
                nodes += self.perft_verify(mg, cm, depth - 1)?;
            }
            self.undo_move(cm);

            let after = Invariants::capture(self);
            if before != after {
                return Err(format!(
                    "make/unmake is not a bijection for {}{}\n  before: {:?}\n  after:  {:?}",
                    position_to_algebraic(mv.from, size),
                    position_to_algebraic(mv.to, size),
                    before,
                    after
                ));
            }
        }
        Ok(nodes)
    }

    // ─── Micro-benchmarks ───────────────────────────────────────────

    pub fn bench(
        &mut self,
        mg: &MoveGenerator,
        cm: &PieceConfigManager,
        perft_depth: u32,
    ) -> BenchResult {
        const WARM: u64 = 200;
        const ITERS: u64 = 3_000;

        // Warm the caches / branch predictors.
        for _ in 0..WARM {
            let _ = self.generate_pseudo_legal_moves(mg, cm);
        }

        let t = Instant::now();
        for _ in 0..ITERS {
            std::hint::black_box(self.generate_pseudo_legal_moves(mg, cm));
        }
        let pseudo_legal_per_sec = rate(ITERS, t.elapsed());

        let t = Instant::now();
        for _ in 0..ITERS {
            std::hint::black_box(self.get_legal_moves(mg, cm));
        }
        let legal_per_sec = rate(ITERS, t.elapsed());

        let moves = self.get_legal_moves(mg, cm);
        let pairs = ITERS * moves.len().max(1) as u64;
        let t = Instant::now();
        for _ in 0..ITERS {
            for mv in &moves {
                self.execute_expanded_move(mv, mg, cm);
                self.undo_move(cm);
            }
        }
        let make_unmake_per_sec = rate(pairs, t.elapsed());

        let t = Instant::now();
        for _ in 0..(ITERS * 10) {
            std::hint::black_box(self.mover_king_in_check(mg));
        }
        let check_tests_per_sec = rate(ITERS * 10, t.elapsed());

        let t = Instant::now();
        let perft_nodes = self.perft(mg, cm, perft_depth);
        let perft_time = t.elapsed();

        BenchResult {
            pseudo_legal_per_sec,
            legal_per_sec,
            make_unmake_per_sec,
            check_tests_per_sec,
            perft_depth,
            perft_nodes,
            perft_time,
        }
    }
}
