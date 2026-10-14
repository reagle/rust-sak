use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

use super::gradient_bar::{EMPTY, EMPTY_SYMBOL, FILLED_SYMBOL};
use super::*;

// --- Spring ---
//
// Ported from MediaSim's `crates/cli/src/progress/spring.rs`.

fn spring_to(target: f64) -> Spring {
    let mut spring = Spring::default();
    spring.target = target;
    spring
}

#[test]
fn spring_reaches_the_target() {
    let mut spring = spring_to(0.5);

    for _ in 0..600 {
        spring.step();
    }

    assert!(spring.is_settled());
    assert!((spring.position - 0.5).abs() < f64::EPSILON);
}

#[test]
fn spring_never_overshoots() {
    let mut spring = spring_to(1.0);

    for _ in 0..600 {
        spring.step();
        assert!(spring.position <= 1.0, "overshot to {}", spring.position);
    }
}

#[test]
fn spring_settles_from_zero_to_one_within_a_second() {
    let mut spring = spring_to(1.0);

    let steps = (1..=60).find(|_| {
        spring.step();
        spring.is_settled()
    });

    assert!(steps.is_some(), "not settled after 60 steps, at {}", spring.position);
}

#[test]
fn a_settled_spring_stays_put() {
    let mut spring = spring_to(0.0);

    spring.step();

    assert!(spring.is_settled());
    assert!(spring.position.abs() < f64::EPSILON);
}

#[test]
#[allow(clippy::float_cmp, reason = "snapping copies the target, so equality is exact")]
fn snapping_lands_exactly_on_the_target() {
    let mut spring = spring_to(0.3);
    spring.step();
    assert!(!spring.is_settled());

    spring.snap();

    assert!(spring.is_settled());
    assert_eq!(spring.position, 0.3);
}

// --- GradientBar ---
//
// Ported from MediaSim's `crates/cli/src/progress/bar.rs`.

const BAR_WIDTH: u16 = 20;

fn render_bar(position: f64, color: bool) -> Buffer {
    let area = Rect::new(0, 0, BAR_WIDTH, 1);
    let mut buf = Buffer::empty(area);
    GradientBar { position, color }.render(area, &mut buf);
    buf
}

fn count(buf: &Buffer, symbol: &str) -> usize {
    buf.content().iter().filter(|cell| cell.symbol() == symbol).count()
}

#[test]
fn bar_has_filled_and_empty_cells() {
    for (position, filled) in [(0.0, 0), (0.5, 10), (1.0, 20)] {
        let buf = render_bar(position, true);

        assert_eq!(count(&buf, FILLED_SYMBOL), filled, "at {position}");
        assert_eq!(
            count(&buf, EMPTY_SYMBOL),
            usize::from(BAR_WIDTH) - filled,
            "at {position}"
        );
    }
}

#[test]
fn gradient_runs_from_start_to_end() {
    let buf = render_bar(1.0, true);

    assert_eq!(buf[(0, 0)].fg, Color::Rgb(0x5a, 0x56, 0xe0));
    assert_eq!(buf[(BAR_WIDTH - 1, 0)].fg, Color::Rgb(0xee, 0x6f, 0xf8));
}

#[test]
fn empty_cells_are_gray() {
    let buf = render_bar(0.5, true);

    assert_eq!(buf[(BAR_WIDTH - 1, 0)].fg, EMPTY);
}

#[test]
fn without_color_cells_keep_the_default() {
    let buf = render_bar(0.5, false);

    assert!(buf.content().iter().all(|cell| cell.fg == Color::Reset));
}

// --- format_eta ---
//
// Ported from MediaSim's `crates/cli/src/progress/eta.rs`.

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

#[test]
fn eta_text_format() {
    assert_eq!(format_eta(None), "--");
    assert_eq!(format_eta(Some(Duration::ZERO)), "0s");
    assert_eq!(format_eta(Some(secs(4.25))), "4.2s");
    assert_eq!(format_eta(Some(secs(4.0))), "4s");
    assert_eq!(format_eta(Some(secs(12.7))), "12s");
    assert_eq!(format_eta(Some(secs(65.0))), "1m5s");
    assert_eq!(format_eta(Some(secs(3723.0))), "1h2m3s");
}

// --- ProgressLine ---
//
// The first six are MediaSim's `crates/cli/src/progress/mod.rs` layout tests, with its count-derived percentage given
// as `fraction` and its always-drawn ETA as `Some(eta)`; their expectations are unchanged.

fn draw(width: u16, line: ProgressLine<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
    terminal.draw(|frame| frame.render_widget(line, frame.area())).unwrap();
    terminal.backend().buffer().content().iter().map(Cell::symbol).collect()
}

/// MediaSim's `ProgressLine { label, completed, total, position, eta, color }`.
fn mediasim<'a>(
    label: &'a str,
    completed: usize,
    total: usize,
    position: f64,
    eta: Option<Duration>,
    color: bool,
) -> ProgressLine<'a> {
    #[allow(clippy::cast_precision_loss)]
    let fraction = completed as f64 / total as f64;
    ProgressLine {
        label,
        counter: Some((completed, total)),
        fraction,
        position,
        eta: Some(eta),
        color,
        label_width: 0,
    }
}

fn half_loaded() -> ProgressLine<'static> {
    mediasim("Loading", 1, 2, 0.5, Some(Duration::from_millis(4200)), true)
}

fn bar_cells(text: &str) -> usize {
    text.chars().filter(|c| matches!(c, '█' | '░')).count()
}

#[test]
fn wide_terminal_gets_the_full_bar() {
    let text = draw(120, half_loaded());

    assert_eq!(bar_cells(&text), 50);
    assert!(text.starts_with("Loading   [1/2]  █"), "{text}");
    assert!(text.trim_end().ends_with("░   50.0%   ETA 4.2s"), "{text}");
}

#[test]
fn narrow_terminal_gets_the_minimum_bar() {
    let text = draw(40, half_loaded());

    assert_eq!(bar_cells(&text), 10);
    assert!(text.starts_with("Loading   [1/2]  █████░░░░░  "), "{text}");
}

#[test]
fn bar_shrinks_to_fit_the_terminal() {
    // The fixed text takes 36 columns here, leaving 24 for the bar.
    let text = draw(60, half_loaded());

    assert_eq!(bar_cells(&text), 24);
    assert!(text.ends_with("ETA 4.2s"), "{text}");
}

#[test]
fn nothing_loaded_shows_no_estimate() {
    let line = mediasim("Loading", 0, 2, 0.0, None, false);

    let text = draw(120, line);

    assert!(text.contains("[0/2]"), "{text}");
    assert!(text.contains("  0.0%   ETA --"), "{text}");
}

#[test]
fn count_is_padded_to_the_width_of_the_total() {
    let line = mediasim("Loading", 3, 12, 0.25, None, false);

    assert!(draw(120, line).contains("[03/12]"));
}

#[test]
fn label_is_shown() {
    let line = ProgressLine {
        label: "Processing",
        ..half_loaded()
    };

    let text = draw(120, line);

    assert!(text.starts_with("Processing   [1/2]  █"), "{text}");
    assert!(text.trim_end().ends_with("░   50.0%   ETA 4.2s"), "{text}");
}

#[test]
fn a_padded_label_keeps_the_bar_in_place() {
    let first_bar_column = |label| {
        let text = draw(
            120,
            ProgressLine {
                label,
                label_width: 13,
                ..half_loaded()
            },
        );
        text.chars().position(|c| c == '█').unwrap()
    };

    assert_eq!(first_bar_column("Upscale"), first_bar_column("Face Recovery"));
    assert_eq!(first_bar_column("Upscale"), "Face Recovery   [1/2]  ".chars().count());
}

#[test]
fn a_label_wider_than_its_width_is_not_cut() {
    let text = draw(
        120,
        ProgressLine {
            label: "Face Recovery",
            label_width: 4,
            ..half_loaded()
        },
    );

    assert!(text.starts_with("Face Recovery   [1/2]  █"), "{text}");
}

#[test]
fn no_counter_draws_no_brackets() {
    let text = draw(
        120,
        ProgressLine {
            counter: None,
            ..half_loaded()
        },
    );

    assert!(!text.contains('[') && !text.contains(']'), "{text}");
    assert!(text.starts_with("Loading   █"), "{text}");
}

#[test]
fn no_eta_column_ends_the_line_at_the_percent() {
    let text = draw(
        120,
        ProgressLine {
            eta: None,
            ..half_loaded()
        },
    );

    assert!(!text.contains("ETA"), "{text}");
    assert!(text.trim_end().ends_with("░   50.0%"), "{text}");
}

#[test]
fn the_percent_is_floored_to_tenths() {
    let percent = |fraction| {
        let text = draw(
            120,
            ProgressLine {
                fraction,
                ..half_loaded()
            },
        );
        text.split_whitespace()
            .find(|word| word.ends_with('%'))
            .unwrap()
            .to_owned()
    };

    assert_eq!(percent(0.9999), "99.9%");
    assert_eq!(percent(1.0), "100.0%");
    assert_eq!(percent(0.29), "29.0%");
    assert_eq!(percent(0.0), "0.0%");
    assert_eq!(percent(1.5), "100.0%");
}

#[test]
fn the_line_is_coloured_only_when_asked() {
    let coloured = |color| {
        let mut terminal = Terminal::new(TestBackend::new(120, 1)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(ProgressLine { color, ..half_loaded() }, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.fg != Color::Reset)
    };

    assert!(coloured(true));
    assert!(!coloured(false));
}

// --- AnimatedProgress ---

/// Renders `progress` once, as a frame of a 120-column session, and returns the number of filled bar cells.
fn frame(progress: &mut AnimatedProgress) -> usize {
    let area = Rect::new(0, 0, 120, 1);
    let mut buf = Buffer::empty(area);
    progress.render(area, &mut buf);
    count(&buf, FILLED_SYMBOL)
}

#[test]
fn animated_progress_moves_monotonically_to_a_new_target_and_settles() {
    let mut progress = AnimatedProgress::new("Upscale");
    progress.fraction = 0.25;
    for _ in 0..120 {
        frame(&mut progress);
    }
    assert!(progress.settled());

    progress.fraction = 0.75;
    assert!(
        !progress.settled(),
        "a new target unsettles the line before it is drawn"
    );

    let mut cells = vec![frame(&mut progress)];
    let mut previous = progress.position();
    let mut frames = 1;
    while !progress.settled() {
        cells.push(frame(&mut progress));
        assert!(
            progress.position() >= previous,
            "moved back from {previous} to {}",
            progress.position()
        );
        previous = progress.position();
        frames += 1;
        assert!(frames <= 60, "not settled after a second of frames");
    }

    assert!(cells.windows(2).all(|pair| pair[0] <= pair[1]), "{cells:?}");
    assert!(
        cells[0] > 13,
        "the first frame already moves past 0.25 of the bar: {cells:?}"
    );
    assert_eq!(*cells.last().unwrap(), 38, "settles on 0.75 of a 50-cell bar");
    assert!((progress.position() - 0.75).abs() < f64::EPSILON);
}

#[test]
fn animated_progress_shows_the_new_percent_at_once() {
    let mut progress = AnimatedProgress::new("Upscale");
    progress.fraction = 0.5;

    let area = Rect::new(0, 0, 120, 1);
    let mut buf = Buffer::empty(area);
    progress.render(area, &mut buf);
    let text: String = buf.content().iter().map(Cell::symbol).collect();

    assert!(text.contains(" 50.0%"), "{text}");
    assert!(progress.position() < 0.5);
}

#[test]
fn snapping_settles_the_animation() {
    let mut progress = AnimatedProgress::new("Upscale");
    progress.fraction = 1.0;
    frame(&mut progress);
    assert!(!progress.settled());

    progress.snap();

    assert!(progress.settled());
    assert_eq!(frame(&mut progress), 50);
}

#[test]
fn a_plain_line_draws_its_text_and_is_always_settled() {
    let mut line = ratatui::text::Line::from("💾 Downloading");
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);

    LiveLine::render(&mut line, area, &mut buf);

    assert!(LiveLine::settled(&line));
    assert_eq!(buf[(3, 0)].symbol(), "D");
}

// --- Spinner ---

const CLOCKWISE: [&str; 8] = ["⣾", "⣷", "⣯", "⣟", "⡿", "⢿", "⣻", "⣽"];

fn spinner_turning(direction: Direction) -> Spinner {
    let mut spinner = Spinner::new();
    spinner.direction = direction;
    spinner
}

fn spinner_frames(spinner: &Spinner) -> Vec<&'static str> {
    (0..8)
        .map(|i| spinner.frame_at(Duration::from_millis(i * 100)))
        .collect()
}

#[test]
fn spinner_turns_clockwise_by_default() {
    assert_eq!(spinner_frames(&Spinner::new()), CLOCKWISE);
}

#[test]
fn spinner_counter_clockwise_matches_bubbles_dot() {
    assert_eq!(
        spinner_frames(&spinner_turning(Direction::CounterClockwise)),
        ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"]
    );
}

#[test]
fn spinner_holds_each_frame_for_100_ms_and_wraps() {
    for direction in [Direction::Clockwise, Direction::CounterClockwise] {
        let spinner = spinner_turning(direction);
        let at = |ms| spinner.frame_at(Duration::from_millis(ms));

        assert_eq!(at(799), at(700), "{direction:?}");
        assert_eq!(at(800), "⣾", "{direction:?}");
        assert_eq!(at(850), at(50), "{direction:?}");
    }
}

#[test]
fn spinner_span_is_styled_only_with_color() {
    let mut spinner = Spinner::new();
    spinner.style = Style::new().fg(Color::Rgb(0xc7, 0x92, 0xe9));

    assert_eq!(spinner.span().style, spinner.style);

    spinner.color = false;
    assert_eq!(spinner.span().style, Style::new());
}

#[test]
fn spinner_default_style_is_the_gradient_end() {
    assert_eq!(Spinner::new().style.fg, Some(Color::Rgb(0xee, 0x6f, 0xf8)));
}

#[test]
fn spinner_widget_draws_its_glyph() {
    let spinner = Spinner::new();
    let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();

    terminal
        .draw(|frame| frame.render_widget(&spinner, frame.area()))
        .unwrap();

    let symbol = terminal.backend().buffer()[(0, 0)].symbol().to_owned();
    assert!(CLOCKWISE.contains(&symbol.as_str()), "drew {symbol:?}");
}

// --- Live ---

#[test]
fn ctrl_c_is_recognised() {
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    let plain_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    let released = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        KeyEventKind::Release,
    ));

    assert!(live::is_ctrl_c(&ctrl_c));
    assert!(!live::is_ctrl_c(&plain_c));
    assert!(
        !live::is_ctrl_c(&released),
        "Windows reports the release too, which is not a second press"
    );
}

#[cfg(unix)]
#[test]
fn the_cursor_reply_is_parsed_zero_based() {
    use ratatui::layout::Position;

    use super::stream_backend::parse_reply;

    assert_eq!(parse_reply(b"\x1b[12;1R"), Some(Position::new(0, 11)));
    assert_eq!(
        parse_reply(b"typed\x1b[3;40R"),
        Some(Position::new(39, 2)),
        "keys typed before are skipped"
    );
    assert_eq!(parse_reply(b"\x1b[12;"), None, "an incomplete reply waits for more");
    assert_eq!(parse_reply(b""), None);
}

#[test]
fn existing_lines_are_one_row_tall() {
    assert_eq!(LiveLine::height(&AnimatedProgress::new("Upscale")), 1);
    assert_eq!(LiveLine::height(&ratatui::text::Line::from("💾 Downloading")), 1);
}

/// A line of a fixed height that records the area of every frame it is given.
struct Rows {
    height: u16,
    areas: Vec<Rect>,
}

impl LiveLine for Rows {
    fn render(&mut self, area: Rect, _buf: &mut Buffer) {
        self.areas.push(area);
    }

    fn height(&self) -> u16 {
        self.height
    }
}

/// Draws one frame of `line` on a `TestBackend` of `rows` rows, in the viewport a session would use, and returns
/// the area the line was given.
fn session_area(height: u16, rows: u16) -> Rect {
    let mut line = Rows {
        height,
        areas: Vec::new(),
    };
    let mut terminal = Terminal::with_options(
        TestBackend::new(40, rows),
        ratatui::TerminalOptions {
            viewport: live::viewport(line.height(), rows),
        },
    )
    .unwrap();
    terminal
        .draw(|frame| line.render(frame.area(), frame.buffer_mut()))
        .unwrap();
    line.areas[0]
}

#[test]
fn a_taller_line_gets_that_many_rows() {
    assert_eq!(session_area(3, 10).height, 3);
}

#[test]
fn a_zero_height_line_gets_one_row() {
    assert_eq!(session_area(0, 10).height, 1);
}

#[test]
fn a_line_taller_than_the_terminal_gets_the_terminal() {
    assert_eq!(session_area(7, 4).height, 4);
}

#[test]
fn the_terminal_is_restored_below_the_bottom_row() {
    assert_eq!(live::restore_row(Rect::new(0, 5, 40, 1), None), 5, "one row, as before");
    assert_eq!(live::restore_row(Rect::new(0, 5, 40, 7), None), 11);
    assert_eq!(live::restore_row(Rect::new(0, 5, 40, 7), Some(5)), 11);
    assert_eq!(
        live::restore_row(Rect::new(0, 5, 40, 7), Some(14)),
        14,
        "below a panic message printed past the viewport"
    );
}
