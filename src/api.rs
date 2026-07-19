//! Thin blocking client for the flick HTTP API (docs/CONTRACTS.md in the
//! flick repo is the binding spec). The TUI is a pure timeline player: the
//! server parses text into timelines, this module fetches and reports.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::Config;

#[derive(Deserialize, Clone)]
pub struct User {
    pub name: String,
    #[serde(default)]
    pub guest: bool,
    pub settings: Settings,
}

#[derive(Deserialize, Clone)]
pub struct Settings {
    pub wpm: u32,
}

#[derive(Deserialize, Clone)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub word_count: usize,
    #[serde(default)]
    pub position: usize,
    #[serde(default)]
    pub last_read_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct Lookup {
    pub exists: bool,
    #[serde(default)]
    pub methods: Vec<String>,
}

/// The reading timeline format: `words[i] = [text, orp_index, weight]`.
#[derive(Deserialize)]
pub struct Timeline {
    pub words: Vec<TimelineWord>,
}

#[derive(Deserialize, Clone, PartialEq, Debug)]
pub struct TimelineWord(pub String, pub usize, pub f32);

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    server: String,
    pub session: Option<String>,
}

impl Client {
    pub fn new(cfg: &Config) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(20))
                .build(),
            server: cfg.server.trim_end_matches('/').to_string(),
            session: cfg.session.clone(),
        }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = self
            .agent
            .request(method, &format!("{}{}", self.server, path));
        match &self.session {
            Some(s) => r.set("Cookie", &format!("flick_session={s}")),
            None => r,
        }
    }

    fn capture_session(&mut self, resp: &ureq::Response) {
        for header in resp.all("set-cookie") {
            if let Some(rest) = header.strip_prefix("flick_session=") {
                let value = rest.split(';').next().unwrap_or("");
                if !value.is_empty() {
                    self.session = Some(value.to_string());
                }
            }
        }
    }

    pub fn me(&self) -> Result<Option<User>, String> {
        match self.req("GET", "/api/auth/me").call() {
            Ok(resp) => Ok(Some(parse(resp)?)),
            Err(ureq::Error::Status(401, _)) => Ok(None),
            Err(e) => Err(err_msg(e)),
        }
    }

    pub fn guest(&mut self) -> Result<User, String> {
        let resp = self
            .req("POST", "/api/auth/guest")
            .call()
            .map_err(err_msg)?;
        self.capture_session(&resp);
        parse(resp)
    }

    pub fn lookup(&self, email: &str) -> Result<Lookup, String> {
        let resp = self
            .req("POST", "/api/auth/lookup")
            .send_json(json!({ "email": email }))
            .map_err(err_msg)?;
        parse(resp)
    }

    /// Sends the current (guest) cookie so the server merges that library.
    pub fn login(&mut self, email: &str, password: &str) -> Result<User, String> {
        let resp = self
            .req("POST", "/api/auth/login")
            .send_json(json!({ "email": email, "password": password }))
            .map_err(err_msg)?;
        self.capture_session(&resp);
        parse(resp)
    }

    pub fn books(&self) -> Result<Vec<Book>, String> {
        parse(self.req("GET", "/api/books").call().map_err(err_msg)?)
    }

    pub fn timeline(&self, id: &str) -> Result<Timeline, String> {
        let path = format!("/api/books/{id}/timeline");
        parse(self.req("GET", &path).call().map_err(err_msg)?)
    }

    pub fn create_book(&self, title: &str, text: &str) -> Result<Book, String> {
        let resp = self
            .req("POST", "/api/books")
            .send_json(json!({ "title": title, "text": text }))
            .map_err(err_msg)?;
        parse(resp)
    }

    pub fn put_position(&self, id: &str, position: usize, read: u32, day: &str) {
        let path = format!("/api/books/{id}/position");
        let _ = self
            .req("PUT", &path)
            .send_json(json!({ "position": position, "read": read, "day": day }));
    }

    pub fn post_session(
        &self,
        book_id: &str,
        started_at: u64,
        duration_ms: u64,
        words: u64,
        avg_wpm: u32,
    ) {
        let _ = self.req("POST", "/api/sessions").send_json(json!({
            "book_id": book_id,
            "started_at": started_at,
            "duration_ms": duration_ms,
            "words": words,
            "avg_wpm": avg_wpm,
        }));
    }

    pub fn patch_wpm(&self, wpm: u32) {
        let _ = self
            .req("PATCH", "/api/auth/me")
            .send_json(json!({ "settings": { "wpm": wpm } }));
    }
}

fn parse<T: serde::de::DeserializeOwned>(resp: ureq::Response) -> Result<T, String> {
    resp.into_json()
        .map_err(|e| format!("bad response from server: {e}"))
}

fn err_msg(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => resp
            .into_json::<serde_json::Value>()
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| format!("server returned {code}")),
        ureq::Error::Transport(t) => t.to_string(),
    }
}
