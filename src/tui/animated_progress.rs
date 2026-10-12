use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::{LiveLine, ProgressLine, Spring};

/// A [`ProgressLine`] whose bar eases towards [`fraction`](Self::fraction) with a [`Spring`], for a
/// [`Live`](super::Live) session.
///
/// The percentage shows `fraction` as soon as it changes; only the bar animates. Each render advances the animation
/// by one frame, and the line is [settled](LiveLine::settled) once the bar has arrived.
///
/// ```
/// use rust_sak::tui::AnimatedProgress;
///
/// let mut progress = AnimatedProgress::new("Upscale");
/// progress.label_width = 13;
/// progress.counter = Some((1, 2));
/// progress.fraction = 0.5;
/// ```
#[derive(Debug, Clone)]
pub struct AnimatedProgress {
    /// The text before the bar. See [`ProgressLine::label`].
    pub label: String,
    /// The columns the label takes at least. See [`ProgressLine::label_width`].
    pub label_width: usize,
    /// The `[n/N]` counter. See [`ProgressLine::counter`].
    pub counter: Option<(usize, usize)>,
    /// How much of the work is done, from 0 to 1: the percentage shown, and where the bar is heading.
    pub fraction: f64,
    /// The ETA column. See [`ProgressLine::eta`].
    pub eta: Option<Option<Duration>>,
    /// Whether to colour the line.
    pub color: bool,
    spring: Spring,
}

impl AnimatedProgress {
    /// An empty, coloured bar labelled `label`, with no counter and no ETA column.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            label_width: 0,
            counter: None,
            fraction: 0.0,
            eta: None,
            color: true,
            spring: Spring::default(),
        }
    }

    /// How much of the bar is filled at the moment, from 0 to 1.
    pub fn position(&self) -> f64 {
        self.spring.position
    }

    fn target(&self) -> f64 {
        self.fraction.clamp(0.0, 1.0)
    }
}

impl LiveLine for AnimatedProgress {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.spring.target = self.target();
        self.spring.step();

        ProgressLine {
            label: &self.label,
            label_width: self.label_width,
            counter: self.counter,
            fraction: self.fraction,
            position: self.spring.position,
            eta: self.eta,
            color: self.color,
        }
        .render(area, buf);
    }

    #[allow(
        clippy::float_cmp,
        reason = "the spring's target is a copy of `target()`, so equality is exact"
    )]
    fn settled(&self) -> bool {
        self.spring.target == self.target() && self.spring.is_settled()
    }

    fn snap(&mut self) {
        self.spring.target = self.target();
        self.spring.snap();
    }
}
