// src/promotion_dialog.rs
use crate::core::Position;
use crate::piece_config::PieceConfigManager;
use crate::texture_manager::TextureManager;
use iced::widget::{Column, Image, button, column, container, row, text};
use iced::{Element, Length, alignment};

#[derive(Debug, Clone)]
pub struct PromotionDialog {
    pub from: Position,
    pub to: Position,
    pub promotion_targets: Vec<usize>,
    pub is_active: bool,
}

impl PromotionDialog {
    pub fn new(from: Position, to: Position, targets: Vec<usize>) -> Self {
        Self {
            from,
            to,
            promotion_targets: targets,
            is_active: true,
        }
    }

    pub fn view<'a>(
        &self,
        texture_manager: &'a TextureManager,
        piece_config: &'a PieceConfigManager,
        current_color: crate::core::PieceColor,
    ) -> Element<'a, crate::Message> {
        if !self.is_active {
            return container(text("")).into();
        }

        let mut pieces_row = row![].spacing(10);

        for &piece_type in &self.promotion_targets {
            if let Some(config) = piece_config.get_piece_by_index(piece_type) {
                let piece = crate::core::Piece::new(current_color, piece_type);

                let mut piece_column = Column::new()
                    .spacing(5)
                    .align_x(alignment::Alignment::Center);

                if let Some(texture_handle) = texture_manager.get_texture(&piece, piece_config) {
                    let image = Image::new(texture_handle.clone())
                        .width(Length::Fixed(64.0))
                        .height(Length::Fixed(64.0));

                    piece_column = piece_column.push(
                        button(image)
                            .on_press(crate::Message::PromotionSelected(piece_type))
                            // FIX: Reverted to the original custom styling closure.
                            .style(
                                |_theme: &iced::Theme, status: button::Status| button::Style {
                                    background: Some(iced::Background::Color(
                                        if matches!(status, button::Status::Hovered) {
                                            iced::Color::from_rgb8(220, 220, 220)
                                        } else {
                                            iced::Color::from_rgb8(240, 240, 240)
                                        },
                                    )),
                                    border: iced::Border {
                                        color: iced::Color::from_rgb8(100, 100, 100),
                                        width: 2.0,
                                        radius: 4.0.into(),
                                    },
                                    ..Default::default()
                                },
                            ),
                    );
                } else {
                    let piece_char = piece.to_char(piece_config);
                    piece_column = piece_column.push(
                        button(text(piece_char.to_string()).size(48))
                            .on_press(crate::Message::PromotionSelected(piece_type))
                            .width(Length::Fixed(64.0))
                            .height(Length::Fixed(64.0)),
                    );
                }

                piece_column = piece_column.push(text(&config.display_name).size(14));

                pieces_row = pieces_row.push(piece_column);
            }
        }

        let dialog_content = column![text("Choose promotion piece:").size(18), pieces_row,]
            .spacing(20)
            .padding(20)
            .align_x(alignment::Alignment::Center);

        container(
            container(dialog_content)
                // FIX: Reverted to the original custom styling closure.
                .style(|_theme: &iced::Theme| container::Style {
                    background: Some(iced::Background::Color(iced::Color::WHITE)),
                    border: iced::Border {
                        color: iced::Color::BLACK,
                        width: 2.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                })
                .padding(10),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .style(|_theme: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba8(
                0, 0, 0, 0.5,
            ))),
            ..Default::default()
        })
        .into()
    }
}
