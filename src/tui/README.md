# `tui` module

A one-line terminal progress display, drawn with [`ratatui`](https://ratatui.rs) in an inline viewport so it stays in the normal scrollback instead of taking over the screen:

```text
Upscale         [1/2]  █████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░   50.0%   ETA 4.2s
```

The bar's filled cells run through a `#5a56e0` → `#ee6ff8` gradient and ease towards their target with a critically damped spring. A **live session** draws the line on stdout or stderr up to 60 times a second, on a thread of its own, while producers on any thread or async task update it. It is **synchronous**: no async runtime is needed or pulled in.

## Enabling

The module is gated behind the `tui` Cargo feature:

```toml
[dependencies]
rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["tui"] }
```

```rust
use rust_sak::tui::{AnimatedProgress, Live, LiveHandle, LiveLine, Target};
```

## Public API

### Widgets

| Item          | Signature                                                                                               | What it does                                                                                                                       |
|---------------|---------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------|
| `ProgressLine` | `struct ProgressLine<'a> { label, label_width, counter, fraction, position, eta, color }`, a `Widget`  | The whole line. The bar fits the row, between 10 and 50 cells; on a narrower row the end of the line is cut off.                   |
| `GradientBar` | `struct GradientBar { position, color }`, a `Widget`                                                    | The bar alone: `position` (0–1) of it filled with `█`, the rest `░`.                                                              |
| `Spring`      | `struct Spring { position, target, .. }` with `step()`, `is_settled()`, `snap()`                        | The animation: one `step()` per frame moves `position` towards `target`, from rest across `0`–`1` in under a second, without overshoot. |
| `format_eta`  | `fn format_eta(eta: Option<Duration>) -> String`                                                        | `--`, then `4.2s` below 10 s, then `12s`, `1m5s`, `1h2m3s`.                                                                        |

`ProgressLine`'s fields:

- **`label`**, padded with spaces to **`label_width`** columns (`0`: its own width). A width that fits the longest label keeps the bar in place while the label changes.
- **`counter`**: `Some((n, total))` draws `[n/total]`, with `n` zero-padded to the width of `total`; `None` draws no brackets.
- **`fraction`** (0–1) is the percentage, **floored to tenths**: `100.0%` appears only at exactly 1, never at `0.9999`.
- **`position`** (0–1) is how full the bar is, usually `fraction` or a value animated towards it.
- **`eta`**: `None` leaves the ETA column out; `Some(None)` draws `ETA --`, for an estimate not known yet.
- **`color`**: without it, the line keeps the terminal's default colours.

### Live session

| Item               | Signature                                                                                                  | What it does                                                                                                                                  |
|--------------------|------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------|
| `Target`           | `enum Target { Stdout, Stderr }`                                                                           | The stream the session draws on.                                                                                                              |
| `LiveLine`         | `trait LiveLine: Send + 'static { fn render(&mut self, Rect, &mut Buffer); fn settled(&self) -> bool; fn snap(&mut self); }` | What a session draws. `settled` and `snap` default to "always settled" and nothing, for a line that does not animate.                         |
| `AnimatedProgress` | `struct AnimatedProgress { label, label_width, counter, fraction, eta, color, .. }`, a `LiveLine`          | A `ProgressLine` whose bar springs towards `fraction`, while the percentage shows `fraction` at once. Settled once the bar arrives.            |
| `Line<'static>`    | `impl LiveLine for ratatui::text::Line<'static>`                                                           | A plain line of text, drawn as it is.                                                                                                         |
| `Live`             | `Live::start(target, line, on_interrupt: impl Fn() + Send + 'static) -> io::Result<Live<L>>`               | Puts the terminal in raw mode and starts drawing `line` on the cursor's row.                                                                  |
|                    | `.settle_cap(Duration) -> Self`                                                                            | How long `finish` waits for the line to settle; one second unless set.                                                                         |
|                    | `.handle() -> LiveHandle<L>`                                                                               | A handle for producers.                                                                                                                       |
|                    | `.finish(self) -> io::Result<()>`                                                                          | Lets the line settle (up to the cap, then snaps it), draws the last frame and restores the terminal, leaving the cursor on the line below.     |
| `LiveHandle`       | `.update(&self, impl FnOnce(&mut L) -> R) -> R`; `Clone + Send + Sync`                                     | Changes the line; the next frame draws it. The line is locked while the closure runs, so it should only set fields.                          |

## Usage

Progress on **stderr**, so stdout stays free for the program's results, updated from a worker thread:

```rust,no_run
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use rust_sak::tui::{AnimatedProgress, Live, Target};

# fn run() -> std::io::Result<()> {
let cancelled = Arc::new(AtomicBool::new(false));
let on_ctrl_c = {
    let cancelled = Arc::clone(&cancelled);
    move || cancelled.store(true, Ordering::Relaxed)
};

let mut line = AnimatedProgress::new("Upscale");
line.label_width = "Face Recovery".len();
line.counter = Some((1, 2));
line.eta = Some(None);

let live = Live::start(Target::Stderr, line, on_ctrl_c)?;
let handle = live.handle();

let worker = std::thread::spawn(move || {
    for tick in 1..=100 {
        // ...a slice of the work...
        handle.update(|line| line.fraction = f64::from(tick) / 100.0);
    }
});
worker.join().expect("the worker panicked");

live.finish()?;
eprintln!("Done");
# Ok(())
# }
```

`cargo run --example tui --features tui` runs a version of this; `--stdout`, `--drop` and `--panic` show the other stream and the other ways a session ends.

## Rules for the caller

- **Nothing else may write to the target stream while a session is up.** It would land on top of the line, and in raw mode a newline does not return the carriage. Print headers before `Live::start` and results after `finish`.
- **Deciding whether to draw is the caller's job.** Check `is_terminal()` and `NO_COLOR` first and pass `color` accordingly; `Live::start` on a stream that is not a terminal returns an `io::ErrorKind::Unsupported` error rather than falling back to something else.
- **Ctrl-C is reported, not acted on.** Raw mode turns it into a key press on every platform, so it no longer sends SIGINT. Each press calls `on_interrupt` and drawing carries on: cancelling, finishing or exiting is the caller's decision, and the bar can stay up while the work winds down.

## Guarantees

- **The terminal is restored however the session ends**: `finish`, dropping the `Live`, an error while drawing, or a panic that unwinds. Raw mode is turned off, the cursor is shown, and it is left on the line below the last frame (or below whatever was printed past it).
- **A panic prints its message in raw mode.** The panic hook runs before unwinding reaches the session, and this module does not replace the application's hook, so the message is drawn without carriage returns. The shell's prompt still comes back on a clean line. Under `panic = "abort"` nothing runs at all, and the terminal is left in raw mode.
- **The cursor position is asked for on the target stream.** An inline viewport needs to know the cursor's row. crossterm asks for it on stdout on Unix, so this module asks on the target stream itself and reads the terminal's reply directly. A session on stderr then works with stdout redirected, and writes nothing to it.

## Platforms

The widgets are tested on ratatui's `TestBackend` on macOS, Windows and Linux. The live session (raw mode, the inline viewport, Ctrl-C as a key) has been checked by hand on macOS only.
