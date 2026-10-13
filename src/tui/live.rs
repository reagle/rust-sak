use std::fmt;
use std::io::{self, ErrorKind, IsTerminal, Stderr, Stdout, Write};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ratatui::backend::Backend;
use ratatui::crossterm::cursor::{MoveTo, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::{execute, terminal};
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};

use super::stream_backend::StreamBackend;
use super::{FPS, LiveLine};

const FRAME: Duration = Duration::from_nanos(1_000_000_000 / FPS);

/// How long [`Live::finish`] lets the line settle unless told otherwise.
const DEFAULT_SETTLE_CAP: Duration = Duration::from_secs(1);

/// The stream a [`Live`] session draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Standard output.
    Stdout,
    /// Standard error, for a program whose standard output is kept for its results.
    Stderr,
}

impl Target {
    fn is_terminal(self) -> bool {
        match self {
            Self::Stdout => io::stdout().is_terminal(),
            Self::Stderr => io::stderr().is_terminal(),
        }
    }

    fn stream(self) -> Stream {
        match self {
            Self::Stdout => Stream::Stdout(io::stdout()),
            Self::Stderr => Stream::Stderr(io::stderr()),
        }
    }
}

enum Stream {
    Stdout(Stdout),
    Stderr(Stderr),
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(out) => out.write(buf),
            Self::Stderr(err) => err.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(out) => out.flush(),
            Self::Stderr(err) => err.flush(),
        }
    }
}

/// A [`LiveLine`] drawn on [`LiveLine::height`] terminal rows, in raw mode, up to 60 times a second, by a thread of
/// its own.
///
/// The rows are an inline viewport: they stay in the normal scrollback rather than taking over the screen, and after
/// [`finish`](Self::finish) the last frame is left there with the cursor on the line below it. The height is read
/// once, at [`start`](Self::start); in a shorter terminal the line is given, and clipped to, the rows there are.
///
/// - Producers change the line through a [`LiveHandle`], from any thread or async task.
/// - Raw mode turns Ctrl-C into a key press instead of a signal, on every platform. Each press calls the
///   `on_interrupt` given to [`start`](Self::start), and drawing carries on; stopping the work is up to the caller.
/// - **Nothing else may write to the target stream while the session is up.** It would land on top of the line, and
///   in raw mode a newline does not return the carriage.
/// - The terminal is restored however the session ends: [`finish`](Self::finish), drop, an error, or a panic that
///   unwinds. Under `panic = "abort"` nothing runs, so the terminal is left in raw mode.
///
/// Whether to draw at all (`is_terminal`, `NO_COLOR`) is the caller's decision: starting on a stream that is not a
/// terminal is an error.
pub struct Live<L: LiveLine> {
    line: Arc<Mutex<L>>,
    finish: Sender<Stop>,
    settle_cap: Duration,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl<L: LiveLine> Live<L> {
    /// Starts drawing `line` on `target`, from the row the cursor is on. When fewer than [`LiveLine::height`] rows
    /// are left below it, the terminal scrolls up to make room.
    ///
    /// # Errors
    ///
    /// Fails when `target` is not a terminal, when raw mode cannot be turned on, or when the terminal does not
    /// report where its cursor is. Raw mode is off again when this fails.
    pub fn start(target: Target, line: L, on_interrupt: impl Fn() + Send + 'static) -> io::Result<Self> {
        if !target.is_terminal() {
            let name = match target {
                Target::Stdout => "stdout",
                Target::Stderr => "stderr",
            };
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                format!("{name} is not a terminal"),
            ));
        }

        let session = Session::start(target, line.height())?;
        let line = Arc::new(Mutex::new(line));
        let (finish, finished) = mpsc::channel();

        let shared = Arc::clone(&line);
        let thread = std::thread::Builder::new()
            .name("rust-sak-tui".to_owned())
            .spawn(move || run(session, &shared, &finished, &on_interrupt))?;

        Ok(Self {
            line,
            finish,
            settle_cap: DEFAULT_SETTLE_CAP,
            thread: Some(thread),
        })
    }

    /// Sets how long [`finish`](Self::finish) waits for the line to settle before drawing its last frame. It is one
    /// second unless set.
    #[must_use]
    pub fn settle_cap(mut self, cap: Duration) -> Self {
        self.settle_cap = cap;
        self
    }

    /// A handle for changing the line from elsewhere.
    pub fn handle(&self) -> LiveHandle<L> {
        LiveHandle {
            line: Arc::clone(&self.line),
        }
    }

    /// Lets the line settle (up to the [settle cap](Self::settle_cap)), draws its last frame, and restores the
    /// terminal, with the cursor on the line below.
    ///
    /// # Errors
    ///
    /// Returns the first error writing to the terminal, if drawing failed at any point.
    ///
    /// # Panics
    ///
    /// Resumes the panic if [`LiveLine::render`] panicked; the terminal has been restored by then.
    pub fn finish(mut self) -> io::Result<()> {
        self.stop(self.settle_cap)
    }

    fn stop(&mut self, cap: Duration) -> io::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // A failed send means the frame thread has already ended, on an error that `join` returns.
        let _ = self.finish.send(Stop::Settle(cap));
        thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

impl<L: LiveLine> Drop for Live<L> {
    /// Ends the session at once, with no time to settle. Errors are dropped, and a panic in the frame thread is not
    /// resumed, since this may already be running during one.
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // The panic hook has already printed its message, past the line; a last frame would put the cursor back
            // on the line, and the shell's prompt over the message.
            let stop = if std::thread::panicking() {
                Stop::Abandon
            } else {
                Stop::Settle(Duration::ZERO)
            };
            let _ = self.finish.send(stop);
            let _ = thread.join();
        }
    }
}

/// How the frame thread is told to end.
enum Stop {
    /// Keep drawing until the line settles or the time runs out, then draw the last frame.
    Settle(Duration),
    /// End without drawing again.
    Abandon,
}

impl<L: LiveLine> fmt::Debug for Live<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Live")
            .field("settle_cap", &self.settle_cap)
            .finish_non_exhaustive()
    }
}

/// Changes the line of a [`Live`] session. It is cheap to clone, and can be sent to and shared between threads.
pub struct LiveHandle<L> {
    line: Arc<Mutex<L>>,
}

impl<L> LiveHandle<L> {
    /// Changes the line with `change`, which the next frame draws. The line is locked while `change` runs, which
    /// holds up drawing, so it should only set fields.
    pub fn update<R>(&self, change: impl FnOnce(&mut L) -> R) -> R {
        change(&mut lock(&self.line))
    }
}

impl<L> Clone for LiveHandle<L> {
    fn clone(&self) -> Self {
        Self {
            line: Arc::clone(&self.line),
        }
    }
}

impl<L> fmt::Debug for LiveHandle<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveHandle").finish_non_exhaustive()
    }
}

/// A panic while the line was locked leaves it as it was; drawing it is still better than stopping.
fn lock<L>(line: &Mutex<L>) -> MutexGuard<'_, L> {
    line.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The frame loop: draws `line` every frame and reports Ctrl-C, until `finish` delivers a settle cap (or is dropped)
/// and the line settles or runs out of it, or until `finish` says to abandon it.
fn run<L: LiveLine>(
    mut session: Session,
    line: &Mutex<L>,
    finish: &Receiver<Stop>,
    on_interrupt: &dyn Fn(),
) -> io::Result<()> {
    let mut finishing: Option<(Instant, Duration)> = None;

    loop {
        let frame_started = Instant::now();

        if finishing.is_none() {
            match finish.try_recv() {
                Ok(Stop::Settle(cap)) => finishing = Some((frame_started, cap)),
                Ok(Stop::Abandon) => return Ok(()),
                Err(TryRecvError::Disconnected) => finishing = Some((frame_started, Duration::ZERO)),
                Err(TryRecvError::Empty) => {}
            }
        }

        let mut done = false;
        session.terminal.draw(|frame| {
            let mut line = lock(line);
            done = finishing.is_some_and(|(at, cap)| line.settled() || at.elapsed() >= cap);
            if done {
                line.snap();
            }
            line.render(frame.area(), frame.buffer_mut());
        })?;

        if done {
            return Ok(());
        }

        if event::poll(FRAME.saturating_sub(frame_started.elapsed()))? && is_ctrl_c(&event::read()?) {
            on_interrupt();
        }
    }
}

/// Raw mode turns Ctrl-C into a key press instead of SIGINT, on every platform.
pub(super) fn is_ctrl_c(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
            if modifiers.contains(KeyModifiers::CONTROL)
    )
}

/// Raw mode, turned off again when dropped.
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// The terminal in raw mode with an inline viewport. Dropping it restores the terminal and leaves the cursor on the
/// line below the viewport.
struct Session {
    terminal: Terminal<StreamBackend<Stream>>,
    _raw_mode: RawMode,
}

impl Session {
    fn start(target: Target, height: u16) -> io::Result<Self> {
        // Raw mode first: the inline viewport asks the terminal where the cursor is, and the reply must not be echoed.
        let raw_mode = RawMode::enable()?;
        let backend = StreamBackend::new(target.stream());
        let rows = backend.size()?.height;
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: viewport(height, rows),
            },
        )?;
        Ok(Self {
            terminal,
            _raw_mode: raw_mode,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Below the viewport, or below whatever was written past it: a panic's message is printed by the panic hook
        // before unwinding gets here, still in raw mode. Asked while raw mode is on, so the reply is not echoed.
        let area = self.terminal.get_frame().area();
        let out = self.terminal.backend_mut();
        let row = restore_row(area, out.get_cursor_position().ok().map(|cursor| cursor.y));

        let _ = terminal::disable_raw_mode();
        let _ = execute!(out, MoveTo(0, row), Show);
        let _ = writeln!(out);
        let _ = Write::flush(out);
    }
}

/// The inline viewport for a line `height` rows tall, in a terminal `rows` rows tall. ratatui needs at least one row
/// to anchor the cursor. It clamps the viewport to the terminal itself, but scrolls by the height it was asked for,
/// which would push blank lines into the scrollback; clamping here first scrolls only by the rows there are.
pub(super) fn viewport(height: u16, rows: u16) -> Viewport {
    Viewport::Inline(height.min(rows).max(1))
}

/// The row the cursor goes to before the restoring newline: the viewport's bottom row, or the cursor's own row when
/// something was written further down.
pub(super) fn restore_row(viewport: Rect, cursor: Option<u16>) -> u16 {
    let bottom = viewport.bottom().saturating_sub(1).max(viewport.y);
    cursor.map_or(bottom, |row| row.max(bottom))
}
