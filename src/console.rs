// src/console.rs
//! Unified logging: one global, thread-safe line buffer feeding both the
//! OS stdio console (via `println!`) and the in-app GUI console widget.
//!
//! This used to be `thread_local!`, which was a real bug: tournament and
//! (now) evolution games run their engines on background `std::thread`
//! workers, so any logging done while a worker thread is "in" an engine's
//! `best_move()` call would accumulate in *that thread's own* buffer and
//! never be visible to the GUI thread's `recent_lines()` — the two
//! "consoles" would silently diverge.
//!
//! The fix is to make the buffer crate-global behind a `Mutex`, so every
//! thread in the process (GUI thread, tournament workers, evolution
//! workers) contributes to and reads from the same ledger. Because the
//! write path is a single `Vec`/`VecDeque` push under a short-lived lock,
//! contention is a non-issue at the rates logging actually happens.
//!
//! Going forward, `clog!` is the standard drop-in replacement for
//! `println!` anywhere in this codebase: same call syntax, same stdout
//! output, plus automatic visibility in the GUI console — including from
//! tournament workers, evolution workers, and any future background
//! machinery. There is no reason left to reach for a bare `println!`.
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
const MAX_CONSOLE_LINES: usize = 1000;
fn buffer() -> &'static Mutex<VecDeque<String>> {
    static BUF: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_CONSOLE_LINES)))
}
/// Log a message to both stdout and the shared in-app console buffer.
/// Drop-in replacement for `println!` — use this everywhere instead.
#[macro_export]
macro_rules! clog {
    () => {{
        println!();
        $crate::console::push_line(String::new());
    }};
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("{}", msg);
        $crate::console::push_line(msg);
    }};
}
/// Append one line to the shared buffer, evicting the oldest line if the
/// buffer is full. Safe to call from any thread.
pub fn push_line(line: String) {
    if let Ok(mut buf) = buffer().lock() {
        if buf.len() >= MAX_CONSOLE_LINES {
            buf.pop_front();
        }
        buf.push_back(line);
    }
    // A poisoned mutex (a prior panic while holding the lock) just means
    // we silently drop this line rather than taking down the whole
    // process over a logging hiccup.
}
/// Return the most recent `n` lines from the buffer (chronological
/// order). Safe to call from any thread, though in practice only the GUI
/// thread does (to render the console widget).
pub fn recent_lines(n: usize) -> Vec<String> {
    match buffer().lock() {
        Ok(buf) => {
            let start = buf.len().saturating_sub(n);
            buf.iter().skip(start).cloned().collect()
        }
        Err(_) => Vec::new(),
    }
}
/// Clear the console buffer. Stdout history is obviously untouched.
pub fn clear() {
    if let Ok(mut buf) = buffer().lock() {
        buf.clear();
    }
}
