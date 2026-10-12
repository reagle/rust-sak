//! Draws an animated progress line on stderr while some pretend work runs.
//!
//! Run with: `cargo run --example tui --features tui`
//!
//! - `--stdout` draws on stdout instead.
//! - `--drop` drops the session halfway instead of finishing it.
//! - `--panic` panics halfway, with the session still up.
//!
//! Ctrl-C changes the label instead of stopping, to show that the session only reports it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use rust_sak::tui::{AnimatedProgress, Live, Target};

const STEPS: [&str; 3] = ["Upscale", "Face Recovery", "Denoise"];
const TICKS_PER_STEP: usize = 40;

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let target = if flag("--stdout") {
        Target::Stdout
    } else {
        Target::Stderr
    };

    let mut progress = AnimatedProgress::new(STEPS[0]);
    progress.label_width = STEPS.iter().map(|step| step.len()).max().unwrap_or(0);
    progress.counter = Some((1, STEPS.len()));
    progress.eta = Some(None);

    let interrupts = Arc::new(AtomicUsize::new(0));
    let pressed = Arc::clone(&interrupts);
    let live = Live::start(target, progress, move || {
        pressed.fetch_add(1, Ordering::Relaxed);
    })?;
    let handle = live.handle();

    let started = Instant::now();
    let total = STEPS.len() * TICKS_PER_STEP;
    for tick in 1..=total {
        sleep(Duration::from_millis(25));
        if tick == total / 2 {
            if flag("--drop") {
                drop(live);
                eprintln!("dropped early");
                return Ok(());
            }
            assert!(!flag("--panic"), "the producer panicked halfway");
        }

        let step = (tick - 1) / TICKS_PER_STEP;
        #[allow(clippy::cast_precision_loss)]
        let fraction = tick as f64 / total as f64;
        let left = started.elapsed().mul_f64((1.0 - fraction) / fraction);
        let interrupted = interrupts.load(Ordering::Relaxed);

        handle.update(|line| {
            line.label = if interrupted > 0 {
                format!("Ctrl-C x{interrupted}")
            } else {
                STEPS[step].to_owned()
            };
            line.counter = Some((step + 1, STEPS.len()));
            line.fraction = fraction;
            line.eta = Some(Some(left));
        });
    }

    live.finish()?;
    eprintln!("done, Ctrl-C pressed {} times", interrupts.load(Ordering::Relaxed));
    Ok(())
}
