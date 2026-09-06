---
version: 1
slug: "demos-sign-in-index-html"
primary_target: "demos/sign-in/index.html"
related_targets: ["demos/sign-in/style.css", "demos/sign-in/app.js", "demos/sign-in/server.cjs"]
---

# 自动签到交互演示

Mode: Operate. Scope: an ordinary-user-friendly, local static demonstration of single and batch sign-in task creation, served by Node on port 31235. Preserve the incumbent cream, charcoal and vermilion world in DESIGN.md. This prototype does not change the production sign-in page or establish a new global system.

## Direction contract

THESIS: Choose eligible sites, understand the execution interval, and create independent sign-in tasks with an explicit outcome.
OWN-WORLD: Existing cream workspace, ivory bordered panels, charcoal navigation, vermilion actions and the authored 云母 cat icon. PRODUCT.md supplies Chinese terminology and the requirement that automation results be understandable.
STORY: Open single or batch creation, select sites, choose an interval, review the summary, create, then inspect tasks and simulated execution records.
FIRST VIEWPORT: The task view exposes single and batch creation actions. Opening creation inserts the editor before the table and moves focus to site search. Eligible sites appear first in a list capped at 350px with vertical scrolling.
FORM: Inline two-column editor, with site choice followed by execution settings and a shared summary/footer. At 650px and below it stacks into one column. Wide task tables retain horizontal scrolling; the phone table has a 680px minimum width.
FINISH: Reviewer disposition: ship, with no blocking findings. The parent supplied the finish disposition; this documentation pass inspected source rather than independently repeating browser review. Nonblocking observations: the stacked phone form remains long, and table actions beyond the initial horizontal viewport require discovery through scrolling.

## Implemented surface behavior

- Single creation uses radio selection and a task name populated from the chosen site; batch creation uses checkboxes and select-all for eligible sites matching the current search. Search and counts support finding and reviewing selections.
- Put selectable sites first. Keep unavailable sites visible with explicit reasons: existing task, missing Cookie or unsupported site. Existing tasks prevent duplicate creation.
- Default the interval to 8 hours, with 6, 12, 16, 20 and 24 hour alternatives. Update the creation summary and action count with the selection and interval. Explain automatic matching of sign-in configuration and mark cloud-browser readiness as a demonstration state.
- Disable creation until a site is selected. Validate the single-task name, disable editor controls during simulated creation, then show how many tasks were created and their interval in a status region. New tasks start enabled; cancel and successful creation restore focus to the opener.
- Support task search, pause/enable, simulated immediate execution and the execution-record view. Show empty search results with a clear-search action. Execution progress and completion use explicit text.
- Store examples and all changes only in JavaScript memory. Reloading resets them. The page explicitly states that creation and execution are simulated and send no requests to real sites; there is no actual periodic scheduler or persistence. The Node server serves an allowlisted set of demo files and the existing icon.
- Keep native labeled controls, textual eligibility/status labels, visible focus outlines, selection announcements and reduced-motion treatment.

## Local implementation differences

These observed values belong only to this demo and must not be promoted into DESIGN.md or its sidecar: muted text is `#6c6559` instead of global `#746d60`; positive state uses `#3d6a58` instead of jade `#477564`. Local positive feedback uses `#edf2eb`, `#bbcdbf` and `#355b4b`; selected rows use `#f6eee5`, the editor footer `#f8f5ed`, row dividers `#ece7dc`, and scrollbars `#c5bcac`. Button padding is 8px by 16px and disabled opacity is 0.45. Body line height is 1.5; local section headings include 15px/650 and 18px/650, and the phone page heading is 22px. Local badges use 5–6px corners.

The static rail shrinks from 248px to 210px at 1150px and is hidden at 850px; it is illustrative navigation, without the production mobile menu or dock. Main padding, the 650px editor breakpoint, native select presentation, the 42px search field and local control hover/active fills are prototype decisions. Existing global design guidance remains authoritative for production integration.

## Evidence and boundaries

Source evidence: PRODUCT.md; DESIGN.md; `demos/sign-in/index.html` for composition, copy and semantics; `style.css` for tokens, breakpoints and scrolling; `app.js` for selection, eligibility, summaries and memory-only simulation; `server.cjs` for Node serving on port 31235. The existing icon is `frontend/public/yunmu-icon.svg`.

This is a narrow extension for review and interaction demonstration. Do not infer real site integration, saved credentials, actual scheduling, persistent tasks, working destinations in the static rail or production-ready mobile navigation. No new global tokens, sidecar entries or brand assets are established.
