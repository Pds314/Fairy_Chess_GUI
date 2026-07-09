// src/engine/pure_policy_engine.rs
//
// A pure policy mover: selects moves by statically scoring each legal move,
// with no game-tree search, no make/unmake, and no lookahead. Every feature
// is computed from the current board state and the move's from/to/captures
// metadata.
//
// The engine is variant-agnostic. It derives piece values from average
// mobility, computes board geometry from the actual board size, and treats
// royalty/promotion via the PieceConfigManager's property flags rather than
// hardcoded piece names.
//
// The core tactical primitive is Static Exchange Evaluation (SEE), which
// resolves capture sequences on a single square arithmetically, using the
// precomputed attacker/defender lists from the reverse move database. SEE is
// not search: it considers only recaptures on one square, sorted by value,
// and runs the minimax backward pass over the gain array in O(n).

use crate::core::board::Board;
use crate::core::game_state::{ExpandedMove, GameState};
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::engine::api::{ChessEngine, Evaluation, SearchParams, SearchResult};
use crate::engine::parameters::{EngineParameters, ParameterDef};
use crate::move_generator::MoveGenerator;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Parameter identifiers & definitions
// ─────────────────────────────────────────────────────────────────────────────

// Material & tactics
const P_SEE: &str = "see_weight";
const P_RESCUE: &str = "rescue_weight";
const P_DEFEND: &str = "defend_hanging_weight";

// King interaction
const P_CHECK: &str = "check_weight";
const P_ROYAL_PROX: &str = "royal_proximity_weight";
const P_OWN_KING_EXPOSURE: &str = "own_king_exposure_weight";

// Positional
const P_MOBILITY: &str = "mobility_weight";
const P_CENTER: &str = "center_weight";
const P_ADVANCE: &str = "advance_weight";
const P_THREAT: &str = "threat_weight";

// Special moves
const P_PROMO: &str = "promotion_weight";
const P_CASTLE: &str = "castle_weight";

// Tie-breaking noise (keeps the engine from being boringly deterministic
// when many moves score identically — common in openings)
const P_NOISE: &str = "noise_weight";

pub static PURE_POLICY_PARAMETERS: &[ParameterDef] = &[
    ParameterDef::new(
        P_SEE,
        "SEE (Exchange) Weight",
        "Weight for static exchange evaluation. This is the primary tactical signal: positive SEE means winning material, negative means losing it. Scale this up and the engine becomes a material grabber; scale it down and it plays for position over material.",
        0.0,
        200.0,
        100.0,
        5.0,
    ),
    ParameterDef::new(
        P_RESCUE,
        "Hanging Piece Rescue",
        "Urgency bonus for moving a piece that is currently attacked more than it is defended. This is what makes the engine notice 'oh no my queen is hanging.'",
        0.0,
        150.0,
        80.0,
        5.0,
    ),
    ParameterDef::new(
        P_DEFEND,
        "Defend Hanging Ally",
        "Bonus for moving to a square that defends a currently-hanging friendly piece. Lower than rescue because defending is less reliable than moving away.",
        0.0,
        100.0,
        30.0,
        5.0,
    ),
    ParameterDef::new(
        P_CHECK,
        "Check Delivery",
        "Flat bonus for delivering check. Variant-agnostic: checks are good because they constrain the opponent's reply, regardless of what pieces exist.",
        0.0,
        100.0,
        35.0,
        5.0,
    ),
    ParameterDef::new(
        P_ROYAL_PROX,
        "Royal Proximity",
        "Bonus per square of approach toward the nearest enemy royal. Drives king-hunting in endgames and piece coordination around the enemy king in middlegames.",
        0.0,
        30.0,
        4.0,
        0.5,
    ),
    ParameterDef::new(
        P_OWN_KING_EXPOSURE,
        "Own King Exposure Penalty",
        "Penalty for moves that reduce the number of defenders around our own royal. Keeps the engine from stripping its king naked.",
        0.0,
        50.0,
        15.0,
        2.0,
    ),
    ParameterDef::new(
        P_MOBILITY,
        "Mobility Delta",
        "Weight for the change in the moving piece's theoretical mobility (moves from destination minus moves from origin). Rewards developing pieces to active squares.",
        0.0,
        10.0,
        1.5,
        0.1,
    ),
    ParameterDef::new(
        P_CENTER,
        "Centralization",
        "Weight for moving toward the geometric center of the board. Universal heuristic — central pieces control more squares regardless of variant.",
        0.0,
        10.0,
        2.0,
        0.1,
    ),
    ParameterDef::new(
        P_ADVANCE,
        "Advancement",
        "Weight for moving toward the enemy side of the board. Drives pawn pushes, piece activity, and promotion races.",
        0.0,
        10.0,
        1.0,
        0.1,
    ),
    ParameterDef::new(
        P_THREAT,
        "Threat Creation",
        "Weight for the total value of enemy pieces newly attacked from the destination square. Rewards forks and aggressive piece placement.",
        0.0,
        50.0,
        8.0,
        1.0,
    ),
    ParameterDef::new(
        P_PROMO,
        "Promotion Bonus",
        "Flat bonus for moves that promote, on top of the promotion piece's derived value.",
        0.0,
        200.0,
        60.0,
        5.0,
    ),
    ParameterDef::new(
        P_CASTLE,
        "Castling Bonus",
        "Flat bonus for castling. Castling is almost always correct early; this bonus decays as the game goes on via the move count.",
        0.0,
        100.0,
        40.0,
        5.0,
    ),
    ParameterDef::new(
        P_NOISE,
        "Tie-Break Noise",
        "Magnitude of random noise added to each move's score. At zero the engine is deterministic; small values break ties pseudo-randomly without disturbing clearly-better moves.",
        0.0,
        5.0,
        0.5,
        0.1,
    ),
];

// ─────────────────────────────────────────────────────────────────────────────
// Per-variant derived data
// ─────────────────────────────────────────────────────────────────────────────

/// Data computed once per variant (board size + piece set) and cached.
/// Wiped on `reset_cache()`.
struct VariantCache {
    /// board_size this cache was built for
    board_size: (usize, usize),

    /// Derived piece values, indexed by piece_type. Computed as average
    /// theoretical mobility across all squares on an empty board. This gives
    /// roughly correct *ratios* — a piece that can reach 4× as many squares
    /// is worth about 4× as much — which is what SEE needs.
    piece_values: HashMap<usize, f64>,

    /// Sentinel value for royal pieces in SEE: effectively infinite, so the
    /// exchange always terminates before trading a royal. Set to 100× the
    /// most valuable non-royal piece.
    royal_value: f64,

    /// Smallest non-zero piece value. Used as the unit for scoring.
    unit_value: f64,

    /// Geometric center of the board, for centralization scoring.
    center: (f64, f64),

    /// Max Chebyshev distance from center to any corner. Used to normalize
    /// centralization deltas to roughly [-1, 1].
    center_norm: f64,
}

impl VariantCache {
    fn build(
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let mut piece_values: HashMap<usize, f64> = HashMap::new();
        let squares = (board_size.0 * board_size.1) as f64;

        // Derive piece values from average mobility on an empty board.
        // We use White's perspective — directional asymmetries in piece
        // movement (like pawns) average out, and for SEE we only care about
        // relative magnitudes anyway.
        for (idx, name) in config_manager.piece_order.iter().enumerate() {
            if config_manager.pieces.get(name).is_none() {
                continue;
            }

            let mut total_mobility = 0.0;
            for r in 0..board_size.0 {
                for c in 0..board_size.1 {
                    // move_count=1 means "treat as having moved" — this
                    // excludes one-shot moves like the pawn double-step,
                    // which we don't want inflating pawn values.
                    let moves = move_generator.generate_theoretical_moves_for_pst(
                        (r, c),
                        idx,
                        PieceColor::White,
                        board_size,
                        1,
                    );
                    // Count only moves that can actually capture — a
                    // piece's *fighting* value is what matters for exchanges.
                    // (A piece that can only move to empty squares has zero
                    // trade value.) Fall back to total mobility if the piece
                    // has no capturing moves at all.
                    let capturing: usize = moves.iter().filter(|m| m.rule.can_land_enemy).count();
                    total_mobility += if capturing > 0 {
                        capturing as f64
                    } else {
                        moves.len() as f64 * 0.3
                    };
                }
            }

            let avg = if squares > 0.0 {
                total_mobility / squares
            } else {
                1.0
            };
            piece_values.insert(idx, avg.max(0.1)); // floor at 0.1 to avoid zeros
        }

        // Find the scale: smallest and largest non-royal values.
        let mut min_nonroyal = f64::INFINITY;
        let mut max_nonroyal: f64 = 0.0;
        for (&pt, &v) in &piece_values {
            let is_royal = config_manager
                .get_piece_by_index(pt)
                .map_or(false, |c| c.properties.is_royal || c.properties.is_royalty);
            if !is_royal {
                min_nonroyal = min_nonroyal.min(v);
                max_nonroyal = max_nonroyal.max(v);
            }
        }
        if !min_nonroyal.is_finite() {
            // Degenerate case: every piece is royal. Fall back to raw values.
            min_nonroyal = piece_values.values().cloned().fold(f64::INFINITY, f64::min);
            max_nonroyal = piece_values.values().cloned().fold(0.0_f64, f64::max);
        }
        if !min_nonroyal.is_finite() || min_nonroyal <= 0.0 {
            min_nonroyal = 1.0;
        }
        if max_nonroyal <= 0.0 {
            max_nonroyal = 1.0;
        }

        let center = (
            (board_size.0 as f64 - 1.0) * 0.5,
            (board_size.1 as f64 - 1.0) * 0.5,
        );
        let center_norm = center.0.max(center.1).max(1.0);

        Self {
            board_size,
            piece_values,
            royal_value: max_nonroyal * 100.0,
            unit_value: min_nonroyal,
            center,
            center_norm,
        }
    }

    fn value_of(&self, pt: usize, is_royal: bool) -> f64 {
        if is_royal {
            self.royal_value
        } else {
            *self.piece_values.get(&pt).unwrap_or(&self.unit_value)
        }
    }

    #[inline]
    fn chebyshev(a: Position, b: Position) -> f64 {
        let dr = (a.0 as i32 - b.0 as i32).abs();
        let dc = (a.1 as i32 - b.1 as i32).abs();
        dr.max(dc) as f64
    }

    #[inline]
    fn dist_to_center(&self, p: Position) -> f64 {
        let dr = (p.0 as f64 - self.center.0).abs();
        let dc = (p.1 as f64 - self.center.1).abs();
        dr.max(dc)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-position context (things we compute once and reuse across all moves)
// ─────────────────────────────────────────────────────────────────────────────

/// State shared across scoring all moves in a single position. Building this
/// once avoids redundant attacker-list queries.
struct PositionContext {
    /// For every square holding a piece, the SEE result if that piece is
    /// captured right now. Negative means the piece is safely defended;
    /// positive means it's hanging (or under-defended). We index by the
    /// flattened square index.
    hanging_value: Vec<f64>,

    /// Squares adjacent to (and including) each of our royals. We use this
    /// to detect moves that strip defenders from the king zone.
    our_royal_zone: Vec<Position>,

    /// Enemy royal positions, for proximity scoring.
    enemy_royals: Vec<Position>,

    /// Move number, for decaying the castling bonus.
    move_count: usize,
}

impl PositionContext {
    fn build(
        state: &GameState,
        cache: &VariantCache,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let board = &state.board;
        let us = state.current_turn;
        let them = us.opposite();
        let (rows, cols) = board.size();

        // Precompute hanging status for every occupied square.
        let mut hanging_value = vec![0.0; rows * cols];
        for r in 0..rows {
            for c in 0..cols {
                let pos = (r, c);
                if let Some(piece) = board.get_piece(pos) {
                    // SEE from the perspective of the *attacker* of this
                    // piece. If positive, this piece is losing material if
                    // captured — i.e., it's hanging.
                    let attacker_color = piece.color.opposite();
                    let victim_val = cache.value_of(
                        piece.piece_type,
                        is_royal_piece(piece.piece_type, config_manager),
                    );
                    hanging_value[r * cols + c] = see(
                        board,
                        pos,
                        attacker_color,
                        victim_val,
                        None,
                        cache,
                        move_generator,
                        config_manager,
                    );
                }
            }
        }

        // Build our king zone: every royal square plus its neighbours.
        let mut our_royal_zone = Vec::new();
        for &rp in board.get_royal_positions(us) {
            for dr in -1i32..=1 {
                for dc in -1i32..=1 {
                    let nr = rp.0 as i32 + dr;
                    let nc = rp.1 as i32 + dc;
                    if nr >= 0 && nc >= 0 && (nr as usize) < rows && (nc as usize) < cols {
                        let p = (nr as usize, nc as usize);
                        if !our_royal_zone.contains(&p) {
                            our_royal_zone.push(p);
                        }
                    }
                }
            }
        }

        let enemy_royals: Vec<Position> = board.get_royal_positions(them).to_vec();

        Self {
            hanging_value,
            our_royal_zone,
            enemy_royals,
            move_count: state.move_history.len(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Static Exchange Evaluation
// ─────────────────────────────────────────────────────────────────────────────

/// Compute the material outcome of the best capture sequence on `square`,
/// starting with `side` capturing a piece worth `initial_victim_value`.
///
/// Returns the net material swing from `side`'s perspective. Positive means
/// `side` wins material; zero means even trade; negative means `side` loses.
///
/// If `ignore_attacker` is Some(pos), that square is excluded from the
/// attacker lists — used when scoring a move, to avoid counting the moving
/// piece as defending its own destination.
///
/// This is NOT search. It's arithmetic on precomputed attacker lists. The
/// board is never modified. Complexity is O(A log A) where A is the total
/// number of attackers on the square.
fn see(
    board: &Board,
    square: Position,
    side: PieceColor,
    initial_victim_value: f64,
    ignore_attacker: Option<Position>,
    cache: &VariantCache,
    move_generator: &MoveGenerator,
    config_manager: &PieceConfigManager,
) -> f64 {
    // Gather both sides' attackers, sorted cheapest-first (you always
    // recapture with your least valuable piece).
    let mut gather = |color: PieceColor| -> Vec<f64> {
        let mut vals: Vec<f64> = move_generator
            .get_attackers_to_square(board, square, color)
            .into_iter()
            .filter(|(pos, _)| Some(*pos) != ignore_attacker)
            .map(|(_, piece)| {
                cache.value_of(
                    piece.piece_type,
                    is_royal_piece(piece.piece_type, config_manager),
                )
            })
            .collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        vals
    };

    let mut attackers = [gather(PieceColor::White), gather(PieceColor::Black)];

    // If the side to move has no attacker, there's no capture to evaluate.
    if attackers[side.index()].is_empty() {
        return 0.0;
    }

    // Build the gain array. gains[0] is what you win by making the initial
    // capture. Each subsequent entry is what the *other* side would then win
    // by recapturing, given the piece that just moved onto the square.
    //
    // Using a fixed-size array here to stay allocation-free in the hot path.
    // 32 is generous — even with batteries of sliders, exchanges this deep
    // are essentially nonexistent.
    const MAX_SEE_DEPTH: usize = 32;
    let mut gains = [0.0_f64; MAX_SEE_DEPTH];
    let mut d = 0;

    gains[0] = initial_victim_value;
    let mut on_square = attackers[side.index()].remove(0); // cheapest attacker moves onto the square
    let mut to_move = side.opposite();

    loop {
        d += 1;
        if d >= MAX_SEE_DEPTH {
            break;
        }

        // Check the standing-pat condition: if the previous gain is already
        // negative even if we capture now, the opponent won't bother.
        // (This is the classic SEE short-circuit.)
        gains[d] = on_square - gains[d - 1];
        if gains[d].max(-gains[d - 1]) < 0.0 {
            // The side to move can decline and keep the opponent's loss.
            // But we still need to fill this slot for the backward pass.
        }

        let idx = to_move.index();
        if attackers[idx].is_empty() {
            break;
        }

        // Pop the cheapest attacker of the side to move.
        on_square = attackers[idx].remove(0);
        to_move = to_move.opposite();
    }

    // Minimax backward: each side gets to choose whether to continue the
    // exchange or stand pat with the previous result.
    for i in (1..=d).rev() {
        gains[i - 1] = (-gains[i]).max(gains[i - 1]);
    }

    gains[0]
}

#[inline]
fn is_royal_piece(piece_type: usize, config_manager: &PieceConfigManager) -> bool {
    config_manager
        .get_piece_by_index(piece_type)
        .map_or(false, |c| c.properties.is_royal || c.properties.is_royalty)
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-move feature computation
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of feature values extracted for a single candidate move. Kept as
/// a struct (rather than computed inline) so scoring is a transparent
/// weighted sum — easy to debug, easy to tune, easy to log.
struct MoveFeatures {
    /// SEE on the destination square. Already incorporates the value of
    /// whatever is captured (if anything) and the full recapture chain.
    /// Positive = we win material; negative = we lose it.
    see: f64,

    /// If the moving piece is currently hanging, this is how much we'd lose
    /// by leaving it there. Moving it rescues this value.
    rescue: f64,

    /// Total hanging-value of friendly pieces newly defended by moving here.
    /// Discounted because defending doesn't guarantee safety the way moving
    /// away does.
    defend_hanging: f64,

    /// Does this move deliver check? Detected statically by checking whether
    /// the destination's attack footprint (for this piece type & color)
    /// covers any enemy royal.
    delivers_check: bool,

    /// Change in Chebyshev distance to the nearest enemy royal. Positive
    /// means we got closer.
    royal_proximity_delta: f64,

    /// Did this move take a defender away from our own king zone? Binary
    /// for now — we could count how many zone squares lost a defender, but
    /// the marginal complexity isn't worth it.
    exposes_own_king: bool,

    /// Theoretical mobility at `to` minus theoretical mobility at `from`,
    /// for the moving piece type. Approximate (doesn't account for the
    /// piece itself blocking squares) but the approximation error is small
    /// and symmetric.
    mobility_delta: f64,

    /// Centralization gain, normalized to roughly [-1, 1]. Positive means
    /// moving toward the center.
    center_delta: f64,

    /// Advancement toward the enemy back rank, in raw ranks. Positive means
    /// pushing forward.
    advancement: f64,

    /// Sum of (derived) values of enemy pieces attacked from the new square.
    /// Only counts pieces not already attacked from the old square — we want
    /// *new* threats, not threats we were already making.
    threats_created: f64,

    /// Value of the piece we promote into, or zero. Promotion is also flagged
    /// separately for the flat bonus.
    promotion_value: f64,
    is_promotion: bool,
    is_castle: bool,
}

impl MoveFeatures {
    fn compute(
        mv: &ExpandedMove,
        state: &GameState,
        ctx: &PositionContext,
        cache: &VariantCache,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Self {
        let board = &state.board;
        let us = state.current_turn;
        let them = us.opposite();
        let (rows, cols) = board.size();

        let mover = board.get_piece(mv.from).expect("move from empty square");
        let mover_is_royal = is_royal_piece(mover.piece_type, config_manager);
        let mover_value = cache.value_of(mover.piece_type, mover_is_royal);

        // ── A. Material: SEE ─────────────────────────────────────────────
        // The key question SEE answers: "after all the dust settles on the
        // destination square, how much material did I gain or lose?"
        //
        // For captures, initial_victim is the captured piece's value.
        // For non-captures, initial_victim is zero — we're asking "if I put
        // my piece here, what happens?" SEE then models the opponent's best
        // response: capture our piece (if they can), us recapturing, etc.
        //
        // The `ignore_attacker` parameter is the moving piece's origin
        // square. Without this, a piece moving from a square that attacks
        // `to` would be double-counted as a defender of its own destination.
        // (It can't defend itself after moving there — it *is* the thing
        // being captured in the next round.)
        let initial_victim = mv
            .captures
            .map(|p| cache.value_of(p.piece_type, is_royal_piece(p.piece_type, config_manager)))
            .unwrap_or(0.0);

        // SEE computes the outcome if `them` initiates the exchange on `to`
        // after our piece (worth `mover_value`) lands there. Our net result
        // is what we captured minus what they win back.
        let their_see_on_to = see(
            board,
            mv.to,
            them,
            mover_value,
            Some(mv.from),
            cache,
            move_generator,
            config_manager,
        );
        let see = initial_victim - their_see_on_to.max(0.0);

        // ── A2. Rescue: was the mover hanging? ───────────────────────────
        // If the piece we're moving was under attack and losing the exchange,
        // moving it anywhere safe is worth that much. We use the precomputed
        // hanging table.
        let from_idx = mv.from.0 * cols + mv.from.1;
        let rescue = ctx.hanging_value[from_idx].max(0.0);

        // ── A3. Defend hanging allies ────────────────────────────────────
        // From the destination, what friendly pieces do we now attack (i.e.,
        // defend)? For each one that's hanging, we reduce its hanging value.
        // This is approximate — defending doesn't always save a piece — but
        // it's the right signal. We check the reverse move database to find
        // squares reachable from `to`.
        //
        // Implementation: iterate our hanging pieces and ask whether `to`
        // is among their attackers when the mover is placed there. The
        // cheapest way to check this is to look up the theoretical moves
        // from `to` for our piece type and see if any land on a hanging
        // friend. This is O(moves_from_to), not O(hanging_pieces).
        let mut defend_hanging = 0.0;
        {
            let from_to_footprint =
                attack_footprint(mv.to, mover.piece_type, us, board, move_generator);
            for &dest in &from_to_footprint {
                if let Some(p) = board.get_piece(dest) {
                    if p.color == us {
                        let hang = ctx.hanging_value[dest.0 * cols + dest.1];
                        if hang > 0.0 {
                            // We're adding a defender. This won't always
                            // flip the SEE sign, but it always helps. Credit
                            // half the hanging value as a rough estimate.
                            defend_hanging += hang * 0.5;
                        }
                    }
                }
            }
        }

        // ── B. King interaction ──────────────────────────────────────────
        // Check delivery: does the moving piece, from its new square, attack
        // any enemy royal? We compute this by intersecting the attack
        // footprint from `to` with the enemy royal positions.
        //
        // This isn't perfect — it misses discovered checks (where moving
        // away uncovers another piece's attack on the king). Detecting
        // discovered checks statically requires knowing what was *blocked*
        // by the moving piece, which means re-querying every slider's attack
        // set without the mover present. That's one make/unmake's worth of
        // work, and we're refusing to make/unmake. Direct checks cover the
        // vast majority of checking moves anyway.
        let to_footprint = attack_footprint(mv.to, mover.piece_type, us, board, move_generator);
        let delivers_check = ctx.enemy_royals.iter().any(|rp| to_footprint.contains(rp));

        // Royal proximity: how much closer to the nearest enemy royal did
        // this move bring us? Chebyshev distance because that's how kings
        // move, and "how many king-moves away" is the natural metric for
        // mating nets.
        let royal_proximity_delta = if ctx.enemy_royals.is_empty() {
            0.0
        } else {
            let nearest_from = ctx
                .enemy_royals
                .iter()
                .map(|&rp| VariantCache::chebyshev(mv.from, rp))
                .fold(f64::INFINITY, f64::min);
            let nearest_to = ctx
                .enemy_royals
                .iter()
                .map(|&rp| VariantCache::chebyshev(mv.to, rp))
                .fold(f64::INFINITY, f64::min);
            nearest_from - nearest_to // positive if `to` is closer
        };

        // Own king exposure: did we move a piece OUT of our king zone?
        // If the mover was inside the royal zone and the destination is
        // outside, we've stripped a defender. Royal pieces moving don't
        // count — the king walking away from danger is fine.
        let exposes_own_king = !mover_is_royal
            && ctx.our_royal_zone.contains(&mv.from)
            && !ctx.our_royal_zone.contains(&mv.to);

        // ── C. Positional ────────────────────────────────────────────────
        // Mobility delta: theoretical move count from `to` minus from `from`.
        // "Theoretical" means on an empty board — we use the PST generator,
        // which doesn't consult the actual board state. This overcounts in
        // cluttered positions but the *delta* is what matters, and the
        // error is roughly symmetric between from and to.
        let mobility_from = move_generator
            .generate_theoretical_moves_for_pst(mv.from, mover.piece_type, us, cache.board_size, 1)
            .len() as f64;
        let mobility_to = move_generator
            .generate_theoretical_moves_for_pst(mv.to, mover.piece_type, us, cache.board_size, 1)
            .len() as f64;
        let mobility_delta = mobility_to - mobility_from;

        // Centralization: normalized distance-to-center change. Positive
        // means moving toward the center.
        let center_delta =
            (cache.dist_to_center(mv.from) - cache.dist_to_center(mv.to)) / cache.center_norm;

        // Advancement: rank progress toward the enemy. Row 0 is the top of
        // the board; White pushes toward row 0, Black toward row (n-1).
        let advancement = match us {
            PieceColor::White => mv.from.0 as f64 - mv.to.0 as f64,
            PieceColor::Black => mv.to.0 as f64 - mv.from.0 as f64,
        };

        // ── D. Threat creation ───────────────────────────────────────────
        // Sum the values of enemy pieces newly attacked from `to`. We
        // subtract threats already made from `from`, so only *new* threats
        // count. Threats against royals are capped to avoid them completely
        // dominating the score (check delivery already covers that).
        let from_footprint = attack_footprint(mv.from, mover.piece_type, us, board, move_generator);
        let mut threats_created = 0.0;
        for &sq in &to_footprint {
            if from_footprint.contains(&sq) {
                continue; // already threatened
            }
            if let Some(p) = board.get_piece(sq) {
                if p.color == them {
                    let is_royal = is_royal_piece(p.piece_type, config_manager);
                    let v = if is_royal {
                        // cap royal threat value — check bonus covers this
                        cache.unit_value * 3.0
                    } else {
                        cache.value_of(p.piece_type, false)
                    };
                    threats_created += v;
                }
            }
        }

        // ── E. Special moves ─────────────────────────────────────────────
        let (promotion_value, is_promotion) = match mv.promotion_target {
            Some(pt) => {
                let promo_is_royal = is_royal_piece(pt, config_manager);
                (cache.value_of(pt, promo_is_royal) - mover_value, true)
            }
            None => (0.0, false),
        };

        let is_castle = mv.castling_option.is_some();

        Self {
            see,
            rescue,
            defend_hanging,
            delivers_check,
            royal_proximity_delta,
            exposes_own_king,
            mobility_delta,
            center_delta,
            advancement,
            threats_created,
            promotion_value,
            is_promotion,
            is_castle,
        }
    }
}

/// Get the set of squares a piece of the given type & color would attack
/// from `sq`, on the *current* board. This is real attack coverage: it
/// respects blockers, uses the move database, and only counts moves that can
/// actually land on an enemy piece (can_land_enemy).
///
/// Importantly, this is computed without mutating anything. We're asking the
/// move generator "what attacks does this piece type have from here?" — it
/// doesn't need a piece to actually be present on `sq`.
fn attack_footprint(
    sq: Position,
    piece_type: usize,
    color: PieceColor,
    board: &Board,
    move_generator: &MoveGenerator,
) -> Vec<Position> {
    // generate_moves_with_database requires an actual piece on the square to
    // know the color. For our purposes, we want "hypothetical attacks from
    // here" — so we fall back to the theoretical generator and then filter
    // for blocking ourselves.
    //
    // The theoretical generator gives us all geometrically-reachable squares.
    // We then walk each path and check that the intermediate squares are
    // actually traversable on the current board. This is basically
    // re-implementing is_path_valid but without needing a piece on `sq`.
    let theoretical = move_generator.generate_theoretical_moves_for_pst(
        sq,
        piece_type,
        color,
        board.size(),
        1, // treat as moved — we want normal attack patterns
    );

    let mut footprint = Vec::with_capacity(theoretical.len());
    'moves: for m in theoretical {
        if !m.rule.can_land_enemy {
            continue; // not an attacking move
        }

        // If the move is unblockable (a leaper), the destination is attacked
        // regardless of what's between. Otherwise we need to verify the path.
        if move_generator.is_move_blockable(&m) {
            // Walk intermediate squares. The path includes the origin at [0]
            // and the destination at the end. We check everything in between.
            let steps = &m.path.steps;
            for i in 1..steps.len().saturating_sub(1) {
                let intermediate = (steps[i].0 as usize, steps[i].1 as usize);
                if intermediate == sq {
                    continue; // the origin square — we're pretending we've left it
                }
                if board.get_piece(intermediate).is_some() {
                    continue 'moves; // blocked
                }
            }
        }

        footprint.push(m.destination);
    }

    footprint
}

// ─────────────────────────────────────────────────────────────────────────────
// The engine itself
// ─────────────────────────────────────────────────────────────────────────────

pub struct PurePolicyEngine {
    parameters: EngineParameters,
    cache: Option<VariantCache>,
    /// Simple deterministic PRNG state for tie-breaking noise. Seeded from
    /// the position hash so identical positions get identical noise —
    /// important for reproducibility.
    rng_state: u64,
}

impl PurePolicyEngine {
    pub fn new() -> Self {
        Self {
            parameters: EngineParameters::from_defaults(PURE_POLICY_PARAMETERS),
            cache: None,
            rng_state: 0,
        }
    }

    #[inline]
    fn w(&self, id: &str, default: f64) -> f64 {
        self.parameters.get_or_default(id, default)
    }

    /// xorshift64 — tiny, fast, good enough for tie-breaking.
    fn noise(&mut self) -> f64 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        // Map to [-1, 1]
        (self.rng_state as i64 as f64) / (i64::MAX as f64)
    }

    fn ensure_cache(
        &mut self,
        board_size: (usize, usize),
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        let rebuild = match &self.cache {
            None => true,
            Some(c) => c.board_size != board_size,
        };
        if rebuild {
            self.cache = Some(VariantCache::build(
                board_size,
                move_generator,
                config_manager,
            ));
        }
    }

    /// Weighted sum of features. This IS the policy. Everything above is
    /// feature extraction; this is the decision.
    fn score(&mut self, f: &MoveFeatures, ctx: &PositionContext, cache: &VariantCache) -> f64 {
        // SEE is reported in raw derived-value units. Normalize by the unit
        // value (weakest piece) so that "winning a pawn" is roughly +1
        // before weighting. This keeps weights interpretable across variants
        // with wildly different mobility scales.
        let u = cache.unit_value;

        let mut s = 0.0;

        // Material — the dominant term when it's nonzero
        s += self.w(P_SEE, 100.0) * (f.see / u);
        s += self.w(P_RESCUE, 80.0) * (f.rescue / u);
        s += self.w(P_DEFEND, 30.0) * (f.defend_hanging / u);

        // King safety
        s += self.w(P_CHECK, 35.0) * (f.delivers_check as u8 as f64);
        s += self.w(P_ROYAL_PROX, 4.0) * f.royal_proximity_delta;
        s -= self.w(P_OWN_KING_EXPOSURE, 15.0) * (f.exposes_own_king as u8 as f64);

        // Positional
        s += self.w(P_MOBILITY, 1.5) * f.mobility_delta;
        s += self.w(P_CENTER, 2.0) * f.center_delta;
        s += self.w(P_ADVANCE, 1.0) * f.advancement;
        s += self.w(P_THREAT, 8.0) * (f.threats_created / u);

        // Special moves
        if f.is_promotion {
            s += self.w(P_PROMO, 60.0);
            s += self.w(P_SEE, 100.0) * (f.promotion_value / u); // promotion gain uses SEE weight
        }
        if f.is_castle {
            // Castling is most valuable early, when king safety matters most
            // and the rook is undeveloped. Linear decay over the first
            // ~20 plies, floored at a small positive value.
            let decay = (1.0 - (ctx.move_count as f64 / 20.0)).max(0.1);
            s += self.w(P_CASTLE, 40.0) * decay;
        }

        // Tie-breaking noise
        s += self.w(P_NOISE, 0.5) * self.noise();

        s
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChessEngine impl
// ─────────────────────────────────────────────────────────────────────────────

impl ChessEngine for PurePolicyEngine {
    fn name(&self) -> &str {
        "Pure Policy Engine (Static Scoring)"
    }

    fn best_move(&mut self, params: SearchParams) -> Option<SearchResult> {
        let board_size = params.state.board.size();
        self.ensure_cache(board_size, params.move_generator, params.config_manager);

        // Seed noise from the position hash — same position → same noise →
        // deterministic move selection even with nonzero noise weight.
        self.rng_state = params.state.current_hash() | 1;

        // Generate legal moves. This is the ONLY board mutation in the
        // entire engine (get_legal_moves internally does make/unmake to
        // filter out self-check). We cannot avoid this — we need the legal
        // move list — but we never make/unmake ourselves.
        let legal_moves = params
            .state
            .get_legal_moves(params.move_generator, params.config_manager);

        if legal_moves.is_empty() {
            return None;
        }

        // Take the cache out temporarily so we can borrow it immutably
        // while also borrowing self mutably for score(). Rust's borrow
        // checker doesn't know these don't overlap; this dance makes it so.
        let cache = self.cache.take().expect("cache ensured above");

        // Build once-per-position context (hanging table, royal positions).
        let ctx = PositionContext::build(
            params.state,
            &cache,
            params.move_generator,
            params.config_manager,
        );

        // Score every move.
        let mut best: Option<(ExpandedMove, f64)> = None;
        for mv in &legal_moves {
            let features = MoveFeatures::compute(
                mv,
                params.state,
                &ctx,
                &cache,
                params.move_generator,
                params.config_manager,
            );
            let score = self.score(&features, &ctx, &cache);

            match &best {
                None => best = Some((mv.clone(), score)),
                Some((_, best_score)) if score > *best_score => {
                    best = Some((mv.clone(), score));
                }
                _ => {}
            }
        }

        // Put the cache back.
        self.cache = Some(cache);

        let (best_move, best_score) = best?;

        Some(SearchResult {
            best_move,
            evaluation: Evaluation {
                // Policy score isn't a centipawn evaluation in the usual
                // sense — it's a move-quality score, not a position score.
                // We report it anyway because the UI expects *something*,
                // and it's useful for comparing how much the engine liked
                // its choice relative to alternatives.
                score: best_score.round() as i32,
                mate_in: None,
            },
            depth_reached: 0, // Policy movers have no depth. Honest about it.
        })
    }

    fn stop(&mut self) {
        // Nothing to stop — we're single-pass.
    }

    fn reset_cache(&mut self) {
        self.cache = None;
    }

    fn parameter_definitions(&self) -> Option<&'static [ParameterDef]> {
        Some(PURE_POLICY_PARAMETERS)
    }

    fn get_parameters(&self) -> Option<EngineParameters> {
        Some(self.parameters.clone())
    }

    fn set_parameters(&mut self, p: EngineParameters) -> bool {
        let changed = self.parameters != p;
        self.parameters = p;
        changed
    }
}
