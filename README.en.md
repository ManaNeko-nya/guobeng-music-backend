<p align="center">
  <img src="assets/icon.png" alt="Guobeng Music Server" width="200">
</p>

<p align="center">
  Artwork by <a href="https://www.pixiv.net/artworks/110598995">無题</a> on pixiv · Character: <b>Scaramouche</b> (<i>Genshin Impact</i>)<br>
  The character is © HoYoverse. The name comes from his true name 雷电国崩 — <code>guobeng</code> is the pinyin of 国崩
</p>

> **Warning: AI slop.** If you hate AI, please leave now (?)
>
> **This project is fully managed by AI.** Code, documentation, build scripts and repository configuration are generated and maintained by an AI agent; humans only file requests and review results.

# Guobeng Music Server

<p align="center">
  <a href="README.md">简体中文</a> · <b>English</b>
</p>

---

The **backend** of a personal music library, written in Rust. It compiles to a single executable that serves a JSON API.

It models the `Artist → Album → Track` hierarchy, LRC lyrics, favorite lists, comments, play history and accounts
as one clean REST contract. Your frontend — web, Android, desktop or CLI — only deals with JSON and static media.
It never needs to know what the database looks like or where files are stored.

- **Stack**: Rust · axum · SQLite (single-file database)
- **Contract**: `/api/v1`, JSON responses shaped as `{ code, message, data }`
- **Deployment**: one binary plus one `config.json`. No Python, Node, JVM or external database.

## Features

**Library**

- Full CRUD for artists, albums and tracks; albums and tracks carry cover art, descriptions and other metadata
- **Multi-artist tracks**: a track can credit several artists, displayed in the order they were entered. Collaborators are automatically merged into the album's artist page, so a feature never goes missing from the lead artist's page
- **Album visibility**: public or private. A private album returns **404** to everyone else (not 403 — existence is not leaked), and tracks, lyrics, comments, favorites and history all inherit that rule
- **Audio upload**: the server computes an MD5 for each file; re-uploading the same bytes returns 409 with the existing track ID. Up to 1GB per file, supporting mp3 / flac / wav / m4a / aac / ogg / opus
- **Track ordering**: drag to reorder; the server compacts `order_index` so no gaps are left behind
- **External audio**: or store just an `http(s)` URL and keep the bytes off your server
- **Album descriptions as source markers**: descriptions are editable; imports from Kugou and other catalogs write `kugouid<ID>` there. Keyword search covers descriptions, so import tools can de-duplicate without a dedicated column

**Playback**

- `/media` supports **Range (206)** and HEAD — seeking, resume and progressive playback all work out of the box
- Play history reporting: progress, real duration and completion (≥95% marks it finished), plus "continue this album"

**Lyrics**

- Stored as LRC text, validated for timestamp format on write; malformed input returns 400
- Multiple timestamps per line and `[ti:] [ar:] [al:] [by:] [offset:]` tags are supported
- No third-party lyric scraping: the server stores exactly what you give it

**Comments & favorites**

- Comments live on tracks: a thread plus one level of replies, no deeper nesting
- A favorite must belong to a user-created list, and one track may appear in several lists. Both list order and track order inside a list are user-defined
- **Batch favorite check**: up to 200 tracks per request, so a list page can render every heart in one call

**Accounts & security**

- Register / login / refresh / logout / change password
- Access tokens are 30-minute JWTs; refresh tokens live 7 days, **rotate on every refresh**, and only their SHA-256 digest is stored
- Changing a password invalidates every refresh token for that user immediately
- The `root` account signs in from any address like any other account; **create a separate admin account for daily use** instead of staying on root
- Login throttling: 8 failures per IP + account within 5 minutes locks it for 5 minutes
- Roles: `user` / `admin` / `root`. Unauthorized access returns 403
- Passwords use PBKDF2-HMAC-SHA256 with 600,000 iterations and a 16-byte random salt; the hash records its scheme and iteration count, so old passwords keep working and are rehashed on the next successful login

**Operations**

- Missing keys in `config.json` are written back with their defaults, so the file always shows every option
- `allow_registration` and `allow_user_audio_upload` are **hot-reloaded** — no restart needed
- `/health` for liveness (root path, not under `/api/v1`): the public response only carries `status`; version and stats require an admin token or debug mode; `/docs` is the contract entry point, with the full contract in `docs/API.md`

## Strengths and positioning

**Its edge is not "more features than Navidrome" — in fact it has fewer. It is that a complete self-hosted
music service fits into one static binary and one precisely written contract.**

### One file, zero dependencies, runs on a phone

| | Guobeng Music Server | Navidrome |
|---|---|---|
| Deliverable | **A single executable** | Binary + frontend assets + ffmpeg |
| External dependencies | **None** (no Python / Node / JVM / external database / container) | ffmpeg subprocess (transcoding) |
| Deployment | Drop it in a folder and run; works directly on Android + Termux | Usually Docker; on Android needs proot / root |
| Linking | Linux builds are fully static musl; nothing to install on the target | Relies on the target's glibc ecosystem |
| Cold start | Creates the DB and opens SQLite; no scan phase | Scans the library on start and incrementally; slower as it grows |
| Transcoding | None; originals served as-is | Real-time ffmpeg transcoding |

On a phone this is not an optimisation but a feasibility question: a phone already running another service
pays for the container runtime all the time, and ffmpeg is the main source of CPU spikes and heat once triggered.
In this library 99% of tracks are mp3 at roughly 128 kbps, so serving the original is the best choice anyway —
what it saves is not performance but operational surface.

### Hard edges in the contract and permissions

- **422 pinpoints the field**: `data.errors: [{ field, message }]` maps straight onto form inputs
- **404 does not leak existence**: a private album does not exist for anyone else, and tracks, lyrics, comments, favorites and history inherit that rule
- **416 follows the RFC**: an out-of-range request returns `Content-Range: bytes */total`, so clients do not have to guess
- **401 vs 403**: 401 means signed out, 403 means not allowed — one 403 never wipes the login state
- **Fixed pagination contract**: `page_size` 1–100; out-of-range is a 422 instead of a silent truncation

### Measured on a phone

Deployed on Android + Termux with 752 albums / 2,615 tracks:

```
8 concurrent workers × 4 rounds = 32 requests: 32/32 succeeded, p50 44 ms / p90 56 ms / max 64 ms, 178.7 req/s
Page 250: 29 ms (no degradation) · 100 tracks with joins: 119 ms · search: 44 ms
Audio Range returns 206 + Content-Range · full download MD5 matches the stored file_md5 byte for byte
```

The conclusion is simple: for a library in the tens of thousands, a single user and phone-class hardware,
SQLite plus a single axum process is enough — no cache layer or external dependency required.
**The target scale is a library in the tens of thousands**; beyond that, fuzzy search and deep pagination
would need a different implementation.

### Against Navidrome: not better, just thinner

| | Guobeng Music Server | Navidrome |
|---|---|---|
| Shape | **Backend contract only**; the frontend is entirely yours | Complete product with a built-in web UI |
| Protocol | Its own REST `/api/v1` | Subsonic / OpenSubsonic |
| Client ecosystem | Clients you write (JianYin already speaks both backends) | Dozens of ready-made apps |
| Source of truth | **The library is the truth, files are attachments** | Files are the truth, the library is an index |
| Import | Written over the HTTP API, metadata via API | Scans folders and reads embedded tags |

They are not competitors but two different choices. Navidrome aims for "install and listen";
this project aims for "every byte and every rule stays in your hands". The price is the existing ecosystem,
the payoff is zero dependencies, zero operations and zero compromise — and that price is already paid on the
client side: JianYin implements both this contract and Subsonic, turning "no ecosystem" into "no ecosystem needed".

### Deliberately out of scope, and what it costs

| Trade-off | Cost |
|---|---|
| No transcoding | Mobile data and first-play latency are the client's problem; 99% of this library is mp3 at 128 kbps, so serving originals is cheaper |
| No Subsonic compatibility | Loses the ready-made app ecosystem; covered by the self-written client |
| No scrobbling / share links / multiple libraries | Product-level features, consistent with "backend owns data and permissions only" |
| No built-in player | Fully decoupled from clients; playback UX is defined by them |
| No folder scanning | Ingestion goes through the HTTP API and metadata is the caller's job — in exchange, the library is the source of truth |

These are boundaries, not gaps: every feature added to the server is freedom taken away from the frontend.
The project itself does two things: **keep the data and permissions correct, and document the API precisely.**

## Design notes

**The contract is the product; the code is the implementation.**
Only the shapes defined in `dto.rs` are exposed. What a database row looks like (`models.rs`) never leaks out.
Changing a table should not force a frontend change — that is the point of the split.

**Errors share the same structure.**
Business endpoints use `{ code, message, data }`, where `code` is the HTTP status and 0 means success.
Structural validation failures (a missing field, `page_size=101`) return 422 with
`data.errors: [{field, message}]`, so the frontend can map each error straight onto a form field.

**Permissions decided in one place.**
The rule "a private album does not exist for anyone else" lives in a single spot in `services.rs`,
and tracks, lyrics, comments, favorites and history all reuse it. One copy of the rule means no handler
can silently forget the filter.

**Transaction boundaries at the routing layer.**
`services.rs` mutates objects but never commits; the route decides when to commit. A request either
succeeds as a whole or rolls back as a whole, never leaving half-written data.

**Query assembly in the repository layer.**
`repo.rs` owns the shared SELECT fragments and `sqlutil.rs` generates placeholders, so the same join
is not rewritten in a dozen places and forgotten in one of them.

## Code layout

```
guobeng-music-server/
├── Cargo.toml
├── src/
│   ├── main.rs            # Startup, root bootstrap, middleware, router assembly
│   ├── settings.rs        # Process config: CLI / env vars / config.json
│   ├── runtime_config.rs  # Hot-reloaded allow_* flags
│   ├── db.rs              # SQLite pool and schema
│   ├── models.rs          # Database rows (internal)
│   ├── dto.rs             # Serialization shapes (the contract)
│   ├── payload.rs         # Request bodies and field validation
│   ├── extract.rs         # Auth extractor, pagination, JSON error envelope
│   ├── services.rs        # Visibility, permissions, ordering, artist links
│   ├── hydrate.rs         # Batch assembly of nested DTOs (avoids N+1)
│   ├── repo.rs            # Shared SELECT fragments
│   ├── sqlutil.rs         # IN placeholder generation
│   ├── security.rs        # PBKDF2, JWT, refresh tokens, login throttling
│   ├── media.rs           # Upload validation and storage
│   ├── lyrics.rs          # LRC validation
│   ├── timeutil.rs        # Time formatting
│   ├── error.rs           # Error type to envelope mapping
│   ├── state.rs           # Shared request state
│   └── routes/            # 11 route modules + mod.rs
│       ├── auth.rs  users.rs  artists.rs  albums.rs  tracks.rs
│       ├── comments.rs  favorites.rs  history.rs
│       └── files.rs  media.rs  health.rs  mod.rs
├── assets/icon.png        # Project icon (copyright belongs to the original artist, see below)
├── docs/API.md            # API contract (required reading for frontend work)
└── .github/workflows/     # CI builds: Windows x64 / Linux x64 / Linux arm64
```

## Build & run

Requires a Rust toolchain (1.80+).

```bash
cargo build --release

./target/release/guobeng-music-server                    # 127.0.0.1:8000, data in ./data
./target/release/guobeng-music-server --host 0.0.0.0     # listen externally
./target/release/guobeng-music-server --port 9000
./target/release/guobeng-music-server --base-dir /music --data-dir /music/data
./target/release/guobeng-music-server --init-only        # create the DB and root user, then exit
```

On Windows you can also double-click `build_user.bat`; it asks for a target platform
(Windows / Linux / macOS) and architecture (x86_64 / arm64), and tells you exactly what is missing
if a cross toolchain is not installed.

First launch creates `data/music.db` and a root account, writes a random password to
`data/root_password.txt` and prints it to the terminal. root is not restricted by login origin;
create an admin account for daily use instead of staying on root.

**Cloud builds**: `.github/workflows/build.yml` produces all three platform binaries on GitHub runners,
so no cross toolchain is needed locally. Trigger it from Actions → build → Run workflow, or push a `v*` tag.

| Artifact | Platform | Notes |
|---|---|---|
| `guobeng-music-server-windows-x86_64.exe` | Windows x64 | Static CRT, no VCRUNTIME needed |
| `guobeng-music-server-linux-x86_64` | Linux x64 | Fully static musl, no glibc dependency |
| `guobeng-music-server-linux-arm64` | Linux arm64 | Same, cross-compiled |

Both Linux artifacts are fully static: CentOS 7, Ubuntu 18.04, Alpine and Android's Termux all run them
with no extra packages. The trade-off is that you do not get the distro's glibc security updates —
when libc gets patched, rebuild. On Android, run from `$HOME` (`/sdcard` is mounted noexec).

### Configuration

`config.json` lives in the working directory (or wherever `--base-dir` points) and is created on first launch:

```json
{
  "allow_registration": true,
  "allow_user_audio_upload": true,
  "backend_port": 8000,
  "host": "127.0.0.1"
}
```

- Both `allow_*` flags take effect **immediately**, no restart. root / admin bypass the upload flag
- `backend_port` and `host` are read at startup only; restart after changing them
- `host` must be `0.0.0.0` to accept external connections; the default only serves localhost
- Port precedence: `--port` > `MUSIC_SERVER_PORT` > `config.json` > 8000
- Host precedence: `--host` > `MUSIC_SERVER_HOST` > `config.json` > `127.0.0.1`
- Missing keys are written back with defaults; existing values are never overwritten, so a single
  `--port 9000` run does not make 9000 the new default

Other environment variables: `MUSIC_SERVER_SECRET_KEY`, `MUSIC_SERVER_ADMIN_ONLY_CREATE`,
`MUSIC_SERVER_CORS_ORIGINS`, `MUSIC_SERVER_DEBUG`, `MUSIC_SERVER_DATA_DIR`,
`MUSIC_SERVER_ENABLE_DOCS`.

## Frontend integration

**Full contract: [`docs/API.md`](docs/API.md)** — request/response shapes, permissions and error codes
for every endpoint. Once the server runs, `http://127.0.0.1:8000/docs` serves the contract entry point;
the full contract lives in `docs/API.md`.

Three things to internalize before writing any client code:

**1. JSON responses use the same envelope**

```json
{ "code": 0, "message": "ok", "data": { } }
```

`code` is the HTTP status; 0 means success. Payload always lives under `data` — do not look for fields
at the response root.

**2. Auth is Bearer, and refresh rotates**

```bash
# Log in and get a token pair
curl -X POST http://127.0.0.1:8000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"account":"root","password":"..."}'

# Then attach the access token to every request
curl http://127.0.0.1:8000/api/v1/albums \
  -H "Authorization: Bearer <access_token>"
```

Access tokens expire after 30 minutes. Exchange a refresh token for a new pair — **the old refresh token
is invalidated immediately**, so never retry a failed refresh with the old one; send the user to login instead.
Only 401 means "logged out". 403 is a permission problem and must not clear the session.

**3. 422 tells you which field failed**

```json
{
  "code": 422,
  "message": "request validation failed",
  "data": { "errors": [{ "field": "account", "message": "required field is missing" }] }
}
```

`field` maps directly onto your form fields, so you can highlight each one.

**Player integration**: audio streams from `/media/*` with Range support. Hand the URL to `<audio>`,
`AVPlayer` or ExoPlayer — no segmentation work required. Report progress with `POST /history`, and restore
state on the album page with `GET /history/album/{id}/last`.

## Icon copyright

The project icon `assets/icon.png` is **copyright of the original artist, 無题**.

- Original artwork: <https://www.pixiv.net/artworks/110598995>
- The character is **Scaramouche** from *Genshin Impact*; the character is © HoYoverse
- Scope of use: project identity and documentation only, non-commercial, no modifications made
- This project claims no rights to the image; all rights remain with the original artist

**If you are the rights holder and object to this use**, open an issue titled "Icon copyright"
with your identity and request — we will remove `assets/icon.png` and every reference to it promptly.

## License

Released under the **MIT** license — full text in [LICENSE](LICENSE). Anyone is free to
use, modify, redistribute and sell this software without asking for permission; the only
requirement is keeping the copyright and permission notices. No warranty of any kind.

The project icon `assets/icon.png` is **not covered by MIT**; all rights belong to the
original artist, see the copyright section above.
