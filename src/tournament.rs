// src/tournament.rs
//
// Automated engine-vs-engine tournament infrastructure.
//
// The tournament is a state machine driven by the GUI tick. It owns the
// pairing schedule and the ELO bookkeeping, but it does NOT own the game
// state or the engines. Those remain in ChessGui / GameController.
//
// ── Scheduling ──────────────────────────────────────────────────────────
//
// Rather than precompute a fixed schedule, we draw pairings on demand from
// a pool of "credits" — each engine starts with N credits (N = number of
// games it should play) and each game consumes one credit from each
// participant.
//
// When drawing a pairing, we use rating-gap rejection sampling: a proposed
// pairing between a 1800 and a 1400 is less likely to be accepted than one
// between two 1600s. Rejected pairings don't consume credits; we just draw
// again. This makes engines gravitate toward their skill neighborhood as
// ratings separate, without anyone playing fewer games than anyone else.
//
// The rejection probability is derived from the ELO expected-score formula.
// A game where the favorite is expected to score 0.95 tells you almost
// nothing you didn't already know; we accept it with low probability so
// the slot goes to a more informative matchup instead. But we never push
// acceptance all the way to zero — ratings are only comparable across the
// pool if there's *some* flow of games between brackets.

use crate::core::game_state::MateStatus;
use crate::core::piece::PieceColor;
use crate::engine::EngineType;
use std::collections::HashMap;
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────
// Game outcome
// ─────────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOutcome {
    WhiteWins,
    BlackWins,
    Draw,
    /// Game hit the ply limit without a natural result. Scored as a draw
    /// for ELO purposes but logged distinctly.
    AdjudicatedDraw,
}

impl GameOutcome {
    pub fn white_score(self) -> f64 {
        match self {
            GameOutcome::WhiteWins => 1.0,
            GameOutcome::Draw | GameOutcome::AdjudicatedDraw => 0.5,
            GameOutcome::BlackWins => 0.0,
        }
    }

    pub fn from_mate_status(status: MateStatus, side_to_move: PieceColor) -> Option<Self> {
        match status {
            MateStatus::Checkmate => Some(match side_to_move {
                PieceColor::White => GameOutcome::BlackWins,
                PieceColor::Black => GameOutcome::WhiteWins,
            }),
            MateStatus::OpponentLostByCheck => Some(match side_to_move {
                PieceColor::White => GameOutcome::WhiteWins,
                PieceColor::Black => GameOutcome::BlackWins,
            }),
            MateStatus::Stalemate => Some(GameOutcome::Draw),
            MateStatus::Ongoing => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Termination — *why* a game ended, orthogonal to *who scored what*.
// GameOutcome is the 1/½/0 abstraction the ELO math needs; Termination is
// the diagnostic detail the report wants. We keep both so ELO scoring
// stays a single match on four variants.
// ─────────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Termination {
    // Decisive
    Checkmate,
    OpponentLeftInCheck,
    /// Side to move has no pieces at all. Only reachable in variants
    /// without royal pieces — with a royal on the board, you'd hit
    /// Checkmate before the piece count could reach zero.
    Extinction,
    Forfeit,
    // Draws
    Stalemate,
    FiftyMoveRule,
    Repetition,
    InsufficientMaterial,
    PlyLimit,
}

impl Termination {
    pub fn is_draw(self) -> bool {
        use Termination::*;
        matches!(
            self,
            Stalemate | FiftyMoveRule | Repetition | InsufficientMaterial | PlyLimit
        )
    }

    pub fn label(self) -> &'static str {
        use Termination::*;
        match self {
            Checkmate => "checkmate",
            OpponentLeftInCheck => "opp. left in check",
            Extinction => "extinction",
            Forfeit => "forfeit",
            Stalemate => "stalemate",
            FiftyMoveRule => "fifty-move rule",
            Repetition => "repetition",
            InsufficientMaterial => "insufficient material",
            PlyLimit => "ply limit (adj.)",
        }
    }

    pub const ALL: [Termination; 9] = {
        use Termination::*;
        [
            Checkmate,
            Extinction,
            OpponentLeftInCheck,
            Forfeit,
            Stalemate,
            Repetition,
            FiftyMoveRule,
            InsufficientMaterial,
            PlyLimit,
        ]
    };
}

// ─────────────────────────────────────────────────────────────────────────
// ELO tracking
// ─────────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Default)]
pub struct MatchupRecord {
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// Sum of plies across all games in this (ordered) pairing. Kept here
    /// so the average is O(1); everything finer‑grained is derived from
    /// the game log on demand.
    pub total_plies: u64,
}

impl MatchupRecord {
    pub fn total(&self) -> u32 {
        self.white_wins + self.draws + self.black_wins
    }

    pub fn white_score_rate(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            return 0.5;
        }
        (self.white_wins as f64 + 0.5 * self.draws as f64) / t as f64
    }

    pub fn avg_plies(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            self.total_plies as f64 / t as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct GameRecord {
    pub game_number: usize,
    pub white: EngineType,
    pub black: EngineType,
    pub outcome: GameOutcome,
    pub termination: Termination,
    pub plies: usize,
    pub white_time: Duration,
    pub black_time: Duration,
    pub white_rating_after: f64,
    pub black_rating_after: f64,
}

#[derive(Debug, Clone)]
pub struct RatingPoint {
    pub game_number: usize,
    pub engine: EngineType,
    pub rating: f64,
}

pub struct EloTracker {
    initial_rating: f64,
    k_factor: f64,
    ratings: HashMap<EngineType, f64>,
    history: Vec<RatingPoint>,
    matchups: HashMap<(EngineType, EngineType), MatchupRecord>,
    game_log: Vec<GameRecord>,
}

impl EloTracker {
    pub fn new() -> Self {
        Self {
            initial_rating: 1500.0,
            k_factor: 32.0,
            ratings: HashMap::new(),
            history: Vec::new(),
            matchups: HashMap::new(),
            game_log: Vec::new(),
        }
    }

    pub fn rating(&self, engine: &EngineType) -> f64 {
        *self.ratings.get(engine).unwrap_or(&self.initial_rating)
    }

    pub fn history(&self) -> &[RatingPoint] {
        &self.history
    }

    pub fn game_log(&self) -> &[GameRecord] {
        &self.game_log
    }

    pub fn matchup(&self, white: &EngineType, black: &EngineType) -> MatchupRecord {
        self.matchups
            .get(&(white.clone(), black.clone()))
            .cloned()
            .unwrap_or_default()
    }

    pub fn participants(&self) -> Vec<EngineType> {
        let mut engines: Vec<EngineType> = self.ratings.keys().cloned().collect();
        engines.sort_by_key(|e| e.name().to_string());
        engines
    }

    /// Expected score for `a` vs `b` under the ELO logistic. This is the
    /// bedrock of the whole rating system: a 200-point advantage predicts
    /// ~0.76, a 400-point advantage ~0.91. We expose it here because the
    /// scheduler uses it for rejection sampling.
    pub fn expected_score(&self, a: &EngineType, b: &EngineType) -> f64 {
        let ra = self.rating(a);
        let rb = self.rating(b);
        1.0 / (1.0 + 10f64.powf((rb - ra) / 400.0))
    }

    pub fn record(
        &mut self,
        game_number: usize,
        white: EngineType,
        black: EngineType,
        outcome: GameOutcome,
        termination: Termination,
        plies: usize,
        white_time: Duration,
        black_time: Duration,
    ) -> (f64, f64) {
        let r_white = self.rating(&white);
        let r_black = self.rating(&black);

        let expected_white = 1.0 / (1.0 + 10f64.powf((r_black - r_white) / 400.0));
        let actual_white = outcome.white_score();

        let white_delta = self.k_factor * (actual_white - expected_white);
        let black_delta = -white_delta;

        let new_white = r_white + white_delta;
        let new_black = r_black + black_delta;

        self.ratings.insert(white.clone(), new_white);
        self.ratings.insert(black.clone(), new_black);

        if game_number == 1 || !self.history.iter().any(|p| p.engine == white) {
            self.history.push(RatingPoint {
                game_number: game_number.saturating_sub(1),
                engine: white.clone(),
                rating: r_white,
            });
        }
        if game_number == 1 || !self.history.iter().any(|p| p.engine == black) {
            self.history.push(RatingPoint {
                game_number: game_number.saturating_sub(1),
                engine: black.clone(),
                rating: r_black,
            });
        }

        self.history.push(RatingPoint {
            game_number,
            engine: white.clone(),
            rating: new_white,
        });
        self.history.push(RatingPoint {
            game_number,
            engine: black.clone(),
            rating: new_black,
        });

        let rec = self
            .matchups
            .entry((white.clone(), black.clone()))
            .or_default();
        match outcome {
            GameOutcome::WhiteWins => rec.white_wins += 1,
            GameOutcome::BlackWins => rec.black_wins += 1,
            GameOutcome::Draw | GameOutcome::AdjudicatedDraw => rec.draws += 1,
        }
        rec.total_plies += plies as u64;

        self.game_log.push(GameRecord {
            game_number,
            white,
            black,
            outcome,
            termination,
            plies,
            white_time,
            black_time,
            white_rating_after: new_white,
            black_rating_after: new_black,
        });

        (white_delta, black_delta)
    }

    pub fn rating_bounds(&self) -> (f64, f64) {
        if self.history.is_empty() {
            return (self.initial_rating - 100.0, self.initial_rating + 100.0);
        }

        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for p in &self.history {
            lo = lo.min(p.rating);
            hi = hi.max(p.rating);
        }

        if hi - lo < 100.0 {
            let mid = (hi + lo) * 0.5;
            lo = mid - 50.0;
            hi = mid + 50.0;
        }
        (lo - 50.0, hi + 50.0)
    }

    /// Head-to-head record between two engines, aggregating both colors.
    /// Returns (a_wins, b_wins, draws).
    pub fn head_to_head(&self, a: &EngineType, b: &EngineType) -> (u32, u32, u32) {
        let ab = self.matchup(a, b);
        let ba = self.matchup(b, a);

        let a_wins = ab.white_wins + ba.black_wins;
        let b_wins = ab.black_wins + ba.white_wins;
        let draws = ab.draws + ba.draws;

        (a_wins, b_wins, draws)
    }

    /// Print final standings and the head-to-head results matrix.
    pub fn print_results_matrix(&self) {
        let mut engines = self.participants();
        if engines.is_empty() {
            println!("No tournament data to display.");
            return;
        }

        engines.sort_by(|a, b| {
            self.rating(b)
                .partial_cmp(&self.rating(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        println!("\n🏁 Tournament complete!");
        println!("\n═══ Final Standings ═══");
        for (i, e) in engines.iter().enumerate() {
            println!("  {}. {} — {:.0}", i + 1, e.name(), self.rating(e));
        }

        let name_width = 20;
        let cell_width = 9;

        println!("\n═══ Head-to-Head Results (W/L/D from row's perspective) ═══");

        print!("{:>width$} │", "", width = name_width);
        for e in &engines {
            let short = short_name(e.name(), cell_width);
            print!("{:^width$}│", short, width = cell_width);
        }
        println!();

        print!("{:─>width$}─┼", "", width = name_width);
        for _ in &engines {
            print!("{:─>width$}┼", "", width = cell_width);
        }
        println!();

        for row in &engines {
            let rn = short_name(row.name(), name_width);
            print!("{:>width$} │", rn, width = name_width);

            for col in &engines {
                if row == col {
                    print!("{:^width$}│", "---", width = cell_width);
                } else {
                    let (w, l, d) = self.head_to_head(row, col);
                    let cell = format!("{}/{}/{}", w, l, d);
                    print!("{:^width$}│", cell, width = cell_width);
                }
            }
            println!();
        }

        let total_games = self.game_log.len();
        let total_decisive = self
            .game_log
            .iter()
            .filter(|g| matches!(g.outcome, GameOutcome::WhiteWins | GameOutcome::BlackWins))
            .count();
        let total_draws = total_games - total_decisive;

        println!();
        println!(
            "Total: {} games ({} decisive, {} draws, {:.0}% draw rate)",
            total_games,
            total_decisive,
            total_draws,
            if total_games > 0 {
                total_draws as f64 / total_games as f64 * 100.0
            } else {
                0.0
            }
        );
        println!();
    }

    // ─────────────────────────────────────────────────────────────────
    // Detailed reporting
    //
    // Everything below is computed by scanning `game_log`. That's O(G)
    // per section, which for any tournament that fits in memory is
    // effectively instant and keeps `record()` — the hot path during a
    // parallel tournament — free of per‑stat bookkeeping.
    // ─────────────────────────────────────────────────────────────────

    /// Full multi‑section report. Called after `print_results_matrix()`
    /// at tournament end, and on demand via the `tstats` terminal command.
    pub fn print_detailed_report(&self) {
        if self.game_log.is_empty() {
            println!("No tournament data to report.");
            return;
        }
        self.print_global_section();
        self.print_per_engine_section();
        self.print_colour_split_matrix();
        println!("(Use `tstats <engineA> <engineB>` for per‑pairing detail.)\n");
    }

    fn print_global_section(&self) {
        let g = self.game_log.len();
        let mut white_w = 0u32;
        let mut draws = 0u32;
        let mut black_w = 0u32;
        let mut term_hist: HashMap<Termination, u32> = HashMap::new();
        let mut total_plies = 0u64;
        let mut min_p = usize::MAX;
        let mut max_p = 0usize;

        for r in &self.game_log {
            match r.outcome {
                GameOutcome::WhiteWins => white_w += 1,
                GameOutcome::BlackWins => black_w += 1,
                GameOutcome::Draw | GameOutcome::AdjudicatedDraw => draws += 1,
            }
            *term_hist.entry(r.termination).or_insert(0) += 1;
            total_plies += r.plies as u64;
            min_p = min_p.min(r.plies);
            max_p = max_p.max(r.plies);
        }

        let decisive = white_w + black_w;
        let white_score = (white_w as f64 + 0.5 * draws as f64) / g as f64;

        println!("\n═══ Tournament Report ═══\n");
        println!("── Global ──");
        println!(
            "  Games: {}   Decisive: {} ({:.1}%)   Draws: {} ({:.1}%)",
            g,
            decisive,
            pct(decisive, g as u32),
            draws,
            pct(draws, g as u32),
        );
        println!(
            "  White score: {:.1}%   (W {} / D {} / L {})",
            white_score * 100.0,
            white_w,
            draws,
            black_w
        );
        println!(
            "  Game length: avg {:.1} plies   (shortest {}, longest {})",
            total_plies as f64 / g as f64,
            min_p,
            max_p
        );

        println!("\n  Termination breakdown:");
        for t in Termination::ALL {
            if let Some(&n) = term_hist.get(&t) {
                if n > 0 {
                    println!(
                        "    {:<22} {:>5}  ({:>5.1}%)",
                        t.label(),
                        n,
                        pct(n, g as u32)
                    );
                }
            }
        }
        println!();
    }

    fn print_per_engine_section(&self) {
        let mut engines = self.participants();
        engines.sort_by(|a, b| {
            self.rating(b)
                .partial_cmp(&self.rating(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        println!("── Per engine ──");
        for e in &engines {
            self.print_one_engine(e);
        }
        println!();
    }

    fn print_one_engine(&self, e: &EngineType) {
        // Two passes would be tidier, but one pass with a handful of
        // counters is fine and keeps allocation to the term histogram.
        let mut games = 0u32;
        let (mut w, mut d, mut l) = (0u32, 0u32, 0u32);
        let (mut wg_w, mut wg_s) = (0u32, 0.0f64); // as‑white games / score
        let (mut bg_w, mut bg_s) = (0u32, 0.0f64); // as‑black games / score
        let (mut p_win, mut n_win) = (0u64, 0u32);
        let (mut p_draw, mut n_draw) = (0u64, 0u32);
        let (mut p_loss, mut n_loss) = (0u64, 0u32);
        let mut draw_terms: HashMap<Termination, u32> = HashMap::new();
        let mut think = Duration::ZERO;

        for r in &self.game_log {
            let (is_white, my_score, my_time) = if r.white == *e {
                (true, r.outcome.white_score(), r.white_time)
            } else if r.black == *e {
                (false, 1.0 - r.outcome.white_score(), r.black_time)
            } else {
                continue;
            };
            games += 1;
            think += my_time;

            if is_white {
                wg_w += 1;
                wg_s += my_score;
            } else {
                bg_w += 1;
                bg_s += my_score;
            }

            if my_score > 0.75 {
                w += 1;
                p_win += r.plies as u64;
                n_win += 1;
            } else if my_score < 0.25 {
                l += 1;
                p_loss += r.plies as u64;
                n_loss += 1;
            } else {
                d += 1;
                p_draw += r.plies as u64;
                n_draw += 1;
                *draw_terms.entry(r.termination).or_insert(0) += 1;
            }
        }

        if games == 0 {
            return;
        }

        let score = (w as f64 + 0.5 * d as f64) / games as f64;
        let wr = if wg_w > 0 { wg_s / wg_w as f64 } else { 0.0 };
        let br = if bg_w > 0 { bg_s / bg_w as f64 } else { 0.0 };

        println!(
            "  {:<30} {:>6.0}   {}g   +{} ={} -{}   {:.1}%",
            short_name(e.name(), 30),
            self.rating(e),
            games,
            w,
            d,
            l,
            score * 100.0
        );
        println!(
            "      as White: {:>3}g {:>5.1}%    as Black: {:>3}g {:>5.1}%    Δ {:+.1}%",
            wg_w,
            wr * 100.0,
            bg_w,
            br * 100.0,
            (wr - br) * 100.0
        );
        println!(
            "      avg plies — win {:>5.1} / draw {:>5.1} / loss {:>5.1}     think time {:>6.1}s ({:.2}s/game)",
            avg(p_win, n_win),
            avg(p_draw, n_draw),
            avg(p_loss, n_loss),
            think.as_secs_f64(),
            think.as_secs_f64() / games as f64,
        );
        if d > 0 {
            let mut parts: Vec<String> = Vec::new();
            for t in Termination::ALL {
                if let Some(&n) = draw_terms.get(&t) {
                    if n > 0 {
                        parts.push(format!("{} {}", t.label(), n));
                    }
                }
            }
            println!("      draws by: {}", parts.join(", "));
        }
    }

    /// Head‑to‑head matrix showing, per cell, the row engine's score rate
    /// against the column engine split by colour. Large asW/asB gaps flag
    /// a matchup where one side has found a colour‑specific exploit.
    fn print_colour_split_matrix(&self) {
        let mut engines = self.participants();
        if engines.len() < 2 {
            return;
        }
        engines.sort_by(|a, b| {
            self.rating(b)
                .partial_cmp(&self.rating(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let name_w = 20;
        let cell_w = 13; // "100.0/100.0"

        println!("── Colour‑split matrix (row's score%  asWhite / asBlack) ──");
        print!("{:>w$} │", "", w = name_w);
        for e in &engines {
            print!("{:^w$}│", short_name(e.name(), cell_w), w = cell_w);
        }
        println!();
        print!("{:─>w$}─┼", "", w = name_w);
        for _ in &engines {
            print!("{:─>w$}┼", "", w = cell_w);
        }
        println!();

        for row in &engines {
            print!("{:>w$} │", short_name(row.name(), name_w), w = name_w);
            for col in &engines {
                if row == col {
                    print!("{:^w$}│", "---", w = cell_w);
                    continue;
                }
                let as_w = self.matchup(row, col); // row had white
                let as_b = self.matchup(col, row); // row had black
                let cell = match (as_w.total(), as_b.total()) {
                    (0, 0) => "·".to_string(),
                    (_, 0) => format!("{:>5.1}/  —  ", as_w.white_score_rate() * 100.0),
                    (0, _) => format!("  —  /{:>5.1}", (1.0 - as_b.white_score_rate()) * 100.0),
                    _ => format!(
                        "{:>5.1}/{:>5.1}",
                        as_w.white_score_rate() * 100.0,
                        (1.0 - as_b.white_score_rate()) * 100.0,
                    ),
                };
                print!("{:^w$}│", cell, w = cell_w);
            }
            println!();
        }
        println!();
    }

    /// Deep dive on a single unordered pairing — both directions side by
    /// side, with length stats and termination breakdown per direction.
    pub fn print_pairing_detail(&self, a: &EngineType, b: &EngineType) {
        println!("\n── Pairing detail: {} vs {} ──", a.name(), b.name());
        for (w, bl, tag) in [(a, b, "A as White"), (b, a, "A as Black")] {
            let rec = self.matchup(w, bl);
            let mut min_p = usize::MAX;
            let mut max_p = 0usize;
            let mut terms: HashMap<Termination, u32> = HashMap::new();
            for g in self
                .game_log
                .iter()
                .filter(|g| g.white == *w && g.black == *bl)
            {
                min_p = min_p.min(g.plies);
                max_p = max_p.max(g.plies);
                *terms.entry(g.termination).or_insert(0) += 1;
            }
            if rec.total() == 0 {
                println!("  {}: no games", tag);
                continue;
            }
            // Score from A's perspective regardless of which direction.
            let a_score = if w == a {
                rec.white_score_rate()
            } else {
                1.0 - rec.white_score_rate()
            };
            println!(
                "  {}: {}g  +{} ={} -{}   A scores {:.1}%   plies avg {:.1} (min {}, max {})",
                tag,
                rec.total(),
                if w == a {
                    rec.white_wins
                } else {
                    rec.black_wins
                },
                rec.draws,
                if w == a {
                    rec.black_wins
                } else {
                    rec.white_wins
                },
                a_score * 100.0,
                rec.avg_plies(),
                min_p,
                max_p,
            );
            let mut parts: Vec<String> = Vec::new();
            for t in Termination::ALL {
                if let Some(&n) = terms.get(&t) {
                    if n > 0 {
                        parts.push(format!("{} {}", t.label(), n));
                    }
                }
            }
            if !parts.is_empty() {
                println!("      endings: {}", parts.join(", "));
            }
        }
        println!();
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Credit-pool scheduler with rating-gap rejection
// ─────────────────────────────────────────────────────────────────────────

/// How many times to retry drawing a pairing before giving up and accepting
/// whatever we have. This bounds the scheduler's work per game to O(1).
/// At 40 attempts with a 10% floor, the chance of zero acceptances is
/// 0.9^40 ≈ 1.5% — and in that case we force-accept the best-seen, so
/// nothing stalls.
const MAX_REJECTION_ATTEMPTS: u32 = 40;

/// Floor on acceptance probability. Without this, a 600-point gap would
/// accept at ~0.6%, and the unlucky bottom engine could burn hundreds of
/// rejections waiting for a peer that doesn't exist. 0.10 means even the
/// most lopsided pairing gets through one time in ten — enough to keep
/// cross-bracket calibration alive without letting mismatches dominate.
const MIN_ACCEPTANCE: f64 = 0.10;

/// The pairing pool. Each engine holds some number of game credits;
/// drawing a pairing consumes one credit from each of two engines.
///
/// We keep credits indexed parallel to a fixed engine list, not in a
/// HashMap — the list is small, we iterate it constantly during weighted
/// draws, and Vec is cache-friendlier.
struct PairingPool {
    engines: Vec<EngineType>,
    credits: Vec<u32>,
    /// Per-engine count of games played with each color. Used to assign
    /// white to whoever has had it less often, keeping color distribution
    /// balanced even though pairings are stochastic.
    white_games: Vec<u32>,
    /// xorshift64 state. Seeded once at tournament start so the whole
    /// draw sequence is reproducible from the seed.
    rng: u64,
}

impl PairingPool {
    fn new(engines: Vec<EngineType>, games_per_engine: u32, seed: u64) -> Self {
        let n = engines.len();
        Self {
            engines,
            credits: vec![games_per_engine; n],
            white_games: vec![0; n],
            rng: seed | 1,
        }
    }

    fn total_credits(&self) -> u32 {
        self.credits.iter().sum()
    }

    fn rand_u64(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn rand_f64(&mut self) -> f64 {
        // 53 bits of mantissa → [0, 1). Standard trick.
        (self.rand_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Draw one engine index, weighted by remaining credits. `exclude` lets
    /// the caller mask out the first-drawn engine when picking the second.
    /// Returns None only if no engine (besides the excluded one) has credit
    /// left.
    ///
    /// Weighting by credit is what keeps game counts even: an engine that's
    /// fallen behind (because it keeps getting rejected) has more remaining
    /// credit, so it's more likely to be proposed — a self-correcting
    /// pressure toward equal game counts.
    fn draw_weighted(&mut self, exclude: Option<usize>) -> Option<usize> {
        let total: u32 = self
            .credits
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != exclude)
            .map(|(_, &c)| c)
            .sum();

        if total == 0 {
            return None;
        }

        let pick = (self.rand_u64() % total as u64) as u32;
        let mut acc = 0u32;
        for (i, &c) in self.credits.iter().enumerate() {
            if Some(i) == exclude {
                continue;
            }
            acc += c;
            if pick < acc {
                return Some(i);
            }
        }
        unreachable!("weighted draw fell off the end; total was {}", total)
    }

    /// Acceptance probability for a pairing, derived from expected score.
    ///
    /// We map `expected ∈ [0.5, 1.0]` → `accept ∈ [1.0, MIN_ACCEPTANCE]`
    /// linearly. A perfectly even matchup (expected 0.5) is always taken;
    /// a maximally lopsided one (expected → 1.0) bottoms out at the floor.
    ///
    /// Linear rather than something sharper because the ELO curve itself is
    /// already sigmoid — a 400-point gap lands at expected=0.91 → accept
    /// ≈0.26, which feels right. A 200-point gap (expected=0.76) accepts
    /// at ≈0.57, so moderate mismatches still happen regularly.
    fn acceptance_probability(expected: f64) -> f64 {
        let lopsidedness = (expected - 0.5).abs() * 2.0; // 0 even, 1 certain
        let raw = 1.0 - lopsidedness;
        raw.max(MIN_ACCEPTANCE)
    }

    /// Draw one pairing, with rating-gap rejection.
    ///
    /// Returns (white, black). Always succeeds unless fewer than two
    /// engines have credit remaining — that's the scheduler's termination
    /// signal.
    ///
    /// If one engine is stranded with leftover credits at the end (odd
    /// totals can do this), it sits out its last game. With N engines and
    /// G games each, this happens only when N×G is odd, meaning one credit
    /// out of the whole tournament goes unspent. Acceptable.
    fn draw_pairing(&mut self, elo: &EloTracker) -> Option<(EngineType, EngineType)> {
        // Best rejected pairing so far, tracked for the fallback case.
        // "Best" means highest acceptance probability = closest ratings.
        let mut best_rejected: Option<(usize, usize, f64)> = None;

        for _ in 0..MAX_REJECTION_ATTEMPTS {
            let a = self.draw_weighted(None)?;
            let b = match self.draw_weighted(Some(a)) {
                Some(b) => b,
                // Only one engine has credit left. It sits out; we're done.
                None => return None,
            };

            let expected = elo.expected_score(&self.engines[a], &self.engines[b]);
            let p_accept = Self::acceptance_probability(expected);

            if self.rand_f64() < p_accept {
                return Some(self.commit(a, b));
            }

            // Rejected. Remember it if it's the closest we've seen, in case
            // every attempt gets rejected.
            if best_rejected.map_or(true, |(_, _, p)| p_accept > p) {
                best_rejected = Some((a, b, p_accept));
            }
        }

        // Fell through all attempts. Force-accept the least-bad rejected
        // pairing. This is rare (requires the pool to be very polarized
        // *and* bad luck), but it guarantees forward progress.
        //
        // We can't reach here with best_rejected == None: the loop only
        // bails early on draw failure (returning None above), and any
        // completed iteration populates best_rejected.
        let (a, b, _) = best_rejected.expect("loop completed but saw no pairings");
        Some(self.commit(a, b))
    }

    /// Finalize a pairing: decrement credits, assign colors, return the
    /// (white, black) tuple.
    fn commit(&mut self, a: usize, b: usize) -> (EngineType, EngineType) {
        self.credits[a] -= 1;
        self.credits[b] -= 1;

        // Whoever has had white fewer times gets it now. Ties go to `a`,
        // which is arbitrary but deterministic given the RNG sequence.
        let (white_idx, black_idx) = if self.white_games[a] <= self.white_games[b] {
            (a, b)
        } else {
            (b, a)
        };
        self.white_games[white_idx] += 1;

        (
            self.engines[white_idx].clone(),
            self.engines[black_idx].clone(),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tournament state machine
// ─────────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq)]
pub enum TournamentPhase {
    Inactive,
    SettingUpGame,
    Playing,
    Complete,
}

pub struct Tournament {
    pub phase: TournamentPhase,

    /// Pairing pool. Present only while a tournament is live.
    pool: Option<PairingPool>,

    /// The pairing currently being played (or about to be). Stashed here
    /// because `finish_game()` needs it after the game ends.
    current: Option<(EngineType, EngineType)>,

    /// Total games to play. Computed at start() as N × games_per_engine / 2.
    /// Fixed for the whole tournament; the pool may technically have one
    /// stranded credit at the end, but this count is what the progress bar
    /// and graph use.
    total_games: usize,

    games_played: usize,

    /// How many pairings have been handed out via `take_next_pairing`.
    /// Distinct from `games_played` while games are in flight.
    games_dispatched: usize,

    pub max_plies: usize,
    pub elo: EloTracker,
    pub participants: Vec<EngineType>,

    /// Games each engine plays. Old interpretation was "games per unordered
    /// pair"; now it's per-engine because pairings aren't fixed anymore.
    /// Keeping the field name avoids touching the GUI input box.
    pub games_per_pairing: usize,

    /// Maximum number of games to run concurrently. 1 reproduces the old
    /// serial behaviour (and its deterministic pairing/ELO order).
    pub parallelism: usize,
}

impl Tournament {
    pub fn new() -> Self {
        Self {
            phase: TournamentPhase::Inactive,
            pool: None,
            current: None,
            total_games: 0,
            games_played: 0,
            games_dispatched: 0, // NEW
            max_plies: 400,
            elo: EloTracker::new(),
            participants: Vec::new(),
            games_per_pairing: 10,
            parallelism: 1, // NEW
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.phase,
            TournamentPhase::SettingUpGame | TournamentPhase::Playing
        )
    }

    pub fn is_playing(&self) -> bool {
        self.phase == TournamentPhase::Playing
    }

    pub fn total_games(&self) -> usize {
        self.total_games
    }

    pub fn games_played(&self) -> usize {
        self.games_played
    }

    pub fn current_pairing(&self) -> Option<&(EngineType, EngineType)> {
        self.current.as_ref()
    }

    pub fn start(&mut self, seed: u64) -> Result<(), &'static str> {
        if self.participants.len() < 2 {
            return Err("need at least two engines");
        }

        let engines: Vec<EngineType> = self
            .participants
            .iter()
            .filter(|e| !e.is_human())
            .cloned()
            .collect();

        if engines.len() < 2 {
            return Err("need at least two non-human engines");
        }

        let games_per_engine = self.games_per_pairing as u32;
        let n = engines.len();
        self.total_games = (n * games_per_engine as usize) / 2;

        self.pool = Some(PairingPool::new(engines, games_per_engine, seed));
        self.games_played = 0;
        self.games_dispatched = 0;
        self.current = None;

        // Workers are spawned by the GUI; we just flip to Playing.
        self.phase = TournamentPhase::Playing;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.phase = TournamentPhase::Inactive;
        self.pool = None;
        self.current = None;
    }

    pub fn clear_history(&mut self) {
        self.elo = EloTracker::new();
    }

    pub fn begin_game(&mut self) {
        debug_assert_eq!(self.phase, TournamentPhase::SettingUpGame);
        self.phase = TournamentPhase::Playing;
    }

    // No longer used.
    //     pub fn finish_game(&mut self, outcome: GameOutcome, plies: usize) -> Option<(f64, f64)> {
    //     debug_assert_eq!(self.phase, TournamentPhase::Playing);
    //     let (white, black) = self.current.clone()?;
    //     let game_num = self.games_played + 1;
    //     let deltas = self.elo.record(game_num, white, black, outcome, plies);
    //     self.games_played += 1;
    //
    //     // Draw the next pairing before deciding the phase. If the pool is
    //     // exhausted, we're done regardless of games_played vs total_games
    //     // (they should agree, but the pool is authoritative).
    //     self.draw_next_pairing();
    //
    //     self.phase = if self.current.is_none() || self.games_played >= self.total_games {
    //         self.pool = None;
    //         TournamentPhase::Complete
    //     } else {
    //         TournamentPhase::SettingUpGame
    //     };
    //
    //     Some(deltas)
    // }

    // ─── Parallel‑schedule API ──────────────────────────────────────────
    //
    // The GUI pulls pairings on demand and reports results as games
    // complete, possibly out of order.

    /// Draw the next pairing from the pool. Returns `None` once the
    /// schedule is exhausted. Updates `current` so the UI can still show
    /// "most recently dispatched" if it wants.
    pub fn take_next_pairing(&mut self) -> Option<(EngineType, EngineType)> {
        if self.games_dispatched >= self.total_games {
            return None;
        }
        let drawn = self.pool.as_mut()?.draw_pairing(&self.elo);
        match drawn {
            Some(p) => {
                self.games_dispatched += 1;
                self.current = Some(p.clone());
                Some(p)
            }
            None => {
                // Pool can't produce another pairing (stranded credit).
                self.pool = None;
                None
            }
        }
    }

    /// Are there still pairings left to dispatch?
    pub fn has_more_pairings(&self) -> bool {
        self.pool.is_some() && self.games_dispatched < self.total_games
    }

    /// Record a completed game. Safe to call in any order.
    pub fn record_game(
        &mut self,
        white: EngineType,
        black: EngineType,
        outcome: GameOutcome,
        termination: Termination,
        plies: usize,
        white_time: Duration,
        black_time: Duration,
    ) -> (f64, f64) {
        let game_num = self.games_played + 1;
        let deltas = self.elo.record(
            game_num,
            white,
            black,
            outcome,
            termination,
            plies,
            white_time,
            black_time,
        );
        self.games_played += 1;
        deltas
    }

    /// Transition to Complete and release the pool.
    pub fn mark_complete(&mut self) {
        self.phase = TournamentPhase::Complete;
        self.pool = None;
        self.current = None;
    }

    /// Pull the next pairing from the pool into `self.current`. Leaves
    /// `current` as None if the pool can't produce another game.
    fn draw_next_pairing(&mut self) {
        self.current = self
            .pool
            .as_mut()
            .and_then(|pool| pool.draw_pairing(&self.elo));
    }

    pub fn toggle_participant(&mut self, engine: EngineType) -> bool {
        if let Some(idx) = self.participants.iter().position(|e| *e == engine) {
            self.participants.remove(idx);
            false
        } else {
            self.participants.push(engine);
            true
        }
    }

    pub fn is_participant(&self, engine: &EngineType) -> bool {
        self.participants.contains(engine)
    }
}

impl Default for Tournament {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

/// Truncate a display name for table columns. Counts chars, not bytes,
/// so emoji prefixes on personality names don't break the layout.
fn short_name(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        name.to_string()
    } else {
        let truncated: String = name.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

fn pct(n: u32, d: u32) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 * 100.0 / d as f64
    }
}

fn avg(sum: u64, n: u32) -> f64 {
    if n == 0 { 0.0 } else { sum as f64 / n as f64 }
}
