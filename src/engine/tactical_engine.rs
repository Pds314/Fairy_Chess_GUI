// src/engine/tactical_engine.rs
//
// Priority engine: mate > check > capture > close on the enemy royals.
//
// Rewritten to do exactly ONE make/unmake per candidate move. The previous
// version cloned the entire `GameState` three times per candidate (once for
// the mate probe, once for the check probe, once for the distance probe) and
// located royal pieces by scanning the board and consulting the
// `PieceConfigManager`. `Board` tracks royal/royalty positions incrementally.

use crate::core::game_state::{ExpandedMove, MateStatus};
use crate::core::{GameState, PieceColor, Position};
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};

pub struct TacticalEngine;

impl TacticalEngine {
    pub fn new() -> Self {
        TacticalEngine
    }
}

impl Default for TacticalEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct Probe {
    mate: bool,
    check: bool,
    dist: f64,
}

fn result_of(mv: &ExpandedMove, score: i32, mate_in: Option<i32>) -> SearchResult {
    SearchResult {
        best_move: mv.clone(),
        evaluation: Evaluation { score, mate_in },
        depth_reached: 1,
    }
}

/// Royal + royalty squares, straight out of `Board`'s incremental lists.
fn protected_of(state: &GameState, color: PieceColor) -> Vec<Position> {
    let b = &state.board;
    let mut v = b.get_royal_positions(color).to_vec();
    v.extend_from_slice(b.get_royalty_positions(color));
    v
}

/// Total squared distance from our pieces to the enemy royal centroid.
/// Called with the board in the *post-move* state, so `state.current_turn`
/// is the opponent.
fn distance_score(state: &GameState) -> f64 {
    let enemy = state.current_turn;
    let ours = enemy.opposite();

    let royals = protected_of(state, enemy);
    if royals.is_empty() {
        return f64::MAX;
    }

    let n = royals.len() as f64;
    let ar = royals.iter().map(|p| p.0 as f64).sum::<f64>() / n;
    let ac = royals.iter().map(|p| p.1 as f64).sum::<f64>() / n;

    let (rows, cols) = state.board.size();
    let mut total = 0.0;
    for r in 0..rows {
        for c in 0..cols {
            if let Some(p) = state.board.get_piece((r, c)) {
                if p.color == ours {
                    let dr = r as f64 - ar;
                    let dc = c as f64 - ac;
                    total += dr * dr + dc * dc;
                }
            }
        }
    }
    total
}

impl ChessEngine for TacticalEngine {
    fn name(&self) -> &str {
        "Tactical Priority Engine (Mate/Check/Capture/Push)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let SearchParams {
            state,
            move_generator,
            config_manager,
            ..
        } = params;

        let legal = state.get_legal_moves(move_generator, config_manager);
        if legal.is_empty() {
            return None;
        }

        let mut probes: Vec<Probe> = Vec::with_capacity(legal.len());
        for mv in &legal {
            state.execute_expanded_move(mv, move_generator, config_manager);
            let mate = matches!(
                state.get_mate_status(move_generator, config_manager),
                                MateStatus::Checkmate
            );
            let check = !mate && state.is_in_check_fast(move_generator);
            let dist = distance_score(state);
            state.undo_move(config_manager);
            probes.push(Probe { mate, check, dist });
        }

        if let Some(i) = probes.iter().position(|p| p.mate) {
            return Some(result_of(&legal[i], 999_999, Some(1)));
        }
        if let Some(i) = probes.iter().position(|p| p.check) {
            return Some(result_of(&legal[i], 1000, None));
        }
        if let Some(i) = legal.iter().position(|m| m.captures.is_some()) {
            return Some(result_of(&legal[i], 500, None));
        }

        let (i, best) = probes
        .iter()
        .enumerate()
        .min_by(|a, b| {
            a.1.dist
            .partial_cmp(&b.1.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        let score = if best.dist.is_finite() && best.dist < i32::MAX as f64 {
            -(best.dist as i32)
        } else {
            0
        };
        Some(result_of(&legal[i], score, None))
    }

    fn stop(&mut self) {}
}
