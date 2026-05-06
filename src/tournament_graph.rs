// src/tournament_graph.rs
//
// Canvas rendering for the tournament ELO graph.
//
// The graph is a pure function of the EloTracker state — no internal
// mutable state, redrawn from scratch each time the cache is invalidated.
// This keeps it simple and correct at the cost of a bit of redundant work,
// which is fine because the graph only redraws when a game completes (a few
// times per second at most, usually much less).

use crate::Message;
use crate::engine::EngineType;
use crate::tournament::{EloTracker, RatingPoint};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text as CanvasText};
use iced::{Color, Point, Rectangle, Size, Theme, mouse};
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Color palette for engine lines
// ─────────────────────────────────────────────────────────────────────────────
/// Deterministic color assignment per engine. We want colors that are
/// visually distinct and consistent across runs (so "the blue line" always
/// means the same engine). A fixed palette indexed by hash-mod-N does this.
///
/// The palette is chosen for distinguishability on both light and dark
/// backgrounds, avoiding pure red/green for colorblind accessibility. If
/// there are more engines than palette entries, colors repeat — at that
/// point the legend is the disambiguator.
const PALETTE: &[Color] = &[
    Color::from_rgb(0.12, 0.47, 0.71), // blue
    Color::from_rgb(0.89, 0.47, 0.00), // orange
    Color::from_rgb(0.17, 0.63, 0.17), // green
    Color::from_rgb(0.58, 0.40, 0.74), // purple
    Color::from_rgb(0.55, 0.34, 0.29), // brown
    Color::from_rgb(0.89, 0.47, 0.76), // pink
    Color::from_rgb(0.50, 0.50, 0.50), // gray
    Color::from_rgb(0.74, 0.74, 0.13), // olive
    Color::from_rgb(0.09, 0.75, 0.81), // cyan
];

fn engine_color(engine: &EngineType) -> Color {
    // Hash the engine's display name for a stable index. We can't hash
    // EngineType directly (no Hash derive), and adding one would be a
    // change to registry.rs we'd rather avoid. Name is stable and unique.
    let name = engine.name();
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

// ─────────────────────────────────────────────────────────────────────────────
// Layout constants
// ─────────────────────────────────────────────────────────────────────────────
const PADDING_LEFT: f32 = 55.0; // room for Y-axis labels
const PADDING_RIGHT: f32 = 15.0;
const PADDING_TOP: f32 = 15.0;
const PADDING_BOTTOM: f32 = 30.0; // room for X-axis labels
const LEGEND_ROW_HEIGHT: f32 = 18.0;
const LEGEND_SWATCH: f32 = 12.0;

// ─────────────────────────────────────────────────────────────────────────────
// The canvas program
// ─────────────────────────────────────────────────────────────────────────────
pub struct TournamentGraph<'a> {
    pub elo: &'a EloTracker,
    pub total_games: usize,
    pub cache: &'a canvas::Cache,
}

impl<'a> TournamentGraph<'a> {
    /// Transform (game_number, rating) into canvas coordinates.
    fn plot_point(
        &self,
        game: usize,
        rating: f64,
        plot: Rectangle,
        x_max: usize,
        y_lo: f64,
        y_hi: f64,
    ) -> Point {
        // X: game 0 at left edge, game x_max at right edge. Guard against
        // division by zero when there's only one game.
        let x_frac = if x_max > 0 {
            game as f32 / x_max as f32
        } else {
            0.0
        };
        // Y: higher rating = higher on screen (canvas Y grows downward, so
        // invert).
        let y_frac = ((rating - y_lo) / (y_hi - y_lo)) as f32;
        Point::new(
            plot.x + x_frac * plot.width,
            plot.y + (1.0 - y_frac) * plot.height,
        )
    }

    fn draw_axes(&self, frame: &mut Frame, plot: Rectangle, x_max: usize, y_lo: f64, y_hi: f64) {
        let axis_color = Color::from_rgb(0.6, 0.6, 0.6);
        let grid_color = Color::from_rgba(0.0, 0.0, 0.0, 0.08);
        let label_color = Color::from_rgb(0.3, 0.3, 0.3);

        // Frame box.
        let box_path = Path::rectangle(
            Point::new(plot.x, plot.y),
            Size::new(plot.width, plot.height),
        );
        frame.stroke(
            &box_path,
            Stroke::default().with_color(axis_color).with_width(1.0),
        );

        // Y-axis gridlines & labels. We pick ~5 gridlines, rounded to the
        // nearest 25 ELO (ELO is conventionally reported in whole numbers
        // and 25 is a readable increment).
        let y_range = y_hi - y_lo;
        let y_step_raw = y_range / 5.0;
        let y_step = (y_step_raw / 25.0).ceil() * 25.0;
        let y_start = (y_lo / y_step).ceil() * y_step;
        let mut y = y_start;

        while y <= y_hi {
            let p = self.plot_point(0, y, plot, x_max, y_lo, y_hi);
            let grid = Path::line(
                Point::new(plot.x, p.y),
                Point::new(plot.x + plot.width, p.y),
            );
            frame.stroke(
                &grid,
                Stroke::default().with_color(grid_color).with_width(1.0),
            );
            frame.fill_text(CanvasText {
                content: format!("{:.0}", y),
                position: Point::new(plot.x - 8.0, p.y),
                color: label_color,
                size: iced::Pixels(11.0),
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
            y += y_step;
        }

        // X-axis labels at start, middle, end. More would clutter.
        for &g in &[0, x_max / 2, x_max] {
            if x_max == 0 && g > 0 {
                continue;
            }
            let p = self.plot_point(g, y_lo, plot, x_max, y_lo, y_hi);
            frame.fill_text(CanvasText {
                content: g.to_string(),
                position: Point::new(p.x, plot.y + plot.height + 6.0),
                color: label_color,
                size: iced::Pixels(11.0),
                horizontal_alignment: iced::alignment::Horizontal::Center,
                vertical_alignment: iced::alignment::Vertical::Top,
                ..Default::default()
            });
        }

        // Axis titles.
        frame.fill_text(CanvasText {
            content: "Game".to_string(),
            position: Point::new(plot.x + plot.width / 2.0, plot.y + plot.height + 20.0),
            color: label_color,
            size: iced::Pixels(10.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Top,
            ..Default::default()
        });
    }

    fn draw_lines(&self, frame: &mut Frame, plot: Rectangle, x_max: usize, y_lo: f64, y_hi: f64) {
        // Group rating points by engine, in game-number order. The tracker
        // emits them in chronological order already, but grouping here
        // keeps us robust to future changes.
        let mut by_engine: HashMap<EngineType, Vec<&RatingPoint>> = HashMap::new();
        for p in self.elo.history() {
            by_engine.entry(p.engine.clone()).or_default().push(p);
        }

        for (engine, points) in &by_engine {
            if points.len() < 2 {
                // A single point isn't a line. We'll draw a dot instead so
                // the engine still shows up visually.
                if let Some(p) = points.first() {
                    let pt = self.plot_point(p.game_number, p.rating, plot, x_max, y_lo, y_hi);
                    let dot = Path::circle(pt, 3.0);
                    frame.fill(&dot, engine_color(engine));
                }
                continue;
            }

            let color = engine_color(engine);
            // Build a polyline. Iced's Path builder lets us chain line_to
            // calls; we move_to the first point, then line_to the rest.
            let path = Path::new(|builder| {
                let mut iter = points.iter();
                if let Some(first) = iter.next() {
                    builder.move_to(self.plot_point(
                        first.game_number,
                        first.rating,
                        plot,
                        x_max,
                        y_lo,
                        y_hi,
                    ));
                }
                for p in iter {
                    builder.line_to(self.plot_point(
                        p.game_number,
                        p.rating,
                        plot,
                        x_max,
                        y_lo,
                        y_hi,
                    ));
                }
            });
            frame.stroke(&path, Stroke::default().with_color(color).with_width(2.0));
        }
    }

    fn draw_legend(&self, frame: &mut Frame, bounds: Size) {
        // Legend goes in the top-left of the plot area, one row per engine.
        // We sort by current rating (descending) so the standings are
        // readable at a glance — strongest engine at the top.
        let mut engines = self.elo.participants();
        engines.sort_by(|a, b| {
            self.elo
                .rating(b)
                .partial_cmp(&self.elo.rating(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let legend_x = PADDING_LEFT + 10.0;
        let legend_y = PADDING_TOP + 10.0;

        // Semi-transparent background box so the legend is readable over
        // the lines.
        if !engines.is_empty() {
            let legend_h = engines.len() as f32 * LEGEND_ROW_HEIGHT + 8.0;
            // Width is hard to predict without measuring text; we use a
            // generous fixed width and live with it. The graph panel is
            // wide enough.
            let legend_w = (bounds.width - PADDING_LEFT - PADDING_RIGHT - 20.0).min(240.0);
            frame.fill_rectangle(
                Point::new(legend_x - 4.0, legend_y - 4.0),
                Size::new(legend_w, legend_h),
                Color::from_rgba(1.0, 1.0, 1.0, 0.85),
            );
        }

        for (i, engine) in engines.iter().enumerate() {
            let row_y = legend_y + i as f32 * LEGEND_ROW_HEIGHT;
            let color = engine_color(engine);

            // Color swatch.
            frame.fill_rectangle(
                Point::new(legend_x, row_y),
                Size::new(LEGEND_SWATCH, LEGEND_SWATCH),
                color,
            );

            // Engine name and current rating. Truncate long names.
            let name = engine.name();
            let display_name = if name.len() > 22 {
                format!("{}…", &name[..21])
            } else {
                name.to_string()
            };

            frame.fill_text(CanvasText {
                content: format!("{}  {:.0}", display_name, self.elo.rating(engine)),
                position: Point::new(legend_x + LEGEND_SWATCH + 6.0, row_y + LEGEND_SWATCH / 2.0),
                color: Color::from_rgb(0.2, 0.2, 0.2),
                size: iced::Pixels(11.0),
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
    }

    fn draw_empty_state(&self, frame: &mut Frame, bounds: Size) {
        frame.fill_text(CanvasText {
            content: "No games played yet".to_string(),
            position: Point::new(bounds.width / 2.0, bounds.height / 2.0),
            color: Color::from_rgb(0.6, 0.6, 0.6),
            size: iced::Pixels(14.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
    }
}

impl<'a> canvas::Program<Message> for TournamentGraph<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let geom = self.cache.draw(renderer, bounds.size(), |frame| {
            let size = frame.size();
            // White background — keeps the graph readable regardless of
            // the surrounding theme.
            frame.fill_rectangle(Point::ORIGIN, size, Color::from_rgb(0.99, 0.99, 0.99));

            if self.elo.history().is_empty() {
                self.draw_empty_state(frame, size);
                return;
            }

            let plot = Rectangle {
                x: PADDING_LEFT,
                y: PADDING_TOP,
                width: size.width - PADDING_LEFT - PADDING_RIGHT,
                height: size.height - PADDING_TOP - PADDING_BOTTOM,
            };

            // X axis: either the full scheduled length (so the graph doesn't
            // rescale as games accumulate) or the number of games played
            // (if the tournament is complete and had fewer games than
            // expected — shouldn't happen, but robustness).
            let x_max = self
                .total_games
                .max(
                    self.elo
                        .history()
                        .iter()
                        .map(|p| p.game_number)
                        .max()
                        .unwrap_or(1),
                )
                .max(1);

            let (y_lo, y_hi) = self.elo.rating_bounds();
            self.draw_axes(frame, plot, x_max, y_lo, y_hi);
            self.draw_lines(frame, plot, x_max, y_lo, y_hi);
            self.draw_legend(frame, size);
        });
        vec![geom]
    }

    // Graph is display-only; no interaction.
    fn update(
        &self,
        _state: &mut Self::State,
        _event: canvas::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        (canvas::event::Status::Ignored, None)
    }
}
