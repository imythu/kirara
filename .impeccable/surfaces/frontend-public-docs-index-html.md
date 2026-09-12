---
version: 1
slug: "frontend-public-docs-index-html"
primary_target: "frontend/public/docs/index.html"
related_targets: ["frontend/public/docs/style.css", "frontend/src/App.tsx"]
---

# 云母使用文档

Mode: Read. A static Chinese usage manual for the existing PT management product. Inherits DESIGN.md's lavender surfaces, plum text, violet links and device Chinese sans-serif stack. This addition creates no durable system change; DESIGN.md and the global sidecar remain unchanged.

## Direction contract

THESIS: Help users configure the application and follow its workflows through directly addressable, readable instructions.
OWN-WORLD: Established lavender palette, quiet borders, system fonts and the existing Kirara icon.
STORY: Open the book beside GitHub, choose a topic, read its procedure, and return to the workspace.
FIRST VIEWPORT: Branded header, contents and introduction; the mobile contents remain visible as two columns.
FORM: A desktop sticky contents rail beside a constrained reading column, becoming a single page flow at 800px and below.
FINISH: `.impeccable/review/usage-docs/finish-review.md` records a ship disposition with no material findings.

## Implemented surface behavior

- `frontend/public/docs/index.html` and its relative `style.css` form a script-free document, readable without API access. Assets are served from public files and included in the frontend build.
- The App.tsx book link sits immediately beside GitHub, uses `./docs/index.html`, opens a new tab with `noopener noreferrer`, and announces that behavior in its accessible label and title.
- Sixteen sections cover getting started, sites, PTD import/backup, Cookie synchronization, downloaders/space analysis, media subscriptions, quality, search/queue, RSS, traffic tasks, sign-in, labels, overview/statistics/logs, settings, backup/migration and common questions. PTD guidance includes the 蜂巢 workflow. OpenList and seed transfer are explicitly excluded from this manual at the user's request; this does not change product capability.
- Native fragment anchors support contents navigation and reload. The document includes a skip link, heading hierarchy, ordered procedures, semantic tables, visible focus, target-heading emphasis and a return-to-top link. Workspace return links navigate in the same tab.
- Desktop layout is capped at 1180px with a 232px sticky contents column, 64px gap and 74ch reading measure. Body text is 15px with 1.85 line height. At 800px and below the contents become a two-column grid with 44px minimum link height; at 380px and below padding and brand sizing tighten.
- Quiet lavender callouts and thin section dividers organize long-form reading. Tables have local horizontal overflow. Print CSS hides navigation, removes the column layout and discourages breaks after headings or inside rows/callouts.

## Evidence and limits

Parent-reported verification passed the frontend build, public/dist asset checks, desktop/mobile new-tab entry, anchors and reload, 320px/390px overflow checks, and API-independent reading. This documentation pass inspected source and the finish review; it did not rerun those checks. Native packaged desktop behavior has not been validated. Print output and every lower-page table were not visually reviewed.

Screenshots under `.impeccable/review/usage-docs/`: `desktop.png`, `mobile.png`, `desktop-rss.png`, `mobile-rss.png`, `entry-desktop.png`, and `entry-mobile.png`. The desktop application entry screenshot contains a loading state but establishes icon placement. Final PTD copy was source-reviewed without a new screenshot capture.

The finish review notes a nonblocking affordance mismatch: the workspace return link displays a northeast arrow despite same-tab navigation. The mobile contents' substantial first-screen height follows the chosen layout.
