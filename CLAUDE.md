# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

云母 (Kirara) is a PT (private tracker) management tool: TMDB-driven auto subscriptions, multi-site search, qBittorrent downloads, RSS downloading, brush (刷流) tasks, auto sign-in, site stats, scheduled HTTP tasks, and PTD cookie import/WebDAV sync. Rust (axum + SQLite) backend, React/Vite frontend, Tauri desktop shell. UI text, docs and most comments are in Chinese.

## Web dev server rules (from AGENTS.md — mandatory)

- Manage dev services only via `./dev.sh start|stop|restart|status` from the repo root (wraps `scripts/dev.py`; manages both backend and frontend). `KIRARA_DATA_DIR=/path ./dev.sh start` picks a data dir (default `data/`); restarts reuse it. Logs: `.dev/frontend.log`, `.dev/backend.log`.
- The web UI always listens on `0.0.0.0:1234`; the backend on `127.0.0.1:3000`, reached through the Vite proxy (`/api`, `/dav/ptd`).
- Port 1234 must accept any origin. Both `server` and `preview` in `frontend/vite.config.ts` must keep `host: "0.0.0.0"`, `allowedHosts: true`, `cors: true`, `strictPort: true`. Never add Host/Origin allowlists. If the port is busy, inspect the existing service; don't switch ports.
- Frontend is started from the repo root with `npm --prefix frontend run dev` (preview: `npm --prefix frontend run preview`).
- After starting, verify the page and the `/api` proxy work, and check that a custom Host header and a cross-origin Origin are not blocked. Then give the user `http://<machine address>:1234` (`http://localhost:1234` locally).
- These rules cover the web UI only. Tauri desktop dev uses its own configured address and port.

## Commands

```bash
# Frontend (must be built before building the Rust crate: frontend/dist is embedded via rust-embed)
npm --prefix frontend ci
npm --prefix frontend run build
npm --prefix frontend run check:rss              # scoped tsc checks
npm --prefix frontend run check:scheduled-tasks

# Backend
cargo run                                         # web service, default 127.0.0.1:3000 (-H/-p/-d or KIRARA_HOST/PORT/DATA_DIR)
cargo build --release
cargo test --locked --package kirara --lib --tests   # what CI runs
cargo test --package kirara --lib search::           # one module
cargo test --package kirara --lib <test_name>        # one test
cargo test --test rss_downloader                     # one integration test file (tests/)
cargo test --locked --package kirara-desktop --tests

# search-encoder crate (embedded semantic search model + precomputed site vectors)
cargo test -p search-encoder --release
cargo run -p search-encoder --release --example assets -- verify

# Desktop (Tauri 2)
npm exec --prefix frontend -- tauri dev
npm exec --prefix frontend -- tauri build --bundles nsis   # or dmg
```

Frontend browser tests in `frontend/tests/*.browser.cjs` are standalone Playwright scripts. They mock the API and run against a running Vite server, e.g. `PLAYWRIGHT_MODULE=/path/to/playwright RSS_TEST_URL=http://127.0.0.1:4189 node frontend/tests/rss.browser.cjs`. See each file's header for its env vars.

## Architecture

**One library, two entry points.** `src/lib.rs` exposes `kirara::start(ServerOptions) -> ServerHandle`. `src/main.rs` (CLI, args in `src/cli.rs`) and `src-tauri/src/main.rs` (desktop) both call it. `start` runs the server on its own thread and Tokio runtime so shutdown cancels every detached descendant task. `run()` in `lib.rs` is the composition root: it opens the DB, builds the shared services, spawns every scheduler, and serves axum until cancelled. Shutdown then stops the schedulers and releases media/RSS DB leases.

**Desktop transport.** The Tauri app starts the same server on a private endpoint (`src/listener.rs`: `ListenEndpoint` supports TCP or local sockets). The frontend doesn't use HTTP directly. `frontend/src/lib/api.ts` switches to `desktopFetch` (`frontend/src/lib/desktop.ts`), which goes through Tauri IPC to `src-tauri/src/bridge.rs`, and that proxies to the backend (`sse.rs` handles the log stream). New API endpoints therefore work on desktop automatically, as long as they go through `api.ts`.

**Backend layout (`src/`).**
- `web.rs` + `web/`: one large axum router under `/api/...`, plus SPA asset serving from the embedded `frontend/dist`. Feature routers (e.g. `web/rss.rs`, `web/search.rs`, `web/scheduled_task.rs`) are nested in.
- `db.rs` + `db/`: a single SQLite `Database` (rusqlite, blocking calls behind async wrappers). The schema is created and migrated in place inside `Database::open` with `CREATE TABLE IF NOT EXISTS` / conditional `ALTER TABLE` and one-off migration functions. There is no migrations directory, so schema changes go there. New installs create `kirara.db`; legacy `rflush.db` is still honored.
- Background workers, each started in `lib.rs::run`: `media/scheduler` (TMDB subscriptions → search → download queue), `rss_download/scheduler`, `brush/scheduler`, `sign_in/scheduler`, `site_stats`, `collector` (downloader snapshots feeding `stats`), `tag_rule`, `torrent_watcher`, `relocation`, `monitor`, `webdav` (PTD cookie receiver on the same port at `/dav/ptd/`).
- Site integration has two layers. `site/` handles account/user stats per site type (NexusPHP, M-Team, Unit3D, Gazelle…), driven partly by declarative rules in `site/rules.rs` and `site/ptd_rules_generated.rs`. `indexer/` handles torrent search per site type, pooled in `indexer/pool.rs`. `search/` does result normalization and semantic matching using `crates/search-encoder`.
- `downloader/`: qBittorrent only, behind a client pool (`DownloaderClientPool`).
- `net/`: shared HTTP client factory with proxy support and rate limiting.
- `sign_in/`: per-site signers (`sign_in/signers/`), including Cloudflare/Turnstile handling via a remote browser (Lightpanda/Browserless over CDP).
- Media and RSS downloads use persistent job queues with leases, retries, dedup and reconciliation against qBittorrent state. Leases must be released on shutdown, and `lib.rs` already does this.

**PTD site rules.** Site parsing rules are distilled from PT-depiler definitions by `tools/gen_ptd_site_rules.py`. The reviewable snapshots are `tools/ptd_site_rules.json/.rs`, and the output is merged into `src/site/rules.rs` / `src/ptd_site_catalog.rs`. See `doc/ptd-site-rules.md` for the workflow.

**Frontend (`frontend/src`).** React 19 + Tailwind + shadcn-style components (`components/`), one file per page in `pages/`, shared API types in `types.ts`, API client in `lib/api.ts` (RSS uses `lib/rss-api.ts`). Built-in user docs live in `frontend/public/docs/`. Visual design tokens and tone are specified in `DESIGN.md`, and product scope/constraints in `PRODUCT.md`. Read both before UI work.

## Product constraints to preserve

- Saved credentials (cookies, API keys, passkeys, downloader passwords, TMDB tokens) are never returned by config APIs. On update, an empty field keeps the stored value; only an explicit clear deletes it.
- There is no built-in auth. The CLI binds `127.0.0.1` by default, and the WebDAV receiver password must not be described as UI login.
- At most one sign-in task per site, and paused tasks count toward that limit (enforced in the DB).
- Releases are cut by CI (`chore: release x.y.z [skip ci]` commits). Versions live in `Cargo.toml` and `frontend/package.json`.
