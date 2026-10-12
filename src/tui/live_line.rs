use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

/// What a [`Live`](super::Live) session draws: one row, redrawn every frame.
pub trait LiveLine: Send + 'static {
    /// Draws the line into `area`, a single row. It takes `&mut self` so an animated line can advance by one frame.
    fn render(&mut self, area: Rect, buf: &mut Buffer);

    /// Whether the line has finished animating. [`Live::finish`](super::Live::finish) keeps drawing until it has,
    /// up to its settle cap. A line that does not animate is always settled.
    fn settled(&self) -> bool {
        true
    }

    /// Jumps to the end of any animation, so the last frame shows the final state. Called by
    /// [`Live::finish`](super::Live::finish) when the settle cap is reached first.
    fn snap(&mut self) {}
}

/// A plain line of text, such as a status message, drawn as it is.
impl LiveLine for Line<'static> {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Widget::render(&*self, area, buf);
    }
}
