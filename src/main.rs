mod api;
mod app;
mod config;
mod ui;

use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use api::{Book, Client};
use app::App;
use config::Config;
use ui::Library;

const PUBLIC_SERVER: &str = "https://app.myflick.app";
const USAGE: &str = "\
usage: flick-tui              browse and read your flick library
       flick-tui <file>       upload a UTF-8 text/markdown file and read it ('-' = stdin)
       flick-tui login        sign in with email + password (guest library merges in)
       flick-tui logout       drop the stored session
       flick-tui --server URL point at another server (e.g. your self-hosted instance)";

fn main() {
    if let Err(e) = run_cli() {
        eprintln!("flick-tui: {e}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let mut server_flag = None;
    if let Some(i) = args.iter().position(|a| a == "--server") {
        if i + 1 >= args.len() {
            return Err("--server needs a URL".into());
        }
        server_flag = Some(args.remove(i + 1));
        args.remove(i);
    }

    let mut cfg = match (config::load(), server_flag) {
        (_, Some(url)) => {
            // re-pointing drops the session — it belongs to the old server
            let cfg = Config {
                server: normalize_url(&url)?,
                session: None,
            };
            config::save(&cfg);
            println!("server set to {}", cfg.server);
            cfg
        }
        (Some(cfg), None) => cfg,
        (None, None) => first_start()?,
    };

    match args.first().map(String::as_str) {
        Some("login") => login(&mut cfg),
        Some("logout") => {
            cfg.session = None;
            config::save(&cfg);
            println!("logged out of {}", cfg.server);
            Ok(())
        }
        file => run_app(cfg, file.map(String::from)),
    }
}

/// First start: default to the public server, offer the own-instance escape.
fn first_start() -> Result<Config, String> {
    if !io::stdin().is_terminal() {
        let cfg = Config {
            server: PUBLIC_SERVER.into(),
            session: None,
        };
        config::save(&cfg);
        eprintln!("first start: using the public server {PUBLIC_SERVER} (change with `flick-tui --server URL`)");
        return Ok(cfg);
    }
    println!("FLICK_ first start\n");
    println!("Where should flick connect?");
    println!("  [Enter]  the public server — {PUBLIC_SERVER}");
    println!("  or paste the URL of your own self-hosted instance\n");
    print!("server> ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let line = line.trim();
    let server = if line.is_empty() {
        PUBLIC_SERVER.to_string()
    } else {
        normalize_url(line)?
    };
    let cfg = Config {
        server,
        session: None,
    };
    config::save(&cfg);
    println!(
        "connected to {} — change any time with `flick-tui --server URL`\n",
        cfg.server
    );
    Ok(cfg)
}

fn normalize_url(s: &str) -> Result<String, String> {
    let s = s.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        Ok(s.to_string())
    } else {
        Err(format!(
            "server URL must start with http(s):// — got \"{s}\""
        ))
    }
}

fn login(cfg: &mut Config) -> Result<(), String> {
    print!("email> ");
    io::stdout().flush().ok();
    let mut email = String::new();
    io::stdin()
        .read_line(&mut email)
        .map_err(|e| e.to_string())?;
    let email = email.trim().to_string();
    if email.is_empty() {
        return Err("no email given".into());
    }
    let mut client = Client::new(cfg);
    let lookup = client.lookup(&email)?;
    if !lookup.exists {
        return Err(format!(
            "no account with that email — create one at {} first",
            cfg.server
        ));
    }
    if !lookup.methods.iter().any(|m| m == "password") {
        return Err(
            "this account has no password (SSO / email-code only) — sign in on the web".into(),
        );
    }
    let password = prompt_hidden("password> ")?;
    let user = client.login(&email, &password)?; // guest cookie rides along → library merges
    cfg.session = client.session.clone();
    config::save(cfg);
    println!("signed in as {} on {}", user.name, cfg.server);
    Ok(())
}

fn prompt_hidden(label: &str) -> Result<String, String> {
    print!("{label}");
    io::stdout().flush().ok();
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut out = String::new();
    let result = loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(out.clone()),
                KeyCode::Backspace => {
                    out.pop();
                }
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err("aborted".to_string())
                }
                KeyCode::Char(c) => out.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    disable_raw_mode().ok();
    println!();
    result
}

fn read_input(arg: &str) -> Result<(String, String), String> {
    if arg == "-" {
        let mut s = String::new();
        io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        return Ok(("stdin".into(), s));
    }
    let text = std::fs::read_to_string(arg).map_err(|e| format!("{arg}: {e}"))?;
    let title = Path::new(arg)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| arg.to_string());
    Ok((title, text))
}

fn run_app(mut cfg: Config, file: Option<String>) -> Result<(), String> {
    let mut client = Client::new(&cfg);
    let user = match client.me() {
        Ok(Some(u)) => u,
        Ok(None) => {
            let u = client.guest()?;
            cfg.session = client.session.clone();
            config::save(&cfg);
            u
        }
        Err(e) => return Err(format!("cannot reach {}: {e}", cfg.server)),
    };
    let who = format!(
        "{} @ {}",
        if user.guest {
            "GUEST".into()
        } else {
            user.name.to_uppercase()
        },
        cfg.server
            .trim_start_matches("https://")
            .trim_start_matches("http://")
    );
    let mut synced_wpm = user.settings.wpm.clamp(app::WPM_MIN, app::WPM_MAX);

    let initial = match file {
        Some(arg) => {
            let (title, text) = read_input(&arg)?;
            let book = client.create_book(&title, &text)?;
            Some(open_reader(&client, &book, synced_wpm)?)
        }
        None => None,
    };

    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableFocusChange);
    let result = event_loop(&mut terminal, &client, initial, &who, &mut synced_wpm);
    let _ = execute!(io::stdout(), DisableFocusChange);
    ratatui::restore();
    result
}

enum Screen {
    Library(Library),
    Reader(App),
}

fn load_library(client: &Client, who: &str) -> Result<Library, String> {
    let mut books = client.books()?;
    sort_books(&mut books);
    Ok(Library::new(books, who.to_string()))
}

/// In-progress by recency, then unread, then finished — the web ordering rule.
fn sort_books(books: &mut [Book]) {
    books.sort_by_key(|b| {
        let finished = b.word_count > 0 && b.position >= b.word_count;
        let group = if finished {
            2
        } else if b.position > 0 {
            0
        } else {
            1
        };
        (group, -b.last_read_at.unwrap_or(0))
    });
}

fn open_reader(client: &Client, book: &Book, wpm: u32) -> Result<App, String> {
    let timeline = client.timeline(&book.id)?;
    if timeline.words.is_empty() {
        return Err(format!("\"{}\" has an empty timeline", book.title));
    }
    Ok(App::new(
        book.title.clone(),
        book.id.clone(),
        timeline.words,
        wpm,
        book.position,
    ))
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Fire-and-forget position/read report so playback never stalls on I/O.
fn report_async(client: &Client, app: &mut App) {
    let (position, read) = app.take_report();
    let client = client.clone();
    let book_id = app.book_id.clone();
    let day = today();
    std::thread::spawn(move || client.put_position(&book_id, position, read, &day));
}

/// Blocking final flush on reader exit: position, session summary, wpm.
fn close_reader(client: &Client, app: &mut App, synced_wpm: &mut u32) {
    app.pause();
    let (position, read) = app.take_report();
    client.put_position(&app.book_id, position, read, &today());
    if let Some(s) = app.session_summary() {
        client.post_session(
            &app.book_id,
            s.started_at,
            s.duration_ms,
            s.words,
            s.avg_wpm,
        );
    }
    if app.wpm != *synced_wpm {
        client.patch_wpm(app.wpm);
        *synced_wpm = app.wpm;
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: &Client,
    initial: Option<App>,
    who: &str,
    synced_wpm: &mut u32,
) -> Result<(), String> {
    let mut screen = match initial {
        Some(app) => Screen::Reader(app),
        None => Screen::Library(load_library(client, who)?),
    };
    let mut last = Instant::now();
    loop {
        match &screen {
            Screen::Library(lib) => terminal.draw(|f| ui::draw_library(f, lib)),
            Screen::Reader(app) => terminal.draw(|f| ui::draw_reader(f, app)),
        }
        .map_err(|e| e.to_string())?;

        // frame-loop accumulator: fast poll while playing, lazy redraw when idle
        let timeout = match &screen {
            Screen::Reader(app) if app.playing => Duration::from_millis(8),
            _ => Duration::from_millis(250),
        };
        if event::poll(timeout).map_err(|e| e.to_string())? {
            let ev = event::read().map_err(|e| e.to_string())?;
            match &mut screen {
                Screen::Reader(app) => match ev {
                    Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            close_reader(client, app, synced_wpm);
                            screen = Screen::Library(load_library(client, who)?);
                            last = Instant::now();
                            continue;
                        }
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            close_reader(client, app, synced_wpm);
                            return Ok(());
                        }
                        KeyCode::Char(' ') => {
                            let was_playing = app.playing;
                            app.toggle_play();
                            if was_playing {
                                report_async(client, app);
                            }
                        }
                        KeyCode::Left => app.prev_sentence(),
                        KeyCode::Right => app.next_sentence(),
                        KeyCode::Up => app.bump_wpm(app::WPM_STEP as i32),
                        KeyCode::Down => app.bump_wpm(-(app::WPM_STEP as i32)),
                        KeyCode::Char('z') => app.zen = !app.zen,
                        KeyCode::Char('r') => app.restart(),
                        _ => {}
                    },
                    Event::FocusLost if app.playing => {
                        app.pause();
                        report_async(client, app);
                    }
                    _ => {}
                },
                Screen::Library(lib) => match ev {
                    Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(())
                        }
                        KeyCode::Up | KeyCode::Char('k') => lib.up(),
                        KeyCode::Down | KeyCode::Char('j') => lib.down(),
                        KeyCode::Char('r') => *lib = load_library(client, who)?,
                        KeyCode::Enter => {
                            if let Some(book) = lib.books.get(lib.sel).cloned() {
                                screen = Screen::Reader(open_reader(client, &book, *synced_wpm)?);
                                last = Instant::now();
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
            }
        }
        if let Screen::Reader(app) = &mut screen {
            let was_finished = app.finished;
            let now = Instant::now();
            app.tick((now - last).as_secs_f64() * 1000.0);
            last = now;
            if app.playing && app.ms_since_report >= app::REPORT_EVERY_MS {
                report_async(client, app);
            }
            if app.finished && !was_finished {
                // reaching END_ counts: flush the final position promptly
                report_async(client, app);
            }
        } else {
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(id: &str, word_count: usize, position: usize, last_read_at: Option<i64>) -> Book {
        Book {
            id: id.into(),
            title: id.into(),
            word_count,
            position,
            last_read_at,
        }
    }

    #[test]
    fn library_order_is_in_progress_then_unread_then_finished() {
        let mut books = vec![
            book("done", 10, 10, Some(50)),
            book("unread", 10, 0, None),
            book("old", 10, 3, Some(1)),
            book("recent", 10, 5, Some(9)),
        ];
        sort_books(&mut books);
        let ids: Vec<&str> = books.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["recent", "old", "unread", "done"]);
    }

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            normalize_url(" https://flick.example.org/ ").unwrap(),
            "https://flick.example.org"
        );
        assert_eq!(
            normalize_url("http://localhost:8484").unwrap(),
            "http://localhost:8484"
        );
        assert!(normalize_url("flick.example.org").is_err());
        assert!(normalize_url("ftp://flick.example.org").is_err());
    }
}
