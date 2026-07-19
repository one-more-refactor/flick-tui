use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::TimelineWord;

const RAMP_MS: f64 = 3000.0;
const RAMP_START: f64 = 0.6;
const RAMP_FLOOR: f64 = 100.0;
pub const WPM_MIN: u32 = 100;
pub const WPM_MAX: u32 = 1200;
pub const WPM_STEP: u32 = 25;
const MAX_DELTA_MS: f64 = 250.0;
/// Position/read reports go out this often while playing (web parity).
pub const REPORT_EVERY_MS: f64 = 5000.0;

pub struct App {
    pub title: String,
    pub book_id: String,
    pub words: Vec<TimelineWord>,
    /// suffix[i] = sum of weights i..n (for exact time-left).
    suffix: Vec<f64>,
    pub idx: usize,
    pub wpm: u32,
    pub playing: bool,
    pub finished: bool,
    pub zen: bool,
    acc_ms: f64,
    ramp_ms: f64,
    // session + stats reporting
    pub started_at: Option<u64>,
    active_ms: f64,
    consumed: u64,
    last_reported_idx: usize,
    pub ms_since_report: f64,
}

pub struct SessionSummary {
    pub started_at: u64,
    pub duration_ms: u64,
    pub words: u64,
    pub avg_wpm: u32,
}

impl App {
    pub fn new(
        title: String,
        book_id: String,
        words: Vec<TimelineWord>,
        wpm: u32,
        start: usize,
    ) -> Self {
        let mut suffix = vec![0.0; words.len() + 1];
        for i in (0..words.len()).rev() {
            suffix[i] = suffix[i + 1] + words[i].2 as f64;
        }
        let idx = start.min(words.len().saturating_sub(1));
        Self {
            title,
            book_id,
            words,
            suffix,
            idx,
            wpm: wpm.clamp(WPM_MIN, WPM_MAX),
            playing: false,
            finished: false,
            zen: false,
            acc_ms: 0.0,
            ramp_ms: 0.0,
            started_at: None,
            active_ms: 0.0,
            consumed: 0,
            last_reported_idx: idx,
            ms_since_report: 0.0,
        }
    }

    pub fn tick(&mut self, delta_ms: f64) {
        if !self.playing {
            return;
        }
        let delta = delta_ms.min(MAX_DELTA_MS);
        self.ramp_ms += delta;
        self.acc_ms += delta;
        self.active_ms += delta;
        self.ms_since_report += delta;
        loop {
            let need = self.words[self.idx].2 as f64 * 60_000.0 / self.effective_wpm();
            if self.acc_ms < need {
                break;
            }
            self.acc_ms -= need;
            self.consumed += 1;
            if self.idx + 1 < self.words.len() {
                self.idx += 1;
            } else {
                self.playing = false;
                self.finished = true;
                self.acc_ms = 0.0;
                break;
            }
        }
    }

    pub fn effective_wpm(&self) -> f64 {
        ramp_wpm(self.wpm, self.ramp_ms)
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.pause();
        } else {
            if self.finished {
                self.idx = 0;
                self.finished = false;
            }
            self.playing = true;
            self.ramp_ms = 0.0;
            self.acc_ms = 0.0;
            if self.started_at.is_none() {
                self.started_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs());
            }
        }
    }

    pub fn pause(&mut self) {
        if self.playing {
            self.playing = false;
            self.acc_ms = 0.0;
        }
    }

    pub fn restart(&mut self) {
        self.idx = 0;
        self.acc_ms = 0.0;
        self.finished = false;
    }

    pub fn bump_wpm(&mut self, delta: i32) {
        let w = self.wpm as i32 + delta;
        self.wpm = (w.max(WPM_MIN as i32) as u32).min(WPM_MAX);
    }

    /// Consume the pending stats report: current position + words read since
    /// the last report (server clamps to 0–500 per report anyway).
    pub fn take_report(&mut self) -> (usize, u32) {
        let read = self.idx.saturating_sub(self.last_reported_idx).min(500) as u32;
        self.last_reported_idx = self.idx;
        self.ms_since_report = 0.0;
        (self.idx, read)
    }

    /// Session summary for `POST /api/sessions` — only when it is worth
    /// keeping (≥10 s active and ≥20 words, per the contract).
    pub fn session_summary(&self) -> Option<SessionSummary> {
        let started_at = self.started_at?;
        if self.active_ms < 10_000.0 || self.consumed < 20 {
            return None;
        }
        let avg_wpm = (self.consumed as f64 / (self.active_ms / 60_000.0)).round() as u32;
        Some(SessionSummary {
            started_at,
            duration_ms: self.active_ms as u64,
            words: self.consumed,
            avg_wpm,
        })
    }

    /// ← jump to the start of the current sentence; if already there, the previous one.
    pub fn prev_sentence(&mut self) {
        let start = sentence_start(&self.words, self.idx);
        self.idx = if self.idx > start {
            start
        } else {
            sentence_start(&self.words, start.saturating_sub(1))
        };
        self.acc_ms = 0.0;
        self.finished = false;
        self.last_reported_idx = self.last_reported_idx.min(self.idx);
    }

    pub fn next_sentence(&mut self) {
        let last = self.words.len() - 1;
        self.idx = sentence_end(&self.words, self.idx).map_or(last, |e| (e + 1).min(last));
        self.acc_ms = 0.0;
        self.finished = false;
    }

    /// The current sentence as (timeline index, text) pairs, for the paused context strip.
    pub fn current_sentence(&self) -> Vec<(usize, &str)> {
        let start = sentence_start(&self.words, self.idx);
        let end = sentence_end(&self.words, self.idx).unwrap_or(self.words.len() - 1);
        (start..=end)
            .map(|i| (i, self.words[i].0.as_str()))
            .collect()
    }

    pub fn progress(&self) -> f64 {
        (self.idx + 1) as f64 / self.words.len() as f64
    }

    pub fn ms_left(&self) -> f64 {
        self.suffix[self.idx] * 60_000.0 / self.wpm as f64
    }
}

fn ramp_wpm(target: u32, ramp_ms: f64) -> f64 {
    let frac = (ramp_ms / RAMP_MS).min(1.0);
    (target as f64 * (RAMP_START + (1.0 - RAMP_START) * frac)).max(RAMP_FLOOR)
}

pub fn is_sentence_end(token: &str) -> bool {
    let core = token.trim_end_matches(['"', '\'', ')', ']', '}', '»', '“', '”', '’']);
    core.ends_with(['.', '!', '?', '…'])
}

fn sentence_start(words: &[TimelineWord], idx: usize) -> usize {
    (0..idx)
        .rev()
        .find(|&j| is_sentence_end(&words[j].0))
        .map_or(0, |j| j + 1)
}

fn sentence_end(words: &[TimelineWord], idx: usize) -> Option<usize> {
    (idx..words.len()).find(|&j| is_sentence_end(&words[j].0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(tokens: &[&str]) -> Vec<TimelineWord> {
        tokens
            .iter()
            .map(|t| TimelineWord(t.to_string(), 0, 1.0))
            .collect()
    }

    fn app(tokens: &[&str], wpm: u32) -> App {
        App::new("t".into(), "b".into(), words(tokens), wpm, 0)
    }

    #[test]
    fn sentence_boundaries() {
        assert!(is_sentence_end("end."));
        assert!(is_sentence_end("really?!"));
        assert!(is_sentence_end("quote.\""));
        assert!(is_sentence_end("nested.)"));
        assert!(!is_sentence_end("comma,"));
        assert!(!is_sentence_end("word"));

        let w = words(&["One", "two.", "Three", "four", "five.", "Six"]);
        assert_eq!(sentence_start(&w, 0), 0);
        assert_eq!(sentence_start(&w, 1), 0);
        assert_eq!(sentence_start(&w, 3), 2);
        assert_eq!(sentence_start(&w, 5), 5);
        assert_eq!(sentence_end(&w, 0), Some(1));
        assert_eq!(sentence_end(&w, 2), Some(4));
        assert_eq!(sentence_end(&w, 5), None);
    }

    #[test]
    fn ramp_starts_at_60_percent_and_settles() {
        assert_eq!(ramp_wpm(500, 0.0), 300.0);
        assert_eq!(ramp_wpm(500, 1500.0), 400.0);
        assert_eq!(ramp_wpm(500, 3000.0), 500.0);
        assert_eq!(ramp_wpm(500, 10_000.0), 500.0);
        // the floor keeps slow targets readable during ramp
        assert_eq!(ramp_wpm(150, 0.0), 100.0);
    }

    #[test]
    fn tick_advances_with_carry_and_finishes() {
        let mut app = app(&["a", "b", "c"], 600); // 100 ms/word at weight 1.0
        app.toggle_play();
        // effective wpm ramps from 360, so the first word needs ~166 ms
        app.tick(100.0);
        assert_eq!(app.idx, 0);
        app.tick(100.0);
        assert_eq!(app.idx, 1);
        app.tick(1000.0); // clamped to 250 ms — never fast-forwards unseen words
        assert!(app.idx <= 2);
        while app.playing {
            app.tick(16.0);
        }
        assert!(app.finished);
        assert_eq!(app.idx, 2);
    }

    #[test]
    fn report_counts_advanced_words_once() {
        let mut app = app(&["a", "b", "c", "d", "e"], 300);
        app.idx = 3;
        let (pos, read) = app.take_report();
        assert_eq!((pos, read), (3, 3));
        let (_, read) = app.take_report();
        assert_eq!(read, 0);
        // going back never yields a negative count, re-reading counts again
        app.prev_sentence();
        assert_eq!(app.idx, 0);
        app.idx = 2;
        let (_, read) = app.take_report();
        assert_eq!(read, 2);
    }

    #[test]
    fn session_summary_gates_short_sessions() {
        let mut app = app(&["a"; 100], 600);
        app.toggle_play();
        assert!(app.session_summary().is_none()); // nothing consumed yet
        for _ in 0..200 {
            app.tick(100.0);
        }
        let s = app.session_summary().expect("long enough session");
        assert!(s.words >= 20);
        assert!(s.duration_ms >= 10_000);
        assert!(s.avg_wpm > 0);
    }
}
