use crate::app::ChessGui;
use crate::clog;
use crate::messages::Message;
use crate::ui::etext;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, row, scrollable, text, text_input, Column};
use iced::{Background, Border, Color, Element, Font, Length};
// Monochrome palette for the in-app console.
const TERM_BG: Color = Color::from_rgb(0.08, 0.09, 0.11);
const TERM_BORDER: Color = Color::from_rgb(0.25, 0.27, 0.30);
const TERM_FG: Color = Color::from_rgb(0.86, 0.88, 0.90);
const TERM_FG_DIM: Color = Color::from_rgb(0.55, 0.58, 0.62);
impl ChessGui {
    pub(crate) fn create_terminal_section(&self) -> Element<'_, Message> {
        let toggle_label = if self.show_console {
            "▾ Console"
        } else {
            "▸ Console"
        };
        let toggle_btn = button(etext(toggle_label).size(12))
        .on_press(Message::ToggleConsole)
        .padding(4)
        .style(|_theme, status| {
            let base = iced::widget::button::Style {
                background: None,
                text_color: TERM_FG_DIM,
                border: iced::Border::default(),
               shadow: iced::Shadow::default(),
            };
            match status {
                iced::widget::button::Status::Hovered => iced::widget::button::Style {
                    text_color: Color::WHITE,
                    ..base
                },
                _ => base,
            }
        });
        let header = row![toggle_btn, etext("Terminal").size(16)].spacing(8);
        let mut section = column![header].spacing(5);
        if self.show_console {
            // Drain by value so the produced Text widgets own their String
            // content. Borrowing from a local Vec would couple the
            // returned Element to a stack frame that is about to be
            // dropped.
            let lines = crate::console::recent_lines(120);
            let mut output_col = Column::new().spacing(1).padding(6);
            if lines.is_empty() {
                output_col = output_col.push(
                    text("(console is empty — type `help` below)")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .shaping(Shaping::Advanced)
                    .style(|_| iced::widget::text::Style {
                        color: Some(TERM_FG_DIM),
                    }),
                );
            } else {
                for line in lines {
                    // `line` is moved into the widget — no borrow of a local.
                    output_col = output_col.push(
                        text(line)
                        .size(11)
                        .font(Font::MONOSPACE)
                        .shaping(Shaping::Advanced)
                        .style(|_| iced::widget::text::Style {
                            color: Some(TERM_FG),
                        }),
                    );
                }
            }
            let output = container(
                scrollable(output_col)
                .height(Length::Fixed(180.0))
                .width(Length::Fill),
            )
            .style(|_| container::Style {
                background: Some(Background::Color(TERM_BG)),
                   border: Border {
                       color: TERM_BORDER,
                       width: 1.0,
                       radius: 4.0.into(),
                   },
                   text_color: Some(TERM_FG),
                   ..Default::default()
            });
            section = section.push(output);
        }
        let terminal_input = text_input("Enter command (try 'help')", &self.terminal_input)
        .on_input(Message::TerminalInputChanged)
        .on_submit(Message::TerminalCommand)
        .font(Font::MONOSPACE)
        .width(Length::Fill);
        section.push(terminal_input).into()
    }
}
// Silence unused-import warning when the module is built standalone.
#[allow(dead_code)]
fn _force_clog_used() {
    let _ = clog!();
}
