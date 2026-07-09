// src/attack_table.rs
//! Optional incremental coverage ("attack") table.
//!
//! ── Definition ───────────────────────────────────────────────────────────
//! Square `D` is **covered** by the piece on `F` iff some movement pattern
//! reaches `D` from `F` with `can_land_enemy`, and every *blockable*
//! intermediate square of that path satisfies its pass-permissions on the
//! current board. Three deliberate consequences:
//!
//!   * The occupancy of `D` itself is ignored ⇒ "covers" = attacks ∪
//!     defends. This is the quantity SEE and territory evaluation want, and
//!     unlike `MoveGenerator::is_square_attacked` it does not consult
//!     `can_land_friendly`.
//!   * Castling patterns are excluded; castling is not an attack.
//!     (`generate_theoretical_moves_for_pst` already drops them.)
//!   * Flight-capture (`%`) cells are covered. A `%` step forces
//!     `can_pass_enemy`/`can_pass_friendly`, so the ray isn't blocked there.
//!
//! Because coverage ignores the destination's occupancy, a piece's coverage
//! set can change only if:
//!   (a) its own square changed occupant, or
//!   (b) a square in its *influence mask* (the union of its blocker squares)
//!       changed occupancy, or
//!   (c) its `move_count` changed — which implies (a).
//!
//! Hence, given a set Δ of changed squares:
//!   phase 1: recompute every square in Δ
//!   phase 2: recompute every occupied `f ∉ Δ` with `influence[f] ∩ Δ ≠ ∅`
//!
//! Phase 2 is one bitset AND per piece (a single `u64` on 8×8). Its worst
//! case is a full rebuild plus a cheap scan, so no rebuild threshold exists.
//!
//! ── Cost when disabled ───────────────────────────────────────────────────
//! `GameState::attack_table` is `Option<Box<AttackTable>>`. Mutation sites
//! pay 2–5 perfectly-predicted not-taken branches per ply — against the
//! `HashMap` insert `finalize_state_update` already performs.
//!
//! ── Laziness ─────────────────────────────────────────────────────────────
//! `mark_dirty` is O(1); `sync` only runs when an engine queries. A legality
//! probe (`is_move_legal` → execute → check → undo) never queries, so it
//! costs two dirty marks and nothing else.
//!
//! ── Divergence from `is_square_attacked` (documented, intentional) ───────
//! `MoveGenerator::is_square_attacked` additionally requires an actual enemy
//! occupant for flight threats, and uses `can_land_at` (so a friendly-
//! occupied square is not "attacked"). This table is therefore NOT a drop-in
//! replacement for check detection.

use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::move_generator::{
    blocker_checks, blockers_clear, flight_capture_indices, BlockCheck, MoveGenerator,
};
use smallvec::SmallVec;
use std::cell::Cell;
use std::fmt;
use std::sync::Arc;

/// Assumed empty-square probability, used only when deriving the *average*
/// coverage of a piece type (a variant-agnostic material proxy).
const EMPTY_PROB: f32 = 0.7;

// ─────────────────────────────────────────────────────────────────────────
// Bitset
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Bitset {
    words: Box<[u64]>,
}

impl Bitset {
    pub fn new(bits: usize) -> Self {
        let words = bits.max(1).div_ceil(64);
        Self {
            words: vec![0u64; words].into_boxed_slice(),
        }
    }
    #[inline(always)]
    pub fn set(&mut self, i: usize) {
        self.words[i >> 6] |= 1u64 << (i & 63);
    }
    #[inline(always)]
    pub fn clear(&mut self, i: usize) {
        self.words[i >> 6] &= !(1u64 << (i & 63));
    }
    #[inline(always)]
    pub fn test(&self, i: usize) -> bool {
        (self.words[i >> 6] >> (i & 63)) & 1 != 0
    }
    #[inline]
    pub fn clear_all(&mut self) {
        for w in self.words.iter_mut() {
            *w = 0;
        }
    }
    #[inline(always)]
    pub fn intersects(&self, other: &Bitset) -> bool {
        self.words
        .iter()
        .zip(other.words.iter())
        .any(|(a, b)| a & b != 0)
    }
    #[inline]
    pub fn count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }
    #[inline]
    pub fn iter(&self) -> BitsetIter<'_> {
        BitsetIter {
            words: &self.words,
            wi: 0,
            cur: self.words.first().copied().unwrap_or(0),
        }
    }
}

pub struct BitsetIter<'a> {
    words: &'a [u64],
    wi: usize,
    cur: u64,
}

impl Iterator for BitsetIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        loop {
            if self.cur != 0 {
                let b = self.cur.trailing_zeros() as usize;
                self.cur &= self.cur - 1;
                return Some(self.wi * 64 + b);
            }
            self.wi += 1;
            if self.wi >= self.words.len() {
                return None;
            }
            self.cur = self.words[self.wi];
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Precomputed rays (immutable, shared via Arc)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CoverageRay {
    pub dest: u16,
    pub requires_unmoved: bool,
    pub blockers: SmallVec<[BlockCheck; 6]>,
    /// Flight-capture (`%`) cells. Empty for every variant that never uses `%`.
    pub flight: SmallVec<[u16; 2]>,
}

pub struct CoverageRays {
    board_size: (usize, usize),
    cols: usize,
    flat: usize,
    num_pieces: usize,
    rays: Vec<Vec<CoverageRay>>,
    influence: Vec<Bitset>,
    avg_coverage: Vec<f32>,
}

impl fmt::Debug for CoverageRays {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoverageRays")
        .field("board_size", &self.board_size)
        .field("num_pieces", &self.num_pieces)
        .finish()
    }
}

impl CoverageRays {
    /// Build from the move generator's *theoretical* (empty-board) tracer.
    /// That tracer already excludes castling patterns and applies the
    /// variant's zone filters, so we inherit both for free. `move_count = 0`
    /// keeps `requires_unmoved` rays in the table; they're skipped per-piece
    /// at query time.
    ///
    /// Must be called *after* `MoveGenerator::precompute_moves_for_board`
    /// (which `GameState::init_with_board` always does), because the zone
    /// bitmaps are resolved there.
    pub fn build(mg: &MoveGenerator, board_size: (usize, usize), num_pieces: usize) -> Self {
        let (rows, cols) = board_size;
        let flat = rows * cols;
        let slots = num_pieces * 2 * flat.max(1);
        let mut rays: Vec<Vec<CoverageRay>> = Vec::with_capacity(slots);
        let mut influence: Vec<Bitset> = Vec::with_capacity(slots);
        let mut avg_coverage = vec![0.0f32; num_pieces];

        for pt in 0..num_pieces {
            let mut total = 0.0f32;
            for ci in 0..2usize {
                let color = if ci == 0 {
                    PieceColor::White
                } else {
                    PieceColor::Black
                };
                for r in 0..rows {
                    for c in 0..cols {
                        let from_flat = r * cols + c;
                        let mut list: Vec<CoverageRay> = Vec::new();
                        let mut inf = Bitset::new(flat);

                        for mwp in
                            mg.generate_theoretical_moves_for_pst((r, c), pt, color, board_size, 0)
                            {
                                let rule = &*mwp.rule;
                                if !rule.can_land_enemy {
                                    continue;
                                }
                                let dest_flat = mwp.destination.0 * cols + mwp.destination.1;

                                let mut flight: SmallVec<[u16; 2]> = SmallVec::new();
                                if rule.has_flight_capture {
                                    for i in flight_capture_indices(rule, &mwp.path) {
                                        let p = mwp.path.steps[i + 1];
                                        let fc = p.0 as usize * cols + p.1 as usize;
                                        if fc != from_flat {
                                            flight.push(fc as u16);
                                        }
                                    }
                                }

                                // The null-move pattern `?` has dest == from and
                                // (by default) can_land_enemy. A piece must never
                                // "cover" its own square, or SEE self-defends.
                                if dest_flat == from_flat && flight.is_empty() {
                                    continue;
                                }

                                let blockers = blocker_checks(rule, &mwp.path);
                                for b in &blockers {
                                    inf.set(b.row as usize * cols + b.col as usize);
                                }
                                if !rule.requires_unmoved {
                                    total += EMPTY_PROB.powi(blockers.len() as i32);
                                }
                                list.push(CoverageRay {
                                    dest: dest_flat as u16,
                                    requires_unmoved: rule.requires_unmoved,
                                    blockers,
                                    flight,
                                });
                            }
                            rays.push(list);
                            influence.push(inf);
                    }
                }
            }
            avg_coverage[pt] = if flat > 0 {
                total / (2.0 * flat as f32)
            } else {
                0.0
            };
        }

        Self {
            board_size,
            cols,
            flat,
            num_pieces,
            rays,
            influence,
            avg_coverage,
        }
    }

    #[inline(always)]
    fn slot(&self, pt: usize, ci: usize, f: usize) -> usize {
        (pt * 2 + ci) * self.flat + f
    }
    #[inline(always)]
    pub fn rays(&self, pt: usize, ci: usize, f: usize) -> &[CoverageRay] {
        &self.rays[self.slot(pt, ci, f)]
    }
    #[inline(always)]
    pub fn influence(&self, pt: usize, ci: usize, f: usize) -> &Bitset {
        &self.influence[self.slot(pt, ci, f)]
    }
    #[inline]
    pub fn num_pieces(&self) -> usize {
        self.num_pieces
    }
    #[inline]
    pub fn board_size(&self) -> (usize, usize) {
        self.board_size
    }
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }
    #[inline]
    pub fn flat(&self) -> usize {
        self.flat
    }
    #[inline]
    pub fn avg_coverage(&self) -> &[f32] {
        &self.avg_coverage
    }
}

/// The single place where a piece's coverage is computed from rays + board.
/// Shared by the incremental table and the from-scratch reference map, which
/// is exactly why `AttackTable::verify` is a meaningful test.
#[inline]
fn ray_cover(rays: &CoverageRays, board: &Board, f: usize, piece: &Piece, out: &mut Bitset) {
    out.clear_all();
    if piece.piece_type >= rays.num_pieces() {
        return;
    }
    let ci = piece.color.index();
    for ray in rays.rays(piece.piece_type, ci, f) {
        if ray.requires_unmoved && piece.move_count > 0 {
            continue;
        }
        if !blockers_clear(board, &ray.blockers, piece.color) {
            continue;
        }
        let d = ray.dest as usize;
        if d != f {
            out.set(d);
        }
        for &fc in &ray.flight {
            let fc = fc as usize;
            if fc != f {
                out.set(fc);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Query surface — one trait, two implementations
// ─────────────────────────────────────────────────────────────────────────

pub trait CoverageQuery {
    fn cols(&self) -> usize;
    fn attackers_flat(&self, flat: usize, color: PieceColor) -> &[u16];
    fn covers_flat(&self, flat: usize) -> &[u16];

    #[inline]
    fn attackers(&self, sq: Position, color: PieceColor) -> &[u16] {
        self.attackers_flat(sq.0 * self.cols() + sq.1, color)
    }
    #[inline]
    fn covers(&self, from: Position) -> &[u16] {
        self.covers_flat(from.0 * self.cols() + from.1)
    }
    #[inline]
    fn is_attacked(&self, sq: Position, color: PieceColor) -> bool {
        !self.attackers(sq, color).is_empty()
    }
    #[inline]
    fn square(&self, flat: u16) -> Position {
        let c = self.cols();
        ((flat as usize) / c, (flat as usize) % c)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Reference (from-scratch) map
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CoverageMap {
    rows: usize,
    cols: usize,
    covers: Vec<SmallVec<[u16; 12]>>,
    covered_by: [Vec<SmallVec<[u16; 4]>>; 2],
    scratch: Bitset,
}

impl CoverageMap {
    pub fn new(board_size: (usize, usize)) -> Self {
        let (rows, cols) = board_size;
        let flat = (rows * cols).max(1);
        Self {
            rows,
            cols,
            covers: vec![SmallVec::new(); flat],
            covered_by: [vec![SmallVec::new(); flat], vec![SmallVec::new(); flat]],
            scratch: Bitset::new(flat),
        }
    }

    pub fn recompute(&mut self, rays: &CoverageRays, board: &Board) {
        for v in self.covers.iter_mut() {
            v.clear();
        }
        for c in 0..2 {
            for v in self.covered_by[c].iter_mut() {
                v.clear();
            }
        }

        let mut scratch = std::mem::take(&mut self.scratch);
        for r in 0..self.rows {
            for c in 0..self.cols {
                let Some(p) = board.get_piece((r, c)) else {
                    continue;
                };
                let f = r * self.cols + c;
                let ci = p.color.index();
                ray_cover(rays, board, f, &p, &mut scratch);
                for d in scratch.iter() {
                    self.covers[f].push(d as u16);
                    self.covered_by[ci][d].push(f as u16);
                }
            }
        }
        self.scratch = scratch;
    }
}

impl CoverageQuery for CoverageMap {
    #[inline]
    fn cols(&self) -> usize {
        self.cols
    }
    #[inline]
    fn attackers_flat(&self, flat: usize, color: PieceColor) -> &[u16] {
        &self.covered_by[color.index()][flat]
    }
    #[inline]
    fn covers_flat(&self, flat: usize) -> &[u16] {
        &self.covers[flat]
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Incremental table
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct AttackStats {
    pub full_builds: u64,
    pub syncs: u64,
    pub dirty_marks: u64,
    pub piece_recomputes: u64,
    pub scanned_pieces: u64,
    pub queries: u64,
}

#[derive(Clone)]
pub struct AttackTable {
    rays: Arc<CoverageRays>,
    rows: usize,
    cols: usize,
    flat: usize,
    /// (piece_type, color_idx) as of the last sync.
    occupant: Vec<Option<(u16, u8)>>,
    covers: Vec<SmallVec<[u16; 12]>>,
    covered_by: [Vec<SmallVec<[u16; 4]>>; 2],
    occ: Bitset,
    dirty_mask: Bitset,
    dirty_list: Vec<u16>,
    scan_buf: Vec<u16>,
    scratch: Bitset,
    stats: AttackStats,
    queries: Cell<u64>,
}

impl fmt::Debug for AttackTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttackTable")
        .field("size", &(self.rows, self.cols))
        .field("pending_dirty", &self.dirty_list.len())
        .field("stats", &self.stats)
        .finish()
    }
}

impl AttackTable {
    pub fn new(rays: Arc<CoverageRays>, board: &Board) -> Self {
        let (rows, cols) = board.size();
        let flat = (rows * cols).max(1);
        let mut t = Self {
            rays,
            rows,
            cols,
            flat,
            occupant: vec![None; flat],
            covers: vec![SmallVec::new(); flat],
            covered_by: [vec![SmallVec::new(); flat], vec![SmallVec::new(); flat]],
            occ: Bitset::new(flat),
            dirty_mask: Bitset::new(flat),
            dirty_list: Vec::with_capacity(16),
            scan_buf: Vec::with_capacity(16),
            scratch: Bitset::new(flat),
            stats: AttackStats::default(),
            queries: Cell::new(0),
        };
        t.rebuild(board);
        t
    }

    #[inline(always)]
    pub fn mark_dirty(&mut self, p: Position) {
        if p.0 >= self.rows || p.1 >= self.cols {
            return;
        }
        let f = p.0 * self.cols + p.1;
        if !self.dirty_mask.test(f) {
            self.dirty_mask.set(f);
            self.dirty_list.push(f as u16);
        }
        self.stats.dirty_marks += 1;
    }

    pub fn rebuild(&mut self, board: &Board) {
        self.stats.full_builds += 1;
        for v in self.covers.iter_mut() {
            v.clear();
        }
        for c in 0..2 {
            for v in self.covered_by[c].iter_mut() {
                v.clear();
            }
        }
        for o in self.occupant.iter_mut() {
            *o = None;
        }
        self.occ.clear_all();

        let rays = Arc::clone(&self.rays);
        for r in 0..self.rows {
            for c in 0..self.cols {
                if board.get_piece((r, c)).is_some() {
                    self.recompute(board, r * self.cols + c, &rays);
                }
            }
        }
        self.dirty_mask.clear_all();
        self.dirty_list.clear();
    }

    pub fn sync(&mut self, board: &Board) {
        if self.dirty_list.is_empty() {
            return;
        }
        self.stats.syncs += 1;
        let rays = Arc::clone(&self.rays);

        // Phase 1 — squares whose occupant changed.
        for i in 0..self.dirty_list.len() {
            let f = self.dirty_list[i] as usize;
            self.recompute(board, f, &rays);
        }

        // Phase 2 — pieces whose blocker-influence intersects the dirty set.
        let mut buf = std::mem::take(&mut self.scan_buf);
        buf.clear();
        {
            let dirty = &self.dirty_mask;
            let occupant = &self.occupant;
            for f in self.occ.iter() {
                if dirty.test(f) {
                    continue; // already handled in phase 1
                }
                if let Some((pt, ci)) = occupant[f] {
                    let pt = pt as usize;
                    if pt < rays.num_pieces()
                        && rays.influence(pt, ci as usize, f).intersects(dirty)
                        {
                            buf.push(f as u16);
                        }
                }
            }
        }
        self.stats.scanned_pieces += self.occ.count() as u64;
        for &f in &buf {
            self.recompute(board, f as usize, &rays);
        }
        self.scan_buf = buf;

        for i in 0..self.dirty_list.len() {
            let f = self.dirty_list[i] as usize;
            self.dirty_mask.clear(f);
        }
        self.dirty_list.clear();
    }

    fn recompute(&mut self, board: &Board, f: usize, rays: &CoverageRays) {
        self.stats.piece_recomputes += 1;

        // Withdraw the previous occupant's coverage.
        let mut covers = std::mem::take(&mut self.covers[f]);
        if let Some((_, ci)) = self.occupant[f] {
            let ci = ci as usize;
            for &d in covers.iter() {
                let v = &mut self.covered_by[ci][d as usize];
                if let Some(i) = v.iter().position(|&x| x == f as u16) {
                    v.swap_remove(i);
                }
            }
        }
        covers.clear();

        match board.get_piece((f / self.cols, f % self.cols)) {
            None => {
                self.occupant[f] = None;
                self.occ.clear(f);
            }
            Some(p) => {
                let ci = p.color.index();
                self.occupant[f] = Some((p.piece_type as u16, ci as u8));
                self.occ.set(f);
                let mut scratch = std::mem::take(&mut self.scratch);
                ray_cover(rays, board, f, &p, &mut scratch);
                for d in scratch.iter() {
                    covers.push(d as u16);
                    self.covered_by[ci][d].push(f as u16);
                }
                self.scratch = scratch;
            }
        }
        self.covers[f] = covers;
    }

    /// Compare against a from-scratch rebuild. Only meaningful on a synced
    /// table; returns `false` if anything is still pending.
    pub fn verify(&self, board: &Board) -> bool {
        if !self.dirty_list.is_empty() {
            return false;
        }
        let mut reference = CoverageMap::new((self.rows, self.cols));
        reference.recompute(&self.rays, board);
        for f in 0..self.flat {
            let mut a = self.covers[f].to_vec();
            let mut b = reference.covers[f].to_vec();
            a.sort_unstable();
            b.sort_unstable();
            if a != b {
                return false;
            }
            for ci in 0..2 {
                let mut a = self.covered_by[ci][f].to_vec();
                let mut b = reference.covered_by[ci][f].to_vec();
                a.sort_unstable();
                b.sort_unstable();
                if a != b {
                    return false;
                }
            }
        }
        true
    }

    pub fn stats(&self) -> AttackStats {
        let mut s = self.stats;
        s.queries = self.queries.get();
        s
    }
    pub fn reset_stats(&mut self) {
        self.stats = AttackStats::default();
        self.queries.set(0);
    }
    #[inline]
    pub fn rays_arc(&self) -> &Arc<CoverageRays> {
        &self.rays
    }
    #[inline]
    pub fn pending_dirty(&self) -> usize {
        self.dirty_list.len()
    }
}

impl CoverageQuery for AttackTable {
    #[inline]
    fn cols(&self) -> usize {
        self.cols
    }
    #[inline]
    fn attackers_flat(&self, flat: usize, color: PieceColor) -> &[u16] {
        self.queries.set(self.queries.get() + 1);
        &self.covered_by[color.index()][flat]
    }
    #[inline]
    fn covers_flat(&self, flat: usize) -> &[u16] {
        &self.covers[flat]
    }
}
