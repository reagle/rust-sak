use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::{GradientBar, fg, format_eta};

pub(super) const GRAY: (u8, u8, u8) = (0x68, 0x68, 0x68);
pub(super) const GREEN: (u8, u8, u8) = (0x00, 0xc2, 0x02);
pub(super) const MAGENTA: (u8, u8, u8) = (0xc7, 0x92, 0xe9);

const MAX_BAR_WIDTH: u16 = 50;
const MIN_BAR_WIDTH: u16 = 10;

/// `<label>   [n/N]  <bar>  xx.x%   ETA …` on one row, with the [`GradientBar`] as wide as fits, between 10 and 50
/// cells.
///
/// - The label is padded to `label_width` columns, so the bar stays in one place while the label changes.
/// - The `[n/N]` counter is drawn only when `counter` is `Some`, with `n` zero-padded to the width of `N`.
/// - The percentage is `fraction` floored to tenths, so `100.0%` appears only once the work is all done.
/// - The ETA column is left out when `eta` is `None`; `Some(None)` draws `ETA --`, for an estimate not known yet.
///
/// On a row too narrow for the whole line, the bar shrinks to 10 cells and then the end of the line is cut off.
///
/// ```
/// use std::time::Duration;
/// use rust_sak::tui::ProgressLine;
///
/// let line = ProgressLine {
///     label: "Loading",
///     counter: Some((1, 2)),
///     fraction: 0.5,
///     position: 0.5,
///     eta: Some(Some(Duration::from_millis(4200))),
///     color: true,
///     ..ProgressLine::default()
/// };
/// // On a wide row: Loading   [1/2]  █████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░   50.0%   ETA 4.2s
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct ProgressLine<'a> {
    /// The text before the bar, such as the current step.
    pub label: &'a str,
    /// The columns the label takes at least, padded with spaces; `0` means the label's own width.
    pub label_width: usize,
    /// The `[n/N]` counter, as `(n, N)`.
    pub counter: Option<(usize, usize)>,
    /// How much of the work is done, from 0 to 1, shown as the percentage.
    pub fraction: f64,
    /// How much of the bar is filled, from 0 to 1. Usually [`fraction`](Self::fraction), or a value animated
    /// towards it.
    pub position: f64,
    /// The ETA column: `None` leaves it out, `Some(None)` draws `ETA --`.
    pub eta: Option<Option<Duration>>,
    /// Whether to colour the line.
    pub color: bool,
}

impl Widget for ProgressLine<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let gray = fg(GRAY, self.color);

        let label = Span::raw(self.label);
        let padding = self.label_width.saturating_sub(label.width());
        let mut left = vec![label, Span::raw(" ".repeat(padding + 3))];
        if let Some((completed, total)) = self.counter {
            let digits = total.to_string().len();
            left.extend([
                Span::styled("[", gray),
                Span::raw(format!("{completed:0digits$}")).bold(),
                Span::styled("/", gray),
                Span::raw(total.to_string()).bold(),
                Span::styled("]", gray),
                Span::raw("  "),
            ]);
        }
        let left = Line::from(left);

        let mut right = vec![
            Span::raw("  "),
            Span::styled(
                format!("{:5.1}%", floor_to_tenths(self.fraction)),
                fg(GREEN, self.color),
            ),
        ];
        if let Some(eta) = self.eta {
            right.extend([
                Span::raw("   "),
                Span::styled(format!("ETA {}", format_eta(eta)), fg(MAGENTA, self.color)),
            ]);
        }
        let right = Line::from(right);

        let left_width = u16::try_from(left.width()).unwrap_or(u16::MAX);
        let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
        let bar_width = area
            .width
            .saturating_sub(left_width.saturating_add(right_width))
            .clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH);

        // Each piece is clipped to `area`, so on a row too narrow for the whole line the end is cut off.
        let bar_x = area.x.saturating_add(left_width);
        let right_x = bar_x.saturating_add(bar_width);
        left.render(Rect::new(area.x, area.y, left_width, 1).intersection(area), buf);
        GradientBar {
            position: self.position,
            color: self.color,
        }
        .render(Rect::new(bar_x, area.y, bar_width, 1).intersection(area), buf);
        right.render(Rect::new(right_x, area.y, right_width, 1).intersection(area), buf);
    }
}

/// `fraction` (0 to 1) as a percentage floored to tenths. The small nudge keeps a fraction such as `0.29`, stored as
/// `0.28999…`, from showing as `28.9`.
pub(super) fn floor_to_tenths(fraction: f64) -> f64 {
    ((fraction.clamp(0.0, 1.0) * 1000.0 + 1e-6).floor() / 10.0).min(100.0)
}
