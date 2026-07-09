//! Move-generator data types and pure helpers that operate on them.

use crate::core::board::Board;
use crate::core::ghost::{Ghost, GhostFlags};
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use smallvec::SmallVec;
use std::sync::Arc;

// ─── Step & path permissions ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Permissions {
    pub can_pass_empty: bool,
    pub can_pass_enemy: bool,
    pub can_pass_friendly: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self::all()
    }
}

impl Permissions {
    pub fn all() -> Self {
        Self { can_pass_empty: true, can_pass_enemy: true, can_pass_friendly: true }
    }
    pub fn empty_only() -> Self {
        Self { can_pass_empty: true, can_pass_enemy: false, can_pass_friendly: false }
    }
    pub fn none() -> Self {
        Self { can_pass_empty: false, can_pass_enemy: false, can_pass_friendly: false }
    }
}

// ─── MoveStep ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MoveStep {
    pub directions: SmallVec<[(i8, i8); 8]>,
    pub length: usize,
    pub is_repeatable: bool,
    pub max_repetitions: Option<usize>,
    pub resets_center: bool,
    pub is_optional_stop: bool,
    pub permissions: Permissions,

    /// If set, this step leaves a ghost on **the square it departed from**,
    /// aliasing the piece's final destination.
    ///
    /// DSL: `e` → `CAPTURE_EP`, `E` → `CAPTURE_OPEN`, `&` → `CASTLE_TARGET`,
    /// `'` → a bare ghost (flags `NONE`), which still projects royalty if its
    /// owner is royal. Markers compose: `E&`, `'&`.
    pub creates_ghost: Option<GhostFlags>,

    pub repetition_permissions: Permissions,
    pub lock_to_previous_direction: bool,

    pub captures_in_flight_enemy: bool,
    pub captures_in_flight_friendly: bool,
}

impl Default for MoveStep {
    fn default() -> Self {
        Self {
            directions: SmallVec::new(),
            length: 1,
            is_repeatable: false,
            max_repetitions: None,
            resets_center: false,
            is_optional_stop: false,
            permissions: Permissions::all(),
            creates_ghost: None,
            repetition_permissions: Permissions::empty_only(),
            lock_to_previous_direction: false,
            captures_in_flight_enemy: false,
            captures_in_flight_friendly: false,
        }
    }
}

// ─── Paths & moves ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MovePath {
    /// `n + 1` entries: the origin, then the square reached by each sub-step.
    pub steps: SmallVec<[(u8, u8); 10]>,
    /// `n` entries: which `CompiledMove::steps` index produced each sub-step.
    pub step_indices: SmallVec<[u8; 10]>,
}

#[derive(Debug, Clone)]
pub struct MoveWithPath {
    pub destination: Position,
    pub rule: Arc<CompiledMove>,
    pub path: MovePath,
}

#[derive(Debug, Clone)]
pub struct CastlingOption {
    pub king_to: Position,
    pub rook_from: Position,
    pub rook_to: Position,
    pub rook_piece: Piece,
}

// ─── Compiled move pattern ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompiledMove {
    pub steps: Vec<MoveStep>,
    pub requires_unmoved: bool,
    pub can_land_empty: bool,
    pub can_land_enemy: bool,
    pub can_land_friendly: bool,
    pub is_irreversible: bool,
    pub captures_en_passant: bool,
    pub is_king_castle: bool,
    pub is_rook_castle: bool,
    pub from_zone: Option<String>,
    pub to_zone: Option<String>,
    pub capture_filter: Option<usize>,
    pub has_zones: bool,
    pub has_flight_capture: bool,
}

impl Default for CompiledMove {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            requires_unmoved: false,
            can_land_empty: true,
            can_land_enemy: true,
            can_land_friendly: false,
            is_irreversible: false,
            captures_en_passant: false,
            is_king_castle: false,
            is_rook_castle: false,
            from_zone: None,
            to_zone: None,
            capture_filter: None,
            has_zones: false,
            has_flight_capture: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledPiece {
    pub moves: Vec<Arc<CompiledMove>>,
    pub properties: crate::piece_config::PieceProperties,
}

// ─── Precomputed database entries ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PrecomputedMove {
    pub destination: Position,
    pub pattern_index: usize,
    pub is_blockable: bool,
    pub path: MovePath,
}

#[derive(Debug, Clone)]
pub struct MoveToSquare {
    pub from: Position,
    pub pattern_index: usize,
    pub is_blockable: bool,
    pub path: MovePath,
    pub is_flight_threat: bool,
}

// ─── Capture resolution — THE rule, one copy ────────────────────────────

/// Could `pattern` capture `victim` by landing on it?
#[inline(always)]
pub fn can_capture_piece(pattern: &CompiledMove, victim: &Piece) -> bool {
    pattern.can_land_enemy && pattern.capture_filter.map_or(true, |t| t == victim.piece_type)
}

/// What is removed from the board by landing on `dest`, and from where?
///
/// **Real occupant first; a ghost alias only on an empty square.** The result
/// is colour-agnostic: a friendly occupant is returned too, because a pattern
/// with `@` (`can_land_friendly`) really does remove it. Callers that care
/// about legality consult `can_land_at`; callers that care about the board
/// mutation consult this.
///
/// This is the **single** implementation of the capture rule.
/// `MoveChain::build_move`, `generate_pseudo_legal_moves`,
/// `GameState::attempt_move`, `Search::compact_to_expanded` and `can_land_at`
/// all route through it, so they cannot disagree about who the victim is when
/// a castling rook ends up standing on a transit ghost.
#[inline]
pub fn resolve_landing_capture(
    board: &Board,
    dest: Position,
    pattern: &CompiledMove,
) -> Option<(Piece, Position)> {
    if let Some(p) = board.get_piece(dest) {
        return Some((p, dest));
    }
    let g = board.ghost_at(dest)?;
    if !g.allows_capture(pattern.captures_en_passant) {
        return None;
    }
    let owner = g.owner();
    board.get_piece(owner).map(|victim| (victim, owner))
}

/// Can a piece of `color` moving under `pattern` land on `dest`?
#[inline]
pub fn can_land_at(
    board: &Board,
    dest: Position,
    color: PieceColor,
    pattern: &CompiledMove,
) -> bool {
    match resolve_landing_capture(board, dest, pattern) {
        Some((victim, _)) if victim.color != color => can_capture_piece(pattern, &victim),
        Some(_) => pattern.can_land_friendly,
        None => pattern.can_land_empty,
    }
}

// ─── Ghost construction (pure: pattern + path only) ─────────────────────

/// The ghosts a move leaves behind.
///
/// Each flagged step ghosts the square it **departed** (`path.steps[i]`),
/// pointing at the piece's final destination. The destination itself is never
/// ghosted — the piece is actually standing there.
///
/// Pure, so castling expansion can compute it *before* the move is applied.
pub fn ghosts_for(mwp: &MoveWithPath, _from: Position, to: Position) -> SmallVec<[Ghost; 4]> {
    let mut out: SmallVec<[Ghost; 4]> = SmallVec::new();
    let n = mwp.path.step_indices.len();
    for i in 0..n {
        let pu = mwp.path.steps[i];
        let pos = (pu.0 as usize, pu.1 as usize);
        if pos == to {
            continue;
        }
        let si = mwp.path.step_indices[i] as usize;
        if let Some(step) = mwp.rule.steps.get(si) {
            if let Some(flags) = step.creates_ghost {
                out.push(Ghost::new(pos, to, flags));
            }
        }
    }
    out
}

/// Squares a castling partner may land on.
///
/// Preferred form: ghosts carrying `CASTLE_TARGET` (DSL `&`). Because the flag
/// lives on the ghost, a declared rook destination is *necessarily* also a
/// transit assertion.
///
/// Legacy fallback (no `&` anywhere, as in the shipped `FIDE.pieces`): every
/// square the king *arrives* on, excluding its origin and destination.
pub fn castle_target_squares(
    mwp: &MoveWithPath,
    from: Position,
    to: Position,
) -> SmallVec<[Position; 2]> {
    let mut out: SmallVec<[Position; 2]> = SmallVec::new();

    let ghosts = ghosts_for(mwp, from, to);
    if ghosts.iter().any(|g| g.is_castle_target()) {
        for g in ghosts.iter().filter(|g| g.is_castle_target()) {
            let sq = g.square();
            if sq != to && !out.contains(&sq) {
                out.push(sq);
            }
        }
        return out;
    }

    let n = mwp.path.step_indices.len();
    for i in 0..n {
        let pu = mwp.path.steps[i + 1];
        let sq = (pu.0 as usize, pu.1 as usize);
        if sq != from && sq != to && !out.contains(&sq) {
            out.push(sq);
        }
    }
    out
}

// ─── Blockability ───────────────────────────────────────────────────────

pub fn is_move_blockable(mwp: &MoveWithPath) -> bool {
    let pattern = &mwp.rule;
    if pattern.is_king_castle || pattern.is_rook_castle {
        return true;
    }
    let mut rep_count = [0u32; 32];
    for (i, &step_idx) in mwp.path.step_indices.iter().enumerate() {
        let is_last = i == mwp.path.step_indices.len() - 1;
        let step = &pattern.steps[step_idx as usize];
        rep_count[step_idx as usize] += 1;

        if !is_last {
            let next = mwp.path.step_indices[i + 1];
            let same = next == step_idx;
            if same && rep_count[step_idx as usize] as usize % step.length == 0 {
                if !step.repetition_permissions.can_pass_empty
                    || !step.repetition_permissions.can_pass_enemy
                    || !step.repetition_permissions.can_pass_friendly
                    {
                        return true;
                    }
            } else if !step.permissions.can_pass_empty
                || !step.permissions.can_pass_enemy
                || !step.permissions.can_pass_friendly
                {
                    return true;
                }
        }
    }
    false
}

pub fn count_blocking_squares(mwp: &MoveWithPath) -> u32 {
    let pattern = &mwp.rule;
    if pattern.is_king_castle || pattern.is_rook_castle {
        return mwp.path.steps.len().saturating_sub(2) as u32;
    }

    let mut block_count = 0u32;
    let mut rep_count = [0u32; 32];
    for (i, &step_idx) in mwp.path.step_indices.iter().enumerate() {
        let is_last = i == mwp.path.step_indices.len() - 1;
        let step = &pattern.steps[step_idx as usize];
        rep_count[step_idx as usize] += 1;

        if !is_last {
            let next = mwp.path.step_indices[i + 1];
            let same = next == step_idx;
            let blocked = if same && rep_count[step_idx as usize] as usize % step.length == 0 {
                !step.repetition_permissions.can_pass_empty
                || !step.repetition_permissions.can_pass_enemy
                || !step.repetition_permissions.can_pass_friendly
            } else {
                !step.permissions.can_pass_empty
                || !step.permissions.can_pass_enemy
                || !step.permissions.can_pass_friendly
            };
            if blocked {
                block_count += 1;
            }
        }
    }
    block_count
}

// ─── Flight capture ─────────────────────────────────────────────────────

#[inline]
pub fn flight_capture_indices(rule: &CompiledMove, path: &MovePath) -> SmallVec<[usize; 4]> {
    let mut out: SmallVec<[usize; 4]> = SmallVec::new();
    if !rule.has_flight_capture {
        return out;
    }
    let n = path.step_indices.len();
    for i in 0..n {
        if i + 1 == n {
            continue;
        }
        let step_idx = path.step_indices[i] as usize;
        if let Some(step) = rule.steps.get(step_idx) {
            if step.captures_in_flight_enemy || step.captures_in_flight_friendly {
                out.push(i);
            }
        }
    }
    out
}

/// Pieces captured IN FLIGHT along this move's path. May include friendly
/// pieces. Non-allocating no-op when the pattern has no `%`.
#[inline]
pub fn flight_captures(
    board: &Board,
    mwp: &MoveWithPath,
    mover_color: PieceColor,
) -> SmallVec<[(Position, Piece); 2]> {
    let mut out: SmallVec<[(Position, Piece); 2]> = SmallVec::new();
    for i in flight_capture_indices(&mwp.rule, &mwp.path) {
        let step_idx = mwp.path.step_indices[i] as usize;
        let step = &mwp.rule.steps[step_idx];
        let pu = mwp.path.steps[i + 1];
        let pos = (pu.0 as usize, pu.1 as usize);
        if let Some(p) = board.get_piece(pos) {
            let wanted = if p.color == mover_color {
                step.captures_in_flight_friendly
            } else {
                step.captures_in_flight_enemy
            };
            if wanted {
                out.push((pos, p));
            }
        }
    }
    out
}

// ─── Blocker extraction (shared by InfluenceEngine & the attack table) ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockCheck {
    pub row: u8,
    pub col: u8,
    pub perm_bits: u8,
}

pub fn blocker_checks(rule: &CompiledMove, path: &MovePath) -> SmallVec<[BlockCheck; 6]> {
    let mut out: SmallVec<[BlockCheck; 6]> = SmallVec::new();
    let idxs = &path.step_indices;
    let steps = &path.steps;
    let n = idxs.len();
    if n == 0 {
        return out;
    }

    let mut rep_count = [0u32; 32];
    for i in 0..n {
        let step_idx = idxs[i] as usize;
        if step_idx >= rule.steps.len() || step_idx >= 32 {
            continue;
        }
        let step = &rule.steps[step_idx];
        rep_count[step_idx] += 1;

        if i + 1 == n {
            break;
        }
        let pos = steps[i + 1];
        let continuing = idxs[i + 1] as usize == step_idx;
        let at_rep_boundary =
        continuing && step.length > 0 && (rep_count[step_idx] as usize) % step.length == 0;

        let (pe, px, pf) = if at_rep_boundary {
            (
                step.repetition_permissions.can_pass_empty,
             step.repetition_permissions.can_pass_enemy,
             step.repetition_permissions.can_pass_friendly,
            )
        } else {
            (
                step.permissions.can_pass_empty,
             step.permissions.can_pass_enemy,
             step.permissions.can_pass_friendly,
            )
        };

        if !(pe && px && pf) {
            out.push(BlockCheck {
                row: pos.0,
                col: pos.1,
                perm_bits: (pe as u8) | ((px as u8) << 1) | ((pf as u8) << 2),
            });
        }
    }
    out
}

#[inline(always)]
pub fn blockers_clear(board: &Board, blockers: &[BlockCheck], mover: PieceColor) -> bool {
    for bc in blockers {
        let bits = bc.perm_bits;
        let ok = match board.get_piece((bc.row as usize, bc.col as usize)) {
            None => bits & 0b001 != 0,
            Some(p) if p.color != mover => bits & 0b010 != 0,
            Some(_) => bits & 0b100 != 0,
        };
        if !ok {
            return false;
        }
    }
    true
}
