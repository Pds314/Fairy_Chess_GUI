use crate::app::ChessGui;
use crate::engine::parameters::ParameterDef;
use crate::engine::EngineType;
use crate::messages::Message;
use crate::ui::etext;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input, Column,
};
use iced::{Element, Font, Length};
impl ChessGui {
    pub(crate) fn create_evolution_section(&self) -> Element<'_, Message> {
        let header = etext("🧬 Parameter Evolution").size(16);
        // Controls that make sense regardless of whether evolution is
        // running: none of these change the shape of the population, so
        // they're safe to adjust live (parallelism and the ply cap only
        // affect future dispatches; autosave just controls a periodic
        // checkpoint write).
        let live_controls = self.create_evolution_live_controls();
        let export_row = row![
            button(etext("📤 Export Best"))
            .on_press_maybe(self.evolution.best().is_some().then_some(Message::EvolutionExportBest)),
            match self.evolution.best() {
                Some(best) => text(format!("#{} — rating {:.0}", best.id, best.rating)).size(11),
                None => text("(no individuals yet)").size(11),
            },
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let lock_menu = self.create_lock_menu_section();
        let body: Element<_> = if self.evolution.is_active() {
            let progress = text(format!(
                "Running: {} individuals of {} — {} games played, {} in flight",
                self.evolution.population.len(),
                                        self.evolution.base_engine.name(),
                                        self.evolution.games_played,
                                        self.evolution_workers.len(),
            ))
            .size(12);
            let stop_btn = button(etext("Stop Evolution")).on_press(Message::EvolutionStop);
            let mut pop_col = Column::new().spacing(1);
            for ind in self.evolution.sorted_by_rating().into_iter().take(20) {
                pop_col = pop_col.push(
                    text(format!(
                        "#{:<4} gen {:<3} {:>6.0}  {}g (+{} ={} -{})",
                                 ind.id, ind.generation, ind.rating, ind.games_played, ind.wins, ind.draws, ind.losses
                    ))
                    .size(10)
                    .font(Font::MONOSPACE),
                );
            }
            column![
                progress,
                stop_btn,
                scrollable(pop_col).height(Length::Fixed(140.0)),
            ]
            .spacing(6)
            .into()
        } else {
            let engines: Vec<EngineType> = EngineType::all()
            .into_iter()
            .filter(|e| !e.is_human())
            .collect();
            let engine_pick = pick_list(
                engines,
                Some(self.evolution_base_engine.clone()),
                                        Message::EvolutionBaseEngineSelected,
            )
            .width(Length::Fixed(220.0));
            let pop_input = text_input("population", &self.evolution_population_input)
            .on_input(Message::EvolutionPopulationChanged)
            .width(Length::Fixed(70.0));
            let play_bias_input = text_input("play bias", &self.evolution_play_bias_input)
            .on_input(Message::EvolutionPlayBiasChanged)
            .width(Length::Fixed(70.0));
            let repl_bias_input = text_input("repl bias", &self.evolution_replication_bias_input)
            .on_input(Message::EvolutionReplicationBiasChanged)
            .width(Length::Fixed(70.0));
            let mutation_input = text_input("mutation", &self.evolution_mutation_scale_input)
            .on_input(Message::EvolutionMutationScaleChanged)
            .width(Length::Fixed(70.0));
            let repro_input = text_input("repro (0=auto)", &self.evolution_repro_rate_input)
            .on_input(Message::EvolutionReproRateChanged)
            .width(Length::Fixed(90.0));
            let crossover_cb =
            checkbox("Crossover", self.evolution_crossover).on_toggle(Message::EvolutionCrossoverToggled);
            let can_start = self
            .evolution_population_input
            .parse::<usize>()
            .map(|n| n >= 2)
            .unwrap_or(false)
            && !self.tournament.is_active();
            let start_btn =
            button("Start Evolution").on_press_maybe(can_start.then_some(Message::EvolutionStart));
            column![
                row![text("Engine:").size(12), engine_pick].spacing(6),
                row![
                    text("Pop:").size(12),
                    pop_input,
                    text("Play bias:").size(12),
                    play_bias_input,
                    text("Repl bias:").size(12),
                    repl_bias_input,
                    text("Mutation:").size(12),
                    mutation_input,
                    text("Repro:").size(12),
                    repro_input,
                    crossover_cb,
                ]
                .spacing(6),
                start_btn,
                text("Mutation is a fraction of each parameter's own clamping range.").size(10),
            ]
            .spacing(6)
            .into()
        };
        container(column![header, live_controls, export_row, lock_menu, body].spacing(8))
        .padding(10)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb8(240, 248, 250))),
               border: iced::Border {
                   color: iced::Color::from_rgb8(160, 200, 210),
               width: 1.0,
               radius: 5.0.into(),
               },
               ..Default::default()
        })
        .into()
    }
    /// Parallelism, ply cap, and autosave controls. Shared between the
    /// pre-start configuration view and the running-status view, and
    /// live-adjustable in both: these never resize the population, so
    /// there's no reason to gate them behind "stop first."
    fn create_evolution_live_controls(&self) -> Element<'_, Message> {
        let max_plies_input = text_input("max plies", &self.evolution_max_plies_input)
        .on_input(Message::EvolutionMaxPliesInputChanged)
        .width(Length::Fixed(70.0));
        let parallel_input = text_input("threads", &self.evolution_parallelism_input)
        .on_input(Message::EvolutionParallelismInputChanged)
        .width(Length::Fixed(60.0));
        let cpu_hint = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
        let autosave_cb = checkbox("Autosave best", self.evolution_autosave)
        .on_toggle(Message::EvolutionAutosaveToggled);
        let autosave_path = text_input(
            "path (relative = under assets/personalities)",
                                       &self.evolution_autosave_path,
        )
        .on_input(Message::EvolutionAutosavePathChanged)
        .width(Length::Fixed(260.0));
        column![
            row![
                text("Max plies:").size(12),
                max_plies_input,
                text("Parallel:").size(12),
                parallel_input,
                text(format!("(cpu: {})", cpu_hint)).size(10),
            ]
            .spacing(6),
            row![autosave_cb, autosave_path].spacing(6),
            text("Autosave checkpoints the current best individual every replication cycle.")
            .size(10),
        ]
        .spacing(6)
        .into()
    }
    /// Which parameter-definition list to show in the lock checklist:
    /// prefer the actually-running (or most-recently-run) set so the
    /// checkboxes reflect reality once evolution has started, falling
    /// back to a preview of the currently-selected-but-not-yet-started
    /// engine so locks can be set up before the first run.
    fn current_evolution_param_defs(&self) -> &'static [ParameterDef] {
        let existing = self.evolution.param_defs();
        if !existing.is_empty() {
            return existing;
        }
        self.evolution_base_engine
        .create()
        .and_then(|e| e.parameter_definitions())
        .unwrap_or(&[])
    }
    /// Collapsible "which parameters may evolution mutate" checklist.
    /// Locking a parameter pins it at the engine's default for every
    /// individual, including ones created by future replication events.
    fn create_lock_menu_section(&self) -> Element<'_, Message> {
        let toggle_label = if self.show_lock_menu {
            "▾ Locked Parameters"
        } else {
            "▸ Locked Parameters"
        };
        let toggle = button(etext(toggle_label).size(12)).on_press(Message::ToggleEvolutionLockMenu);
        let mut section = column![row![
            toggle,
            text(format!("({} locked)", self.evolution_locked_params.len())).size(10),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)]
        .spacing(4);
        if self.show_lock_menu {
            let defs = self.current_evolution_param_defs();
            if defs.is_empty() {
                section = section.push(
                    text("Select an engine with tunable parameters to see its list.").size(11),
                );
            } else {
                let mut col = Column::new().spacing(2);
                for def in defs {
                    let id = def.id.to_string();
                    let locked = self.evolution_locked_params.contains(def.id);
                    col = col.push(
                        checkbox(def.display_name, locked)
                        .size(14)
                        .text_size(11)
                        .on_toggle(move |v| Message::EvolutionParamLockToggled(id.clone(), v)),
                    );
                }
                section = section.push(scrollable(col).height(Length::Fixed(160.0)));
            }
        }
        section.into()
    }
}
