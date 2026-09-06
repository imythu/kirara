---
version: 1
slug: "frontend-src-components-webdav-sync-panel-tsx"
primary_target: "frontend/src/components/webdav-sync-panel.tsx"
related_targets: ["frontend/src/pages/sites-page.tsx"]
---

# PTD Cookie 自动同步

Mode: Operate. Scope: WebDAV receive configuration/results and the two mode buttons in the existing backup dialog. Preserve shared controls, global styles and the incumbent design system. This extension inherits the collection index form from `frontend-src-app-tsx.md`; it introduces no new world, assets or tokens.

## Direction contract

THESIS: Legible PT configuration and automatic sync.
OWN-WORLD: Incumbent cream, vermilion and charcoal, with quiet bordered surfaces and shared controls per DESIGN.md.
STORY: Configure once, copy PTD connection settings, inspect automatic results.
FIRST VIEWPORT: Expose enable, username, update-versus-skip policy, automatic site creation and save. Configuration precedes connection instructions and history; narrow layouts stack fields and wrap actions. The 390×1000 regression view includes the save action; shorter viewports retain dialog scrolling.
FORM: Incumbent collection index; seed d26f3862. The existing backup dialog contains two explicit mode buttons, with primary selection and outline alternative.
FINISH: Reviewer verdict ship after making mobile save visible and placing retry errors beside their affected records. Parent-reported browser regression passed at both widths. Evidence: `.impeccable/review/webdav-sync/{1440,390}-{settings,results,retry-error}.png`.

## Implemented surface behavior

- Shared inputs and selects carry the settings; checkbox labels remain readable touch targets. The receiver shares the main Web port; no separate listener fields are shown.
- The primary save action precedes connection instructions and results. Generating a replacement password also saves current settings, with explanatory copy and a quieter outline action.
- Generated passwords are shown only in the current panel. Address/password copy controls have accessible names; the address uses the current Web origin and /dav/ptd/. Desktop IPC mode explains that a Web server is required.
- Service state, received time and record processing status have explicit text. Upload success means received; processing outcomes appear in the history.
- Thin dividers separate records. Counts summarize results, skip reasons expand in place, and eligible failed records carry their own retry action and retry error.

## Boundaries

Keep this configuration-to-connection-to-history composition scoped to this receive panel. The one-time 16px panel heading is a local decision, not new typography or layout tokens. Do not infer changes to application authentication, shared dialog behavior, fonts, artwork or the broader backup workflow.

Same-port revision: browser regression passed at both widths; earlier reviewer screenshots record the initial independent-port layout.
