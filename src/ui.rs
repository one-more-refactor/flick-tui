use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::api::Book;
use crate::app::App;

const ACCENT: Color = Color::Red;
const DIM: Color = Color::DarkGray;
const RAIL_WIDTH: u16 = 41;

// ---------------------------------------------------------------- library

pub struct Library {
    pub books: Vec<Book>,
    pub sel: usize,
    pub who: String,
}

impl Library {
    pub fn new(books: Vec<Book>, who: String) -> Self {
        Self { books, sel: 0, who }
    }

    pub fn up(&mut self) {
        self.sel = self.sel.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.sel + 1 < self.books.len() {
            self.sel += 1;
        }
    }
}

pub fn draw_library(f: &mut Frame, lib: &Library) {
    let area = f.area();
    if area.height < 5 || area.width < 20 {
        return;
    }
    draw_wordmark_header(f, &lib.who, row(area, 0));

    let list_top = 2u16;
    let visible = area.height.saturating_sub(list_top + 2) as usize;
    if visible == 0 {
        return;
    }
    if lib.books.is_empty() {
        let hint = Line::from(Span::styled(
            " LIBRARY EMPTY — add books on the web, or: flick-tui <file>",
            Style::new().fg(DIM),
        ));
        f.render_widget(Paragraph::new(hint), row(area, list_top + 1));
    }
    let offset = lib
        .sel
        .saturating_sub(visible / 2)
        .min(lib.books.len().saturating_sub(visible));
    for (screen_i, book_i) in (offset..lib.books.len()).take(visible).enumerate() {
        let b = &lib.books[book_i];
        let selected = book_i == lib.sel;
        let pct = (b.position * 100)
            .checked_div(b.word_count)
            .unwrap_or(0)
            .min(100);
        let marker = if selected { "▶ " } else { "  " };
        let left = format!(
            "{marker}{:02}  {}",
            book_i + 1,
            truncated(&b.title.to_uppercase(), 48)
        );
        let right = format!("{} W · {pct:>3}% ", b.word_count);
        let dots =
            (area.width as usize).saturating_sub(left.chars().count() + right.chars().count() + 2);
        let row_style = if selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let line = Line::from(vec![
            Span::styled(
                left,
                if selected {
                    row_style.add_modifier(Modifier::BOLD)
                } else {
                    row_style
                },
            ),
            Span::styled(
                format!(" {} ", "·".repeat(dots)),
                row_style.fg(if selected { Color::Reset } else { DIM }),
            ),
            Span::styled(right, row_style),
        ]);
        f.render_widget(Paragraph::new(line), row(area, list_top + screen_i as u16));
    }
    help_line(
        f,
        " ↑ ↓ SELECT · ENTER READ · R REFRESH · Q QUIT",
        row(area, area.height - 1),
    );
}

// ----------------------------------------------------------------- reader

pub fn draw_reader(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.height < 5 || area.width < 20 {
        return;
    }
    if !app.zen {
        let state = if app.finished {
            "END_"
        } else if app.playing {
            "PLAYING"
        } else {
            "PAUSED"
        };
        let right = format!("{} · {state}", truncated(&app.title.to_uppercase(), 40));
        draw_wordmark_header(f, &right, row(area, 0));
        draw_status(f, app, row(area, area.height - 2));
        let help = if app.finished {
            " SPACE / R RESTART · Q LIBRARY"
        } else {
            " SPACE PLAY/PAUSE · ← → SENTENCE · ↑ ↓ WPM · Z ZEN · R RESTART · Q LIBRARY"
        };
        help_line(f, help, row(area, area.height - 1));
    }
    draw_progress(f, app, row(area, area.height - if app.zen { 1 } else { 3 }));
    draw_stage(f, app, area);
}

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1)
}

fn draw_wordmark_header(f: &mut Frame, right: &str, area: Rect) {
    let right = format!("{right} ");
    let pad = (area.width as usize).saturating_sub(7 + right.chars().count());
    let spans = vec![
        Span::styled(" FLICK", Style::new().add_modifier(Modifier::BOLD)),
        Span::styled("_", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::new().fg(DIM)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn help_line(f: &mut Frame, text: &str, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncated(text, area.width as usize),
            Style::new().fg(DIM),
        ))),
        area,
    );
}

fn draw_stage(f: &mut Frame, app: &App, area: Rect) {
    let mid = area.y + area.height / 2;
    let center_col = area.x + area.width / 2;

    // guide rails with the accent notch over the pivot column
    let rail_w = RAIL_WIDTH.min(area.width.saturating_sub(4)) | 1; // odd, so the notch centers
    let rail_x = center_col.saturating_sub(rail_w / 2);
    let half = (rail_w / 2) as usize;
    let rail = |notch: &str| {
        Line::from(vec![
            Span::styled("─".repeat(half), Style::new().fg(DIM)),
            Span::styled(notch.to_string(), Style::new().fg(ACCENT)),
            Span::styled("─".repeat(half), Style::new().fg(DIM)),
        ])
    };
    let rail_area = |y: u16| Rect::new(rail_x, y, rail_w, 1);
    if mid >= area.y + 2 && mid + 2 < area.y + area.height {
        f.render_widget(Paragraph::new(rail("┬")), rail_area(mid - 2));
        f.render_widget(Paragraph::new(rail("┴")), rail_area(mid + 2));
    }

    // the word, pivot letter pinned to the center column
    let TimelineWordParts { pre, pivot, post } = split_word(app);
    let start = center_col.saturating_sub(pre.chars().count() as u16);
    let word_line = Line::from(vec![
        Span::styled(pre, Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(pivot, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(post, Style::new().add_modifier(Modifier::BOLD)),
    ]);
    let word_w = area.width.saturating_sub(start - area.x);
    f.render_widget(Paragraph::new(word_line), Rect::new(start, mid, word_w, 1));

    // paused ⇒ context strip: the current sentence, current word inverse-video
    if !app.playing && !app.zen && mid + 4 < area.y + area.height {
        draw_context(f, app, row(area, mid + 4 - area.y));
    }
}

struct TimelineWordParts {
    pre: String,
    pivot: String,
    post: String,
}

fn split_word(app: &App) -> TimelineWordParts {
    let w = &app.words[app.idx];
    let chars: Vec<char> = w.0.chars().collect();
    let orp = w.1.min(chars.len().saturating_sub(1));
    TimelineWordParts {
        pre: chars[..orp].iter().collect(),
        pivot: chars.get(orp).map(|c| c.to_string()).unwrap_or_default(),
        // `get`: an empty token (a word that was all control characters, or
        // an empty string from the server) renders blank instead of panicking
        post: chars
            .get(orp + 1..)
            .map(|rest| rest.iter().collect())
            .unwrap_or_default(),
    }
}

fn draw_context(f: &mut Frame, app: &App, area: Rect) {
    let sentence = app.current_sentence();
    let max = area.width.saturating_sub(2) as usize;
    let cur = sentence
        .iter()
        .position(|&(i, _)| i == app.idx)
        .unwrap_or(0);

    // grow a window around the current word until the width is spent
    let (mut lo, mut hi) = (cur, cur);
    let mut len = sentence[cur].1.chars().count();
    loop {
        let grew_left = lo > 0 && len + sentence[lo - 1].1.chars().count() < max;
        if grew_left {
            lo -= 1;
            len += sentence[lo].1.chars().count() + 1;
        }
        let grew_right = hi + 1 < sentence.len() && len + sentence[hi + 1].1.chars().count() < max;
        if grew_right {
            hi += 1;
            len += sentence[hi].1.chars().count() + 1;
        }
        if !grew_left && !grew_right {
            break;
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    for (k, &(i, text)) in sentence[lo..=hi].iter().enumerate() {
        if k > 0 {
            spans.push(Span::styled(" ", Style::new().fg(DIM)));
        }
        if i == app.idx {
            spans.push(Span::styled(
                text.to_string(),
                Style::new().add_modifier(Modifier::REVERSED),
            ));
        } else {
            spans.push(Span::styled(text.to_string(), Style::new().fg(DIM)));
        }
    }
    let start = area.x + (area.width.saturating_sub(len as u16)) / 2;
    let w = area.width.saturating_sub(start - area.x);
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(start, area.y, w, 1),
    );
}

fn draw_progress(f: &mut Frame, app: &App, area: Rect) {
    let w = area.width.saturating_sub(2) as usize;
    let filled = ((app.progress() * w as f64).round() as usize).min(w);
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("█".repeat(filled), Style::new().fg(ACCENT)),
        Span::styled("░".repeat(w - filled), Style::new().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let secs = (app.ms_left() / 1000.0).ceil() as u64;
    let left = format!(
        " {} / {} · {:.0}%",
        app.idx + 1,
        app.words.len(),
        app.progress() * 100.0
    );
    let right = format!("{}:{:02} LEFT · {} WPM ", secs / 60, secs % 60, app.wpm);
    let dots =
        (area.width as usize).saturating_sub(left.chars().count() + right.chars().count() + 2);
    let line = Line::from(vec![
        Span::styled(left, Style::new().fg(DIM)),
        Span::styled(format!(" {} ", "·".repeat(dots)), Style::new().fg(DIM)),
        Span::styled(right, Style::new().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn truncated(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Timeline;
    use ratatui::backend::CrosstermBackend;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    /// The bytes ratatui would actually write to the terminal for two frames.
    fn terminal_bytes(app: &mut App) -> String {
        let mut out: Vec<u8> = Vec::new();
        {
            let opts = TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            };
            let mut t = Terminal::with_options(CrosstermBackend::new(&mut out), opts).unwrap();
            t.draw(|f| draw_reader(f, app)).unwrap();
            app.idx = 1;
            t.draw(|f| draw_reader(f, app)).unwrap();
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn hostile_book_text_cannot_inject_terminal_escapes() {
        let timeline: Timeline = serde_json::from_str(
            r#"{"words":[["a\u001b]52;c;cHduZWQ=\u0007b",0,1.0],["x\u001b[2Jy\u009b31m",0,1.0]]}"#,
        )
        .unwrap();
        let book: crate::api::Book = serde_json::from_str(
            r#"{"id":"b","title":"T\u001b]0;PWNED\u0007itle","word_count":2}"#,
        )
        .unwrap();
        let mut app = App::new(book.title, book.id, timeline.words, 300, 0);
        let out = terminal_bytes(&mut app);
        assert!(!out.contains("\x1b]"), "OSC sequence reached the terminal");
        assert!(
            !out.contains("\x1b[2J"),
            "clear-screen reached the terminal"
        );
        assert!(!out.contains('\u{9b}'), "C1 CSI reached the terminal");
        assert!(!out.contains('\x07'), "BEL reached the terminal");
    }

    #[test]
    fn empty_token_does_not_panic() {
        let words = vec![crate::api::TimelineWord(String::new(), 3, 1.0)];
        let app = App::new("t".into(), "b".into(), words, 300, 0);
        let mut t = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw_reader(f, &app)).unwrap();
    }
}
