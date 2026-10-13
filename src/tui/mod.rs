//! A terminal progress display, one line by default.
//!
//! - [`GradientBar`], [`ProgressLine`] and [`format_eta`] draw the line: `Loading   [1/2]  ████░░░░  50.0%   ETA 4.2s`,
//!   with the bar's cells running through a colour gradient.
//! - [`Spring`] eases the bar towards its target, one frame at a time.
//! - [`LiveLine`] is what a live session draws: [`AnimatedProgress`], a plain [`Line`](ratatui::text::Line), or a
//!   line of the caller's own.
//! - [`Live`] draws a [`LiveLine`], on as many rows as it asks for, on stdout or stderr until it is finished, and
//!   [`LiveHandle`] changes it from any thread.
//!
//! ```no_run
//! use rust_sak::tui::{AnimatedProgress, Live, Target};
//!
//! # fn run() -> std::io::Result<()> {
//! let live = Live::start(Target::Stderr, AnimatedProgress::new("Upscale"), || {})?;
//! live.handle().update(|line| line.fraction = 0.5);
//! live.finish()?;
//! # Ok(())
//! # }
//! ```

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod animated_progress;
mod eta;
mod gradient_bar;
mod live;
mod live_line;
mod progress_line;
mod spring;
mod stream_backend;

pub use animated_progress::AnimatedProgress;
pub use eta::format_eta;
pub use gradient_bar::GradientBar;
pub use live::{Live, LiveHandle, Target};
pub use live_line::LiveLine;
pub use progress_line::ProgressLine;
pub use spring::Spring;

use ratatui::style::{Color, Style};

/// How many frames the display draws per second, at most. [`Spring`] steps its animation by one such frame.
const FPS: u64 = 60;

/// A style with `foreground` when `color`, and plain otherwise.
fn fg(foreground: impl Into<Color>, color: bool) -> Style {
    if color {
        Style::new().fg(foreground.into())
    } else {
        Style::new()
    }
}

#[cfg(test)]
mod tests;
