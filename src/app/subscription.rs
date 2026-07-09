use crate::app::ChessGui;
use crate::messages::Message;
use iced::{time, Theme};
use std::time::Duration;
impl ChessGui {
    pub fn theme(&self) -> Theme {
        Theme::Light
    }
    pub fn subscription(&self) -> iced::Subscription<Message> {
        let mut subs: Vec<iced::Subscription<Message>> = Vec::new();
        if self.tournament.is_active() || !self.tournament_workers.is_empty() {
            subs.push(time::every(Duration::from_millis(50)).map(|_| Message::TournamentTick));
        }
        if self.evolution.is_active() || !self.evolution_workers.is_empty() {
            subs.push(time::every(Duration::from_millis(50)).map(|_| Message::EvolutionTick));
        }
        if subs.is_empty() {
            if self.engine_job.is_some() || self.pending_engine_move {
                subs.push(time::every(Duration::from_millis(33)).map(|_| Message::Tick));
            } else if self.is_game_ongoing() {
                subs.push(time::every(Duration::from_millis(100)).map(|_| Message::Tick));
            }
        }
        iced::Subscription::batch(subs)
    }
}
