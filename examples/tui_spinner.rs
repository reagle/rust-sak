//! Draws `Downloading… <spinner> <n>% Complete` on stderr, turning clockwise for a few seconds and then
//! counter-clockwise.
//!
//! Run with: `cargo run --example tui_spinner --features tui`
//!
//! The producer only sets the percentage, a few times a second. The spinner turns at a steady 10 frames a second
//! regardless, because every frame the session draws reads the clock.

use std::thread::sleep;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;
use rust_sak::tui::{Direction, Live, LiveLine, Spinner, Target};

const TICKS: u32 = 20;
const TICK: Duration = Duration::from_millis(150);

/// A line of text around a spinner, rebuilt on every frame so the glyph is the current one.
struct Downloading {
    spinner: Spinner,
    percent: u32,
}

impl LiveLine for Downloading {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Line::from(vec![
            "Downloading… ".into(),
            self.spinner.span(),
            format!(" {}% Complete", self.percent).into(),
        ])
        .render(area, buf);
    }
}

fn main() -> std::io::Result<()> {
    for direction in [Direction::Clockwise, Direction::CounterClockwise] {
        eprintln!("{direction:?}:");

        let mut spinner = Spinner::new();
        spinner.direction = direction;
        let live = Live::start(Target::Stderr, Downloading { spinner, percent: 0 }, || {})?;
        let handle = live.handle();

        for tick in 1..=TICKS {
            sleep(TICK);
            handle.update(|line| line.percent = tick * 100 / TICKS);
        }

        live.finish()?;
    }

    eprintln!("done");
    Ok(())
}
