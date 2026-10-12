use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

pub(super) const GRADIENT_START: (u8, u8, u8) = (0x5a, 0x56, 0xe0);
pub(super) const GRADIENT_END: (u8, u8, u8) = (0xee, 0x6f, 0xf8);
pub(super) const EMPTY: Color = Color::Rgb(0x60, 0x60, 0x60);

pub(super) const FILLED_SYMBOL: &str = "█";
pub(super) const EMPTY_SYMBOL: &str = "░";

/// A one-row bar that fills `position` (0 to 1) of its area with `█`, coloured from `#5a56e0` to `#ee6ff8` across the
/// full bar width, and the rest with `░` in `#606060`.
///
/// Without colour, the cells keep the terminal's default style and only the symbols show the progress.
#[derive(Debug, Clone, Copy)]
pub struct GradientBar {
    /// How much of the bar is filled, from 0 to 1; values outside that range are clamped.
    pub position: f64,
    /// Whether to colour the cells.
    pub color: bool,
}

impl Widget for GradientBar {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let width = area.width;
        let filled = (self.position.clamp(0.0, 1.0) * f64::from(width)).round() as u16;

        for i in 0..width {
            let (symbol, color) = if i < filled {
                (
                    FILLED_SYMBOL,
                    gradient(f64::from(i) / f64::from(width.saturating_sub(1).max(1))),
                )
            } else {
                (EMPTY_SYMBOL, EMPTY)
            };
            buf[(area.x + i, area.y)]
                .set_symbol(symbol)
                .set_style(super::fg(color, self.color));
        }
    }
}

/// The gradient colour at `t` (0 to 1), by linear interpolation in RGB.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn gradient(t: f64) -> Color {
    let lerp = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    let ((r1, g1, b1), (r2, g2, b2)) = (GRADIENT_START, GRADIENT_END);
    Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
}
