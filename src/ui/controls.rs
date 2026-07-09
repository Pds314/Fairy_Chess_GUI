use crate::app::ChessGui;
use crate::engine::EngineType;
use crate::messages::{Message, UiPanel, DEPTHS};
use crate::ui::etext;
use iced::widget::{button, checkbox, column, pick_list, row, slider, text, text_input};
use iced::{Element, Length};
use std::path::Path;
impl ChessGui {
    /// Dispatch to whichever tab is currently active. Replaces the old
    /// monolithic panel that put every control on screen simultaneously.
    pub(crate) fn create_active_panel(&self) -> Element<'_, Message> {
        match self.active_panel {
            UiPanel::Game => self.create_game_panel(),
            UiPanel::Engines => self.create_engines_panel(),
            UiPanel::Tournament => self.create_tournament_section(),
            UiPanel::Evolution => self.create_evolution_section(),
        }
    }
    fn create_game_panel(&self) -> Element<'_, Message> {
        let file_browser = self.create_file_browser_section();
        let game_actions = row![
            self.create_undo_button(),
            self.create_redo_button(),
            self.create_reset_button(),
            button("Generate Moves").on_press(Message::GenerateMoves),
            button("Print PGN").on_press(Message::PrintPgn),
        ]
        .spacing(10);
        let analysis_actions = column![
            row![
                button("Evaluate Position").on_press_maybe(
                    self.is_game_ongoing().then_some(Message::EvaluatePosition)
                ),
                button("Evaluate Moves")
                .on_press_maybe(self.is_game_ongoing().then_some(Message::EvaluateMoves)),
                button("Make Best Move")
                .on_press_maybe(self.is_game_ongoing().then_some(Message::MakeBestMove)),
            ]
            .spacing(10),
            row![
                button(etext("🔬 Analyze Position"))
                .on_press_maybe(self.is_game_ongoing().then_some(Message::AnalyzePosition))
                .style(|_theme, status| {
                    let base = iced::widget::button::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgb8(
                            70, 130, 180,
                        ))),
                        text_color: iced::Color::WHITE,
                        border: iced::Border::default(),
                       shadow: iced::Shadow::default(),
                    };
                    match status {
                        iced::widget::button::Status::Hovered => {
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(
                                    iced::Color::from_rgb8(100, 150, 200),
                                )),
                                ..base
                            }
                        }
                        _ => base,
                    }
                }),
                if self.game_controller.supports_analysis() {
                    etext("✅ Advanced analysis available").size(12)
                } else {
                    etext("⚠️ Use PST Engine for advanced analysis").size(12)
                }
            ]
            .spacing(10),
        ]
        .spacing(8);
        column![file_browser, game_actions, analysis_actions]
        .spacing(15)
        .width(Length::Fill)
        .into()
    }
    fn create_engines_panel(&self) -> Element<'_, Message> {
        // --- White engine row ---
        let white_selector = pick_list(
            EngineType::all(),
                                       Some(self.game_controller.get_white_engine_type().clone()),
                                       Message::WhiteEngineSelected,
        )
        .width(Length::Fill);
        let white_depth = pick_list(
            DEPTHS.to_vec(),
                                    Some(self.game_controller.get_white_search_depth()),
                                    Message::WhiteDepthSelected,
        )
        .placeholder("Depth");
        let white_time = text_input("Time (s)", &self.white_time_input)
        .on_input(Message::WhiteTimeInputChanged)
        .width(Length::Fixed(70.0));
        let white_respect = text_input("Respect", &self.white_time_respect_input)
        .on_input(Message::WhiteTimeRespectChanged)
        .width(Length::Fixed(60.0));
        let white_params = self.create_parameter_controls(
            self.game_controller.get_white_engine_parameter_defs(),
                                                          self.game_controller.get_white_engine_parameters(),
                                                          Message::WhiteEngineParameterChanged,
        );
        // --- Black engine row ---
        let black_selector = pick_list(
            EngineType::all(),
                                       Some(self.game_controller.get_black_engine_type().clone()),
                                       Message::BlackEngineSelected,
        )
        .width(Length::Fill);
        let black_depth = pick_list(
            DEPTHS.to_vec(),
                                    Some(self.game_controller.get_black_search_depth()),
                                    Message::BlackDepthSelected,
        )
        .placeholder("Depth");
        let black_time = text_input("Time (s)", &self.black_time_input)
        .on_input(Message::BlackTimeInputChanged)
        .width(Length::Fixed(70.0));
        let black_respect = text_input("Respect", &self.black_time_respect_input)
        .on_input(Message::BlackTimeRespectChanged)
        .width(Length::Fixed(60.0));
        let black_params = self.create_parameter_controls(
            self.game_controller.get_black_engine_parameter_defs(),
                                                          self.game_controller.get_black_engine_parameters(),
                                                          Message::BlackEngineParameterChanged,
        );
        // --- Eval engine row ---
        let eval_selector = pick_list(
            EngineType::all(),
                                      Some(self.game_controller.get_eval_engine_type().clone()),
                                      Message::EvalEngineSelected,
        )
        .width(Length::Fill);
        let eval_depth = pick_list(
            DEPTHS.to_vec(),
                                   Some(self.game_controller.get_eval_search_depth()),
                                   Message::EvalDepthSelected,
        )
        .placeholder("Depth");
        let eval_time = text_input("Time (s)", &self.eval_time_input)
        .on_input(Message::EvalTimeInputChanged)
        .width(Length::Fixed(80.0));
        let eval_params = self.create_parameter_controls(
            self.game_controller.get_eval_engine_parameter_defs(),
                                                         self.game_controller.get_eval_engine_parameters(),
                                                         Message::EvalEngineParameterChanged,
        );
        let auto_play = checkbox("Auto-play", self.game_controller.is_auto_play())
        .on_toggle(Message::AutoPlayToggled);
        let unlimited = checkbox(
            "Unlimited depth with time",
            self.game_controller.get_unlimited_depth_with_time(),
        )
        .on_toggle(Message::UnlimitedDepthToggled);
        row![
            column![
                text("White Player").size(16),
                row![white_selector, white_depth, white_time, white_respect].spacing(5),
                white_params,
                text("Black Player").size(16),
                row![black_selector, black_depth, black_time, black_respect].spacing(5),
                black_params,
            ]
            .spacing(8)
            .width(Length::FillPortion(2)),
            column![
                text("Analysis Engine").size(16),
                row![eval_selector, eval_depth, eval_time].spacing(5),
                eval_params,
                row![auto_play.size(20), unlimited.size(16)].spacing(10),
                text("Time Respect: 0.0-1.0 (adjusts time based on clock difference)").size(12),
            ]
            .spacing(8)
            .width(Length::FillPortion(2)),
        ]
        .spacing(20)
        .into()
    }
    fn create_file_browser_section(&self) -> Element<'_, Message> {
        let current_display = if let Some(ref file) = self.current_game_file {
            text(format!(
                "Current: {}",
                Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
            ))
            .size(14)
        } else {
            text("Current: Default").size(14)
        };
        let file_selector = pick_list(
            self.game_file_items.clone(),
                                      self.selected_game_file.clone(),
                                      Message::GameFileSelected,
        )
        .placeholder("Browse for game files...")
        .width(Length::Fill);
        let can_load = self
        .selected_game_file
        .as_ref()
        .map_or(false, |item| matches!(item, crate::asset_manager::BrowserItem::File(_)));
        let load_btn =
        button("Load Game").on_press_maybe(can_load.then_some(Message::LoadGameFile));
        column![
            text("Game File").size(16),
            current_display,
            row![file_selector, load_btn].spacing(10)
        ]
        .spacing(8)
        .into()
    }
    fn create_undo_button(&self) -> Element<'_, Message> {
        button("Undo Move")
        .on_press_maybe(self.game_state.can_undo().then_some(Message::UndoMove))
        .into()
    }
    fn create_redo_button(&self) -> Element<'_, Message> {
        button("Redo Move")
        .on_press_maybe(self.game_state.can_redo().then_some(Message::RedoMove))
        .into()
    }
    fn create_reset_button(&self) -> Element<'_, Message> {
        button("Reset Board").on_press(Message::ResetBoard).into()
    }
    pub(crate) fn create_parameter_controls(
        &self,
        param_defs: Option<&'static [crate::engine::parameters::ParameterDef]>,
        current_params: Option<crate::engine::parameters::EngineParameters>,
        message_fn: fn(String, f64) -> Message,
    ) -> Element<'_, Message> {
        let Some(defs) = param_defs else {
            return text("No tunable parameters").size(12).into();
        };
        if defs.is_empty() {
            return text("No tunable parameters").size(12).into();
        }
        let params = current_params.unwrap_or_default();
        let mut col = column![].spacing(8);
        for def in defs {
            let current_value = params.get_or_default(def.id, def.default);
            let param_id = def.id.to_string();
            let param_slider = slider(
                def.min as f32..=def.max as f32,
                current_value as f32,
                move |v| message_fn(param_id.clone(), v as f64),
            )
            .step(if def.step > 0.0 {
                def.step as f32
            } else {
                0.01
            })
            .width(Length::Fixed(150.0));
            col = col.push(
                row![
                    text(format!("{}: {:.2}", def.display_name, current_value))
                    .size(11)
                    .width(Length::Fixed(180.0)),
                           param_slider,
                ]
                .spacing(10),
            );
        }
        col.into()
    }
}
