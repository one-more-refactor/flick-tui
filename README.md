# flick-tui

Terminal client for [flick](https://app.myflick.app) — RSVP speed reading,
one word at a time, in your terminal.

```
 FLICK_                                      WELCOME TO FLICK · PAUSED

              ────────────────────┬────────────────────
                               reading
              ────────────────────┴────────────────────

                    the pivot letter sits still, your eyes stop moving

 ██████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
  12 / 166 · 7%  ··························································  0:26 LEFT · 350 WPM
 SPACE PLAY/PAUSE · ← → SENTENCE · ↑ ↓ WPM · Z ZEN · R RESTART · Q LIBRARY
```

Words are flashed at a fixed focal point with the ORP (optimal recognition
point) letter pinned in place, paced by a research-grounded weight model —
rare words, numbers, and sentence ends get more time. The engine runs
server-side; this client plays the resulting reading timelines.

## Install

```sh
cargo install --git https://github.com/one-more-refactor/flick-tui
```

## Use

```sh
flick-tui              # browse and read your library
flick-tui notes.md     # upload a text/markdown file and read it now
cat article.txt | flick-tui -   # same, from stdin
flick-tui login        # sign in (your guest library merges into the account)
flick-tui logout
flick-tui --server https://flick.example.org   # use your own instance
```

On first start it asks where to connect: press Enter for the public server,
or paste the URL of your own self-hosted [flick](https://app.myflick.app)
instance. No account needed — a guest session is created automatically, and
your library, positions, streaks and stats live on the server (shared with
the web app and any other client).

### Keys

| key | action |
|---|---|
| `Space` | play / pause (never autoplays; restarts at the end) |
| `←` / `→` | back / forward one sentence |
| `↑` / `↓` | WPM ±25 (synced to your account) |
| `z` | zen mode — word and progress only |
| `r` | restart the book |
| `q` / `Esc` | back to library / quit |

Pausing shows the current sentence with the current word highlighted, so you
never lose the thread. Every play start ramps from 60% to full speed over
~3 seconds. Losing terminal focus pauses playback.

## Config

`~/.config/flick/tui.json` — the server URL and your session token. That's
all the state there is; reading positions live on the server.

## License

AGPL-3.0-only. The flick backend this client talks to is part of the flick
project; this repo contains only the terminal player.
