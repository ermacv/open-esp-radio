//! Wall-clock durations of long task phases, printed as they end so a slow
//! run shows where its time went.
use std::time::{Duration, Instant};

/// Run `work` and print how long the phase `label` took, whether it
/// succeeded or not.
pub fn timed<T>(label: &str, work: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = work();
    eprintln!("{}", line(label, start.elapsed()));
    result
}

fn line(label: &str, elapsed: Duration) -> String {
    format!("phase {label}: {:.1} s", elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_line_names_the_phase_and_its_seconds() {
        assert_eq!(
            line("build radio probe", Duration::from_millis(61_250)),
            "phase build radio probe: 61.2 s"
        );
    }

    #[test]
    fn a_failed_phase_is_timed_and_returns_its_error() {
        let result: Result<(), &str> = timed("failing", || Err("failed"));
        assert_eq!(result, Err("failed"));
    }
}
