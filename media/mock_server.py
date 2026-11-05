#!/usr/bin/env python3
"""Stand-in flick server for recording flick-tui demos (stdlib only).

Implements just the slice of the flick HTTP API (docs/CONTRACTS.md in the
flick repo) that flick-tui calls, with a seeded library of public-domain
text. Timelines follow the contract's ORP table; weights use a simplified
version of the engine's model (length, digits, clause/sentence wrap-up,
normalized to mean 1.0) — close enough to *look* right on camera, not a
replacement for flick-core.

    python3 media/mock_server.py [--port 8765]

Never use real personal data here: everything below is fiction or public
domain.
"""

import argparse
import json
import re
import secrets
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

INTRO = (
    "Welcome to flick. Words appear one at a time, right where your eyes "
    "already are. The red letter is the pivot: keep looking at it and let "
    "the words come to you. Press space to pause, and the whole sentence "
    "shows up underneath. Arrow keys skip sentences or change the speed. "
    "Press z for zen mode when you want nothing but the words."
)

# O. Henry, "The Gift of the Magi" (1905) — public domain.
MAGI = (
    "One dollar and eighty-seven cents. That was all. And sixty cents of it "
    "was in pennies. Pennies saved one and two at a time by bulldozing the "
    "grocer and the vegetable man and the butcher until one's cheeks burned "
    "with the silent imputation of parsimony that such close dealing "
    "implied. Three times Della counted it. One dollar and eighty-seven "
    "cents. And the next day would be Christmas. There was clearly nothing "
    "to do but flop down on the shabby little couch and howl. So Della did "
    "it. Which instigates the moral reflection that life is made up of "
    "sobs, sniffles, and smiles, with sniffles predominating."
)

# Lewis Carroll, "Alice's Adventures in Wonderland" (1865) — public domain.
ALICE = (
    "Alice was beginning to get very tired of sitting by her sister on the "
    "bank, and of having nothing to do: once or twice she had peeped into "
    "the book her sister was reading, but it had no pictures or "
    "conversations in it, \"and what is the use of a book,\" thought Alice, "
    "\"without pictures or conversations?\" So she was considering in her "
    "own mind (as well as she could, for the hot day made her feel very "
    "sleepy and stupid), whether the pleasure of making a daisy-chain would "
    "be worth the trouble of getting up and picking the daisies, when "
    "suddenly a White Rabbit with pink eyes ran close by her."
)

# Mary Shelley, "Frankenstein" (1818) — public domain.
FRANKENSTEIN = (
    "You will rejoice to hear that no disaster has accompanied the "
    "commencement of an enterprise which you have regarded with such evil "
    "forebodings. I arrived here yesterday, and my first task is to assure "
    "my dear sister of my welfare and increasing confidence in the success "
    "of my undertaking."
)

SEED = [
    # (id, title, text, position, last_read_at, finished)
    ("intro", "Welcome to flick", INTRO, 0, None, True),
    ("magi", "The Gift of the Magi", MAGI, 0, None, False),
    ("alice", "Alice in Wonderland", ALICE, 31, 1_762_300_000, False),
    ("frank", "Frankenstein", FRANKENSTEIN, 0, None, False),
]

SENTENCE_END = re.compile(r"[.!?…][\"'”’)\]]*$")
CLAUSE_END = re.compile(r"[,;:—][\"'”’)\]]*$")


def orp_index(token: str) -> int:
    core = re.sub(r"[^\w]", "", token)
    n = len(core)
    if n <= 1:
        k = 0
    elif n <= 5:
        k = 1
    elif n <= 9:
        k = 2
    elif n <= 13:
        k = 3
    else:
        k = 4
    lead = len(token) - len(token.lstrip("\"'“‘([{"))
    return min(lead + k, max(len(token) - 1, 0))


def timeline(text: str) -> list:
    tokens = text.split()
    raw = []
    for tok in tokens:
        core = re.sub(r"[^\w]", "", tok)
        w = 1 + 0.055 * max(0, len(core) - 6)
        if any(c.isdigit() for c in core):
            w *= 1.5
        if SENTENCE_END.search(tok):
            w *= 2.0
        elif CLAUSE_END.search(tok):
            w *= 1.5
        raw.append(min(max(w, 0.45), 3.4))
    mean = sum(raw) / len(raw) if len(raw) >= 20 else 1.0
    return [
        [tok, orp_index(tok), round(min(max(w / mean, 0.4), 3.6), 2)]
        for tok, w in zip(tokens, raw)
    ]


class State:
    def __init__(self):
        self.sessions = set()
        self.wpm = 350
        self.books = {}
        for bid, title, text, pos, last, finished in SEED:
            tl = timeline(text)
            self.books[bid] = {
                "id": bid,
                "title": title,
                "word_count": len(tl),
                "position": len(tl) if finished else pos,
                "last_read_at": last,
                "timeline": tl,
            }

    def public(self, book):
        return {k: v for k, v in book.items() if k != "timeline"}


STATE = State()


class Handler(BaseHTTPRequestHandler):
    server_version = "flick-mock/1"

    def log_message(self, fmt, *args):  # keep VHS recordings quiet
        pass

    # -------------------------------------------------------------- utils
    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            return {}

    def session(self):
        for part in (self.headers.get("Cookie") or "").split(";"):
            name, _, value = part.strip().partition("=")
            if name == "flick_session" and value in STATE.sessions:
                return value
        return None

    def send(self, status, payload=None, cookie=None):
        data = b"" if payload is None else json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        if cookie:
            self.send_header(
                "Set-Cookie", f"flick_session={cookie}; Path=/; HttpOnly"
            )
        self.end_headers()
        self.wfile.write(data)

    def user(self):
        return {"name": "Guest", "guest": True, "settings": {"wpm": STATE.wpm}}

    def route(self, method):
        path = self.path.split("?")[0]
        if method == "POST" and path == "/api/auth/guest":
            token = secrets.token_hex(16)
            STATE.sessions.add(token)
            return self.send(201, self.user(), cookie=token)
        if method == "POST" and path == "/api/auth/lookup":
            return self.send(200, {"exists": False, "methods": []})
        if not self.session():
            return self.send(401, {"error": "unauthorized"})

        if path == "/api/auth/me":
            if method == "PATCH":
                wpm = self.body().get("settings", {}).get("wpm")
                if isinstance(wpm, int):
                    STATE.wpm = wpm
            return self.send(200, self.user())
        if path == "/api/books" and method == "GET":
            return self.send(200, [STATE.public(b) for b in STATE.books.values()])
        if path == "/api/books" and method == "POST":
            req = self.body()
            text = (req.get("text") or "").strip()
            if len(text.split()) < 2:
                return self.send(400, {"error": "text is too short"})
            bid = secrets.token_hex(4)
            tl = timeline(text)
            STATE.books[bid] = {
                "id": bid,
                "title": req.get("title") or "Untitled",
                "word_count": len(tl),
                "position": 0,
                "last_read_at": int(time.time()),
                "timeline": tl,
            }
            return self.send(201, STATE.public(STATE.books[bid]))
        if path == "/api/sessions" and method == "POST":
            return self.send(201, {})

        m = re.fullmatch(r"/api/books/([^/]+)/(timeline|position)", path)
        book = STATE.books.get(m.group(1)) if m else None
        if book and m.group(2) == "timeline" and method == "GET":
            tl = book["timeline"]
            return self.send(200, {"version": 1, "words": tl, "word_count": len(tl)})
        if book and m.group(2) == "position" and method == "PUT":
            pos = self.body().get("position")
            if isinstance(pos, int):
                book["position"] = max(0, min(pos, book["word_count"]))
                book["last_read_at"] = int(time.time())
            return self.send(204)
        return self.send(404, {"error": "not found"})

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_PUT(self):
        self.route("PUT")

    def do_PATCH(self):
        self.route("PATCH")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", type=int, default=8765)
    args = ap.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"flick mock server on http://127.0.0.1:{args.port}", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
