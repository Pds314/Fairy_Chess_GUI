// src/insufficient_material.rs
//
// Insufficient-material ("dead position") draw detection.
//
// A variant's `.game` file may declare exact material combinations that
// are known to be unwinnable for either side. When the board matches one
// of those combinations the game is immediately drawn, without waiting
// for the fifty-move counter or a threefold repetition.
//
// This module does NOT try to *derive* which positions are dead — that's
// undecidable for arbitrary fairy pieces. It purely pattern-matches the
// board's material against a user-supplied list.
//
// ── Performance ─────────────────────────────────────────────────────────
//
// `is_draw()` is called from `GameState::check_draw_conditions`, which
// runs after every make_move — i.e. once per node of every search tree.
// The cost profile is deliberately front-loaded with cheap early-outs:
//
//   1. No rules configured           → 1 branch.         (variant opt-out)
//   2. board.count_pieces() > max    → +1 compare, O(1). (midgame)
//   3. Otherwise                     → ≤64 square reads + k array-compares
//                                      where k = number of rules.
//
// Case 3 only fires in deep endgames, where it typically *saves* time by
// letting Search return 0 instead of exploring a dead subtree.

use crate::core::board::Board;
use crate::piece_config::PieceConfigManager;
use crate::zobrist::MAX_PIECE_TYPES;

/// Per-side material count, indexed by piece_type. Fixed-size so that
/// comparison is a flat 32-byte memcmp and the hot path never allocates.
type Signature = [u8; MAX_PIECE_TYPES];

const EMPTY_SIG: Signature = [0u8; MAX_PIECE_TYPES];

/// One declared dead-material combination. Colour-agnostic: a rule
/// `K vs KN` matches regardless of which colour holds the knight.
#[derive(Debug, Clone)]
struct MaterialRule {
    side_a: Signature,
    side_b: Signature,
    /// |side_a| + |side_b|. Cheap per-rule filter and also what the
    /// table-wide `max_pieces` threshold is derived from.
    total: u32,
    /// The raw text this rule was compiled from. Kept only so the
    /// startup log can echo what was loaded — not used at match time.
    #[allow(dead_code)]
    source: String,
}

/// The compiled rule table. Held by `GameState` alongside
/// `promotion_config`; cloned with it (few hundred bytes, dwarfed by
/// `move_history`).
#[derive(Debug, Clone)]
pub struct InsufficientMaterialRules {
    rules: Vec<MaterialRule>,
    /// Largest `total` across all rules. If the board has more pieces
    /// than this, no rule can possibly match — the O(1) midgame guard.
    max_pieces: u32,
}

impl InsufficientMaterialRules {
    /// An empty table. `is_draw` on this is a single branch.
    pub fn empty() -> Self {
        Self {
            rules: Vec::new(),
            max_pieces: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Does the board's current material exactly match any declared
    /// dead combination?
    ///
    /// This is the hot entry point. See the module header for the cost
    /// breakdown; the short version is "free unless you're already in a
    /// ≤ max_pieces endgame."
    #[inline]
    pub fn is_draw(&self, board: &Board) -> bool {
        // ── Guard 1: feature not in use for this variant ────────────────
        if self.rules.is_empty() {
            return false;
        }

        // ── Guard 2: too much material on the board ─────────────────────
        // count_pieces() is O(1) — it reads two cached u32s.
        let total = board.count_pieces() as u32;
        if total > self.max_pieces {
            return false;
        }

        // ── Build the two colour signatures ─────────────────────────────
        // We're below the threshold, so `total` is small (≤ max_pieces,
        // typically ≤ 4). Scan squares until we've accounted for every
        // piece, then stop — halves the average scan length.
        let (rows, cols) = board.size();
        let mut sig: [Signature; 2] = [EMPTY_SIG; 2];
        let mut found = 0u32;

        'scan: for r in 0..rows {
            for c in 0..cols {
                if let Some(p) = board.get_piece((r, c)) {
                    if p.piece_type >= MAX_PIECE_TYPES {
                        // A piece the signature can't represent. We can't
                        // decide, so we don't — treat as not-a-draw. (This
                        // path is unreachable if compile() succeeded, but
                        // costs nothing to guard.)
                        return false;
                    }
                    let slot = &mut sig[p.color.index()][p.piece_type];
                    *slot = slot.saturating_add(1);
                    found += 1;
                    if found == total {
                        break 'scan;
                    }
                }
            }
        }

        // ── Match (colour-symmetric) ─────────────────────────────────────
        for rule in &self.rules {
            if rule.total != total {
                continue;
            }
            if (sig[0] == rule.side_a && sig[1] == rule.side_b)
                || (sig[0] == rule.side_b && sig[1] == rule.side_a)
            {
                return true;
            }
        }
        false
    }

    /// Compile raw `.game`-file rule strings into a match table.
    ///
    /// `raw` is the accumulated right-hand sides of every
    /// `insufficient_material:` line. Each entry may itself contain
    /// several `;`-separated rules. Piece tokens are resolved against
    /// `config_manager` using the same character/`[name]` conventions as
    /// the `position:` string.
    ///
    /// Malformed rules are warned about and skipped; the rest survive.
    /// If the variant has more piece types than a Signature can hold,
    /// the whole feature is disabled (with a warning) rather than risk
    /// silent false positives.
    pub fn compile(raw: &[String], config_manager: &PieceConfigManager) -> Self {
        if raw.is_empty() {
            return Self::empty();
        }

        let num_types = config_manager.piece_order.len();
        if num_types > MAX_PIECE_TYPES {
            eprintln!(
                "⚠️  insufficient_material: variant has {} piece types \
(> {} supported); dead-position detection disabled.",
                num_types, MAX_PIECE_TYPES
            );
            return Self::empty();
        }

        let mut rules = Vec::new();
        let mut max_pieces = 0u32;

        for line in raw {
            // One config line may hold several rules: "K vs K; K vs KN".
            for rule_str in line.split(';') {
                let rule_str = rule_str.trim();
                if rule_str.is_empty() {
                    continue;
                }
                match compile_one_rule(rule_str, config_manager) {
                    Ok(rule) => {
                        if rule.total > 8 {
                            // Not an error, but a big rule defeats the
                            // midgame guard. Tell the user.
                            eprintln!(
                                "⚠️  insufficient_material rule `{}` has {} \
pieces — the per-move check will run more \
often than you probably want.",
                                rule_str, rule.total
                            );
                        }
                        max_pieces = max_pieces.max(rule.total);
                        rules.push(rule);
                    }
                    Err(e) => {
                        eprintln!(
                            "⚠️  insufficient_material: skipping rule `{}`: {}",
                            rule_str, e
                        );
                    }
                }
            }
        }

        if !rules.is_empty() {
            println!(
                "⚖️  Insufficient-material: {} rule(s) loaded, \
active when ≤ {} pieces remain.",
                rules.len(),
                max_pieces
            );
        }

        Self { rules, max_pieces }
    }
}

impl Default for InsufficientMaterialRules {
    fn default() -> Self {
        Self::empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Parsing
// ─────────────────────────────────────────────────────────────────────────

fn compile_one_rule(
    text: &str,
    config_manager: &PieceConfigManager,
) -> Result<MaterialRule, String> {
    // Accept "vs" (case-insensitive, any surrounding whitespace) or "|"
    // as the side separator. We search for the *word* "vs" so a piece
    // whose bracketed name happens to contain those letters isn't split.
    let (left, right) =
        split_sides(text).ok_or_else(|| "missing side separator (use `vs` or `|`)".to_string())?;

    let side_a = parse_side(left, config_manager)?;
    let side_b = parse_side(right, config_manager)?;

    let total: u32 = side_a.iter().map(|&n| n as u32).sum::<u32>()
        + side_b.iter().map(|&n| n as u32).sum::<u32>();

    if total == 0 {
        return Err("rule has no pieces on either side".into());
    }

    Ok(MaterialRule {
        side_a,
        side_b,
        total,
        source: text.to_string(),
    })
}

/// Split `"K vs KN"` / `"K|KN"` into `("K", "KN")`. `vs` is matched only
/// outside `[...]` brackets so `[Vassal] vs K` parses correctly.
fn split_sides(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'|' if depth == 0 => {
                return Some((text[..i].trim(), text[i + 1..].trim()));
            }
            b'v' | b'V' if depth == 0 => {
                // Match the word "vs": must be followed by 's' and bounded
                // by non-alphanumerics (or string ends) on both sides.
                let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                let has_s = i + 1 < bytes.len() && (bytes[i + 1] == b's' || bytes[i + 1] == b'S');
                let next_ok = i + 2 >= bytes.len() || !bytes[i + 2].is_ascii_alphanumeric();
                if prev_ok && has_s && next_ok {
                    return Some((text[..i].trim(), text[i + 2..].trim()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse one side of a rule — e.g. `"KN"` or `"K[Archbishop]"` — into a
/// Signature. Whitespace between tokens is allowed and ignored.
fn parse_side(text: &str, config_manager: &PieceConfigManager) -> Result<Signature, String> {
    let mut sig = EMPTY_SIG;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }

        let piece_type = if ch == '[' {
            // Bracketed full name, mirroring the `position:` syntax.
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == ']' {
                    break;
                }
                name.push(c);
            }
            find_piece_by_name(&name, config_manager)
                .ok_or_else(|| format!("unknown piece name `[{}]`", name))?
        } else if ch.is_ascii_alphabetic() {
            find_piece_by_char(ch, config_manager)
                .ok_or_else(|| format!("unknown piece character `{}`", ch))?
        } else {
            return Err(format!("unexpected character `{}`", ch));
        };

        if piece_type >= MAX_PIECE_TYPES {
            // Can't happen given the guard in compile(), but stay safe.
            return Err(format!("piece type index {} out of range", piece_type));
        }
        sig[piece_type] = sig[piece_type].saturating_add(1);
    }

    Ok(sig)
}

/// Resolve a single FEN-style piece character to a piece-type index.
/// Same lookup order as `BoardConfig::find_piece_type`: explicit
/// `characters` list first, then first-letter-of-name fallback.
fn find_piece_by_char(ch: char, cm: &PieceConfigManager) -> Option<usize> {
    let upper = ch.to_ascii_uppercase().to_string();
    for (idx, name) in cm.piece_order.iter().enumerate() {
        if let Some(cfg) = cm.pieces.get(name) {
            if cfg.characters.iter().any(|c| c == &upper) {
                return Some(idx);
            }
        }
    }
    let lower = ch.to_ascii_lowercase();
    cm.piece_order
        .iter()
        .position(|name| name.starts_with(lower))
}

/// Resolve a bracketed piece name. Case-insensitive; tries internal key,
/// then `display_name`, then any `texture_name`.
fn find_piece_by_name(name: &str, cm: &PieceConfigManager) -> Option<usize> {
    let needle = name.to_lowercase();
    if let Some(idx) = cm.piece_order.iter().position(|n| n == &needle) {
        return Some(idx);
    }
    for (idx, key) in cm.piece_order.iter().enumerate() {
        if let Some(cfg) = cm.pieces.get(key) {
            if cfg.display_name.to_lowercase() == needle
                || cfg.texture_names.iter().any(|t| t.to_lowercase() == needle)
            {
                return Some(idx);
            }
        }
    }
    None
}
