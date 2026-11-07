//! Thin blocking client for the flick HTTP API (docs/CONTRACTS.md in the
//! flick repo is the binding spec). The TUI is a pure timeline player: the
//! server parses text into timelines, this module fetches and reports.

use std::time::Duration;

use serde::{Deserialize, Deserializer};
use serde_json::json;

use crate::config::Config;

#[derive(Deserialize, Clone)]
pub struct User {
    #[serde(deserialize_with = "clean_string")]
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
    #[serde(deserialize_with = "clean_string")]
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
pub struct TimelineWord(
    #[serde(deserialize_with = "clean_string")] pub String,
    pub usize,
    pub f32,
);

/// Book text and titles are user content (pasted text, uploaded files, web
/// articles, shared books) and a self-hosted server is whatever its operator
/// runs. Neither may write raw control characters to the terminal: ratatui
/// passes cell contents through verbatim, so an ESC in a word would reach the
/// terminal as a live escape sequence (OSC 52 clipboard writes, title changes,
/// screen clears). Whitespace controls become a space; every other control
/// character (C0, DEL, C1 — ESC and CSI included) is dropped.
pub fn clean(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' | '\n' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

fn clean_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    String::deserialize(d).map(|s| clean(&s))
}

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
            .and_then(|v| v["error"].as_str().map(clean))
            .unwrap_or_else(|| format!("server returned {code}")),
        ureq::Error::Transport(t) => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_escape_sequences_and_c1_controls() {
        assert_eq!(clean("a\x1b]52;c;cHduZWQ=\x07b"), "a]52;c;cHduZWQ=b");
        assert_eq!(clean("x\x1b[2Jy\u{9b}31m\u{7f}"), "x[2Jy31m");
        assert_eq!(clean("two\nlines\tand\rtabs"), "two lines and tabs");
        assert_eq!(clean("Grüße — ok"), "Grüße — ok");
    }

    #[test]
    fn server_strings_are_cleaned_on_the_way_in() {
        let t: Timeline =
            serde_json::from_str(r#"{"words":[["ev\u001b]0;PWNED\u0007il",1,1.0]]}"#).unwrap();
        assert_eq!(t.words[0].0, "ev]0;PWNEDil");
        let b: Book =
            serde_json::from_str(r#"{"id":"b1","title":"T\u001b[2J\u009bitle","word_count":3}"#)
                .unwrap();
        assert_eq!(b.title, "T[2Jitle");
        let u: User =
            serde_json::from_str(r#"{"name":"m\u001b]52;c;eA==\u0007e","settings":{"wpm":300}}"#)
                .unwrap();
        assert_eq!(u.name, "m]52;c;eA==e");
    }
}
