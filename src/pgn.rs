// src/pgn.rs
use crate::core::game_state::{ExpandedMove, GameMove};
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::core::GameState;
use crate::move_generator::MoveGenerator;
use crate::notation::position_to_algebraic;
use crate::piece_config::PieceConfigManager;
use std::collections::HashMap;

pub struct PgnExporter;

impl PgnExporter {
    pub fn export_game(
        initial_state: &GameState,
        moves: &[GameMove],
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
        game_file: Option<&str>,
    ) -> String {
        let mut pgn = String::new();
        pgn.push_str("[Event \"Fairy Chess Game\"]\n");
        pgn.push_str("[Site \"Fairy Chess GUI\"]\n");
        let date = chrono::Local::now().format("%Y.%m.%d").to_string();
        pgn.push_str(&format!("[Date \"{}\"]\n", date));
        pgn.push_str("[White \"Player 1\"]\n");
        pgn.push_str("[Black \"Player 2\"]\n");
        let result = Self::determine_result(initial_state);
        pgn.push_str(&format!("[Result \"{}\"]\n", result));
        if let Some(file) = game_file {
            pgn.push_str(&format!("[Setup \"{}\"]\n", file));
        }
        let fen = initial_state.get_position_string(config_manager);
        if !Self::is_standard_position(&fen) {
            pgn.push_str(&format!("[FEN \"{}\"]\n", fen));
        }
        pgn.push('\n');
        pgn.push_str(&Self::format_moves(initial_state, moves, move_generator, config_manager));
        pgn.push_str(&format!(" {}\n", result));
        pgn
    }

    pub fn format_moves(
        initial_state: &GameState,
        moves: &[GameMove],
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> String {
        let mut result = String::new();
        let mut state = initial_state.clone();
        let mut move_number = 1;

        for (i, game_move) in moves.iter().enumerate() {
            if i % 2 == 0 {
                result.push_str(&format!("{}. ", move_number));
            }
            result.push_str(&Self::move_to_algebraic(&state, game_move, move_generator, config_manager));

            if let Some(piece) = state.board.get_piece(game_move.from) {
                if let Some(mwp) = move_generator.get_move_rule(&state.board, game_move.from, game_move.to, piece.piece_type) {
                    if let Some((rook_from, rook_to)) = game_move.castling_rook_move() {
                        if let Some(rook_piece) = state.board.get_piece(rook_from) {
                            let option = crate::move_generator::CastlingOption {
                                king_to: game_move.to, rook_from, rook_to, rook_piece,
                            };
                            state.execute_castling(game_move.from, game_move.to, &mwp, &option, config_manager, move_generator);
                        }
                    } else {
                        state.make_move(game_move.from, game_move.to, &mwp, config_manager, game_move.promoted_to());
                    }
                }
            }

            if i % 2 == 1 {
                move_number += 1;
                if move_number % 4 == 1 { result.push('\n'); } else { result.push(' '); }
            } else {
                result.push(' ');
            }
        }
        result.trim().to_string()
    }

    fn move_to_algebraic(
        state: &GameState,
        game_move: &GameMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> String {
        let board_size = state.board.size();
        let from = game_move.from;
        let to = game_move.to;

        let Some(piece) = state.board.get_piece(from) else { return "??".to_string() };
        let Some(piece_config) = config_manager.get_piece_by_index(piece.piece_type) else { return "??".to_string() };

        let mut notation = String::new();

        if let Some((rook_from, _)) = game_move.castling_rook_move() {
            if rook_from.1 > from.1 { notation.push_str("O-O"); } else { notation.push_str("O-O-O"); }
        } else {
            let piece_symbol = piece_config.characters.first()
            .and_then(|s| s.chars().next()).unwrap_or('?').to_ascii_uppercase();
            if piece_symbol != 'P' { notation.push(piece_symbol); }

            let disambiguation = Self::get_disambiguation(state, from, to, piece.piece_type, move_generator);
            notation.push_str(&disambiguation);

            if game_move.is_capture() {
                if piece_symbol == 'P' && disambiguation.is_empty() {
                    notation.push((b'a' + from.1 as u8) as char);
                }
                notation.push('x');
            }
            notation.push_str(&position_to_algebraic(to, board_size));

            if let Some(promoted_type) = game_move.promoted_to() {
                if let Some(pc) = config_manager.get_piece_by_index(promoted_type) {
                    notation.push('=');
                    notation.push(pc.characters.first().and_then(|s| s.chars().next()).unwrap_or('?').to_ascii_uppercase());
                }
            }

            // Informational only; never parsed back. The exact squares live on
            // the event tape, not in the 64-byte frame.
            if game_move.flight_capture_count() > 0 {
                notation.push_str(&format!(">>x{}", game_move.flight_capture_count()));
            }
            if game_move.is_en_passant_capture() {
                notation.push_str(" e.p.");
            }
        }

        let mut test_state = state.clone();
        if let Some(piece) = test_state.board.get_piece(from) {
            if let Some(mwp) = move_generator.get_move_rule(&test_state.board, from, to, piece.piece_type) {
                if let Some((rook_from, rook_to)) = game_move.castling_rook_move() {
                    if let Some(rook_piece) = test_state.board.get_piece(rook_from) {
                        let option = crate::move_generator::CastlingOption { king_to: to, rook_from, rook_to, rook_piece };
                        test_state.execute_castling(from, to, &mwp, &option, config_manager, move_generator);
                    }
                } else {
                    test_state.make_move(from, to, &mwp, config_manager, game_move.promoted_to());
                }
                if test_state.is_in_check(move_generator, config_manager) {
                    if test_state.get_legal_moves(move_generator, config_manager).is_empty() {
                        notation.push('#');
                    } else {
                        notation.push('+');
                    }
                }
            }
        }
        notation
    }

    fn get_disambiguation(
        state: &GameState,
        from: Position,
        to: Position,
        piece_type: usize,
        move_generator: &MoveGenerator,
    ) -> String {
        let Some(moving_piece) = state.board.get_piece(from) else { return String::new() };
        let mut same: Vec<Position> = Vec::new();

        for row in 0..state.board.rows() {
            for col in 0..state.board.cols() {
                let pos = (row, col);
                if pos == from { continue; }
                if let Some(piece) = state.board.get_piece(pos) {
                    if piece.piece_type == piece_type && piece.color == moving_piece.color
                        && move_generator.get_move_rule(&state.board, pos, to, piece_type).is_some()
                        {
                            same.push(pos);
                        }
                }
            }
        }
        if same.is_empty() { return String::new(); }

        let mut need_file = false;
        let mut need_rank = false;
        for &o in &same {
            if o.1 == from.1 { need_rank = true; }
            if o.0 == from.0 { need_file = true; }
        }
        if !need_file && !need_rank { need_file = true; }

        let mut result = String::new();
        if need_file { result.push((b'a' + from.1 as u8) as char); }
        if need_rank { result.push_str(&(state.board.rows() - from.0).to_string()); }
        result
    }

    fn is_standard_position(fen: &str) -> bool {
        fen == "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"
    }

    fn determine_result(state: &GameState) -> &'static str {
        match &state.game_result {
            Some(crate::core::GameResult::Winner(PieceColor::White)) => "1-0",
            Some(crate::core::GameResult::Winner(PieceColor::Black)) => "0-1",
            Some(crate::core::GameResult::Draw(_)) => "1/2-1/2",
            Some(crate::core::GameResult::Ongoing) => "*",
            None => "*",
        }
    }
}

// ═══════════════════════════════════════════════════════
// PGN IMPORTER
// ═══════════════════════════════════════════════════════

struct ParsedAlgebraic {
    piece_symbol: char,
    from_file: Option<usize>,
    from_rank: Option<usize>,
    destination: Position,
    #[allow(dead_code)]
    is_capture: bool,
    promotion_symbol: Option<char>,
}

pub struct PgnImporter;

impl PgnImporter {
    pub fn load_pgn(
        pgn_text: &str,
        state: &mut GameState,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Result<usize, String> {
        let (_headers, move_text) = Self::split_headers_and_moves(pgn_text);
        let tokens = Self::tokenize_move_text(&move_text);
        let mut applied = 0;

        for token in &tokens {
            let legal_moves = state.get_legal_moves(move_generator, config_manager);
            if legal_moves.is_empty() { break; }

            if let Some(mv) = Self::find_matching_move(token, &legal_moves, state, config_manager) {
                state.execute_expanded_move(&mv, move_generator, config_manager);
                applied += 1;
            } else {
                return Err(format!("Move {} ('{}') could not be matched to any legal move", applied + 1, token));
            }
        }
        Ok(applied)
    }

    pub fn parse_headers(pgn: &str) -> HashMap<String, String> {
        Self::split_headers_and_moves(pgn).0
    }

    fn split_headers_and_moves(pgn: &str) -> (HashMap<String, String>, String) {
        let mut headers = HashMap::new();
        let mut move_lines = Vec::new();
        let mut in_headers = true;

        for line in pgn.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if in_headers { in_headers = false; }
                continue;
            }
            if in_headers && trimmed.starts_with('[') && trimmed.ends_with(']') {
                let inner = &trimmed[1..trimmed.len() - 1];
                if let Some(space_idx) = inner.find(' ') {
                    headers.insert(inner[..space_idx].to_string(), inner[space_idx + 1..].trim().trim_matches('"').to_string());
                }
            } else {
                in_headers = false;
                move_lines.push(trimmed);
            }
        }
        (headers, move_lines.join(" "))
    }

    fn tokenize_move_text(text: &str) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        for word in text.split_whitespace() {
            if word == "1-0" || word == "0-1" || word == "1/2-1/2" || word == "*" { continue; }
            if word.ends_with('.') && word[..word.len() - 1].chars().all(|c| c.is_ascii_digit()) { continue; }
            if word == "e.p." {
                if let Some(last) = tokens.last_mut() { last.push_str(" e.p."); }
                continue;
            }
            if word.starts_with('{') { continue; }
            tokens.push(word.to_string());
        }
        tokens
    }

    fn find_matching_move(
        token: &str,
        legal_moves: &[ExpandedMove],
        state: &GameState,
        config_manager: &PieceConfigManager,
    ) -> Option<ExpandedMove> {
        let clean = token.trim_end_matches('+').trim_end_matches('#');
        let clean = match clean.find(">>") { Some(i) => &clean[..i], None => clean };

        if clean == "O-O-O" || clean == "0-0-0" { return Self::find_castling_move(legal_moves, false); }
        if clean == "O-O" || clean == "0-0" { return Self::find_castling_move(legal_moves, true); }

        let parsed = Self::parse_algebraic(clean, state, config_manager)?;

        for mv in legal_moves {
            if mv.to != parsed.destination { continue; }
            let piece = state.board.get_piece(mv.from)?;
            let cfg = config_manager.get_piece_by_index(piece.piece_type)?;
            let sym = cfg.characters.first()?.chars().next()?.to_ascii_uppercase();
            if sym != parsed.piece_symbol { continue; }
            if let Some(file) = parsed.from_file { if mv.from.1 != file { continue; } }
            if let Some(rank) = parsed.from_rank { if mv.from.0 != rank { continue; } }

            if let Some(promo_sym) = parsed.promotion_symbol {
                match mv.promotion_target {
                    Some(pt) => {
                        let pt_cfg = config_manager.get_piece_by_index(pt)?;
                        if pt_cfg.characters.first()?.chars().next()?.to_ascii_uppercase() != promo_sym { continue; }
                    }
                    None => continue,
                }
            } else if mv.promotion_target.is_some() {
                continue;
            }
            return Some(mv.clone());
        }
        None
    }

    fn find_castling_move(legal_moves: &[ExpandedMove], kingside: bool) -> Option<ExpandedMove> {
        for mv in legal_moves {
            if let Some(ref opt) = mv.castling_option {
                if (opt.rook_from.1 > mv.from.1) == kingside {
                    return Some(mv.clone());
                }
            }
        }
        None
    }

    fn parse_algebraic(
        notation: &str,
        state: &GameState,
        config_manager: &PieceConfigManager,
    ) -> Option<ParsedAlgebraic> {
        let board_size = state.board.size();
        let as_str = notation.trim_end_matches(" e.p.");
        let chars: Vec<char> = as_str.chars().collect();
        if chars.is_empty() { return None; }

        let mut idx = 0;
        let piece_symbol;
        if chars[0].is_ascii_uppercase() && chars.len() > 1 {
            let maybe = chars[0];
            let known = config_manager.piece_order.iter().any(|name| {
                config_manager.pieces.get(name).map_or(false, |cfg| {
                    cfg.characters.iter().any(|c| c.chars().next().map(|ch| ch.to_ascii_uppercase()) == Some(maybe))
                })
            });
            if known { piece_symbol = maybe; idx += 1; } else { piece_symbol = 'P'; }
        } else {
            piece_symbol = 'P';
        }

        let rest: Vec<char> = chars[idx..].to_vec();
        let mut clean = Vec::new();
        let mut is_capture = false;
        for &c in &rest {
            if c == 'x' { is_capture = true; } else { clean.push(c); }
        }

        let mut promotion_symbol = None;
        if clean.len() >= 2 && clean[clean.len() - 2] == '=' {
            promotion_symbol = Some(clean[clean.len() - 1].to_ascii_uppercase());
            clean.truncate(clean.len() - 2);
        }

        let dest_start = (0..clean.len()).rev().find(|&i| clean[i].is_ascii_lowercase())?;
        let dest_str: String = clean[dest_start..].iter().collect();
        let destination = crate::notation::algebraic_to_position(&dest_str, board_size)?;

        let mut from_file = None;
        let mut from_rank = None;
        for &c in &clean[..dest_start] {
            if c.is_ascii_lowercase() {
                from_file = Some((c as u8 - b'a') as usize);
            } else if c.is_ascii_digit() {
                let rank_num = c.to_digit(10)? as usize;
                if rank_num > 0 && rank_num <= board_size.0 {
                    from_rank = Some(board_size.0 - rank_num);
                }
            }
        }

        Some(ParsedAlgebraic { piece_symbol, from_file, from_rank, destination, is_capture, promotion_symbol })
    }
}
