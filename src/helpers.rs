// In your helpers module
// Swapped PerformanceTracker out for PerformanceSnapshot here
use crate::core::game_state::{ExpandedMove, MoveGenerationResult, PerformanceSnapshot};
use crate::core::position::Position;
use crate::notation::position_to_algebraic;
use crate::piece_config::PieceConfigManager;
use crate::clog;
use std::collections::HashMap;

pub fn format_move(
    mv: &ExpandedMove,
    config_manager: &PieceConfigManager,
    board_size: (usize, usize),
) -> String {
    let from_notation = position_to_algebraic(mv.from, board_size);
    let to_notation = position_to_algebraic(mv.to, board_size);
    let mut result = from_notation;
    if mv.captures.is_some() {
        result.push('x');
    } else {
        result.push('-');
    }
    result.push_str(&to_notation);
    if let Some(ref castling) = mv.castling_option {
        let rook_notation = position_to_algebraic(castling.rook_from, board_size);
        result.push_str(&format!(" (castle with {})", rook_notation));
    }
    if let Some(promo_type) = mv.promotion_target {
        if let Some(piece_config) = config_manager.get_piece_by_index(promo_type) {
            result.push_str(&format!("={}", piece_config.display_name));
        }
    }
    if mv.captures_position != mv.captures.map(|_| mv.to) {
        result.push_str(" e.p.");
    }
    result
}

pub fn print_move_generation_results(
    result: &MoveGenerationResult,
    config_manager: &PieceConfigManager,
    board_size: (usize, usize),
) {
    match result {
        MoveGenerationResult::Moves(moves) => {
            clog!("\n=== Pseudo-legal Moves ({} total) ===", moves.len());
            let mut moves_by_from: HashMap<Position, Vec<&ExpandedMove>> = HashMap::new();
            for mv in moves {
                moves_by_from.entry(mv.from).or_default().push(mv);
            }
            let mut positions: Vec<_> = moves_by_from.keys().cloned().collect();
            positions.sort_by_key(|&(r, c)| (r, c));
            for pos in positions {
                if let Some(moves) = moves_by_from.get(&pos) {
                    clog!(
                        "\nFrom {}: ({} moves)",
                          position_to_algebraic(pos, board_size),
                          moves.len()
                    );
                    for mv in moves {
                        let mut line =
                        format!("  {}", format_move(mv, config_manager, board_size));
                        if let Some(captured) = mv.captures {
                            if let Some(cap_config) =
                                config_manager.get_piece_by_index(captured.piece_type)
                                {
                                    line.push_str(&format!(" (captures {})", cap_config.display_name));
                                }
                        }
                        clog!("{}", line);
                    }
                }
            }
        }
        MoveGenerationResult::Checkmate {
            move_that_captures_royal,
        } => {
            clog!("\n=== CHECKMATE - Royal Can Be Captured! ===");
            clog!(
                "Fatal move: {}",
                format_move(move_that_captures_royal, config_manager, board_size)
            );
            if let Some(captured) = move_that_captures_royal.captures {
                if let Some(royal_config) = config_manager.get_piece_by_index(captured.piece_type) {
                    clog!(
                        "{} {} can be captured!",
                        if royal_config.properties.is_royal {
                            "Royal (R)"
                        } else {
                            "Royalty (r)"
                        },
                        royal_config.display_name
                    );
                }
            }
            clog!(
                "\nThis position is illegal - the previous player left their royal piece in check!"
            );
        }
    }
}

// Updated argument to accept `&PerformanceSnapshot` instead
pub fn print_search_stats(stats: &PerformanceSnapshot, duration: std::time::Duration) {
    clog!("\n--- Search Statistics ---");
    clog!("Time elapsed: {:.2?}", duration);
    clog!("Moves generated: {}", stats.moves_generated);
    clog!(
        "Moves made/undone: {}/{}",
        stats.moves_made,
        stats.moves_undone
    );
    clog!(
        "Pseudo-legal generations: {}",
        stats.pseudo_legal_generations
    );
    clog!("Legal move checks: {}", stats.legal_move_checks);
    clog!("Check tests: {}", stats.check_tests);
    clog!("Mate status checks: {}", stats.mate_status_checks);
    let total_operations = stats.moves_made + stats.pseudo_legal_generations + stats.check_tests;
    if duration.as_millis() > 0 {
        let ops_per_sec = (total_operations as f64 * 1000.0) / duration.as_millis() as f64;
        clog!("Operations/second: {:.0}", ops_per_sec);
        let moves_per_sec = (stats.moves_generated as f64 * 1000.0) / duration.as_millis() as f64;
        clog!("Moves generated/second: {:.0}", moves_per_sec);
    }
}
