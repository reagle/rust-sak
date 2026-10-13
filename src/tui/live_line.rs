use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

/// What a [`Live`](super::Live) session draws: one row unless [`height`](Self::height) says more, redrawn every frame.
pub trait LiveLine: Send + 'static {
    /// Draws the line into `area`, [`height`](Self::height) rows tall, or fewer when the terminal is shorter. It takes
    /// `&mut self` so an animated line can advance by one frame.
    fn render(&mut self, area: Rect, buf: &mut Buffer);

    /// Whether the line has finished animating. [`Live::finish`](super::Live::finish) keeps drawing until it has,
    /// up to its settle cap. A line that does not animate is always settled.
    fn settled(&self) -> bool {
        true
    }

    /// Jumps to the end of any animation, so the last frame shows the final state. Called by
    /// [`Live::finish`](super::Live::finish) when the settle cap is reached first.
    fn snap(&mut self) {}

    /// How many rows the session reserves for the line. Read once, when [`Live::start`](super::Live::start) is
    /// called, and fixed for the session's life; `0` is treated as `1`.
    fn height(&self) -> u16 {
        1
    }
}

/// A plain line of text, such as a status message, drawn as it is.
impl LiveLine for Line<'static> {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Widget::render(&*self, area, buf);
    }
}
