use std::time::Duration;

/// Formats an estimate of the time left the way Go prints a duration: `--` when there is none, tenths of a second
/// below 10 s (`4.2s`), whole seconds above that (`12s`, `1m5s`, `1h2m3s`).
///
/// ```
/// use std::time::Duration;
/// use rust_sak::tui::format_eta;
///
/// assert_eq!(format_eta(None), "--");
/// assert_eq!(format_eta(Some(Duration::from_millis(4250))), "4.2s");
/// assert_eq!(format_eta(Some(Duration::from_secs(65))), "1m5s");
/// ```
pub fn format_eta(eta: Option<Duration>) -> String {
    let Some(eta) = eta else { return "--".to_owned() };

    if eta < Duration::from_secs(10) {
        let tenths = eta.as_millis() / 100;
        return match tenths % 10 {
            0 => format!("{}s", tenths / 10),
            fraction => format!("{}.{fraction}s", tenths / 10),
        };
    }

    let secs = eta.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m{seconds}s"),
        _ => format!("{hours}h{minutes}m{seconds}s"),
    }
}
