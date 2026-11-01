> **This project is fully managed by AI.** Code, documentation, build scripts and repository configuration are generated and maintained by an AI agent; humans only file requests and review results.

<p align="center">
  <img src="assets/icon.png" alt="Guobeng Music Server" width="200">
</p>

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
- **Contract**: `/api/v1`, every response shaped as `{ code, message, data }`
- **Deployment**: one binary plus one `config.json`. No Python, Node, JVM or external database.

## Features

**Library**

- Full CRUD for artists, albums and tracks; albums and tracks carry cover art, descriptions and other metadata
- **Multi-artist tracks**: a track can credit several artists, displayed in the order they were entered. Collaborators are automatically merged into the album's artist page, so a feature never goes missing from the lead artist's page
- **Album visibility**: public or private. A private album returns **404** to everyone else (not 403 — existence is not leaked), and tracks, lyrics, comments, favorites and history all inherit that rule
- **Audio upload**: the server computes an MD5 for each file; re-uploading the same bytes returns 409 with the existing track ID. Up to 1GB per file, supporting mp3 / flac / wav / m4a / aac / ogg / opus
- **Track ordering**: drag to reorder; the server compacts `order_index` so no gaps are left behind
- **External audio**: or store just an `http(s)` URL and keep the bytes off your server
- **External album IDs**: `external_id` is globally unique, which makes de-duplication trivial when importing from another catalog

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
- The `root` account may only sign in from localhost (`127.0.0.1` / `::1`); its token is treated as anonymous from anywhere else
- Login throttling: 8 failures per IP + account within 5 minutes locks it for 5 minutes
- Roles: `user` / `admin` / `root`. Unauthorized access returns 403

**Operations**

- Missing keys in `config.json` are written back with their defaults, so the file always shows every option
- `allow_registration` and `allow_user_audio_upload` are **hot-reloaded** — no restart needed
- `/health` for liveness, `/docs` for a one-page API reference

## How it differs

Compared against **Navidrome**, the most common self-hosted music server and the exact opposite end of the spectrum.

| | Guobeng Music Server | Navidrome |
|---|---|---|
| Shape | Backend API only; you write the frontend | Complete product with a built-in React web UI |
| Protocol | Its own RESTful JSON contract | Subsonic / OpenSubsonic API |
| Client ecosystem | Only the clients you build | Dozens of ready-made mobile and desktop apps |
| Import | Upload via HTTP, metadata via API | Scans a local music folder and reads embedded tags |
| Transcoding | None; original files served as-is | On-the-fly ffmpeg transcoding |
| Integrations | None | Last.fm / ListenBrainz scrobbling, public share links, multiple libraries |
| Multi-artist | First-class, backed by a dedicated join table | Follows the Subsonic model, album artist first |
| Data model | Artists / albums / tracks + comments + favorite lists + play history | Subsonic-shaped, per-user starred / rating / playcount |

**Which one to pick**

- Want something that works out of the box with an existing mobile app → Navidrome.
- Want a clean backend contract with full control over the frontend, or want to embed a music library into your own product → this.

**Deliberately out of scope**: transcoding, scrobbling, a built-in player, folder scanning, Subsonic compatibility.
These are boundaries, not gaps — every feature added to the server is freedom taken away from the frontend.
The only promise here is: **keep the data and permissions correct, and document the API precisely.**

## Design notes

**The contract is the product; the code is the implementation.**
Only the shapes defined in `dto.rs` are exposed. What a database row looks like (`models.rs`) never leaks out.
Changing a table should not force a frontend change — that is the point of the split.

**One error shape, everywhere.**
Every response, including failures, is `{ code, message, data }`, where `code` is the HTTP status and 0 means success.
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
`data/root_password.txt` and prints it to the terminal. root may only sign in from localhost.

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
for every endpoint. Once the server runs, `http://127.0.0.1:8000/docs` also serves a quick reference.

Three things to internalize before writing any client code:

**1. Every response uses the same envelope**

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
- Scope of use: project identity and documentation only, non-commercial, no modifications made
- This project claims no rights to the image; all rights remain with the original artist

**If you are the rights holder and object to this use**, open an issue titled "Icon copyright"
with your identity and request — we will remove `assets/icon.png` and every reference to it promptly.

## License

Source code is released under the MIT license. The project icon is not covered by MIT;
see the copyright section above.
