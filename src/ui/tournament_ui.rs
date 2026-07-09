use crate::app::ChessGui;
use crate::constants;
use crate::engine::EngineType;
use crate::messages::Message;
use crate::tournament::TournamentPhase;
use crate::tournament_graph::TournamentGraph;
use crate::ui::etext;
use iced::widget::{button, canvas, checkbox, column, container, row, text, text_input, Column, Row};
use iced::{Element, Length};

impl ChessGui {
    pub(crate) fn create_tournament_section(&self) -> Element<'_, Message> {
        let header = etext("🏆 Tournament").size(16);

        let phase_controls: Element<_> = match self.tournament.phase {
            TournamentPhase::Inactive | TournamentPhase::Complete => {
                let engines: Vec<EngineType> = EngineType::all()
                    .into_iter()
                    .filter(|e| !e.is_human())
                    .collect();
                let mid = (engines.len() + 1) / 2;
                let mut col_a = Column::new().spacing(4);
                let mut col_b = Column::new().spacing(4);
                for (i, engine) in engines.iter().enumerate() {
                    let checked = self.tournament.is_participant(engine);
                    let e = engine.clone();
                    let label = {
                        let n = engine.name();
                        if n.len() > 24 {
                            format!("{}…", &n[..23])
                        } else {
                            n.to_string()
                        }
                    };
                    let cb = checkbox(label, checked)
                        .on_toggle(move |_| Message::TournamentToggleParticipant(e.clone()))
                        .size(14)
                        .text_size(11);
                    if i < mid {
                        col_a = col_a.push(cb);
                    } else {
                        col_b = col_b.push(cb);
                    }
                }
                let participant_grid = Row::new().push(col_a).push(col_b).spacing(15);
                let games_input = text_input("games", &self.tournament_games_input)
                    .on_input(Message::TournamentGamesInputChanged)
                    .width(Length::Fixed(50.0));
                let plies_input = text_input("max plies", &self.tournament_max_plies_input)
                    .on_input(Message::TournamentMaxPliesInputChanged)
                    .width(Length::Fixed(60.0));
                let par_input = text_input("threads", &self.tournament_parallelism_input)
                    .on_input(Message::TournamentParallelismInputChanged)
                    .width(Length::Fixed(50.0));
                let cpu_hint = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1);
                let n_participants = self
                    .tournament
                    .participants
                    .iter()
                    .filter(|e| !e.is_human())
                    .count();
                let can_start = n_participants >= 2;
                let start_btn = button("Start Tournament")
                    .on_press_maybe(can_start.then_some(Message::TournamentStart));
                let total_preview = if can_start {
                    text(format!(
                        "→ {} games total",
                        n_participants * self.tournament.games_per_pairing / 2
                    ))
                    .size(11)
                } else {
                    text("(select ≥ 2 engines)").size(11)
                };
                let clear_btn = button("Clear History").on_press_maybe(
                    (!self.tournament.elo.game_log().is_empty())
                        .then_some(Message::TournamentClearHistory),
                );
                column![
                    text("Participants:").size(12),
                    participant_grid,
                    row![
                        text("Games/engine:").size(12),
                        games_input,
                        text("Max plies:").size(12),
                        plies_input,
                        text("Parallel:").size(12),
                        par_input,
                        text(format!("(cpu: {})", cpu_hint)).size(10),
                    ]
                    .spacing(8),
                    row![start_btn, clear_btn, total_preview].spacing(10),
                ]
                .spacing(6)
                .into()
            }
            TournamentPhase::SettingUpGame | TournamentPhase::Playing => {
                let played = self.tournament.games_played();
                let total = self.tournament.total_games();
                let running = self.tournament_workers.len();
                let progress =
                    text(format!("Completed {}/{} · {} running", played, total, running)).size(14);

                let mut workers_col = Column::new().spacing(2);
                for (idx, w) in self.tournament_workers.iter().enumerate() {
                    let marker = if idx == 0 { "▶ " } else { "  " };
                    workers_col = workers_col.push(
                        text(format!(
                            "{}{} vs {} — ply {}",
                            marker,
                            w.pairing.0.name(),
                            w.pairing.1.name(),
                            w.plies
                        ))
                        .size(10),
                    );
                }

                let stop_btn = button(etext("Stop Tournament"))
                    .on_press(Message::TournamentStop)
                    .style(|_, status| {
                        let base = iced::widget::button::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                180, 70, 70,
                            ))),
                            text_color: iced::Color::WHITE,
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        };
                        match status {
                            iced::widget::button::Status::Hovered => {
                                iced::widget::button::Style {
                                    background: Some(iced::Background::Color(
                                        iced::Color::from_rgb8(200, 90, 90),
                                    )),
                                    ..base
                                }
                            }
                            _ => base,
                        }
                    });

                column![progress, workers_col, stop_btn].spacing(6).into()
            }
        };

        container(column![header, phase_controls].spacing(8))
            .padding(10)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    252, 248, 240,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(220, 200, 160),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    pub(crate) fn create_tournament_graph(&self) -> Element<'_, Message> {
        let (rows, _) = self.game_state.board.size();
        let board_height = constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;
        canvas(TournamentGraph {
            elo: &self.tournament.elo,
            total_games: self
                .tournament
                .total_games()
                .max(self.tournament.elo.game_log().len()),
            cache: &self.tournament_graph_cache,
        })
        .width(Length::Fixed(400.0))
        .height(Length::Fixed(board_height))
        .into()
    }
}
