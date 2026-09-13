# cURL import verification

- Scope: scheduled HTTP task cURL (Bash) import; existing lavender Operate UI.
- Browser: Chromium desktop 1440 × 1050 and mobile 390 × 844; desktop.png and mobile.png capture the parsed import dialog with details revealed. Mobile uses viewport capture for the fixed modal.
- Verified against real API: multiline JSON/Cookie import, explicit full-value disclosure, no changes before Apply, edit invalidates parsed result, unsupported TLS option error, apply preserves task name and proxy preference. No page errors or horizontal overflow.
- Persisted a paused temporary task, loaded its config before import, imported new Cookie/JSON, saved and fetched back. Verified URL/auth/body and preservation of interval, enabled flag, proxy and expected status. Temporary task deleted afterward; no request execution.
- Arbitrary Host/Origin frontend and API proxy returned 200; Access-Control-Allow-Origin: *.
- Static detector: no findings on changed TSX targets.
