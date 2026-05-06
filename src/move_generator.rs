// src/move_generator.rs
use crate::board_config::ZoneConfig;
use crate::core::board::Board;
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use smallvec::{SmallVec, smallvec};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
        Self {
            can_pass_empty: true,
            can_pass_enemy: true,
            can_pass_friendly: true,
        }
    }
    pub fn empty_only() -> Self {
        Self {
            can_pass_empty: true,
            can_pass_enemy: false,
            can_pass_friendly: false,
        }
    }
    pub fn none() -> Self {
        Self {
            can_pass_empty: false,
            can_pass_enemy: false,
            can_pass_friendly: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MoveStep {
    pub directions: HashSet<(i8, i8)>,
    pub length: usize,
    pub is_repeatable: bool,
    pub max_repetitions: Option<usize>,
    /// `#` — after this step the dot-product "no doubling back" origin is
    /// reset to the square just reached, so the next step may turn freely.
    pub resets_center: bool,
    pub is_optional_stop: bool,
    pub permissions: Permissions,
    pub creates_en_passant: Option<bool>,
    pub repetition_permissions: Permissions,
    /// `=` — this step must use the *same* delta the previous step used.
    /// Lets multi-step riders (cannon, grasshopper, lance-hoppers …) be
    /// written once instead of once per direction.
    pub lock_to_previous_direction: bool,
    pub partner_lands_here: bool,
}

impl Default for MoveStep {
    fn default() -> Self {
        Self {
            directions: HashSet::new(),
            length: 1,
            is_repeatable: false,
            max_repetitions: None,
            resets_center: false,
            is_optional_stop: false,
            permissions: Permissions::all(),
            creates_en_passant: None,
            repetition_permissions: Permissions::empty_only(),
            lock_to_previous_direction: false,
            partner_lands_here: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MovePath {
    pub steps: SmallVec<[(u8, u8); 10]>,
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
    /// Set iff either `from_zone` or `to_zone` is Some. Lets the hot
    /// paths skip two Option checks per pattern in the common case.
    pub has_zones: bool,
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledPiece {
    pub moves: Vec<Arc<CompiledMove>>,
    pub properties: crate::piece_config::PieceProperties,
}

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
}

#[derive(Debug, Clone)]
pub struct MoveGenerator {
    compiled_pieces: HashMap<usize, CompiledPiece>,
    move_database: HashMap<usize, [Vec<Vec<PrecomputedMove>>; 2]>,
    reverse_move_database: HashMap<usize, [Vec<Vec<MoveToSquare>>; 2]>,

    /// Declarative zone definitions as loaded from the `.game` file.
    /// Kept so we can re‑resolve if board size changes and so external
    /// callers (drawing, debug) can still introspect regions.
    zones: ZoneConfig,

    /// Resolved O(1) membership bitmaps: name → [white, black], each a
    /// flat `rows*cols` bool vector. Built in `precompute_moves_for_board`
    /// from `self.zones` once the board size is known. All hot‑path zone
    /// tests go through `in_zone()` which reads these, never `self.zones`.
    zone_bitmaps: HashMap<String, [Vec<bool>; 2]>,
    zone_board_cols: usize,
}

impl MoveGenerator {
    pub fn new(config_manager: &PieceConfigManager) -> Result<Self, String> {
        let mut compiled_pieces = HashMap::new();

        for (i, piece_name) in config_manager.piece_order.iter().enumerate() {
            if let Some(piece_config) = config_manager.pieces.get(piece_name) {
                let compiled = Self::compile_moveset(&piece_config.moveset, config_manager)?;
                let arc_moves = compiled.into_iter().map(Arc::new).collect();
                compiled_pieces.insert(
                    i,
                    CompiledPiece {
                        moves: arc_moves,
                        properties: piece_config.properties.clone(),
                    },
                );
            }
        }

        Ok(MoveGenerator {
            compiled_pieces,
            move_database: HashMap::new(),
            reverse_move_database: HashMap::new(),
            zones: ZoneConfig::default(),
            zone_bitmaps: HashMap::new(),
            zone_board_cols: 0,
        })
    }

    /// Called once per variant load, *before* precompute. Stores the
    /// declarative form; flattening to bitmaps happens in
    /// `precompute_moves_for_board` when the board size is known.
    pub fn set_zones(&mut self, zones: ZoneConfig) {
        for cp in self.compiled_pieces.values() {
            for m in &cp.moves {
                for z in [m.from_zone.as_deref(), m.to_zone.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    if !zones.has(z) {
                        println!(
                            "⚠️  move pattern references undefined zone '{}'; pattern will be inactive",
                            z
                        );
                    }
                }
            }
        }
        self.zones = zones;
        // Stale until precompute runs; clear so an accidental early call to
        // in_zone() (there shouldn't be one) is at least deterministic.
        self.zone_bitmaps.clear();
    }

    // ── Zone resolution & lookup ────────────────────────────────────────

    fn resolve_zone_bitmaps(&mut self, board_size: (usize, usize)) {
        self.zone_bitmaps.clear();
        self.zone_board_cols = board_size.1;

        for (name, zone) in &self.zones.zones {
            self.zone_bitmaps
                .insert(name.clone(), zone.resolve(board_size));
        }
    }

    /// O(1) zone membership. Undefined zone → `false` (pattern inactive),
    /// matching the previous `ZoneConfig::contains` semantics.
    #[inline]
    fn in_zone(&self, name: &str, pos: Position, color: PieceColor) -> bool {
        let ci = match color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };
        match self.zone_bitmaps.get(name) {
            Some(maps) => {
                let idx = pos.0 * self.zone_board_cols + pos.1;
                maps[ci].get(idx).copied().unwrap_or(false)
            }
            None => false,
        }
    }

    #[inline]
    fn pattern_from_ok(&self, p: &CompiledMove, from: Position, color: PieceColor) -> bool {
        if !p.has_zones {
            return true;
        }
        match &p.from_zone {
            None => true,
            Some(z) => self.in_zone(z, from, color),
        }
    }

    #[inline]
    fn pattern_to_ok(&self, p: &CompiledMove, dest: Position, color: PieceColor) -> bool {
        if !p.has_zones {
            return true;
        }
        match &p.to_zone {
            None => true,
            Some(z) => self.in_zone(z, dest, color),
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Theoretical / PST generation (board-agnostic)
    // ────────────────────────────────────────────────────────────────────

    pub fn generate_theoretical_moves_for_pst(
        &self,
        from: Position,
        piece_type: usize,
        color: PieceColor,
        board_size: (usize, usize),
        move_count: u32,
    ) -> Vec<MoveWithPath> {
        let mut final_moves = Vec::new();

        if let Some(compiled_piece) = self.compiled_pieces.get(&piece_type) {
            for pattern in &compiled_piece.moves {
                if (pattern.requires_unmoved && move_count > 0)
                    || pattern.is_king_castle
                    || pattern.is_rook_castle
                {
                    continue;
                }
                // Zone gates — keeps PST engines from scoring unreachable
                // squares (Xiangqi advisor outside the palace etc.).
                if !self.pattern_from_ok(pattern, from, color) {
                    continue;
                }

                let mut moves_with_paths = Vec::new();
                self.trace_path_theoretical(
                    board_size,
                    from,
                    from,
                    color,
                    pattern,
                    &pattern.steps,
                    None,
                    smallvec![(from.0 as u8, from.1 as u8)],
                    smallvec![],
                    &mut moves_with_paths,
                );
                for m in moves_with_paths {
                    if self.pattern_to_ok(pattern, m.destination, color) {
                        final_moves.push(m);
                    }
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
        let Some(compiled_piece) = self.compiled_pieces.get(&piece_type) else {
            return out;
        };

        for pattern in &compiled_piece.moves {
            if only_unmoved && pattern.requires_unmoved {
                continue;
            }
            if pattern.is_king_castle || pattern.is_rook_castle {
                continue;
            }
            if !self.pattern_from_ok(pattern, from, color) {
                continue;
            }

            let mut moves = Vec::new();
            self.trace_path_theoretical(
                board_size,
                from,
                from,
                color,
                pattern,
                &pattern.steps,
                None,
                smallvec![(from.0 as u8, from.1 as u8)],
                smallvec![],
                &mut moves,
            );
            for m in moves {
                if self.pattern_to_ok(pattern, m.destination, color) {
                    out.push((
                        m.destination,
                        pattern.can_land_enemy,
                        pattern.can_land_empty,
                    ));
                }
            }
        }
        out
    }

    pub fn count_blocking_squares(&self, move_with_path: &MoveWithPath) -> u32 {
        let pattern = &move_with_path.rule;

        if pattern.is_king_castle || pattern.is_rook_castle {
            return move_with_path.path.steps.len().saturating_sub(2) as u32;
        }

        let mut block_count = 0;
        let mut step_repetition_count = [0; 32];

        for (i, &step_idx) in move_with_path.path.step_indices.iter().enumerate() {
            let is_last = i == move_with_path.path.step_indices.len() - 1;
            let step = &pattern.steps[step_idx as usize];
            step_repetition_count[step_idx as usize] += 1;

            if !is_last {
                let next_step_idx = move_with_path.path.step_indices[i + 1];
                let is_continuing_same_step = next_step_idx == step_idx;

                let is_blocked = if is_continuing_same_step
                    && step_repetition_count[step_idx as usize] % step.length == 0
                {
                    !step.repetition_permissions.can_pass_empty
                        || !step.repetition_permissions.can_pass_enemy
                        || !step.repetition_permissions.can_pass_friendly
                } else {
                    !step.permissions.can_pass_empty
                        || !step.permissions.can_pass_enemy
                        || !step.permissions.can_pass_friendly
                };

                if is_blocked {
                    block_count += 1;
                }
            }
        }

        block_count
    }

    #[inline]
    fn skip_direction(
        step: &MoveStep,
        delta: (i8, i8),
        direction_origin: Position,
        current_pos: Position,
        prev_dir: Option<(i8, i8)>,
    ) -> bool {
        if step.lock_to_previous_direction {
            if let Some(pd) = prev_dir {
                return delta != pd;
            }
        }
        if direction_origin != current_pos {
            let vr = current_pos.0 as i32 - direction_origin.0 as i32;
            let vc = current_pos.1 as i32 - direction_origin.1 as i32;
            if vr * (delta.0 as i32) + vc * (delta.1 as i32) < 0 {
                return true;
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_path_theoretical(
        &self,
        board_size: (usize, usize),
        direction_origin: Position,
        current_pos: Position,
        color: PieceColor,
        pattern: &Arc<CompiledMove>,
        remaining_steps: &[MoveStep],
        prev_dir: Option<(i8, i8)>,
        current_path: SmallVec<[(u8, u8); 10]>,
        current_step_indices: SmallVec<[u8; 10]>,
        moves: &mut Vec<MoveWithPath>,
    ) {
        if remaining_steps.is_empty() {
            return;
        }

        let step = &remaining_steps[0];
        let step_index = pattern.steps.len() - remaining_steps.len();
        let is_last_step = remaining_steps.len() == 1;
        let y_axis_multiplier = if color == PieceColor::White { 1 } else { -1 };
        let (rows, cols) = board_size;

        for &dir in &step.directions {
            let delta = (dir.0 * y_axis_multiplier, dir.1);
            if Self::skip_direction(step, delta, direction_origin, current_pos, prev_dir) {
                continue;
            }

            let repetitions = if step.is_repeatable {
                step.max_repetitions.unwrap_or(usize::MAX)
            } else {
                1
            };

            let mut last_pos_in_repetition = current_pos;
            let mut accum_path = current_path.clone();
            let mut accum_indices = current_step_indices.clone();

            'rep_loop: for _ in 1..=repetitions {
                let mut dest = last_pos_in_repetition;
                for k in 1..=step.length {
                    let r = last_pos_in_repetition.0 as i32 + delta.0 as i32 * k as i32;
                    let c = last_pos_in_repetition.1 as i32 + delta.1 as i32 * k as i32;
                    if !(0..rows as i32).contains(&r) || !(0..cols as i32).contains(&c) {
                        break 'rep_loop;
                    }
                    dest = (r as usize, c as usize);
                    accum_path.push((dest.0 as u8, dest.1 as u8));
                    accum_indices.push(step_index as u8);
                }

                if step.is_optional_stop {
                    moves.push(MoveWithPath {
                        destination: dest,
                        rule: pattern.clone(),
                        path: MovePath {
                            steps: accum_path.clone(),
                            step_indices: accum_indices.clone(),
                        },
                    });
                }

                if is_last_step {
                    moves.push(MoveWithPath {
                        destination: dest,
                        rule: pattern.clone(),
                        path: MovePath {
                            steps: accum_path.clone(),
                            step_indices: accum_indices.clone(),
                        },
                    });
                } else {
                    let next_origin = if step.resets_center {
                        dest
                    } else {
                        direction_origin
                    };
                    self.trace_path_theoretical(
                        board_size,
                        next_origin,
                        dest,
                        color,
                        pattern,
                        &remaining_steps[1..],
                        Some(delta),
                        accum_path.clone(),
                        accum_indices.clone(),
                        moves,
                    );
                }
                last_pos_in_repetition = dest;
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Precompute database
    // ────────────────────────────────────────────────────────────────────

    pub fn precompute_moves_for_board(&mut self, board_size: (usize, usize)) {
        // Resolve declarative zones → flat bitmaps now that board size is known.
        self.resolve_zone_bitmaps(board_size);

        self.move_database.clear();
        self.reverse_move_database.clear();

        let total_squares = board_size.0 * board_size.1;

        for (&piece_type, compiled_piece) in &self.compiled_pieces {
            let mut white_moves = Vec::with_capacity(total_squares);
            let mut black_moves = Vec::with_capacity(total_squares);
            let mut white_reverse = vec![Vec::new(); total_squares];
            let mut black_reverse = vec![Vec::new(); total_squares];

            for row in 0..board_size.0 {
                for col in 0..board_size.1 {
                    let from = (row, col);

                    let white_precomputed = self.precompute_from_square(
                        board_size,
                        from,
                        PieceColor::White,
                        &compiled_piece.moves,
                    );
                    for pm in &white_precomputed {
                        let to_index = pm.destination.0 * board_size.1 + pm.destination.1;
                        white_reverse[to_index].push(MoveToSquare {
                            from,
                            pattern_index: pm.pattern_index,
                            is_blockable: pm.is_blockable,
                            path: pm.path.clone(),
                        });
                    }
                    white_moves.push(white_precomputed);

                    let black_precomputed = self.precompute_from_square(
                        board_size,
                        from,
                        PieceColor::Black,
                        &compiled_piece.moves,
                    );
                    for pm in &black_precomputed {
                        let to_index = pm.destination.0 * board_size.1 + pm.destination.1;
                        black_reverse[to_index].push(MoveToSquare {
                            from,
                            pattern_index: pm.pattern_index,
                            is_blockable: pm.is_blockable,
                            path: pm.path.clone(),
                        });
                    }
                    black_moves.push(black_precomputed);
                }
            }

            self.move_database
                .insert(piece_type, [white_moves, black_moves]);
            self.reverse_move_database
                .insert(piece_type, [white_reverse, black_reverse]);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_path_for_precomputation(
        &self,
        board_size: (usize, usize),
        direction_origin: Position,
        current_pos: Position,
        color: PieceColor,
        pattern: &Arc<CompiledMove>,
        remaining_steps: &[MoveStep],
        prev_dir: Option<(i8, i8)>,
        current_path: SmallVec<[(u8, u8); 10]>,
        current_step_indices: SmallVec<[u8; 10]>,
        moves: &mut Vec<MoveWithPath>,
    ) {
        if remaining_steps.is_empty() {
            return;
        }

        let step = &remaining_steps[0];
        let step_index = pattern.steps.len() - remaining_steps.len();
        let is_last_step = remaining_steps.len() == 1;
        let y_axis_multiplier = if color == PieceColor::White { 1 } else { -1 };
        let (rows, cols) = board_size;

        for &dir in &step.directions {
            let delta = (dir.0 * y_axis_multiplier, dir.1);
            if Self::skip_direction(step, delta, direction_origin, current_pos, prev_dir) {
                continue;
            }

            let repetitions = if step.is_repeatable {
                step.max_repetitions.unwrap_or(usize::MAX)
            } else {
                1
            };
            let mut last_pos_in_repetition = current_pos;
            let mut accum_path = current_path.clone();
            let mut accum_indices = current_step_indices.clone();

            'rep_loop: for _ in 1..=repetitions {
                let mut dest = last_pos_in_repetition;
                for k in 1..=step.length {
                    let r = last_pos_in_repetition.0 as i32 + delta.0 as i32 * k as i32;
                    let c = last_pos_in_repetition.1 as i32 + delta.1 as i32 * k as i32;
                    if !(0..rows as i32).contains(&r) || !(0..cols as i32).contains(&c) {
                        break 'rep_loop;
                    }
                    dest = (r as usize, c as usize);
                    accum_path.push((dest.0 as u8, dest.1 as u8));
                    accum_indices.push(step_index as u8);
                }

                if step.is_optional_stop
                    && (pattern.can_land_empty
                        || pattern.can_land_enemy
                        || pattern.can_land_friendly)
                {
                    moves.push(MoveWithPath {
                        destination: dest,
                        rule: pattern.clone(),
                        path: MovePath {
                            steps: accum_path.clone(),
                            step_indices: accum_indices.clone(),
                        },
                    });
                }

                if is_last_step {
                    if pattern.can_land_empty || pattern.can_land_enemy || pattern.can_land_friendly
                    {
                        moves.push(MoveWithPath {
                            destination: dest,
                            rule: pattern.clone(),
                            path: MovePath {
                                steps: accum_path.clone(),
                                step_indices: accum_indices.clone(),
                            },
                        });
                    }
                } else {
                    let next_origin = if step.resets_center {
                        dest
                    } else {
                        direction_origin
                    };
                    self.trace_path_for_precomputation(
                        board_size,
                        next_origin,
                        dest,
                        color,
                        pattern,
                        &remaining_steps[1..],
                        Some(delta),
                        accum_path.clone(),
                        accum_indices.clone(),
                        moves,
                    );
                }
                last_pos_in_repetition = dest;
            }
        }
    }

    pub fn is_move_blockable(&self, move_with_path: &MoveWithPath) -> bool {
        let pattern = &move_with_path.rule;
        if pattern.is_king_castle || pattern.is_rook_castle {
            return true;
        }

        let mut step_repetition_count = [0; 32];
        for (i, &step_idx) in move_with_path.path.step_indices.iter().enumerate() {
            let is_last = i == move_with_path.path.step_indices.len() - 1;
            let step = &pattern.steps[step_idx as usize];
            step_repetition_count[step_idx as usize] += 1;

            if !is_last {
                let next_step_idx = move_with_path.path.step_indices[i + 1];
                let is_continuing_same_step = next_step_idx == step_idx;

                if is_continuing_same_step
                    && step_repetition_count[step_idx as usize] % step.length == 0
                {
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

    fn is_path_valid(
        &self,
        board: &Board,
        path: &MovePath,
        color: PieceColor,
        pattern: &CompiledMove,
    ) -> bool {
        let mut step_repetition_count = [0; 32];

        for (i, &step_idx) in path.step_indices.iter().enumerate() {
            let pos_u8 = path.steps[i + 1];
            let pos = (pos_u8.0 as usize, pos_u8.1 as usize);
            let is_last = i == path.step_indices.len() - 1;

            let step = &pattern.steps[step_idx as usize];
            step_repetition_count[step_idx as usize] += 1;

            if !is_last {
                let next_step_idx = path.step_indices[i + 1];
                let is_continuing_same_step = next_step_idx == step_idx;

                let can_pass = if is_continuing_same_step
                    && step_repetition_count[step_idx as usize] % step.length == 0
                {
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
                if !can_pass {
                    return false;
                }
            } else if !self.can_land_at(board, pos, color, pattern) {
                return false;
            }
        }
        true
    }

    pub fn get_attackers_to_square(
        &self,
        board: &Board,
        target: Position,
        attacking_color: PieceColor,
    ) -> Vec<(Position, Piece)> {
        let mut attackers = Vec::new();
        let color_idx = match attacking_color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };
        let target_index = target.0 * board.cols() + target.1;

        for (&piece_type, reverse_moves) in &self.reverse_move_database {
            if let Some(moves_to_target) = reverse_moves[color_idx].get(target_index) {
                for mts in moves_to_target {
                    if let Some(piece) = board.get_piece(mts.from) {
                        if piece.color == attacking_color && piece.piece_type == piece_type {
                            if let Some(cp) = self.compiled_pieces.get(&piece_type) {
                                if let Some(pattern) = cp.moves.get(mts.pattern_index) {
                                    if pattern.requires_unmoved && piece.move_count > 0 {
                                        continue;
                                    }
                                    let ok = if mts.is_blockable {
                                        self.is_path_valid(
                                            board,
                                            &mts.path,
                                            attacking_color,
                                            pattern,
                                        )
                                    } else {
                                        self.can_land_at(board, target, attacking_color, pattern)
                                    };
                                    if ok {
                                        attackers.push((mts.from, piece));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        attackers
    }

    pub fn is_square_attacked(
        &self,
        board: &Board,
        target: Position,
        attacking_color: PieceColor,
    ) -> bool {
        let color_idx = match attacking_color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };
        let target_index = target.0 * board.cols() + target.1;

        for (&piece_type, reverse_moves) in &self.reverse_move_database {
            if let Some(moves_to_target) = reverse_moves[color_idx].get(target_index) {
                for mts in moves_to_target {
                    if let Some(piece) = board.get_piece(mts.from) {
                        if piece.color == attacking_color && piece.piece_type == piece_type {
                            if let Some(cp) = self.compiled_pieces.get(&piece_type) {
                                if let Some(pattern) = cp.moves.get(mts.pattern_index) {
                                    if pattern.requires_unmoved && piece.move_count > 0 {
                                        continue;
                                    }
                                    let ok = if mts.is_blockable {
                                        self.is_path_valid(
                                            board,
                                            &mts.path,
                                            attacking_color,
                                            pattern,
                                        )
                                    } else {
                                        self.can_land_at(board, target, attacking_color, pattern)
                                    };
                                    if ok {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn precompute_from_square(
        &self,
        board_size: (usize, usize),
        from: Position,
        color: PieceColor,
        patterns: &[Arc<CompiledMove>],
    ) -> Vec<PrecomputedMove> {
        let mut precomputed = Vec::new();

        for (pattern_idx, pattern) in patterns.iter().enumerate() {
            // from‑zone gate (bitmap, O(1)).
            if !self.pattern_from_ok(pattern, from, color) {
                continue;
            }

            let mut moves_with_paths = Vec::new();
            self.trace_path_for_precomputation(
                board_size,
                from,
                from,
                color,
                pattern,
                &pattern.steps,
                None,
                smallvec![(from.0 as u8, from.1 as u8)],
                smallvec![],
                &mut moves_with_paths,
            );

            for mwp in moves_with_paths {
                // to‑zone gate (bitmap, O(1)).
                if !self.pattern_to_ok(pattern, mwp.destination, color) {
                    continue;
                }

                let is_blockable = self.is_move_blockable(&mwp);
                precomputed.push(PrecomputedMove {
                    destination: mwp.destination,
                    pattern_index: pattern_idx,
                    is_blockable,
                    path: mwp.path.clone(),
                });
            }
        }
        precomputed
    }

    pub fn generate_moves_with_database(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
    ) -> Vec<MoveWithPath> {
        let mut moves = Vec::new();

        let piece = match board.get_piece(from) {
            Some(p) => p,
            None => return moves,
        };
        let color_idx = match piece.color {
            PieceColor::White => 0,
            PieceColor::Black => 1,
        };

        if let Some(piece_moves) = self.move_database.get(&piece_type) {
            let pos_index = from.0 * board.cols() + from.1;
            if let Some(precomputed) = piece_moves[color_idx].get(pos_index) {
                if let Some(cp) = self.compiled_pieces.get(&piece_type) {
                    for pm in precomputed {
                        if let Some(pattern) = cp.moves.get(pm.pattern_index) {
                            if pattern.requires_unmoved && piece.move_count > 0 {
                                continue;
                            }
                            let ok = if pm.is_blockable {
                                self.is_path_valid(board, &pm.path, piece.color, pattern)
                            } else {
                                self.can_land_at(board, pm.destination, piece.color, pattern)
                            };
                            if ok {
                                moves.push(MoveWithPath {
                                    destination: pm.destination,
                                    rule: pattern.clone(),
                                    path: pm.path.clone(),
                                });
                            }
                        }
                    }
                }
            }
        } else {
            return self.generate_moves_with_details(board, from, piece_type);
        }

        moves
    }

    fn can_land_at(
        &self,
        board: &Board,
        dest: Position,
        color: PieceColor,
        pattern: &CompiledMove,
    ) -> bool {
        let victim: Option<Piece> = match board.get_en_passant_target(dest) {
            Some(ep) if ep.capturable_by_all || pattern.captures_en_passant => {
                board.get_piece(ep.piece_position)
            }
            _ => board.get_piece(dest),
        };

        match victim {
            None => pattern.can_land_empty,
            Some(p) if p.color != color => {
                pattern.can_land_enemy && pattern.capture_filter.map_or(true, |t| t == p.piece_type)
            }
            Some(_) => pattern.can_land_friendly,
        }
    }

    pub fn get_castling_pieces(&self, board: &Board, color: PieceColor) -> Vec<(Position, usize)> {
        let mut castling_pieces = Vec::new();
        for (pos, piece) in board.get_pieces_by_color(color) {
            if let Some(cp) = self.compiled_pieces.get(&piece.piece_type) {
                if cp.moves.iter().any(|m| m.is_rook_castle) {
                    castling_pieces.push((pos, piece.piece_type));
                }
            }
        }
        castling_pieces
    }

    pub fn can_piece_castle(&self, board: &Board, pos: Position, piece_type: usize) -> bool {
        let piece = match board.get_piece(pos) {
            Some(p) => p,
            None => return false,
        };
        if let Some(cp) = self.compiled_pieces.get(&piece_type) {
            for pattern in &cp.moves {
                if pattern.is_rook_castle {
                    if pattern.requires_unmoved && piece.move_count > 0 {
                        continue;
                    }
                    return true;
                }
            }
        }
        false
    }

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

        let king_piece = board.get_piece(king_from).unwrap();
        let partners = self.get_castling_pieces(board, king_piece.color);

        let any_marker = king_move.rule.steps.iter().any(|s| s.partner_lands_here);
        let candidates: Vec<Position> = king_move
            .path
            .step_indices
            .iter()
            .enumerate()
            .filter(|&(_, &si)| !any_marker || king_move.rule.steps[si as usize].partner_lands_here)
            .map(|(j, _)| {
                let p = king_move.path.steps[j + 1];
                (p.0 as usize, p.1 as usize)
            })
            .filter(|&sq| sq != king_from && sq != king_to)
            .collect();

        for &sq in &candidates {
            for &(rook_pos, pt) in &partners {
                if !self.can_piece_castle(board, rook_pos, pt) {
                    continue;
                }
                for rm in self.generate_moves_with_details(board, rook_pos, pt) {
                    if rm.destination == sq && rm.rule.is_rook_castle {
                        let ok = match board.get_piece(sq) {
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

    // ────────────────────────────────────────────────────────────────────
    // Notation compiler
    // ────────────────────────────────────────────────────────────────────

    fn compile_moveset(
        moveset: &str,
        cm: &PieceConfigManager,
    ) -> Result<Vec<CompiledMove>, String> {
        moveset
            .split(',')
            .map(|s| Self::compile_single_move(s, cm))
            .collect()
    }

    fn compile_single_move(
        move_str: &str,
        cm: &PieceConfigManager,
    ) -> Result<CompiledMove, String> {
        fn has_more_steps(it: &std::iter::Peekable<std::str::Chars>) -> bool {
            it.clone().any(|c| c == '+' || c == 'x')
        }

        let mut compiled = CompiledMove::default();
        let mut chars = move_str.chars().peekable();

        if move_str.trim() == "?" {
            compiled.steps.push(MoveStep {
                directions: HashSet::from([(0, 0)]),
                length: 1,
                is_optional_stop: true,
                ..Default::default()
            });
            compiled.can_land_friendly = true;
            return Ok(compiled);
        }

        loop {
            if compiled.steps.is_empty() {
                while chars.peek().map_or(false, |c| c.is_whitespace()) {
                    chars.next();
                }
                if chars.peek() == Some(&'[') {
                    chars.next();
                    let mut name = String::new();
                    for c in chars.by_ref() {
                        if c == ']' {
                            break;
                        }
                        name.push(c);
                    }
                    compiled.from_zone = Some(name.trim().to_string());
                }
            }

            while chars.peek().map_or(false, |c| c.is_whitespace()) {
                chars.next();
            }

            let mut step = MoveStep::default();
            let mut add_dirs = HashSet::new();
            let mut sub_dirs = HashSet::new();
            let mut is_subtractive = false;

            loop {
                match chars.peek() {
                    Some('-') => {
                        is_subtractive = true;
                        chars.next();
                    }
                    Some('=') => {
                        step.lock_to_previous_direction = true;
                        chars.next();
                    }
                    Some('^') => {
                        if is_subtractive {
                            sub_dirs.insert((-1, 0));
                        } else {
                            add_dirs.insert((-1, 0));
                        }
                        chars.next();
                    }
                    Some('v') => {
                        if is_subtractive {
                            sub_dirs.insert((1, 0));
                        } else {
                            add_dirs.insert((1, 0));
                        }
                        chars.next();
                    }
                    Some('<') => {
                        if is_subtractive {
                            sub_dirs.insert((0, -1));
                        } else {
                            add_dirs.insert((0, -1));
                        }
                        chars.next();
                    }
                    Some('>') => {
                        if is_subtractive {
                            sub_dirs.insert((0, 1));
                        } else {
                            add_dirs.insert((0, 1));
                        }
                        chars.next();
                    }
                    Some(c) if c.is_ascii_digit() => {
                        let mut num_str = String::new();
                        while let Some(d) = chars.peek() {
                            if d.is_ascii_digit() {
                                num_str.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                        step.length = num_str.parse().unwrap_or(1);
                    }
                    _ => break,
                }
            }

            let base_move = match chars.peek() {
                Some(c) if *c == '+' || *c == 'x' => chars.next().unwrap(),
                _ => break,
            };
            let base_dirs = if base_move == '+' {
                vec![(0, 1), (1, 0), (0, -1), (-1, 0)]
            } else {
                vec![(1, 1), (1, -1), (-1, 1), (-1, -1)]
            };

            for dir in base_dirs {
                let is_included = add_dirs.is_empty()
                    || add_dirs
                        .iter()
                        .any(|ad| (ad.0 == 0 || ad.0 == dir.0) && (ad.1 == 0 || ad.1 == dir.1));
                let is_excluded = sub_dirs
                    .iter()
                    .any(|sd| (sd.0 == 0 || sd.0 == dir.0) && (sd.1 == 0 || sd.1 == dir.1));
                if is_included && !is_excluded {
                    step.directions.insert(dir);
                }
            }

            let mut pass_perms_overridden = false;
            let mut rep_perms_overridden = false;

            loop {
                match chars.peek() {
                    Some('?') => {
                        step.is_optional_stop = true;
                        chars.next();
                    }
                    Some('#') => {
                        step.resets_center = true;
                        chars.next();
                    }
                    Some('e') => {
                        step.creates_en_passant = Some(false);
                        chars.next();
                    }
                    Some('E') => {
                        step.creates_en_passant = Some(true);
                        chars.next();
                    }
                    Some('&') => {
                        step.partner_lands_here = true;
                        chars.next();
                    }
                    Some('*') => {
                        step.is_repeatable = true;
                        chars.next();
                        let mut num_str = String::new();
                        while let Some(d) = chars.peek() {
                            if d.is_ascii_digit() {
                                num_str.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                        step.max_repetitions = num_str.parse().ok();

                        while let Some(&c) = chars.peek() {
                            if c == '_' || c == '!' || c == '@' {
                                if !rep_perms_overridden {
                                    step.repetition_permissions = Permissions::none();
                                    rep_perms_overridden = true;
                                }
                                match c {
                                    '_' => step.repetition_permissions.can_pass_empty = true,
                                    '!' => step.repetition_permissions.can_pass_enemy = true,
                                    '@' => step.repetition_permissions.can_pass_friendly = true,
                                    _ => {}
                                }
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    Some('_') | Some('!') | Some('@') => {
                        let mut it = chars.clone();
                        it.next();
                        if has_more_steps(&it) || it.peek() == Some(&'*') {
                            if !pass_perms_overridden {
                                step.permissions = Permissions::none();
                                pass_perms_overridden = true;
                            }
                            match chars.next().unwrap() {
                                '_' => step.permissions.can_pass_empty = true,
                                '!' => step.permissions.can_pass_enemy = true,
                                '@' => step.permissions.can_pass_friendly = true,
                                _ => {}
                            }
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            compiled.steps.push(step);
        }

        let mut landing_permissions_overridden = false;
        while let Some(suffix) = chars.peek().cloned() {
            match suffix {
                'u' => {
                    compiled.requires_unmoved = true;
                    chars.next();
                }
                'i' => {
                    compiled.is_irreversible = true;
                    chars.next();
                }
                '~' => {
                    compiled.captures_en_passant = true;
                    chars.next();
                }
                'o' => {
                    compiled.is_king_castle = true;
                    chars.next();
                }
                'O' => {
                    compiled.is_rook_castle = true;
                    chars.next();
                }
                '!' | '_' | '@' => {
                    if !landing_permissions_overridden {
                        compiled.can_land_empty = false;
                        compiled.can_land_enemy = false;
                        compiled.can_land_friendly = false;
                        landing_permissions_overridden = true;
                    }
                    let c = chars.next().unwrap();
                    match c {
                        '!' => {
                            compiled.can_land_enemy = true;
                            if chars.peek() == Some(&'{') {
                                chars.next();
                                let mut name = String::new();
                                for ch in chars.by_ref() {
                                    if ch == '}' {
                                        break;
                                    }
                                    name.push(ch);
                                }
                                let name = name.trim();
                                compiled.capture_filter = resolve_piece_ref(name, cm);
                                if compiled.capture_filter.is_none() {
                                    return Err(format!(
                                        "capture filter !{{{}}}: unknown piece",
                                        name
                                    ));
                                }
                            }
                        }
                        '_' => compiled.can_land_empty = true,
                        '@' => compiled.can_land_friendly = true,
                        _ => {}
                    }
                }
                '[' => {
                    chars.next();
                    let mut name = String::new();
                    for c in chars.by_ref() {
                        if c == ']' {
                            break;
                        }
                        name.push(c);
                    }
                    compiled.to_zone = Some(name.trim().to_string());
                }
                c if c.is_whitespace() => {
                    chars.next();
                }
                _ => break,
            }
        }
        compiled.has_zones = compiled.from_zone.is_some() || compiled.to_zone.is_some();
        Ok(compiled)
    }

    pub fn get_move_rule(
        &self,
        board: &Board,
        from: Position,
        to: Position,
        piece_type: usize,
    ) -> Option<MoveWithPath> {
        self.generate_moves_with_database(board, from, piece_type)
            .into_iter()
            .find(|m| m.destination == to)
    }

    // ────────────────────────────────────────────────────────────────────
    // Live (board-aware) tracer
    // ────────────────────────────────────────────────────────────────────

    pub fn generate_moves_with_details(
        &self,
        board: &Board,
        from: Position,
        piece_type: usize,
    ) -> Vec<MoveWithPath> {
        let mut final_moves = Vec::new();
        let moving_piece = match board.get_piece(from) {
            Some(p) => p,
            None => return final_moves,
        };

        if let Some(cp) = self.compiled_pieces.get(&piece_type) {
            for pattern in &cp.moves {
                if pattern.requires_unmoved && moving_piece.move_count > 0 {
                    continue;
                }
                if !self.pattern_from_ok(pattern, from, moving_piece.color) {
                    continue;
                }

                let mut buf = Vec::new();
                self.trace_path_with_tracking(
                    board,
                    from,
                    from,
                    moving_piece.color,
                    pattern,
                    &pattern.steps,
                    None,
                    smallvec![(from.0 as u8, from.1 as u8)],
                    smallvec![],
                    &mut buf,
                );

                if pattern.to_zone.is_some() {
                    buf.retain(|m| self.pattern_to_ok(pattern, m.destination, moving_piece.color));
                }
                final_moves.extend(buf);
            }
        }
        final_moves
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_path_with_tracking(
        &self,
        board: &Board,
        direction_origin: Position,
        current_pos: Position,
        color: PieceColor,
        pattern: &Arc<CompiledMove>,
        remaining_steps: &[MoveStep],
        prev_dir: Option<(i8, i8)>,
        current_path: SmallVec<[(u8, u8); 10]>,
        current_step_indices: SmallVec<[u8; 10]>,
        moves: &mut Vec<MoveWithPath>,
    ) {
        if remaining_steps.is_empty() {
            return;
        }

        let step = &remaining_steps[0];
        let step_index = pattern.steps.len() - remaining_steps.len();
        let is_last_step = remaining_steps.len() == 1;
        let y_axis_multiplier = if color == PieceColor::White { 1 } else { -1 };
        let (rows, cols) = board.size();

        for &dir in &step.directions {
            let delta = (dir.0 * y_axis_multiplier, dir.1);
            if Self::skip_direction(step, delta, direction_origin, current_pos, prev_dir) {
                continue;
            }

            let repetitions = if step.is_repeatable {
                step.max_repetitions.unwrap_or(usize::MAX)
            } else {
                1
            };

            let mut last_pos_in_repetition = current_pos;
            let mut accum_path = current_path.clone();
            let mut accum_indices = current_step_indices.clone();

            'rep_loop: for _ in 1..=repetitions {
                let mut dest = last_pos_in_repetition;

                for k in 1..=step.length {
                    let r = last_pos_in_repetition.0 as i32 + delta.0 as i32 * k as i32;
                    let c = last_pos_in_repetition.1 as i32 + delta.1 as i32 * k as i32;
                    if !(0..rows as i32).contains(&r) || !(0..cols as i32).contains(&c) {
                        break 'rep_loop;
                    }
                    dest = (r as usize, c as usize);
                    accum_path.push((dest.0 as u8, dest.1 as u8));
                    accum_indices.push(step_index as u8);

                    if k < step.length {
                        let can_pass = match board.get_piece(dest) {
                            None => step.permissions.can_pass_empty,
                            Some(p) if p.color != color => step.permissions.can_pass_enemy,
                            Some(_) => step.permissions.can_pass_friendly,
                        };
                        if !can_pass {
                            break 'rep_loop;
                        }
                    }
                }

                if step.is_optional_stop && self.can_land_at(board, dest, color, pattern) {
                    moves.push(MoveWithPath {
                        destination: dest,
                        rule: pattern.clone(),
                        path: MovePath {
                            steps: accum_path.clone(),
                            step_indices: accum_indices.clone(),
                        },
                    });
                }

                if is_last_step {
                    if self.can_land_at(board, dest, color, pattern) {
                        moves.push(MoveWithPath {
                            destination: dest,
                            rule: pattern.clone(),
                            path: MovePath {
                                steps: accum_path.clone(),
                                step_indices: accum_indices.clone(),
                            },
                        });
                    }
                    if step.is_repeatable {
                        let can_repeat = match board.get_piece(dest) {
                            None => step.repetition_permissions.can_pass_empty,
                            Some(p) if p.color != color => {
                                step.repetition_permissions.can_pass_enemy
                            }
                            Some(_) => step.repetition_permissions.can_pass_friendly,
                        };
                        if !can_repeat {
                            break 'rep_loop;
                        }
                    }
                } else {
                    let can_continue = match board.get_piece(dest) {
                        None => step.permissions.can_pass_empty,
                        Some(p) if p.color != color => step.permissions.can_pass_enemy,
                        Some(_) => step.permissions.can_pass_friendly,
                    };

                    if can_continue {
                        let next_origin = if step.resets_center {
                            dest
                        } else {
                            direction_origin
                        };
                        self.trace_path_with_tracking(
                            board,
                            next_origin,
                            dest,
                            color,
                            pattern,
                            &remaining_steps[1..],
                            Some(delta),
                            accum_path.clone(),
                            accum_indices.clone(),
                            moves,
                        );
                    }

                    if step.is_repeatable {
                        let can_repeat = match board.get_piece(dest) {
                            None => step.repetition_permissions.can_pass_empty,
                            Some(p) if p.color != color => {
                                step.repetition_permissions.can_pass_enemy
                            }
                            Some(_) => step.repetition_permissions.can_pass_friendly,
                        };
                        if !can_repeat {
                            break 'rep_loop;
                        }
                    }
                }
                last_pos_in_repetition = dest;
            }
        }
    }
}

fn resolve_piece_ref(s: &str, cm: &PieceConfigManager) -> Option<usize> {
    if s.chars().count() == 1 {
        let up = s.to_ascii_uppercase();
        for (idx, key) in cm.piece_order.iter().enumerate() {
            if let Some(pc) = cm.pieces.get(key) {
                if pc.characters.iter().any(|c| c == &up) {
                    return Some(idx);
                }
            }
        }
    }
    cm.get_piece_index(s).or_else(|| {
        cm.piece_order.iter().position(|k| {
            cm.pieces
                .get(k)
                .map_or(false, |pc| pc.display_name.eq_ignore_ascii_case(s))
        })
    })
}
