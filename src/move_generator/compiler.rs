//! DSL notation compiler.
//!
//! ── Ghost markers (step suffixes) ────────────────────────────────────────
//!
//! A marked step leaves a ghost on **the square it departed from**, aliasing
//! the piece's final destination. Markers compose.
//!
//!   `e`  CAPTURE_EP      only `~` movers may capture the owner here
//!   `E`  CAPTURE_OPEN    any capture-capable mover may
//!   `&`  CASTLE_TARGET   a castling partner may land here
//!   `'`  (bare ghost)    no behaviour of its own — but it still projects
//!                        royalty if its owner is royal, which is all a
//!                        castling transit assertion needs
//!
//! Royalty projection is *derived from the owner*, never declared. A pawn's
//! ghost never projects; a king's always does.
//!
//! `&` previously meant `partner_lands_here` on the square a step *arrived*
//! on. It now marks the square the step *departed*, so that a rook destination
//! is necessarily also a transit assertion. No shipped `.pieces` file uses
//! `&`; `FIDE.pieces` keeps working via the legacy fallback in
//! `castle_target_squares`.

use crate::core::ghost::GhostFlags;
use crate::move_generator::types::*;
use crate::piece_config::PieceConfigManager;
use smallvec::SmallVec;

pub(crate) fn compile_moveset(
    moveset: &str,
    cm: &PieceConfigManager,
) -> Result<Vec<CompiledMove>, String> {
    moveset.split(',').map(|s| compile_single_move(s, cm)).collect()
}

fn compile_single_move(move_str: &str, cm: &PieceConfigManager) -> Result<CompiledMove, String> {
    fn has_more_steps(it: &std::iter::Peekable<std::str::Chars>) -> bool {
        it.clone().any(|c| c == '+' || c == 'x')
    }

    let mut compiled = CompiledMove::default();
    let mut chars = move_str.chars().peekable();

    // Null / pass move
    if move_str.trim() == "?" {
        compiled.steps.push(MoveStep {
            directions: smallvec::smallvec![(0i8, 0i8)],
                            length: 1,
                            is_optional_stop: true,
                            ..Default::default()
        });
        compiled.can_land_friendly = true;
        return Ok(compiled);
    }

    loop {
        // ── From-zone prefix ────────────────────────────────────────
        if compiled.steps.is_empty() {
            skip_ws(&mut chars);
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
        skip_ws(&mut chars);

        let mut step = MoveStep::default();
        let mut add_dirs: SmallVec<[(i8, i8); 4]> = SmallVec::new();
        let mut sub_dirs: SmallVec<[(i8, i8); 4]> = SmallVec::new();
        let mut is_subtractive = false;

        // ── Direction / modifier prefix ─────────────────────────────
        loop {
            match chars.peek() {
                Some('-') => { is_subtractive = true; chars.next(); }
                Some('=') => { step.lock_to_previous_direction = true; chars.next(); }
                Some('^') => { push_unique(if is_subtractive { &mut sub_dirs } else { &mut add_dirs }, (-1, 0)); chars.next(); }
                Some('v') => { push_unique(if is_subtractive { &mut sub_dirs } else { &mut add_dirs }, (1, 0)); chars.next(); }
                Some('<') => { push_unique(if is_subtractive { &mut sub_dirs } else { &mut add_dirs }, (0, -1)); chars.next(); }
                Some('>') => { push_unique(if is_subtractive { &mut sub_dirs } else { &mut add_dirs }, (0, 1)); chars.next(); }
                Some(c) if c.is_ascii_digit() => {
                    let mut n = String::new();
                    while let Some(d) = chars.peek() {
                        if d.is_ascii_digit() { n.push(chars.next().unwrap()); } else { break; }
                    }
                    step.length = n.parse().unwrap_or(1);
                }
                _ => break,
            }
        }

        // ── Base move character ─────────────────────────────────────
        let base_move = match chars.peek() {
            Some(c) if *c == '+' || *c == 'x' => chars.next().unwrap(),
            _ => break,
        };

        let base_dirs: &[(i8, i8)] = if base_move == '+' {
            &[(0, 1), (1, 0), (0, -1), (-1, 0)]
        } else {
            &[(1, 1), (1, -1), (-1, 1), (-1, -1)]
        };

        for &dir in base_dirs {
            let included = add_dirs.is_empty()
            || add_dirs.iter().any(|ad| (ad.0 == 0 || ad.0 == dir.0) && (ad.1 == 0 || ad.1 == dir.1));
            let excluded = sub_dirs.iter().any(|sd| (sd.0 == 0 || sd.0 == dir.0) && (sd.1 == 0 || sd.1 == dir.1));
            if included && !excluded && !step.directions.contains(&dir) {
                step.directions.push(dir);
            }
        }

        // ── Step suffixes ───────────────────────────────────────────
        let mut pass_overridden = false;
        let mut rep_overridden = false;

        loop {
            match chars.peek() {
                Some('?') => { step.is_optional_stop = true; chars.next(); }
                Some('#') => { step.resets_center = true; chars.next(); }

                // ── Ghost markers ────────────────────────────────
                Some('e') => { add_ghost(&mut step, GhostFlags::CAPTURE_EP); chars.next(); }
                Some('E') => { add_ghost(&mut step, GhostFlags::CAPTURE_OPEN); chars.next(); }
                Some('&') => { add_ghost(&mut step, GhostFlags::CASTLE_TARGET); chars.next(); }
                Some('\'') => { add_ghost(&mut step, GhostFlags::NONE); chars.next(); }

                Some('%') => {
                    chars.next();
                    let mut saw_target = false;
                    loop {
                        match chars.peek() {
                            Some('!') => { step.captures_in_flight_enemy = true; saw_target = true; chars.next(); }
                            Some('@') => { step.captures_in_flight_friendly = true; saw_target = true; chars.next(); }
                            _ => break,
                        }
                    }
                    if !saw_target {
                        step.captures_in_flight_enemy = true;
                    }
                }

                Some('*') => {
                    step.is_repeatable = true;
                    chars.next();
                    let mut n = String::new();
                    while let Some(d) = chars.peek() {
                        if d.is_ascii_digit() { n.push(chars.next().unwrap()); } else { break; }
                    }
                    step.max_repetitions = n.parse().ok();

                    while let Some(&c) = chars.peek() {
                        if c == '_' || c == '!' || c == '@' {
                            if !rep_overridden { step.repetition_permissions = Permissions::none(); rep_overridden = true; }
                            match c {
                                '_' => step.repetition_permissions.can_pass_empty = true,
                                '!' => step.repetition_permissions.can_pass_enemy = true,
                                '@' => step.repetition_permissions.can_pass_friendly = true,
                                _ => {}
                            }
                            chars.next();
                        } else { break; }
                    }
                }

                Some('_') | Some('!') | Some('@') => {
                    let mut it = chars.clone();
                    it.next();
                    if has_more_steps(&it) || it.peek() == Some(&'*') {
                        if !pass_overridden { step.permissions = Permissions::none(); pass_overridden = true; }
                        match chars.next().unwrap() {
                            '_' => step.permissions.can_pass_empty = true,
                            '!' => step.permissions.can_pass_enemy = true,
                            '@' => step.permissions.can_pass_friendly = true,
                            _ => {}
                        }
                    } else { break; }
                }
                _ => break,
            }
        }

        // A step cannot capture-in-flight what it may not traverse. Forced
        // AFTER suffix parsing so a later `_`/`!`/`@` override cannot clobber it.
        if step.captures_in_flight_enemy {
            step.permissions.can_pass_enemy = true;
        }
        if step.captures_in_flight_friendly {
            step.permissions.can_pass_friendly = true;
        }

        compiled.steps.push(step);
    }

    // ── Pattern-level suffixes ──────────────────────────────────────
    let mut land_overridden = false;
    while let Some(suffix) = chars.peek().cloned() {
        match suffix {
            'u' => { compiled.requires_unmoved = true; chars.next(); }
            'i' => { compiled.is_irreversible = true; chars.next(); }
            '~' => { compiled.captures_en_passant = true; chars.next(); }
            'o' => { compiled.is_king_castle = true; chars.next(); }
            'O' => { compiled.is_rook_castle = true; chars.next(); }
            '!' | '_' | '@' => {
                if !land_overridden {
                    compiled.can_land_empty = false;
                    compiled.can_land_enemy = false;
                    compiled.can_land_friendly = false;
                    land_overridden = true;
                }
                let c = chars.next().unwrap();
                match c {
                    '!' => {
                        compiled.can_land_enemy = true;
                        if chars.peek() == Some(&'{') {
                            chars.next();
                            let mut name = String::new();
                            for ch in chars.by_ref() {
                                if ch == '}' { break; }
                                name.push(ch);
                            }
                            let name = name.trim();
                            compiled.capture_filter = resolve_piece_ref(name, cm);
                            if compiled.capture_filter.is_none() {
                                return Err(format!("capture filter !{{{}}}: unknown piece", name));
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
                    if c == ']' { break; }
                    name.push(c);
                }
                compiled.to_zone = Some(name.trim().to_string());
            }
            c if c.is_whitespace() => { chars.next(); }
            _ => break,
        }
    }

    compiled.has_zones = compiled.from_zone.is_some() || compiled.to_zone.is_some();
    compiled.has_flight_capture = compiled
    .steps
    .iter()
    .any(|s| s.captures_in_flight_enemy || s.captures_in_flight_friendly);

    Ok(compiled)
}

// ─── Helpers ────────────────────────────────────────────────────────────

#[inline]
fn add_ghost(step: &mut MoveStep, flag: GhostFlags) {
    let base = step.creates_ghost.unwrap_or(GhostFlags::NONE);
    step.creates_ghost = Some(base | flag);
}

fn skip_ws(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars.peek().map_or(false, |c| c.is_whitespace()) {
        chars.next();
    }
}

fn push_unique(v: &mut SmallVec<[(i8, i8); 4]>, d: (i8, i8)) {
    if !v.contains(&d) {
        v.push(d);
    }
}

pub(crate) fn resolve_piece_ref(s: &str, cm: &PieceConfigManager) -> Option<usize> {
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
