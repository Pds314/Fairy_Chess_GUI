// src/notation.rs
use crate::core::Position;

/// Convert a file (column) index to algebraic notation
/// Supports up to 26 columns (a-z), then continues with aa, ab, etc.
pub fn file_to_algebraic(col: usize) -> String {
    if col < 26 {
        // Single letter: a-z
        ((b'a' + col as u8) as char).to_string()
    } else {
        // Multiple letters: aa, ab, ..., ba, bb, ...
        let first = col / 26 - 1;
        let second = col % 26;
        format!(
            "{}{}",
            (b'a' + first as u8) as char,
            (b'a' + second as u8) as char
        )
    }
}

/// Convert a rank (row) index to algebraic notation
/// Row 0 is the highest rank, so for an 8x8 board: row 0 = rank 8, row 7 = rank 1
pub fn rank_to_algebraic(row: usize, board_rows: usize) -> String {
    (board_rows - row).to_string()
}

/// Convert a position to algebraic notation
pub fn position_to_algebraic(pos: Position, board_size: (usize, usize)) -> String {
    format!(
        "{}{}",
        file_to_algebraic(pos.1),
        rank_to_algebraic(pos.0, board_size.0)
    )
}

/// Parse algebraic file notation to column index
pub fn algebraic_to_file(file_str: &str) -> Option<usize> {
    let chars: Vec<char> = file_str.chars().collect();

    match chars.len() {
        0 => None,
        1 => {
            let ch = chars[0];
            if ch >= 'a' && ch <= 'z' {
                Some((ch as u8 - b'a') as usize)
            } else {
                None
            }
        }
        2 => {
            let first = chars[0];
            let second = chars[1];
            if first >= 'a' && first <= 'z' && second >= 'a' && second <= 'z' {
                let first_val = (first as u8 - b'a') as usize + 1;
                let second_val = (second as u8 - b'a') as usize;
                Some(first_val * 26 + second_val)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse algebraic rank notation to row index
pub fn algebraic_to_rank(rank_str: &str, board_rows: usize) -> Option<usize> {
    rank_str.parse::<usize>().ok().and_then(|rank| {
        if rank > 0 && rank <= board_rows {
            Some(board_rows - rank)
        } else {
            None
        }
    })
}

/// Parse full algebraic notation to position
pub fn algebraic_to_position(notation: &str, board_size: (usize, usize)) -> Option<Position> {
    // Split file and rank
    let mut file_part = String::new();
    let mut rank_part = String::new();

    for ch in notation.chars() {
        if ch.is_ascii_lowercase() {
            file_part.push(ch);
        } else if ch.is_ascii_digit() {
            rank_part.push(ch);
        }
    }

    let col = algebraic_to_file(&file_part)?;
    let row = algebraic_to_rank(&rank_part, board_size.0)?;

    if row < board_size.0 && col < board_size.1 {
        Some((row, col))
    } else {
        None
    }
}
