use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Widget;

use super::gradient_bar::GRADIENT_END;

/// The braille "dot" spinner, stored clockwise: the missing dot goes along the top, down the right column, along the
/// bottom and up the left. [`Direction::CounterClockwise`] reads it backwards from the same first frame.
const FRAMES: [&str; 8] = ["⣾", "⣷", "⣯", "⣟", "⡿", "⢿", "⣻", "⣽"];

/// How long each frame shows: 10 frames a second, the rate of bubbles' `spinner.Dot`.
const FRAME_DURATION: Duration = Duration::from_millis(100);

/// Which way a [`Spinner`] turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    /// `⣾ ⣷ ⣯ ⣟ ⡿ ⢿ ⣻ ⣽`.
    #[default]
    Clockwise,
    /// `⣾ ⣽ ⣻ ⢿ ⡿ ⣟ ⣯ ⣷`, the order of bubbles' `spinner.Dot`.
    CounterClockwise,
}

/// A one-cell braille spinner that moves one frame every 100 ms.
///
/// The frame comes from the time since the spinner was created, not from how often it is drawn, so it turns at the
/// same speed in a 60 fps [`Live`](super::Live) session and in a caller that draws twice a second. Inside a
/// session it animates on its own: the session redraws every frame, and every draw reads the clock.
///
/// It is usually one [`span`](Self::span) among others in a caller's [`Line`](ratatui::text::Line); `&Spinner` is
/// also a [`Widget`] that draws the glyph in the area's first cell.
///
/// ```
/// use ratatui::text::Line;
/// use rust_sak::tui::{Direction, Spinner};
///
/// let mut spinner = Spinner::new();
/// spinner.direction = Direction::CounterClockwise;
///
/// let line = Line::from(vec!["Downloading… ".into(), spinner.span(), " 34% Complete".into()]);
/// ```
#[derive(Debug, Clone)]
pub struct Spinner {
    /// Which way it turns.
    pub direction: Direction,
    /// The glyph's style when `color` is on. Defaults to the gradient's end colour, `#ee6ff8`.
    pub style: Style,
    /// Whether to apply `style`; without it, the glyph keeps the terminal's default style.
    pub color: bool,
    started: Instant,
}

impl Spinner {
    /// A coloured, clockwise spinner on its first frame, started now.
    #[must_use]
    pub fn new() -> Self {
        let (r, g, b) = GRADIENT_END;
        Self {
            direction: Direction::default(),
            style: Style::new().fg(Color::Rgb(r, g, b)),
            color: true,
            started: Instant::now(),
        }
    }

    /// The glyph shown `elapsed` after the spinner started, in its direction.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is taken modulo the frame count first"
    )]
    pub fn frame_at(&self, elapsed: Duration) -> &'static str {
        let step = (elapsed.as_millis() / FRAME_DURATION.as_millis() % FRAMES.len() as u128) as usize;
        let index = match self.direction {
            Direction::Clockwise => step,
            Direction::CounterClockwise => (FRAMES.len() - step) % FRAMES.len(),
        };
        FRAMES[index]
    }

    /// The glyph to show now.
    #[must_use]
    pub fn glyph(&self) -> &'static str {
        self.frame_at(self.started.elapsed())
    }

    /// The glyph to show now, in `style` when `color` is on and plain otherwise.
    #[must_use]
    pub fn span(&self) -> Span<'static> {
        let style = if self.color { self.style } else { Style::new() };
        Span::styled(self.glyph(), style)
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &Spinner {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.span().render(area, buf);
    }
}
