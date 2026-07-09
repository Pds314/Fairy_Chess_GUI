use crate::app::ChessGui;
use crate::constants;
use crate::core::board::Board;
use crate::core::Position;
use crate::drawing::BoardDrawer;
use crate::messages::Message;
use iced::widget::canvas;
use iced::{Element, Length};

impl ChessGui {
    /// Which board to draw. During a tournament we show the first running
    /// worker's latest snapshot; otherwise the real game state.
    pub(crate) fn displayed_board(&self) -> (&Board, Option<(Position, Position)>) {
        if let Some(w) = self.tournament_workers.first() {
            if let Some(snap) = &w.last_snapshot {
                return (&snap.board, snap.last_move);
            }
        }
        if let Some(w) = self.evolution_workers.first() {
            if let Some(snap) = &w.last_snapshot {
                return (&snap.board, snap.last_move);
            }
        }
        (&self.game_state.board, self.last_move_highlight)
    }

    pub(crate) fn create_board_canvas(&self) -> Element<'_, Message> {
        let (board, last_move) = self.displayed_board();
        let (rows, cols) = board.size();
        let board_width = constants::DEFAULT_SQUARE_SIZE * cols as f32 + constants::BOARD_PADDING;
        let board_height = constants::DEFAULT_SQUARE_SIZE * rows as f32 + constants::BOARD_PADDING;
        let canvas_size = board_width.max(board_height);

        canvas(BoardDrawer {
            board,
            selected_square: self.selected_square,
            cache: &self.board_cache,
            texture_manager: &self.texture_manager,
            piece_config: &self.piece_config,
            last_move_highlight: last_move,
        })
        .width(Length::Fixed(canvas_size))
        .height(Length::Fixed(canvas_size))
        .into()
    }
}
