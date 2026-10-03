//! A single-line progress indicator on stderr (only when it is a terminal).

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::util::format_bytes;

const REDRAW_EVERY: Duration = Duration::from_millis(100);
// Keeps the whole line within 80 columns, even with an ETA like "  ~1h 20m left".
const BAR_WIDTH: usize = 10;

pub struct Progress {
    label: &'static str,
    total: u64,
    done: u64,
    started: Instant,
    last_draw: Option<Instant>,
    enabled: bool,
    finished: bool,
}

impl Progress {
    pub fn new(label: &'static str, total: u64) -> Self {
        Progress {
            label,
            total,
            done: 0,
            started: Instant::now(),
            last_draw: None,
            enabled: std::io::stderr().is_terminal(),
            finished: false,
        }
    }

    pub fn add(&mut self, bytes: u64) {
        self.done += bytes;
        if self.enabled && self.last_draw.is_none_or(|t| t.elapsed() >= REDRAW_EVERY) {
            self.draw();
            self.last_draw = Some(Instant::now());
        }
    }

    pub fn finish(&mut self) {
        if self.enabled {
            self.finished = true;
            self.draw();
            eprintln!();
        }
    }

    fn draw(&self) {
        let fraction = if self.total == 0 {
            1.0
        } else {
            (self.done as f64 / self.total as f64).min(1.0)
        };
        let filled = (fraction * BAR_WIDTH as f64) as usize;
        let rate = self.done as f64 / self.started.elapsed().as_secs_f64().max(0.001);
        let eta = if !self.finished && self.done > 0 && self.done < self.total && rate > 0.0 {
            let remaining_bytes = self.total - self.done;
            let remaining_secs = remaining_bytes as f64 / rate;
            let duration = Duration::from_secs(remaining_secs.round() as u64);
            format!("  ~{} left", crate::util::format_duration(duration))
        } else {
            String::new()
        };
        eprint!(
            // \x1b[K clears the rest of the line instead of padding with spaces.
            "\r  {} [{}{}] {:5.1}%  {} / {}  {}/s{}\x1b[K",
            self.label,
            "=".repeat(filled),
            " ".repeat(BAR_WIDTH - filled),
            fraction * 100.0,
            format_bytes(self.done),
            format_bytes(self.total),
            format_bytes(rate as u64),
            eta,
        );
        let _ = std::io::stderr().flush();
    }
}
