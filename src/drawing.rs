// src/drawing.rs
use crate::Message;
use crate::constants::{
    BOARD_PADDING, DARK_SQUARE, HIGHLIGHT_COLOR, LIGHT_SQUARE, PIECE_COLOR_BLACK,
    PIECE_COLOR_WHITE, self,
};
use crate::core::board::Board;
use crate::core::ghost::GhostFlags;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use crate::piece_config::PieceConfigManager;
use crate::texture_manager::TextureManager;
use iced::widget::canvas::{self, Geometry, Path, Stroke, Text as CanvasText};
use iced::{mouse, Color, Point, Rectangle, Size, Theme};

/// Ghosts are the engine's only invisible board state. They are drawn
/// unconditionally — they exist for exactly one ply and there are never more
/// than a handful, so the cost of always showing them is a ring and a line,
/// and the benefit is that "why was this castle illegal" becomes visible.
fn ghost_colors(flags: GhostFlags) -> (Color, Color) {
    if flags.contains(GhostFlags::CASTLE_TARGET) {
        // cyan — rook destination AND transit assertion
        (
            Color::from_rgba(0.10, 0.75, 0.85, 0.26),
         Color::from_rgba(0.05, 0.55, 0.65, 0.90),
        )
    } else if flags.has_capture_alias() {
        // magenta — en-passant style capture alias
        (
            Color::from_rgba(0.85, 0.20, 0.75, 0.24),
         Color::from_rgba(0.62, 0.10, 0.55, 0.90),
        )
    } else {
        // grey — bare transit ghost; projects royalty, captures nothing
        (
            Color::from_rgba(0.50, 0.50, 0.58, 0.24),
         Color::from_rgba(0.30, 0.30, 0.38, 0.90),
        )
    }
}

const GHOST_LINK: Color = Color::from_rgba(0.12, 0.12, 0.18, 0.65);

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
    fn calculate_square_size(&self, bounds: Size) -> f32 {
        let (rows, cols) = self.board.size();
        if rows == 0 || cols == 0 {
            return 0.0;
        }
        let available_width = (bounds.width - constants::BOARD_PADDING) / cols as f32;
        let available_height = (bounds.height - constants::BOARD_PADDING) / rows as f32;
        available_width.min(available_height)
    }

    fn calculate_board_offset(&self, square_size: f32, bounds: Size) -> Point {
        let (rows, cols) = self.board.size();
        let board_width = cols as f32 * square_size;
        let board_height = rows as f32 * square_size;
        Point::new((bounds.width - board_width) / 2.0, (bounds.height - board_height) / 2.0)
    }

    fn position_to_screen(&self, pos: Position, square_size: f32, offset: Point) -> Point {
        Point::new(offset.x + pos.1 as f32 * square_size, offset.y + pos.0 as f32 * square_size)
    }

    fn square_center(&self, pos: Position, square_size: f32, offset: Point) -> Point {
        let tl = self.position_to_screen(pos, square_size, offset);
        Point::new(tl.x + square_size / 2.0, tl.y + square_size / 2.0)
    }

    fn screen_to_position(&self, point: Point, square_size: f32, offset: Point) -> Option<Position> {
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

    fn draw_square(&self, frame: &mut canvas::Frame, pos: Position, square_size: f32, offset: Point) {
        let top_left = self.position_to_screen(pos, square_size, offset);
        let color = self.get_square_color(pos);
        frame.fill_rectangle(top_left, Size::new(square_size, square_size), color);

        if let Some((from, to)) = self.last_move_highlight {
            if pos == from {
                frame.fill_rectangle(top_left, Size::new(square_size, square_size), Color::from_rgba8(255, 255, 150, 0.6));
            } else if pos == to {
                frame.fill_rectangle(top_left, Size::new(square_size, square_size), Color::from_rgba8(255, 235, 100, 0.7));
            }
        }

        if self.selected_square == Some(pos) {
            frame.fill_rectangle(top_left, Size::new(square_size, square_size), *HIGHLIGHT_COLOR);
        }
    }

    fn get_square_color(&self, pos: Position) -> Color {
        if (pos.0 + pos.1) % 2 == 0 {
            *LIGHT_SQUARE
        } else {
            *DARK_SQUARE
        }
    }

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

    fn draw_piece_texture(
        &self,
        frame: &mut canvas::Frame,
        texture_handle: &iced::widget::image::Handle,
        top_left: Point,
        square_size: f32,
    ) {
        let piece_rect = Rectangle { x: top_left.x, y: top_left.y, width: square_size, height: square_size };
        let canvas_image = canvas::Image::new(texture_handle.clone());
        frame.draw_image(piece_rect, canvas_image);
    }

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
        frame.fill_text(CanvasText {
            content: piece_char.to_string(),
                        position: Point::new(top_left.x + square_size / 2.0, top_left.y + square_size / 2.0),
                        color: piece_color,
                        size: iced::Pixels(square_size * 0.6),
                        horizontal_alignment: iced::alignment::Horizontal::Center,
                        vertical_alignment: iced::alignment::Vertical::Center,
                        ..CanvasText::default()
        });
    }

    /// Draw every live ghost: a tinted, ringed square carrying the ghost's
    /// flag glyph, and a link line to the piece it aliases.
    ///
    ///   `e` restricted capture alias (en passant)   magenta
    ///   `E` open capture alias                      magenta
    ///   `&` castle target (and transit assertion)   cyan
    ///   `'` bare transit ghost                      grey
    ///
    /// A filled dot marks the owner. If the owner is royal, the ghost square
    /// counts as occupied by it for check detection — which is exactly how
    /// "you may not castle out of, or through, check" is enforced.
    fn draw_ghost_overlay(&self, frame: &mut canvas::Frame, square_size: f32, offset: Point) {
        for g in self.board.live_ghosts() {
            let (fill, edge) = ghost_colors(g.flags());
            let sq = g.square();
            let owner = g.owner();

            let tl = self.position_to_screen(sq, square_size, offset);
            frame.fill_rectangle(tl, Size::new(square_size, square_size), fill);

            let inset = 3.0;
            let ring = Path::rectangle(
                Point::new(tl.x + inset, tl.y + inset),
                                       Size::new(square_size - inset * 2.0, square_size - inset * 2.0),
            );
            frame.stroke(&ring, Stroke::default().with_color(edge).with_width(2.0));

            frame.fill_text(CanvasText {
                content: g.flags().glyph().to_string(),
                            position: Point::new(tl.x + square_size * 0.16, tl.y + square_size * 0.16),
                            color: edge,
                            size: iced::Pixels(square_size * 0.26),
                            horizontal_alignment: iced::alignment::Horizontal::Center,
                            vertical_alignment: iced::alignment::Vertical::Center,
                            ..CanvasText::default()
            });

            if owner != sq {
                let a = self.square_center(sq, square_size, offset);
                let b = self.square_center(owner, square_size, offset);
                let link = Path::line(a, b);
                frame.stroke(&link, Stroke::default().with_color(GHOST_LINK).with_width(1.5));

                let dot = Path::circle(b, square_size * 0.07);
                frame.fill(&dot, edge);
                let halo = Path::circle(b, square_size * 0.13);
                frame.stroke(&halo, Stroke::default().with_color(edge).with_width(1.5));
            }
        }
    }

    fn draw_board(&self, frame: &mut canvas::Frame, bounds: Size) {
        let square_size = self.calculate_square_size(bounds);
        let offset = self.calculate_board_offset(square_size, bounds);
        let (rows, cols) = self.board.size();

        for row in 0..rows {
            for col in 0..cols {
                let pos = (row, col);
                self.draw_square(frame, pos, square_size, offset);
                if let Some(piece) = self.board.get_piece(pos) {
                    self.draw_piece(frame, piece, pos, square_size, offset);
                }
            }
        }

        // Over the pieces: the castling transit ghost can sit under the rook.
        self.draw_ghost_overlay(frame, square_size, offset);
    }

    fn draw_coordinates(&self, frame: &mut canvas::Frame, bounds: Size) {
        let square_size = self.calculate_square_size(bounds);
        let offset = self.calculate_board_offset(square_size, bounds);
        let (rows, cols) = self.board.size();

        for col in 0..cols {
            let label = crate::notation::file_to_algebraic(col);
            let position = Point::new(
                offset.x + col as f32 * square_size + square_size / 2.0,
                offset.y + rows as f32 * square_size + 10.0,
            );
            self.draw_label(frame, &label, position);
        }

        for row in 0..rows {
            let label = crate::notation::rank_to_algebraic(row, rows);
            let position = Point::new(offset.x - 20.0, offset.y + row as f32 * square_size + square_size / 2.0);
            self.draw_label(frame, &label, position);
        }
    }

    fn draw_label(&self, frame: &mut canvas::Frame, text: &str, position: Point) {
        frame.fill_text(CanvasText {
            content: text.to_string(),
                        position,
                        color: Color::BLACK,
                        size: iced::Pixels(12.0),
                        horizontal_alignment: iced::alignment::Horizontal::Center,
                        vertical_alignment: iced::alignment::Vertical::Center,
                        ..CanvasText::default()
        });
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
                    return (canvas::event::Status::Captured, Some(Message::SquareClicked(board_pos)));
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

// Keep the unused-import lint quiet about BOARD_PADDING, which is consumed
// through `constants::BOARD_PADDING` above.
#[allow(dead_code)]
const _BOARD_PADDING: f32 = BOARD_PADDING;
