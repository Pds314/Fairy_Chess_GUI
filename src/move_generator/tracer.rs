//! Unified recursive path tracer.
//!
//! A single generic `trace_path<H>` replaces the three former
//! copy-paste-modify functions. The compiler monomorphizes each
//! `TraceHandler` implementation to produce the same machine code
//! as the hand-duplicated originals.

use crate::core::board::Board;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::move_generator::types::*;
use smallvec::SmallVec;
use std::sync::Arc;

// ─── Handler trait ──────────────────────────────────────────────────────

pub(crate) trait TraceHandler {
    fn board_size(&self) -> (usize, usize);
    /// Can the piece pass through an intermediate cell within a
    /// multi-cell step (k < step.length)?
    fn can_pass_intermediate(&self, pos: Position, step: &MoveStep, color: PieceColor) -> bool;
    /// Should a move to `dest` be emitted (optional-stop or final)?
    fn should_emit(&self, dest: Position, pattern: &CompiledMove, color: PieceColor) -> bool;
    /// Can the piece continue from `dest` to the next step?
    fn can_continue(&self, dest: Position, step: &MoveStep, color: PieceColor) -> bool;
    /// Can a repeatable step continue past `dest`?
    fn can_repeat(&self, dest: Position, step: &MoveStep, color: PieceColor) -> bool;
}

// ─── Concrete handlers ──────────────────────────────────────────────────

/// Theoretical (PST generation): no board, emit everything.
pub(crate) struct GeometricHandler {
    pub size: (usize, usize),
}

impl TraceHandler for GeometricHandler {
    #[inline(always)] fn board_size(&self) -> (usize, usize) { self.size }
    #[inline(always)] fn can_pass_intermediate(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
    #[inline(always)] fn should_emit(&self, _: Position, _: &CompiledMove, _: PieceColor) -> bool { true }
    #[inline(always)] fn can_continue(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
    #[inline(always)] fn can_repeat(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
}

/// Precompute: no board, but only emit if the pattern's landing flags
/// allow *some* kind of landing (filters degenerate patterns).
pub(crate) struct PrecomputeHandler {
    pub size: (usize, usize),
}

impl TraceHandler for PrecomputeHandler {
    #[inline(always)] fn board_size(&self) -> (usize, usize) { self.size }
    #[inline(always)] fn can_pass_intermediate(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
    #[inline(always)]
    fn should_emit(&self, _: Position, pattern: &CompiledMove, _: PieceColor) -> bool {
        pattern.can_land_empty || pattern.can_land_enemy || pattern.can_land_friendly
    }
    #[inline(always)] fn can_continue(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
    #[inline(always)] fn can_repeat(&self, _: Position, _: &MoveStep, _: PieceColor) -> bool { true }
}

/// Live (board-aware): checks board occupancy for all decisions.
pub(crate) struct LiveHandler<'a> {
    pub board: &'a Board,
}

impl TraceHandler for LiveHandler<'_> {
    #[inline(always)] fn board_size(&self) -> (usize, usize) { self.board.size() }

    #[inline]
    fn can_pass_intermediate(&self, pos: Position, step: &MoveStep, color: PieceColor) -> bool {
        match self.board.get_piece(pos) {
            None => step.permissions.can_pass_empty,
            Some(p) if p.color != color => step.permissions.can_pass_enemy,
            Some(_) => step.permissions.can_pass_friendly,
        }
    }

    #[inline]
    fn should_emit(&self, dest: Position, pattern: &CompiledMove, color: PieceColor) -> bool {
        can_land_at(self.board, dest, color, pattern)
    }

    #[inline]
    fn can_continue(&self, dest: Position, step: &MoveStep, color: PieceColor) -> bool {
        match self.board.get_piece(dest) {
            None => step.permissions.can_pass_empty,
            Some(p) if p.color != color => step.permissions.can_pass_enemy,
            Some(_) => step.permissions.can_pass_friendly,
        }
    }

    #[inline]
    fn can_repeat(&self, dest: Position, step: &MoveStep, color: PieceColor) -> bool {
        match self.board.get_piece(dest) {
            None => step.repetition_permissions.can_pass_empty,
            Some(p) if p.color != color => step.repetition_permissions.can_pass_enemy,
            Some(_) => step.repetition_permissions.can_pass_friendly,
        }
    }
}

// ─── Direction filter ───────────────────────────────────────────────────

#[inline]
pub(crate) fn skip_direction(
    step: &MoveStep,
    delta: (i8, i8),
    direction_origin: Position,
    current_pos: Position,
    prev_dir: Option<(i8, i8)>,
) -> bool {
    if step.lock_to_previous_direction {
        if let Some(pd) = prev_dir {
            if delta != pd { return true; }
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

// ─── Unified recursive trace ────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn trace_path<H: TraceHandler>(
    handler: &H,
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
    let y_mul = if color == PieceColor::White { 1i8 } else { -1 };
    let (rows, cols) = handler.board_size();

    for &dir in step.directions.iter() {
        let delta = (dir.0 * y_mul, dir.1);
        if skip_direction(step, delta, direction_origin, current_pos, prev_dir) {
            continue;
        }

        let repetitions = if step.is_repeatable {
            step.max_repetitions.unwrap_or(usize::MAX)
        } else {
            1
        };

        let mut last_pos = current_pos;
        let mut accum_path = current_path.clone();
        let mut accum_indices = current_step_indices.clone();

        'rep_loop: for _ in 1..=repetitions {
            let mut dest = last_pos;
            for k in 1..=step.length {
                let r = last_pos.0 as i32 + delta.0 as i32 * k as i32;
                let c = last_pos.1 as i32 + delta.1 as i32 * k as i32;
                if !(0..rows as i32).contains(&r) || !(0..cols as i32).contains(&c) {
                    break 'rep_loop;
                }
                dest = (r as usize, c as usize);
                accum_path.push((dest.0 as u8, dest.1 as u8));
                accum_indices.push(step_index as u8);

                if k < step.length && !handler.can_pass_intermediate(dest, step, color) {
                    break 'rep_loop;
                }
            }

            // Optional stop
            if step.is_optional_stop && handler.should_emit(dest, pattern, color) {
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
                if handler.should_emit(dest, pattern, color) {
                    moves.push(MoveWithPath {
                        destination: dest,
                        rule: pattern.clone(),
                        path: MovePath {
                            steps: accum_path.clone(),
                            step_indices: accum_indices.clone(),
                        },
                    });
                }
                if step.is_repeatable && !handler.can_repeat(dest, step, color) {
                    break 'rep_loop;
                }
            } else {
                if handler.can_continue(dest, step, color) {
                    let next_origin = if step.resets_center { dest } else { direction_origin };
                    trace_path(
                        handler,
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
                if step.is_repeatable && !handler.can_repeat(dest, step, color) {
                    break 'rep_loop;
                }
            }

            last_pos = dest;
        }
    }
}
