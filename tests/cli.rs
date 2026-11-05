//! End-to-end tests that run the real `flick-tui` binary against a tiny
//! in-process stand-in for the flick HTTP API (std only, no extra crates).
//!
//! The full-screen TUI needs a real terminal, so these cover everything a
//! user hits *before* the alternate screen opens: argument handling, first
//! start, server switching, guest sessions, uploads, login and error paths.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_flick-tui");

// ------------------------------------------------------------ harness

/// An isolated `$XDG_CONFIG_HOME` / `$HOME` so tests never touch real config.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "flick-tui-test-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sandbox");
        Self { dir }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("flick/tui.json")
    }

    fn write_config(&self, server: &str, session: Option<&str>) {
        let path = self.config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = serde_json::json!({ "server": server, "session": session });
        std::fs::write(path, json.to_string()).unwrap();
    }

    fn config(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.config_path()).unwrap();
        serde_json::from_str(&text).expect("config is JSON")
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(BIN)
            .args(args)
            .env("XDG_CONFIG_HOME", &self.dir)
            .env("HOME", &self.dir)
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn flick-tui");
        // ignore EPIPE: some paths exit without reading stdin
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        child.wait_with_output().expect("wait for flick-tui")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn assert_ok(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
}

/// A clean failure: exit code 1 and `needle` somewhere in stderr.
fn assert_fails(out: &Output, needle: &str) {
    let err = stderr(out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains(needle), "missing {needle:?} in {err}");
}

fn body(req: &Request) -> serde_json::Value {
    serde_json::from_str(&req.body).expect("JSON body")
}

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    cookie: Option<String>,
    body: String,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn json(status: u16, body: serde_json::Value) -> Reply {
    Reply {
        status,
        headers: vec![],
        body: body.to_string(),
    }
}

type Handler = dyn Fn(&Request) -> Reply + Send + Sync;

/// Minimal HTTP/1.1 server: one request per connection, `Connection: close`.
struct MockServer {
    url: String,
    log: Arc<Mutex<Vec<Request>>>,
}

impl MockServer {
    fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let shared = Arc::clone(&log);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (h, log) = (Arc::clone(&handler), Arc::clone(&shared));
                thread::spawn(move || serve(stream, h.as_ref(), &log));
            }
        });
        Self { url, log }
    }

    fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    fn find(&self, method: &str, path: &str) -> Option<Request> {
        self.requests()
            .into_iter()
            .find(|r| r.method == method && r.path == path)
    }
}

fn serve(mut stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Request>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() || first.is_empty() {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let (mut length, mut cookie) = (0usize, None);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "cookie" => cookie = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    let req = Request {
        method,
        path,
        cookie,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let reply = handler(&req);
    log.lock().unwrap().push(req);

    let mut out = format!("HTTP/1.1 {} X\r\n", reply.status);
    out.push_str("Content-Type: application/json\r\nConnection: close\r\n");
    out.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
    for (name, value) in &reply.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&reply.body);
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
}

fn user(name: &str, guest: bool) -> serde_json::Value {
    serde_json::json!({ "name": name, "guest": guest, "settings": { "wpm": 350 } })
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// -------------------------------------------------------------- tests

const OLD: &str = "https://old.example";

#[test]
fn help_prints_usage_and_succeeds() {
    let sb = Sandbox::new("help");
    for flag in ["--help", "-h"] {
        let out = sb.run(&[flag], "");
        assert_ok(&out);
        let text = stdout(&out);
        assert!(text.starts_with("usage: flick-tui"), "{text}");
        assert!(text.contains("--server URL"));
    }
    assert!(!sb.config_path().exists(), "config written");
}

#[test]
fn server_flag_needs_a_value() {
    let sb = Sandbox::new("server-missing");
    let out = sb.run(&["--server"], "");
    assert_fails(&out, "flick-tui: --server needs a URL");
}

#[test]
fn server_flag_rejects_non_http_urls() {
    let sb = Sandbox::new("server-bad");
    let out = sb.run(&["--server", "example.org", "logout"], "");
    assert_fails(&out, "must start with http(s)://");
    assert!(!sb.config_path().exists());
}

#[test]
fn server_flag_switches_server_and_drops_the_session() {
    let sb = Sandbox::new("server-switch");
    sb.write_config(OLD, Some("old-token"));
    let args = ["--server", "http://127.0.0.1:9/", "logout"];
    let out = sb.run(&args, "");
    assert_ok(&out);
    let text = stdout(&out);
    assert!(text.contains("set to http://127.0.0.1:9"), "{text}");
    assert!(text.contains("out of http://127.0.0.1:9"), "{text}");
    let cfg = sb.config();
    assert_eq!(cfg["server"], "http://127.0.0.1:9");
    assert!(cfg["session"].is_null());
}

#[test]
fn first_start_without_a_tty_uses_the_public_server() {
    let sb = Sandbox::new("first-start");
    let out = sb.run(&["logout"], "");
    assert_ok(&out);
    let err = stderr(&out);
    let expected = "using the public server https://app.myflick.app";
    assert!(err.contains(expected), "{err}");
    assert_eq!(sb.config()["server"], "https://app.myflick.app");
}

#[cfg(unix)]
#[test]
fn config_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("perms");
    sb.write_config(OLD, Some("t"));
    let loose = std::fs::Permissions::from_mode(0o644);
    std::fs::set_permissions(sb.config_path(), loose).unwrap();
    assert_ok(&sb.run(&["logout"], ""));
    assert_eq!(mode(&sb.config_path()), 0o600);
}

#[test]
fn unreachable_server_is_a_clear_error() {
    let sb = Sandbox::new("unreachable");
    // grab a free port, then close it so nothing is listening there
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", free.local_addr().unwrap());
    drop(free);
    sb.write_config(&server, None);
    let out = sb.run(&[], "");
    assert_fails(&out, &format!("cannot reach {server}"));
}

#[test]
fn first_contact_creates_and_stores_a_guest_session() {
    let cookie = "flick_session=guest-123; Path=/; HttpOnly";
    let server = MockServer::start(move |r| match r.path.as_str() {
        "/api/auth/me" => json(401, serde_json::json!({})),
        "/api/auth/guest" => Reply {
            status: 201,
            headers: vec![("Set-Cookie", cookie.to_string())],
            body: user("Guest", true).to_string(),
        },
        _ => json(404, serde_json::json!({})),
    });
    let sb = Sandbox::new("guest");
    sb.write_config(&server.url, None);
    // a missing file fails after the session exists but before the TUI opens
    assert_fails(&sb.run(&["missing.txt"], ""), "missing.txt:");
    assert_eq!(sb.config()["session"], "guest-123");
    assert!(server.find("POST", "/api/auth/guest").is_some());
}

#[test]
fn stdin_upload_sends_session_cookie_and_surfaces_server_errors() {
    let server = MockServer::start(|r| {
        let authed = r.cookie.as_deref() == Some("flick_session=tok");
        match r.path.as_str() {
            "/api/auth/me" if authed => json(200, user("Ada", false)),
            "/api/auth/me" => json(401, serde_json::json!({})),
            "/api/books" => json(400, serde_json::json!({ "error": "too short" })),
            _ => json(404, serde_json::json!({})),
        }
    });
    let sb = Sandbox::new("upload");
    sb.write_config(&server.url, Some("tok"));
    assert_fails(&sb.run(&["-"], "Hi."), "flick-tui: too short");
    let upload = server.find("POST", "/api/books").expect("upload");
    let cookie = upload.cookie.as_deref();
    assert_eq!(cookie, Some("flick_session=tok"));
    assert_eq!(body(&upload)["title"], "stdin");
    assert_eq!(body(&upload)["text"], "Hi.");
    assert!(server.find("POST", "/api/auth/guest").is_none());
}

#[test]
fn file_upload_uses_the_file_stem_as_title() {
    let server = MockServer::start(|r| match r.path.as_str() {
        "/api/auth/me" => json(200, user("Ada", false)),
        "/api/books" => json(503, serde_json::json!({})),
        _ => json(404, serde_json::json!({})),
    });
    let sb = Sandbox::new("file-upload");
    sb.write_config(&server.url, Some("tok"));
    let text = "# Notes\n\nSome words.";
    let file = sb.dir.join("field-notes.md");
    std::fs::write(file, text).unwrap();
    // no `error` field in the body → generic status message
    let out = sb.run(&["field-notes.md"], "");
    assert_fails(&out, "server returned 503");
    let upload = server.find("POST", "/api/books").expect("upload");
    assert_eq!(body(&upload)["title"], "field-notes");
    assert_eq!(body(&upload)["text"], text);
}

#[test]
fn login_explains_unknown_accounts() {
    let server = MockServer::start(|r| match r.path.as_str() {
        "/api/auth/lookup" => json(200, serde_json::json!({ "exists": false })),
        _ => json(404, serde_json::json!({})),
    });
    let sb = Sandbox::new("login-unknown");
    sb.write_config(&server.url, None);
    let out = sb.run(&["login"], "reader@example.org\n");
    assert_fails(&out, "no account with that email");
    let lookup = server.find("POST", "/api/auth/lookup").unwrap();
    assert_eq!(body(&lookup)["email"], "reader@example.org");
}

#[test]
fn login_refuses_passwordless_accounts_before_prompting() {
    let server = MockServer::start(|r| {
        let lookup = serde_json::json!({ "exists": true, "methods": ["code"] });
        match r.path.as_str() {
            "/api/auth/lookup" => json(200, lookup),
            _ => json(404, serde_json::json!({})),
        }
    });
    let sb = Sandbox::new("login-sso");
    sb.write_config(&server.url, None);
    let out = sb.run(&["login"], "reader@example.org\n");
    assert_fails(&out, "has no password");
    assert!(server.find("POST", "/api/auth/login").is_none());
}

#[test]
fn login_requires_an_email() {
    let sb = Sandbox::new("login-empty");
    sb.write_config("http://127.0.0.1:9", None);
    assert_fails(&sb.run(&["login"], "\n"), "no email given");
}
