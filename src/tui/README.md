# `tui` module

A terminal progress display, one line by default, drawn with [`ratatui`](https://ratatui.rs) in an inline viewport so it stays in the normal scrollback instead of taking over the screen:

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
| `Spinner`     | `struct Spinner { direction, style, color, .. }` with `new()`, `frame_at(Duration)`, `glyph()`, `span()`; `&Spinner` is a `Widget` | A one-cell braille spinner, `⣾ ⣷ ⣯ ⣟ ⡿ ⢿ ⣻ ⣽`, one frame every 100 ms. `style` defaults to `#ee6ff8` and applies only with `color`. |
| `Direction`   | `enum Direction { Clockwise, CounterClockwise }`, `Clockwise` by default                                | Which way a `Spinner` turns. `CounterClockwise` is `⣾ ⣽ ⣻ ⢿ ⡿ ⣟ ⣯ ⣷`, the order of bubbles' `spinner.Dot`.                     |

`ProgressLine`'s fields:

- **`label`**, padded with spaces to **`label_width`** columns (`0`: its own width). A width that fits the longest label keeps the bar in place while the label changes.
- **`counter`**: `Some((n, total))` draws `[n/total]`, with `n` zero-padded to the width of `total`; `None` draws no brackets.
- **`fraction`** (0–1) is the percentage, **floored to tenths**: `100.0%` appears only at exactly 1, never at `0.9999`.
- **`position`** (0–1) is how full the bar is, usually `fraction` or a value animated towards it.
- **`eta`**: `None` leaves the ETA column out; `Some(None)` draws `ETA --`, for an estimate not known yet.
- **`color`**: without it, the line keeps the terminal's default colours.

`Spinner`'s frame comes from the time since `new()`, **10 frames a second whatever the draw rate**: a 60 fps live session and a caller that draws twice a second see it turn at the same speed, the slower one skipping frames rather than slowing down. A session redraws every frame, so a spinner inside one animates without any `update`. `frame_at` is the pure part, the frame for a given elapsed time, for tests.

A spinner usually sits inside a line of text, so `span()` is the way to draw it: one span in a `Line` of the caller's own.

```rust
use ratatui::text::Line;
use rust_sak::tui::{Direction, Spinner};

let mut spinner = Spinner::new();
spinner.direction = Direction::CounterClockwise;

// Built on every draw, so the glyph is the current one.
let line = Line::from(vec!["Downloading… ".into(), spinner.span(), " 34% Complete".into()]);
```

### Live session

| Item               | Signature                                                                                                  | What it does                                                                                                                                  |
|--------------------|------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------|
| `Target`           | `enum Target { Stdout, Stderr }`                                                                           | The stream the session draws on.                                                                                                              |
| `LiveLine`         | `trait LiveLine: Send + 'static { fn render(&mut self, Rect, &mut Buffer); fn settled(&self) -> bool; fn snap(&mut self); fn height(&self) -> u16; }` | What a session draws. `settled` and `snap` default to "always settled" and nothing, for a line that does not animate; `height` defaults to one row. |
| `AnimatedProgress` | `struct AnimatedProgress { label, label_width, counter, fraction, eta, color, .. }`, a `LiveLine`          | A `ProgressLine` whose bar springs towards `fraction`, while the percentage shows `fraction` at once. Settled once the bar arrives.            |
| `Line<'static>`    | `impl LiveLine for ratatui::text::Line<'static>`                                                           | A plain line of text, drawn as it is.                                                                                                         |
| `Live`             | `Live::start(target, line, on_interrupt: impl Fn() + Send + 'static) -> io::Result<Live<L>>`               | Puts the terminal in raw mode and starts drawing `line` on the cursor's row.                                                                  |
|                    | `.settle_cap(Duration) -> Self`                                                                            | How long `finish` waits for the line to settle; one second unless set.                                                                         |
|                    | `.handle() -> LiveHandle<L>`                                                                               | A handle for producers.                                                                                                                       |
|                    | `.finish(self) -> io::Result<()>`                                                                          | Lets the line settle (up to the cap, then snaps it), draws the last frame and restores the terminal, leaving the cursor on the line below.     |
| `LiveHandle`       | `.update(&self, impl FnOnce(&mut L) -> R) -> R`; `Clone + Send + Sync`                                     | Changes the line; the next frame draws it. The line is locked while the closure runs, so it should only set fields.                          |

### More than one row

A `LiveLine` that returns more than `1` from `height` gets that many rows: a bar over a status list, say, split with ratatui's `Layout` inside `render`.

- **The height is fixed for the session's life.** `Live::start` reads it once; changing what `height` returns afterwards does nothing. A line that will need up to five list rows asks for them from the start, and the rows it has nothing for yet are drawn blank, as empty lines under the drawing.
- **`render` is given the rows there are.** In a terminal shorter than the height, the `Rect` is clamped to the terminal's height, so the line must clip rather than assume its full height. `Layout` does this.
- **Starting near the bottom of the window scrolls the terminal up** to make room, as any inline viewport does. Nothing in the scrollback is overwritten.
- `0` is treated as `1`: the viewport needs a row to anchor to.


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

`cargo run --example tui --features tui` runs a version of this; `--stdout`, `--drop` and `--panic` show the other stream and the other ways a session ends. `cargo run --example tui_rows --features tui` draws a bar over a five-row list that scrolls to the most recent item, and takes `--drop` and `--panic` too. `cargo run --example tui_spinner --features tui` turns a spinner clockwise, then counter-clockwise, inside a line of text.

## Rules for the caller

- **Nothing else may write to the target stream while a session is up.** It would land on top of the line, and in raw mode a newline does not return the carriage. Print headers before `Live::start` and results after `finish`.
- **Deciding whether to draw is the caller's job.** Check `is_terminal()` and `NO_COLOR` first and pass `color` accordingly; `Live::start` on a stream that is not a terminal returns an `io::ErrorKind::Unsupported` error rather than falling back to something else.
- **Ctrl-C is reported, not acted on.** Raw mode turns it into a key press on every platform, so it no longer sends SIGINT. Each press calls `on_interrupt` and drawing carries on: cancelling, finishing or exiting is the caller's decision, and the bar can stay up while the work winds down.

## Guarantees

- **The terminal is restored however the session ends**: `finish`, dropping the `Live`, an error while drawing, or a panic that unwinds. Raw mode is turned off, the cursor is shown, and it is left on the line below the last frame's bottom row (or below whatever was printed past it).
- **A panic prints its message in raw mode.** The panic hook runs before unwinding reaches the session, and this module does not replace the application's hook, so the message is drawn without carriage returns. The shell's prompt still comes back on a clean line. Under `panic = "abort"` nothing runs at all, and the terminal is left in raw mode.
- **The cursor position is asked for on the target stream.** An inline viewport needs to know the cursor's row. crossterm asks for it on stdout on Unix, so this module asks on the target stream itself and reads the terminal's reply directly. A session on stderr then works with stdout redirected, and writes nothing to it.

## Platforms

The widgets are tested on ratatui's `TestBackend` on macOS, Windows and Linux. The live session (raw mode, the inline viewport, Ctrl-C as a key) has been checked by hand on macOS only.
