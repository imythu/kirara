# Release 2.4.9 verification

- Full backend library suite: 652 passed, 2 ignored. The ignored live-browser test was separately run against a local Chromium-backed Browserless function harness and passed.
- Local Chromium exercise: exact Cookie and binary bytes, URL-encoded fields, multipart files/boundary, HTTP method including HEAD, browser User-Agent, redirect on/off. No external site requests were used.
- Proxy routing regression test confirms direct HTTP target proxy and Browserless service-connection proxy are independent, off bypasses proxy, and missing required settings fail clearly.
- Integration found a startup write race in scheduled-task recovery. Recovery now runs immediately after database initialization, before any schedulers start, using an immediate transaction. RSS integration (1) and server lifecycle tests (6) passed afterward.
- Scheduled-task TypeScript check and production frontend build passed. Browser delivery controls preserve independent switches and fit desktop/mobile without horizontal overflow; no page errors.
- UI detector: empty findings. Independent scoped review: ship, with request-introduction copy corrected to remain valid in both modes.
- All package versions and root lock entries synchronized to 2.4.9; release notes added.
- Whole-repository formatting check has pre-existing differences in unrelated downloader/test sources; changed Rust sources were formatted. Whole-project TypeScript baseline limitations are recorded in earlier scheduled-task verification.
- Browserless /function support is required. Its independent remote-browser networking configuration determines target egress; this task's browser proxy switch affects only the service connection. The Chromium check validates the shipped function program locally, not a user's remote provider/account.
