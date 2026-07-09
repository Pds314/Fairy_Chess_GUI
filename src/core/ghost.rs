// src/core/ghost.rs
//! Ghosts: squares that alias a piece standing somewhere else.
//!
//! A ghost is created by a movement step carrying a ghost marker (`e`, `E`,
//! `&`, `'`). It names the square the step **departed from** and points at the
//! square the moving piece **finally landed on**. Everything en-passant-like
//! and everything castling-transit-like is this one primitive.
//!
//! ── Two behaviours, three bits ───────────────────────────────────────────
//!
//! * **Capture alias.** Landing on `square` captures the piece on `owner`.
//!   `CAPTURE_OPEN` = any capture-capable pattern; `CAPTURE_EP` = only
//!   patterns carrying `~`. This is en passant.
//!
//! * **Castle target.** A castling partner may land on `square`. Because the
//!   flag lives on the *ghost*, a rook destination is necessarily also a
//!   transit assertion — you cannot declare one without the other.
//!
//! * **Royalty projection is NOT a flag.** It is derived: if the ghost's owner
//!   is royal (or the last remaining royalty piece), the square counts as
//!   occupied by that piece for check detection. See
//!   `GameState::ghost_projects_royalty`.
//!
//! ── Lifetime ─────────────────────────────────────────────────────────────
//!
//! No `born_ply`, no `lifetime`. Ghosts live in an append-only stack; the live
//! set is the tail created by the most recent transaction. That is
//! simultaneously the **0-ply** set (a castling transit assertion, consumed by
//! `mover_king_in_check` inside the same make/undo pair) and the **1-ply** set
//! (an en-passant capture alias, consumed by the opponent's move generation).
//! Undo is `Vec::truncate`.

use crate::core::position::Position;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};

// ─────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct GhostFlags(u8);

impl GhostFlags {
    /// No behaviour of its own. Still projects royalty if its owner is royal,
    /// which is the entire point of a castling-transit ghost (DSL: `'`).
    pub const NONE: Self = GhostFlags(0);
    /// Any capture-capable pattern landing here captures `owner` (DSL: `E`).
    pub const CAPTURE_OPEN: Self = GhostFlags(1 << 0);
    /// Only patterns with `~` landing here capture `owner` (DSL: `e`).
    pub const CAPTURE_EP: Self = GhostFlags(1 << 1);
    /// A castling partner may land on this square (DSL: `&`).
    pub const CASTLE_TARGET: Self = GhostFlags(1 << 2);

    #[inline(always)]
    pub const fn bits(self) -> u8 {
        self.0
    }
    #[inline(always)]
    pub const fn from_bits(b: u8) -> Self {
        GhostFlags(b)
    }
    #[inline(always)]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    #[inline(always)]
    pub const fn union(self, other: Self) -> Self {
        GhostFlags(self.0 | other.0)
    }
    /// Does this ghost provide *any* capture alias?
    #[inline(always)]
    pub const fn has_capture_alias(self) -> bool {
        (self.0 & (Self::CAPTURE_OPEN.0 | Self::CAPTURE_EP.0)) != 0
    }

    /// The DSL suffixes that would produce these flags, e.g. `"E&"`.
    pub fn dsl_marks(self) -> String {
        let mut s = String::new();
        if self.contains(Self::CAPTURE_EP) {
            s.push('e');
        }
        if self.contains(Self::CAPTURE_OPEN) {
            s.push('E');
        }
        if self.contains(Self::CASTLE_TARGET) {
            s.push('&');
        }
        if s.is_empty() {
            s.push('\'');
        }
        s
    }

    /// One character for board overlays. Castle target wins, then open
    /// capture, then restricted capture, then the bare transit ghost.
    pub fn glyph(self) -> char {
        if self.contains(Self::CASTLE_TARGET) {
            '&'
        } else if self.contains(Self::CAPTURE_OPEN) {
            'E'
        } else if self.contains(Self::CAPTURE_EP) {
            'e'
        } else {
            '\''
        }
    }
}

impl BitOr for GhostFlags {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}
impl BitOrAssign for GhostFlags {
    #[inline(always)]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for GhostFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "NONE");
        }
        let mut first = true;
        let mut tag = |f: &mut fmt::Formatter<'_>, s: &str| -> fmt::Result {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            write!(f, "{}", s)
        };
        if self.contains(Self::CAPTURE_OPEN) {
            tag(f, "CAPTURE_OPEN")?;
        }
        if self.contains(Self::CAPTURE_EP) {
            tag(f, "CAPTURE_EP")?;
        }
        if self.contains(Self::CASTLE_TARGET) {
            tag(f, "CASTLE_TARGET")?;
        }
        Ok(())
    }
}

impl fmt::Debug for GhostFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Ghost
// ─────────────────────────────────────────────────────────────────────────

/// 5 bytes, `align 1`. Coordinates are `u8` because `MAX_BOARD_SIZE == 32`;
/// storing them unpacked avoids threading `cols` through every call site.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ghost {
    square: (u8, u8),
    owner: (u8, u8),
    flags: GhostFlags,
}

const _: () = assert!(std::mem::size_of::<Ghost>() <= 8, "Ghost must fit in 64 bits");

impl Ghost {
    #[inline]
    pub fn new(square: Position, owner: Position, flags: GhostFlags) -> Self {
        Ghost {
            square: (square.0 as u8, square.1 as u8),
            owner: (owner.0 as u8, owner.1 as u8),
            flags,
        }
    }

    #[inline(always)]
    pub fn square(&self) -> Position {
        (self.square.0 as usize, self.square.1 as usize)
    }
    #[inline(always)]
    pub fn owner(&self) -> Position {
        (self.owner.0 as usize, self.owner.1 as usize)
    }
    #[inline(always)]
    pub fn flags(&self) -> GhostFlags {
        self.flags
    }

    #[inline(always)]
    pub fn has_capture_alias(&self) -> bool {
        self.flags.has_capture_alias()
    }

    /// May a mover whose pattern has (or hasn't) the `~` suffix capture the
    /// owner by landing on this square?
    #[inline(always)]
    pub fn allows_capture(&self, mover_captures_en_passant: bool) -> bool {
        self.flags.contains(GhostFlags::CAPTURE_OPEN)
        || (mover_captures_en_passant && self.flags.contains(GhostFlags::CAPTURE_EP))
    }

    #[inline(always)]
    pub fn is_castle_target(&self) -> bool {
        self.flags.contains(GhostFlags::CASTLE_TARGET)
    }
}

impl fmt::Debug for Ghost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Ghost({:?} -> {:?}, {})",
               self.square(),
               self.owner(),
               self.flags
        )
    }
}
