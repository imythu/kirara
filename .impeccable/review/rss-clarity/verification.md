# RSS configuration clarity verification

2026-09-11. Disposition: ready. Scope: beginner-friendly RSS configuration, contextual help, optional settings, and immutable source addresses.

The existing cream surfaces, Chinese type, shared controls, split editor/preview and mobile panel tabs are retained. Each setting explains its purpose or default. Longer explanations expand inline with keyboard and touch support. New rules collapse optional controls; configured groups open when editing. Clearing an optional value leaves its group open. Switching title matching off preserves the editable keyword draft while exclusions and resource requirements continue to apply.

Saved RSS addresses appear as masked text with no edit control. Edit and test requests use the stored address. The backend rejects different addresses with 422 and no changes to history, version or waiting jobs; normal source settings remain editable. The former address-replacement branch has been removed; existing historical data remains compatible.

Two bounded browser passes covered desktop (1440px) and phone (390px). The follow-up fixed optional groups closing when their last value was cleared and an empty backfill preview referring to an unavailable refresh action. Final review found readable field help, working keyboard disclosure, no horizontal overflow, and save errors visible beside retry actions above the mobile dock. The retained screenshots show the final build. API responses in browser checks are explicitly synthetic; server tests use local RSS and qBittorrent fixtures.

Passed:

- `npm --prefix frontend run check:rss`
- `npm --prefix frontend run build`
- `cargo test -p kirara --lib db::rss::tests -- --test-threads=2` — 17 tests
- `cargo test -p kirara --test rss_downloader` — API, delivery, address immutability and restart integration
- `rustfmt --edition 2024 --check src/db/rss.rs tests/rss_downloader.rs`
- `npx --yes @apidevtools/swagger-cli validate doc/openapi.yaml`
- `git diff --check`
- `PLAYWRIGHT_MODULE=/tmp/rflush-playwright-runtime/node_modules/playwright RSS_TEST_URL=http://127.0.0.1:4189 RSS_CAPTURE_DIR=.impeccable/review/rss-clarity node frontend/tests/rss.browser.cjs`

Browser coverage includes source creation/test/edit, no editable saved address, source edit sending no replacement URL, optional-group defaults, clearing values without collapsing, keyboard help, title-mode draft preservation, server preview, historical downloads, save failures and retry request IDs, download recovery, navigation persistence, desktop IPC and background polling. No production data was used or changed. Screenshots are verification evidence; no raster asset was added to the application.

## Unsaved source confirmation fix

The discard prompt now replaces the fixed dialog footer actions instead of appearing at the top of the scrollable form. Cancel, the close button and Escape keep the draft until explicit discard; Continue Editing retains the form and restores focus to Cancel. Confirmation focuses the non-destructive Continue Editing action. Form submission is suppressed while confirmation is displayed.

Type check, production build and the browser suite passed against port 1235. Added desktop/phone regression coverage scrolls to the bottom before cancelling, checks the prompt and both actions are fully visible, verifies focus and draft preservation, and ensures discarding performs no write. Both retained discard-confirmation screenshots were inspected. The running service serves the updated build.

## Separate RSS and site API origins

Removed the save-time requirement that a linked site and RSS URL share an origin. The request layer already scopes authentication to the configured site origin, so RSS links can use a separate domain while attribute lookups and torrent resolution use the linked API. Source URL immutability is unchanged. A local M-Team API fixture on localhost and an RSS server on 127.0.0.1 verify preview, creation, editing, scanning, free/H&R enrichment and torrent fetching. API credentials and private custom headers reach the API only; the RSS request retains browser headers without those credentials.

The separate-origin integration test and all 17 RSS database tests passed. OpenAPI validation and the updated binary build passed. The service was restarted on 0.0.0.0:1235 with its existing data directory; page and RSS API health checks returned 200.

## Linked-site recovery for expired torrent links

Linked-site help now explains that RSS links are tried first and an expired link can be recovered using the site's saved login and the same torrent ID. Longer details remain expandable, with no new settings. Download resolution now follows that order; temporary server/network failures and rate limits retain normal retry/cooldown behavior.

All 58 RSS-related library tests passed, including valid-link preference, 401/403/404/410 and invalid-body recovery, credential isolation, 503/429 handling, stable-identity RSS refresh, and separate RSS/API origins. Frontend type check, production build, and desktop/mobile browser suite passed. Source-dialog screenshots at 1440px and 390px were inspected in /tmp/rss-site-fallback; the updated hint remains readable without overflow.
