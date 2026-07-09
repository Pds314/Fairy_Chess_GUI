pub(crate) mod analysis_ui;
pub(crate) mod board_canvas;
pub(crate) mod controls;
pub(crate) mod evolution_ui;
pub(crate) mod status;
pub(crate) mod terminal_ui;
pub(crate) mod tournament_ui;
pub(crate) mod widgets;
pub(crate) use widgets::etext;
use crate::app::ChessGui;
use crate::constants;
use crate::messages::{Message, UiPanel};
use iced::widget::{button, column, row};
use iced::{Element, Length};
impl ChessGui {
    /// Tab bar for the lower control area. Only one tab's content is
    /// rendered at a time (see `create_active_panel` in controls.rs),
    /// which is the core of de-cluttering what used to be a single flat
    /// stack of every control in the app.
    ///
    /// `label` is `&'static str` (every call site passes a literal), not
    /// `&str`. That's deliberate: `iced::widget::Button<'a, Message>` is
    /// *invariant* over `'a`, so a closure generic over an arbitrary
    /// `&'a str` parameter produces a fresh, unrelated `Button<'a, ..>`
    /// type on every call, and those can't be unified by the `row!`
    /// macro. Pinning the parameter to the one concrete lifetime every
    /// call site actually has (`'static`) makes the closure monomorphic
    /// instead of implicitly generic, which sidesteps the invariance
    /// conflict entirely.
    fn create_tab_bar(&self) -> Element<'_, Message> {
        let make_tab = |panel: UiPanel, label: &'static str| {
            let active = self.active_panel == panel;
            button(etext(label).size(13))
            .padding(8)
            .on_press(Message::UiPanelSelected(panel))
            .style(move |_theme, status| tab_button_style(active, status))
        };
        row![
            make_tab(UiPanel::Game, "🎮 Game"),
            make_tab(UiPanel::Engines, "🤖 Engines"),
            make_tab(UiPanel::Tournament, "🏆 Tournament"),
            make_tab(UiPanel::Evolution, "🧬 Evolution"),
        ]
        .spacing(6)
        .into()
    }
    pub fn view(&self) -> Element<'_, Message> {
        let canvas = self.create_board_canvas();
        let status = self.create_status_display();
        let timer = self.create_timer_display();
        let tab_bar = self.create_tab_bar();
        let active_panel = self.create_active_panel();
        let terminal = self.create_terminal_section();
        let controls_section = iced::widget::scrollable(
            column![tab_bar, active_panel, terminal].spacing(12),
        )
        .height(Length::Shrink)
        .width(Length::Fill);
        let show_tournament_graph =
        self.tournament.is_active() || !self.tournament.elo.game_log().is_empty();
        let main_content = if show_tournament_graph {
            let graph = self.create_tournament_graph();
            let (rows, _) = self.game_state.board.size();
            let board_height =
            constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;
            let graph_section = iced::widget::scrollable(graph)
            .height(Length::Fixed(board_height))
            .width(Length::Fixed(400.0));
            column![
                status,
                timer,
                row![canvas, graph_section].spacing(10),
                controls_section,
            ]
            .spacing(10)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
        } else if let Some(ref analysis) = self.position_analysis {
            let analysis_display = self.create_analysis_display(analysis);
            let (rows, _) = self.game_state.board.size();
            let board_height =
            constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;
            let analysis_section = iced::widget::scrollable(analysis_display)
            .height(Length::Fixed(board_height))
            .width(Length::Fixed(400.0));
            column![
                status,
                timer,
                row![canvas, analysis_section].spacing(10),
                controls_section
            ]
            .spacing(10)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            column![status, timer, canvas, controls_section]
            .spacing(10)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
        };
        if let Some(dialog) = &self.promotion_dialog {
            let dialog_view = dialog.view(
                &self.texture_manager,
                &self.piece_config,
                self.game_state.current_turn,
            );
            iced::widget::stack![main_content, dialog_view].into()
        } else {
            main_content.into()
        }
    }
}
/// Visual style for a tab button: filled/blue when active, flat grey
/// otherwise, with a slightly lighter hover state either way.
fn tab_button_style(
    active: bool,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let base_bg = if active {
        iced::Color::from_rgb8(70, 130, 180)
    } else {
        iced::Color::from_rgb8(225, 225, 225)
    };
    let bg = match (active, status) {
        (true, iced::widget::button::Status::Hovered) => iced::Color::from_rgb8(90, 150, 200),
        (false, iced::widget::button::Status::Hovered) => iced::Color::from_rgb8(208, 208, 208),
        _ => base_bg,
    };
    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: if active {
            iced::Color::WHITE
        } else {
            iced::Color::BLACK
        },
        border: iced::Border {
            color: iced::Color::from_rgb8(180, 180, 180),
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: iced::Shadow::default(),
    }
}
