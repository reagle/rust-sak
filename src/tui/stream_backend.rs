use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// A [`CrosstermBackend`] over `W` that asks for the cursor position on `W` itself.
///
/// An inline viewport needs the cursor's row, and on Unix crossterm asks for it by writing `ESC [ 6 n` to stdout,
/// whatever stream the backend draws on. With the display on stderr and stdout redirected, the question never
/// reaches the terminal (and lands in the redirected file instead), so this asks on the drawing stream and reads the
/// reply from the terminal directly. On Windows crossterm reads the console's own cursor and needs no help.
pub(super) struct StreamBackend<W: Write> {
    inner: CrosstermBackend<W>,
}

impl<W: Write> StreamBackend<W> {
    pub(super) fn new(stream: W) -> Self {
        Self {
            inner: CrosstermBackend::new(stream),
        }
    }
}

impl<W: Write> Write for StreamBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl<W: Write> Backend for StreamBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    #[cfg(unix)]
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.write_all(b"\x1b[6n")?;
        Write::flush(&mut self.inner)?;
        unix::read_cursor_position()
    }

    #[cfg(not(unix))]
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{self, ErrorKind, Read};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use ratatui::layout::Position;

    /// How long the terminal gets to answer, as crossterm allows.
    const TIMEOUT: Duration = Duration::from_secs(2);

    /// Reads the terminal's reply to `ESC [ 6 n`, `ESC [ <row> ; <column> R`, from `/dev/tty`. Raw mode must be on,
    /// so the reply is not echoed and arrives without waiting for a newline. Anything typed before it is dropped.
    pub(super) fn read_cursor_position() -> io::Result<Position> {
        let mut tty = File::open("/dev/tty")?;
        let deadline = Instant::now() + TIMEOUT;
        let mut reply = Vec::new();
        let mut chunk = [0; 64];

        loop {
            if let Some(position) = parse_reply(&reply) {
                return Ok(position);
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !wait_readable(&tty, left)? {
                return Err(io::Error::new(
                    ErrorKind::TimedOut,
                    "the terminal did not report the cursor position",
                ));
            }
            match tty.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "the terminal closed")),
                Ok(n) => reply.extend_from_slice(&chunk[..n]),
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
    }

    /// Waits up to `timeout` for `tty` to have something to read, and tells whether it has.
    fn wait_readable(tty: &File, timeout: Duration) -> io::Result<bool> {
        let mut fd = libc::pollfd {
            fd: tty.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = libc::c_int::try_from(timeout.as_millis())
            .unwrap_or(libc::c_int::MAX)
            .max(1);

        loop {
            // SAFETY: `fd` is a single, valid `pollfd` that outlives the call.
            match unsafe { libc::poll(&raw mut fd, 1, millis) } {
                -1 => {
                    let err = io::Error::last_os_error();
                    if err.kind() != ErrorKind::Interrupted {
                        return Err(err);
                    }
                }
                ready => return Ok(ready > 0),
            }
        }
    }

    /// The zero-based position in the last complete `ESC [ <row> ; <column> R` of `bytes`, if there is one.
    pub(in super::super) fn parse_reply(bytes: &[u8]) -> Option<Position> {
        let end = bytes.iter().rposition(|&b| b == b'R')?;
        let start = bytes[..end].windows(2).rposition(|pair| pair == b"\x1b[")?;
        let text = std::str::from_utf8(&bytes[start + 2..end]).ok()?;
        let (row, column) = text.split_once(';')?;
        let (row, column) = (row.parse::<u16>().ok()?, column.parse::<u16>().ok()?);
        Some(Position::new(column.saturating_sub(1), row.saturating_sub(1)))
    }
}

#[cfg(all(test, unix))]
pub(super) use unix::parse_reply;
