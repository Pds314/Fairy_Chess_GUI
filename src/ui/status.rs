use crate::app::ChessGui;
use crate::core::{DrawReason, GameResult, PieceColor};
use crate::messages::Message;
use crate::ui::etext;
use iced::widget::{container, text};
use iced::{Element, Length, Theme};
use std::time::Duration;

impl ChessGui {
    pub(crate) fn format_game_status(&self) -> String {
        if let Some(w) = self.tournament_workers.first() {
            return format!(
                "🏆 Tournament — showing: {} vs {} (ply {})",
                           w.pairing.0.name(),
                           w.pairing.1.name(),
                           w.plies
            );
        }
        if let Some(w) = self.evolution_workers.first() {
            return format!("🧬 Evolution — showing live game (ply {})", w.plies);
        }
        let base = match &self.game_state.game_result {
            Some(GameResult::Winner(color)) => format!("Game Over: {:?} Wins!", color),
            Some(GameResult::Draw(reason)) => format!(
                "Game Over: Draw by {}",
                match reason {
                    DrawReason::FiftyMoveRule => "fifty-move rule",
                    DrawReason::Repetition => "repetition",
                    DrawReason::Stalemate => "stalemate",
                    DrawReason::InsufficientMaterial => "insufficient material",
                    DrawReason::MutualElimination => "mutual royal elimination",
                }
            ),
            Some(GameResult::Ongoing) => format!(
                "Turn: {} | Fifty-move: {}",
                match self.game_state.current_turn {
                    PieceColor::White => "White",
                    PieceColor::Black => "Black",
                },
                self.game_state.fifty_move_counter
            ),
            None => "Game state unknown".to_string(),
        };
        if self.engine_job.is_some() {
            let who = match self.game_state.current_turn {
                PieceColor::White => "White",
                PieceColor::Black => "Black",
            };
            format!("{} | 🤔 {} is thinking…", base, who)
        } else {
            base
        }
    }

    pub(crate) fn create_status_display(&self) -> Element<'_, Message> {
        etext(self.format_game_status()).size(16).into()
    }

    pub(crate) fn create_timer_display(&self) -> Element<'_, Message> {
        let white_time = self.game_controller.get_white_time();
        let black_time = self.game_controller.get_black_time();
        let thinking = self
            .game_controller
            .get_current_thinking_time(self.game_state.current_turn);
        let white_total = if self.game_state.current_turn == PieceColor::White {
            white_time + thinking
        } else {
            white_time
        };
        let black_total = if self.game_state.current_turn == PieceColor::Black {
            black_time + thinking
        } else {
            black_time
        };
        let timer_text = format!(
            "⏱ White: {:02}:{:02}.{:01} | Black: {:02}:{:02}.{:01}",
            white_total.as_secs() / 60,
            white_total.as_secs() % 60,
            white_total.subsec_millis() / 100,
            black_total.as_secs() / 60,
            black_total.as_secs() % 60,
            black_total.subsec_millis() / 100
        );
        container(etext(timer_text).size(14))
            .padding(5)
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    240, 240, 240,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(200, 200, 200),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }
}
