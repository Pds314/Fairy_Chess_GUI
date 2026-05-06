// src/drawing.rs
use crate::Message;
use crate::constants::{
    self, BOARD_PADDING, DARK_SQUARE, HIGHLIGHT_COLOR, LIGHT_SQUARE, PIECE_COLOR_BLACK,
    PIECE_COLOR_WHITE,
};
use crate::core::board::Board;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use crate::texture_manager::TextureManager;
use iced::widget::canvas::{self, Geometry, Text as CanvasText};
use iced::{Color, Point, Rectangle, Size, Theme, mouse};

/// Handles drawing the chess board and pieces
#[derive(Debug)]
pub struct BoardDrawer<'a> {
    pub board: &'a Board,
    pub selected_square: Option<Position>,
    pub cache: &'a canvas::Cache,
    pub texture_manager: &'a TextureManager,
    pub piece_config: &'a PieceConfigManager,
    pub last_move_highlight: Option<(Position, Position)>,
}

impl<'a> BoardDrawer<'a> {
    /// Calculate the square size based on the canvas dimensions
    fn calculate_square_size(&self, bounds: Size) -> f32 {
        let (rows, cols) = self.board.size();
        if rows == 0 || cols == 0 {
            return 0.0;
        } // Avoid division by zero
        let available_width = (bounds.width - constants::BOARD_PADDING) / cols as f32;
        let available_height = (bounds.height - constants::BOARD_PADDING) / rows as f32;
        available_width.min(available_height)
    }

    /// Calculate board offset to center it in the canvas
    fn calculate_board_offset(&self, square_size: f32, bounds: Size) -> Point {
        let (rows, cols) = self.board.size();
        let board_width = cols as f32 * square_size;
        let board_height = rows as f32 * square_size;

        Point::new(
            (bounds.width - board_width) / 2.0,
            (bounds.height - board_height) / 2.0,
        )
    }

    /// Convert board position to screen coordinates
    fn position_to_screen(&self, pos: Position, square_size: f32, offset: Point) -> Point {
        Point::new(
            offset.x + pos.1 as f32 * square_size,
            offset.y + pos.0 as f32 * square_size,
        )
    }

    /// Convert screen coordinates to board position
    fn screen_to_position(
        &self,
        point: Point,
        square_size: f32,
        offset: Point,
    ) -> Option<Position> {
        let (rows, cols) = self.board.size();

        if point.x < offset.x || point.y < offset.y {
            return None;
        }

        let col = ((point.x - offset.x) / square_size).floor() as usize;
        let row = ((point.y - offset.y) / square_size).floor() as usize;

        if row < rows && col < cols {
            Some((row, col))
        } else {
            None
        }
    }

    /// Draw a single square
    fn draw_square(
        &self,
        frame: &mut canvas::Frame,
        pos: Position,
        square_size: f32,
        offset: Point,
    ) {
        let top_left = self.position_to_screen(pos, square_size, offset);
        let color = self.get_square_color(pos);

        frame.fill_rectangle(top_left, Size::new(square_size, square_size), color);

        // Draw last move highlight
        if let Some((from, to)) = self.last_move_highlight {
            if pos == from {
                // Light yellow for move source
                frame.fill_rectangle(
                    top_left,
                    Size::new(square_size, square_size),
                    Color::from_rgba8(255, 255, 150, 0.6),
                );
            } else if pos == to {
                // Slightly darker yellow for move destination
                frame.fill_rectangle(
                    top_left,
                    Size::new(square_size, square_size),
                    Color::from_rgba8(255, 235, 100, 0.7),
                );
            }
        }

        // Draw highlight if selected (on top of last move highlight)
        if self.selected_square == Some(pos) {
            frame.fill_rectangle(
                top_left,
                Size::new(square_size, square_size),
                *HIGHLIGHT_COLOR,
            );
        }
    }

    /// Get the color for a square
    fn get_square_color(&self, pos: Position) -> Color {
        if (pos.0 + pos.1) % 2 == 0 {
            *LIGHT_SQUARE
        } else {
            *DARK_SQUARE
        }
    }

    /// Draw a piece on the board
    fn draw_piece(
        &self,
        frame: &mut canvas::Frame,
        piece: crate::core::piece::Piece,
        pos: Position,
        square_size: f32,
        offset: Point,
    ) {
        let top_left = self.position_to_screen(pos, square_size, offset);

        if let Some(texture_handle) = self.texture_manager.get_texture(&piece, self.piece_config) {
            self.draw_piece_texture(frame, texture_handle, top_left, square_size);
        } else {
            self.draw_piece_text(frame, piece, top_left, square_size);
        }
    }

    /// Draw a piece using its texture
    fn draw_piece_texture(
        &self,
        frame: &mut canvas::Frame,
        texture_handle: &iced::widget::image::Handle,
        top_left: Point,
        square_size: f32,
    ) {
        let piece_rect = Rectangle {
            x: top_left.x,
            y: top_left.y,
            width: square_size,
            height: square_size,
        };

        let canvas_image = canvas::Image::new(texture_handle.clone());
        frame.draw_image(piece_rect, canvas_image);
    }

    /// Draw a piece using text fallback
    fn draw_piece_text(
        &self,
        frame: &mut canvas::Frame,
        piece: crate::core::piece::Piece,
        top_left: Point,
        square_size: f32,
    ) {
        let piece_char = piece.to_char(self.piece_config);
        let piece_color = match piece.color {
            PieceColor::White => *PIECE_COLOR_WHITE,
            PieceColor::Black => *PIECE_COLOR_BLACK,
        };

        let piece_text = CanvasText {
            content: piece_char.to_string(),
            position: Point::new(
                top_left.x + square_size / 2.0,
                top_left.y + square_size / 2.0,
            ),
            color: piece_color,
            size: iced::Pixels(square_size * 0.6),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..CanvasText::default()
        };

        frame.fill_text(piece_text);
    }

    /// Draw the entire board
    fn draw_board(&self, frame: &mut canvas::Frame, bounds: Size) {
        let square_size = self.calculate_square_size(bounds);
        let offset = self.calculate_board_offset(square_size, bounds);
        let (rows, cols) = self.board.size();

        // Draw all squares
        for row in 0..rows {
            for col in 0..cols {
                let pos = (row, col);
                self.draw_square(frame, pos, square_size, offset);

                // Draw piece if present
                if let Some(piece) = self.board.get_piece(pos) {
                    self.draw_piece(frame, piece, pos, square_size, offset);
                }
            }
        }
    }

    /// Draw coordinate labels around the board
    // In draw_coordinates method
    fn draw_coordinates(&self, frame: &mut canvas::Frame, bounds: Size) {
        let square_size = self.calculate_square_size(bounds);
        let offset = self.calculate_board_offset(square_size, bounds);
        let (rows, cols) = self.board.size();

        // File labels (a, b, c, ... z, aa, ab, ...)
        for col in 0..cols {
            let label = crate::notation::file_to_algebraic(col);
            let position = Point::new(
                offset.x + col as f32 * square_size + square_size / 2.0,
                offset.y + rows as f32 * square_size + 10.0,
            );

            self.draw_label(frame, &label, position);
        }

        // Rank labels (1, 2, 3, ...)
        for row in 0..rows {
            let label = crate::notation::rank_to_algebraic(row, rows);
            let position = Point::new(
                offset.x - 20.0,
                offset.y + row as f32 * square_size + square_size / 2.0,
            );

            self.draw_label(frame, &label, position);
        }
    }

    /// Draw a text label
    fn draw_label(&self, frame: &mut canvas::Frame, text: &str, position: Point) {
        let label = CanvasText {
            content: text.to_string(),
            position,
            color: Color::BLACK,
            size: iced::Pixels(12.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..CanvasText::default()
        };

        frame.fill_text(label);
    }
}

impl<'a> canvas::Program<Message> for BoardDrawer<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            self.draw_board(frame, frame.size());
            self.draw_coordinates(frame, frame.size());
        });

        vec![geometry]
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(pos) = cursor.position_in(bounds) {
                let square_size = self.calculate_square_size(bounds.size());
                let offset = self.calculate_board_offset(square_size, bounds.size());

                if let Some(board_pos) = self.screen_to_position(pos, square_size, offset) {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::SquareClicked(board_pos)),
                    );
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}
