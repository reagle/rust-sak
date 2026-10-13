//! Draws a progress bar over a five-row list of items on stderr, a seven-row live session.
//!
//! Run with: `cargo run --example tui_rows --features tui`
//!
//! - `--drop` drops the session halfway instead of finishing it.
//! - `--panic` panics halfway, with the session still up.
//!
//! The list scrolls to the most recent item. Until it fills, the rows under the last item are drawn blank.

use std::thread::sleep;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use rust_sak::tui::{AnimatedProgress, Live, LiveLine, Target};

const ITEMS: [&str; 8] = [
    "a.jpg", "b.jpg", "c.png", "d.heic", "e.tiff", "f.jpg", "g.webp", "h.dng",
];
const LIST_ROWS: usize = 5;
const TICKS_PER_ITEM: usize = 30;

/// A bar, a blank row, and the last [`LIST_ROWS`] items started so far.
struct BarAndList {
    bar: AnimatedProgress,
    items: Vec<String>,
}

impl LiveLine for BarAndList {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let [bar, _, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        self.bar.render(bar, buf);

        // In a terminal too short for the whole list, the most recent items that fit.
        let shown = self.items.len().saturating_sub(LIST_ROWS.min(usize::from(list.height)));
        for (item, row) in self.items[shown..].iter().zip(list.rows()) {
            Line::from(item.as_str()).render(row, buf);
        }
    }

    fn settled(&self) -> bool {
        self.bar.settled()
    }

    fn snap(&mut self) {
        self.bar.snap();
    }

    #[allow(clippy::cast_possible_truncation, reason = "LIST_ROWS is 5")]
    fn height(&self) -> u16 {
        2 + LIST_ROWS as u16
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);

    let mut bar = AnimatedProgress::new("Enhance");
    bar.counter = Some((0, ITEMS.len()));
    let live = Live::start(Target::Stderr, BarAndList { bar, items: Vec::new() }, || {})?;
    let handle = live.handle();

    let total = ITEMS.len() * TICKS_PER_ITEM;
    for tick in 0..total {
        if tick == total / 2 {
            if flag("--drop") {
                drop(live);
                eprintln!("dropped early");
                return Ok(());
            }
            assert!(!flag("--panic"), "the producer panicked halfway");
        }

        let index = tick / TICKS_PER_ITEM;
        #[allow(clippy::cast_precision_loss)]
        let fraction = (tick + 1) as f64 / total as f64;
        handle.update(|line| {
            if tick % TICKS_PER_ITEM == 0 {
                if let Some(previous) = line.items.last_mut() {
                    *previous = previous.replace("Enhancing", "Enhanced ");
                }
                line.items
                    .push(format!("[{}/{}] Enhancing {}", index + 1, ITEMS.len(), ITEMS[index]));
            }
            line.bar.counter = Some((index + 1, ITEMS.len()));
            line.bar.fraction = fraction;
        });
        sleep(Duration::from_millis(25));
    }

    handle.update(|line| {
        if let Some(last) = line.items.last_mut() {
            *last = last.replace("Enhancing", "Enhanced ");
        }
    });
    live.finish()?;
    eprintln!("done");
    Ok(())
}
