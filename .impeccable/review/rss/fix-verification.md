# RSS form feedback fix verification

Status: verification complete; ready for the reviewer's verdict on the single material fix.

Scope: the one material fix in `finish-review.md`: preserve drafts, associate field validation, and keep submission failures beside visible recovery actions in FeedDialog, RuleEditor and BackfillDialog at 390px and 1440px.

The saved implementation was inspected after the environment restart. Feed and backfill errors are in dialog footers; the rule error is in the save region and scrolls that region into view when submission originated outside the form. Shared `Field` supplies `aria-invalid` and `aria-describedby` for field errors. No implementation was restarted. Impeccable context and detector were not rerun.

- `npm run check:rss`: passed.
- `npm run build`: passed (Vite 7.3.6; production RSS bundle built successfully).
- Browser runtime located at `/tmp/rflush-playwright-runtime/node_modules/playwright-core` (1.55.0); installed Chromium and Noto Sans SC are available.
- `PLAYWRIGHT_MODULE=/tmp/rflush-playwright-runtime/node_modules/playwright-core RSS_TEST_URL=http://127.0.0.1:4189 node frontend/tests/rss.browser.cjs`: passed against the production preview build.
- The temporary preview server on port 4189 was stopped after verification.
- At both widths, the browser checks confirmed that validation/server errors and recovery actions were fully inside the viewport and unobscured after submitting from the bottom, without manual scrolling or stealing input focus. Feed URL and rule name controls expose `aria-invalid=true` and the matching error IDs in `aria-describedby`.
- Draft preservation and reuse of idempotency keys after source-save, rule-save and backfill server failures passed. Existing source management, preview, recovery, hash persistence, overflow, desktop IPC and polling checks also passed.

The browser run reported:

```text
1440px: source CRUD/test, draft+idempotency recovery, server preview, history backfill, run records, delivery recovery, hash persistence and overflow passed
390px: source CRUD/test, draft+idempotency recovery, server preview, history backfill, run records, delivery recovery, hash persistence and overflow passed
Desktop IPC: real API bridge, preview without downloader, disabled rule save passed
Polling: idle 30s, active 5s, hidden pause, editable draft preservation passed
```

All 14 existing screenshot files were overwritten from the current build on 2026-09-10 at 13:37–13:38 UTC. The script added 10 failure screenshots. Every one of the 24 files was opened at original resolution and inspected: correct named surface/state, readable Chinese text, no blank/black or loading captures. The normal views begin at the document top; the error views intentionally capture the submission position at a 1000px viewport height so action visibility is evidence. No additional UI changes or rebuild were needed.

| Evidence | 1440px | 390px | Inspection |
| --- | --- | --- | --- |
| Source list | `desktop.png` | `mobile.png` | Valid |
| Source test dialog | `feed-dialog-1440.png` | `feed-dialog-390.png` | Valid |
| Items and decision details | `items-1440.png` | `items-390.png` | Valid |
| Rule configuration | `rule-config-1440.png` | `rule-config-390.png` | Valid |
| Rule preview | `rule-preview-1440.png` | `rule-preview-390.png` | Valid |
| Download task detail | `job-1440.png` | `job-390.png` | Valid |
| Download list | `downloads-1440.png` | `downloads-390.png` | Valid |
| Feed validation | `feed-validation-1440.png` | `feed-validation-390.png` | Inline error, summary and save action visible |
| Feed server failure | `feed-save-error-1440.png` | `feed-save-error-390.png` | Error and retry action visible |
| Rule validation | `rule-validation-1440.png` | `rule-validation-390.png` | Error and save actions visible above dock |
| Rule server failure | `rule-save-error-1440.png` | `rule-save-error-390.png` | Error and save actions visible above dock |
| Backfill server failure | `backfill-error-1440.png` | `backfill-error-390.png` | Error and retry action visible in footer |

All screenshot paths are relative to `.impeccable/review/rss/`. Tests use explicitly synthetic API responses; this verification makes no claim about live tracker or qBittorrent availability. Backend integration coverage remains with the parent task. The existing detector result is unchanged at `[]`; this pass did not rerun it, restart implementation, or perform another defect hunt.
