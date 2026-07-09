use fairy_chess_gui::core::{
    DrawReason, GameResult, GameState, MateStatus, PieceColor, Position
};
use fairy_chess_gui::piece_config::PieceConfigManager;
use fairy_chess_gui::board_config::BoardConfig;
use fairy_chess_gui::move_generator::MoveGenerator;
use fairy_chess_gui::engine::{RandomEngine, SimpleEngine, ChessEngine, SearchParams};
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────
// Test Setup Helpers
// ─────────────────────────────────────────────────────────────────────────

fn setup_variant(game_dsl: &str, pieces_dsl: &str) -> (GameState, MoveGenerator, PieceConfigManager) {
    let piece_config = PieceConfigManager::parse_config(pieces_dsl)
    .expect("Failed to parse piece DSL.");

    let board_config = BoardConfig::load_or_default(Some(&write_temp_file(game_dsl)));

    let mut move_generator = MoveGenerator::new(&piece_config)
    .expect("Failed to build MoveGenerator.");

    let game_state = GameState::from_config(board_config, &piece_config, &mut move_generator);

    (game_state, move_generator, piece_config)
}

fn write_temp_file(content: &str) -> std::path::PathBuf {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{}", content).unwrap();
    file.into_temp_path().keep().unwrap()
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 1: Complex Movement & The Rose
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_01_massive_rose_movement_no_corruption() {
    let game_dsl = r#"
    position: k14/15/15/15/15/15/15/7O7/15/15/15/15/15/15/14K
    turn: white
    pieces_files: dummy.pieces
    "#;
    let pieces_dsl = r#"
    Rose / rose / O / ^+-v-<x_?# >+-v-<x_?# >+-^-<x_?# v+-^-<x_?# v+-^->x_?# <+-^->x_?# <+-v->x_?# ^+-v->x, ^+-v-<x_?# ^+-v->x_?# <+-v->x_?# <+-^->x_?# v+-^->x_?# v+-^-<x_?# >+-^-<x_?# >+-v-<x, >+-v-<x_?# >+-^-<x_?# v+-^-<x_?# v+-^->x_?# <+-^->x_?# <+-v->x_?# ^+-v->x_?# ^+-v-<x, >+-v-<x_?# ^+-v-<x_?# ^+-v->x_?# <+-v->x_?# <+-^->x_?# v+-^->x_?# v+-^-<x_?# >+-^-<x, >+-^-<x_?# v+-^-<x_?# v+-^->x_?# <+-^->x_?# <+-v->x_?# ^+-v->x_?# ^+-v-<x_?# >+-v-<x, >+-^-<x_?# >+-v-<x_?# ^+-v-<x_?# ^+-v->x_?# <+-v->x_?# <+-^->x_?# v+-^->x_?# v+-^-<x, v+-^-<x_?# v+-^->x_?# <+-^->x_?# <+-v->x_?# ^+-v->x_?# ^+-v-<x_?# >+-v-<x_?# >+-^-<x, v+-^-<x_?# >+-^-<x_?# >+-v-<x_?# ^+-v-<x_?# ^+-v->x_?# <+-v->x_?# <+-^->x_?# v+-^->x, v+-^->x_?# <+-^->x_?# <+-v->x_?# ^+-v->x_?# ^+-v-<x_?# >+-v-<x_?# >+-^-<x_?# v+-^-<x, v+-^->x_?# v+-^-<x_?# >+-^-<x_?# >+-v-<x_?# ^+-v-<x_?# ^+-v->x_?# <+-v->x_?# <+-^->x, <+-^->x_?# <+-v->x_?# ^+-v->x_?# ^+-v-<x_?# >+-v-<x_?# >+-^-<x_?# v+-^-<x_?# v+-^->x, <+-^->x_?# v+-^->x_?# v+-^-<x_?# >+-^-<x_?# >+-v-<x_?# ^+-v-<x_?# ^+-v->x_?# <+-v->x, <+-v->x_?# ^+-v->x_?# ^+-v-<x_?# >+-v-<x_?# >+-^-<x_?# v+-^-<x_?# v+-^->x_?# <+-^->x, <+-v->x_?# <+-^->x_?# v+-^->x_?# v+-^-<x_?# >+-^-<x_?# >+-v-<x_?# ^+-v-<x_?# ^+-v->x, ^+-v->x_?# ^+-v-<x_?# >+-v-<x_?# >+-^-<x_?# v+-^-<x_?# v+-^->x_?# <+-^->x_?# <+-v->x, ^+-v->x_?# <+-v->x_?# <+-^->x_?# v+-^->x_?# v+-^-<x_?# >+-^-<x_?# >+-v-<x_?# ^+-v-<x / p;
    King / king / K / +,x / R;
    "#;

    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);
    let moves = state.get_legal_moves(&mg, &cm);

    assert!(!moves.is_empty(), "Complex piece generated no moves.");
    assert!(state.perft_verify(&mg, &cm, 1).is_ok());
}

#[test]
fn test_02_flight_captures_multiple_targets() {
    let game_dsl = "position: k7/8/8/8/8/8/4p3/4H2K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    Hopper / hopper / H / ^+%!^+ / p;
    Pawn / pawn / P / v+_ / p;
    King / king / K / +,x / R;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);
    let moves = state.get_legal_moves(&mg, &cm);

    let sweeping_capture = moves.iter()
    .find(|m| m.to == (5, 4))
    .expect("Flight capture move not generated");

    state.execute_expanded_move(sweeping_capture, &mg, &cm);

    assert_eq!(state.board.piece_count(PieceColor::Black), 1, "Enemy pawn was not wiped out in flight.");
    assert!(state.perft_verify(&mg, &cm, 1).is_ok());
}

#[test]
fn test_03_friendly_fire_cannibalism() {
    let game_dsl = "position: k7/8/8/8/8/8/1P6/C6K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "Cannibal / cannibal / C / x@ / p;\nPawn / pawn / P / ^+_ / p;\nKing / king / K / +,x / R;";

    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);
    let moves = state.get_legal_moves(&mg, &cm);

    let cannibal_move = moves.iter().find(|m| m.captures.is_some())
    .expect("Cannibal should target friendly pawn");

    state.execute_expanded_move(cannibal_move, &mg, &cm);
    assert_eq!(state.board.piece_count(PieceColor::White), 2, "Cannibal should have eaten its friend.");
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 2: Castling Edge Cases
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_04_castling_through_check_prevented() {
    let game_dsl = "position: 1k1r4/8/8/8/8/8/8/R3K2R\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Rook / rook / R / +*,+*Ou / p;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let moves = state.get_legal_moves(&mg, &cm);
    let castling_moves: Vec<_> = moves.iter().filter(|m| m.castling_option.is_some()).collect();

    assert_eq!(castling_moves.len(), 1, "King was allowed to castle through an attacked square.");
}

#[test]
fn test_05_castling_with_capture_allowed() {
    let game_dsl = "position: 4k3/8/8/8/8/8/5R2/4K1p1\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x,<>+E_!<>+E_!ou / R;
    Rook / rook / R / +*,+*Ou / p;
    Pawn / pawn / P / ^+_ / p;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let moves = state.get_legal_moves(&mg, &cm);

    let castle_capture = moves.iter()
    .find(|m| m.castling_option.is_some() && m.to == (7, 6))
    .expect("Castling move was denied despite '!' permission.");

    state.execute_expanded_move(castle_capture, &mg, &cm);
    state.undo_move(&cm);

    state.perft_verify(&mg, &cm, 1).unwrap();
}

#[test]
fn test_06_multiple_castling_partners() {
    let game_dsl = "position: 4k3/8/8/8/8/8/8/R3K2R\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Rook / rook / R / +*,+*Ou / p;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let moves = state.get_legal_moves(&mg, &cm);
    let castle_count = moves.iter().filter(|m| m.castling_option.is_some()).count();

    assert_eq!(castle_count, 2, "Engine failed to correctly identify unblocked castling partners.");
}

#[test]
fn test_07_castling_requires_unmoved_invalidation() {
    let game_dsl = "position: 4k3/8/8/8/8/8/8/R3K2R\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Rook / rook / R / +*,+*Ou / p;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let initial_moves = state.get_legal_moves(&mg, &cm);
    let initial_castles = initial_moves.iter().filter(|m| m.castling_option.is_some()).count();

    let normal_move = initial_moves.iter().find(|m| m.castling_option.is_none()).unwrap();
    state.execute_expanded_move(normal_move, &mg, &cm);
    state.undo_move(&cm);

    let restored_moves = state.get_legal_moves(&mg, &cm);
    let restored_castles = restored_moves.iter().filter(|m| m.castling_option.is_some()).count();

    assert_eq!(initial_castles, restored_castles, "Castling rights not restored on undo.");
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 3: Royal and Extinction Rules
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_08_multiple_lesser_royals_r_flag() {
    let game_dsl = "position: lll5/8/8/8/8/8/8/7K\nturn: black\npieces_files: dummy.pieces";
    let pieces_dsl = "Lesser / lesser / L / + / r;\nKing / king / K / +,x / R;";

    let (state, mg, _) = setup_variant(game_dsl, pieces_dsl);
    assert_eq!(state.board.royalty_count(PieceColor::Black), 3);
    assert!(!state.is_in_check_fast(&mg));
}

#[test]
fn test_09_single_lesser_royal_behaves_as_king() {
    let game_dsl = "position: l7/8/8/8/8/8/8/R6K\nturn: black\npieces_files: dummy.pieces";
    let pieces_dsl = "Lesser / lesser / L / + / r;\nRook / rook / R / +* / p;\nKing / king / K / +,x / R;";

    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    assert!(state.is_in_check_fast(&mg), "Last 'r' piece did not inherit check protection.");

    let moves = state.get_legal_moves(&mg, &cm);
    assert!(moves.iter().all(|m| !mg.is_square_attacked(&state.board, m.to, PieceColor::White)));
}

#[test]
fn test_10_capture_last_royal_in_flight() {
    let game_dsl = "position: 8/8/8/8/8/8/4l3/4H2K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / + / R;\nHopper / hopper / H / ^+%!^+ / p;\nLesser / lesser / L / + / r;";

    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let status = state.get_mate_status(&mg, &cm);
    assert_eq!(status, MateStatus::OpponentLostByCheck,
               "Engine failed to abort move generation when the last royal was captured in flight.");
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 4: Board Geometries
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_11_micro_board_3x3() {
    let game_dsl = "position: rkr/3/RKR\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;\nRook / rook / R / +* / p;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    assert_eq!(state.board.size(), (3, 3));
    assert!(!state.is_in_check_fast(&mg));

    let nodes = state.perft(&mg, &cm, 2);
    assert!(nodes > 0);
    assert!(state.perft_verify(&mg, &cm, 2).is_ok());
}

#[test]
fn test_12_rectangular_asymmetric_board_3x12() {
    let game_dsl = "position: k11/12/5K6\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    assert_eq!(state.board.size(), (3, 12));
    let moves = state.get_legal_moves(&mg, &cm);
    assert_eq!(moves.len(), 5);
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 5: Ghosts, En Passant, and Legal Filtering
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_13_en_passant_evades_check() {
    let game_dsl = "position: k7/4p3/8/5P2/3K4/8/8/8\nturn: black\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x / R;
    Pawn / pawn / P / ^x!~i,^+_i,^+_^+e_iu / P;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let black_moves = state.get_legal_moves(&mg, &cm);
    let double_push = black_moves.iter()
    .find(|m| m.to == (3, 4))
    .expect("Double push not found");

    state.execute_expanded_move(double_push, &mg, &cm);

    assert!(state.is_in_check_fast(&mg), "Double push did not check the king.");
    assert_eq!(state.board.live_ghosts().len(), 1, "No EP ghost created.");

    let white_moves = state.get_legal_moves(&mg, &cm);
    let ep_capture = white_moves.iter().find(|m| m.captures.is_some()).expect("EP evasion not generated.");

    state.execute_expanded_move(ep_capture, &mg, &cm);
    assert!(!state.is_in_check_fast(&mg), "Check was not evaded via EP capture.");
}

#[test]
fn test_14_pseudo_legal_vs_legal_pin() {
    let game_dsl = "position: k3r3/8/8/8/8/8/4R3/4K3\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;\nRook / rook / R / +* / p;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let pseudo = state.generate_pseudo_legal_moves(&mg, &cm);
    let pseudo_moves = match pseudo {
        fairy_chess_gui::core::MoveGenerationResult::Moves(m) => m,
        _ => panic!("Expected moves"),
    };
    let legal = state.get_legal_moves(&mg, &cm);
    assert!(pseudo_moves.len() > legal.len(), "Legal filtering failed to strip pinned moves.");
}

// ─────────────────────────────────────────────────────────────────────────
// CATEGORY 6: Commands & Mechanics
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_15_optional_stop_modifier() {
    let game_dsl = "position: k7/8/8/8/8/8/8/S6K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "Lancer / lancer / S / >+?>+ / p;\nKing / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let moves = state.get_legal_moves(&mg, &cm);
    let lancer_moves: Vec<_> = moves.iter().filter(|m| m.from == (7, 0)).collect();

    assert_eq!(lancer_moves.len(), 2, "Optional stop failed to generate intermediate move");
}

#[test]
fn test_16_irreversible_resets_fifty_move() {
    let game_dsl = "position: k7/8/8/8/8/8/8/I6K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "Irrev / irrev / I / +i / p;\nKing / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    state.fifty_move_counter = 49;
    let moves = state.get_legal_moves(&mg, &cm);
    state.execute_expanded_move(&moves[0], &mg, &cm);

    assert_eq!(state.fifty_move_counter, 0, "Irreversible flag 'i' failed to reset 50-move counter");
}

#[test]
fn test_17_engine_random_move() {
    let game_dsl = "position: k7/8/8/8/8/8/8/7K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let mut engine = RandomEngine::new();
    let params = SearchParams {
        state: &mut state,
        move_generator: &mg,
        config_manager: &cm,
        time_limit: None,
        depth: 1,
    };

    assert!(engine.best_move(params).is_some());
}

#[test]
fn test_18_strict_perft_baseline() {
    let game_dsl = "position: rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    Knight / knight / N / +x / p;
    Rook / rook / R / +*,+*Ou / p;
    Bishop / bishop / B / x* / p;
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Queen / queen / Q / +*,x* / p;
    Pawn / pawn / P / ^x!~i,^+_i,^+_^+e_iu / P;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    assert_eq!(state.perft(&mg, &cm, 1), 20);
    assert_eq!(state.perft(&mg, &cm, 2), 400);
}

#[test]
fn test_19_pgn_algebraic_parsing_and_export() {
    let game_dsl = "position: 8/8/8/8/8/K7/8/k7\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let legal = state.get_legal_moves(&mg, &cm);
    let mv = &legal[0];

    let alg = fairy_chess_gui::helpers::format_move(mv, &cm, state.board.size());
    assert!(!alg.is_empty());

    state.execute_expanded_move(mv, &mg, &cm);
    let pgn = fairy_chess_gui::pgn::PgnExporter::export_game(&state, &state.move_history, &mg, &cm, None);
    assert!(pgn.contains("[Result"));
}

#[test]
fn test_20_stalemate_detection() {
    let game_dsl = "position: k7/2R5/1R6/8/8/8/8/7K\nturn: black\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;\nRook / rook / R / +* / p;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    assert_eq!(state.get_mate_status(&mg, &cm), MateStatus::Stalemate);
}

#[test]
fn test_21_insufficient_material_draw() {
    let game_dsl = "position: K7/8/8/8/8/8/8/k7\ninsufficient_material: K vs K\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    state.check_draw_conditions();
    assert_eq!(state.game_result, Some(GameResult::Draw(DrawReason::InsufficientMaterial)));
}

#[test]
fn test_22_promotion_zone_execution() {
    let game_dsl = "position: k7/1P6/8/8/8/8/8/7K\npromotion_zones: white:rank:0\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "Pawn / pawn / P / ^+_ / P;\nQueen / queen / Q / +*,x* / p;\nKing / king / K / +,x / R;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let moves = state.get_legal_moves(&mg, &cm);
    let promo_move = moves.iter().find(|m| m.promotion_target.is_some()).expect("No promotion generated");

    state.execute_expanded_move(promo_move, &mg, &cm);

    let dest_piece = state.board.get_piece(promo_move.to).unwrap();
    assert_eq!(dest_piece.piece_type, promo_move.promotion_target.unwrap());
}

#[test]
fn test_23_simple_engine_checkmate_search() {
    let game_dsl = "position: 1R6/8/8/8/8/K7/8/k7\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;\nRook / rook / R / +* / p;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let mut engine = SimpleEngine::new();
    let params = SearchParams {
        state: &mut state,
        move_generator: &mg,
        config_manager: &cm,
        time_limit: None,
        depth: 2,
    };

    let result = engine.best_move(params).unwrap();
    assert!(result.evaluation.score > 900_000, "Failed to find mate-in-1");
}

#[test]
fn test_24_tape_state_preservation_deep_search() {
    let game_dsl = "position: k7/8/8/8/8/8/8/R3K2R\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Rook / rook / R / +*,+*Ou / p;
    "#;
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    let initial_tape = state.tape_len();
    let initial_hash = state.current_hash();

    assert!(state.perft_verify(&mg, &cm, 3).is_ok());

    assert_eq!(state.tape_len(), initial_tape);
    assert_eq!(state.current_hash(), initial_hash);
}

#[test]
fn test_25_fide_no_move_duplication() {
    let game_dsl = "position: r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = r#"
    Knight / knight / N / +x / p;
    Rook / rook / R / +*,+*Ou / p;
    Bishop / bishop / B / x* / p;
    King / king / K / +,x,<>+E_<>+E_ou / R;
    Queen / queen / Q / +*,x* / p;
    Pawn / pawn / P / ^x!~i,^+_i,^+_^+e_ui / P;
    "#;

    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);
    let moves = state.get_legal_moves(&mg, &cm);

    assert_eq!(moves.len(), 48, "Expected exactly 48 unique pseudo-legal moves for Kiwipete.");
}

#[test]
fn test_26_custom_insufficient_material_rules() {
    let game_dsl = "position: k7/8/8/8/8/8/8/K1M5\ninsufficient_material: K vs KM\nturn: white\npieces_files: dummy.pieces";
    let pieces_dsl = "King / king / K / +,x / R;\nMage / mage / M / x / p;";
    let (mut state, mg, cm) = setup_variant(game_dsl, pieces_dsl);

    state.check_draw_conditions();
    assert_eq!(state.game_result, Some(GameResult::Draw(DrawReason::InsufficientMaterial)));
}
