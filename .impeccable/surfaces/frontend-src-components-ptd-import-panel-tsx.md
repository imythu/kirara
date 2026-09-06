---
version: 1
slug: "frontend-src-components-ptd-import-panel-tsx"
primary_target: "frontend/src/components/ptd-import-panel.tsx"
related_targets: ["frontend/src/pages/sites-page.tsx"]
---

# PTD 配置导入

Mode: Operate. Scope: the manual PTD import panel, its site-management entry and selector in the existing backup dialog. Preserve the incumbent collection index, shared controls and global styles. This extension introduces no new visual world, assets or tokens.

## Direction contract

THESIS: Import site Cookies with explicit choices and a readable outcome.
OWN-WORLD: Incumbent cream, vermilion and charcoal, quiet borders and shared controls per DESIGN.md.
STORY: Select a PTD backup, choose how existing and new sites are handled, import, then inspect the result. Export instructions support users who still need a file.
FIRST VIEWPORT: File selection, duplicate policy, automatic site creation and the import action precede results and export instructions. At the reviewed 1440 px and 390 px widths these controls fit in the first viewport; shorter screens retain the existing dialog scroll area.
FORM: Existing backup dialog with explicit primary/outline panel selectors that wrap on narrow screens; a single-column form uses thin dividers for results and instructions.
FINISH: Reviewer verdict ship, with no material finish defect. Evidence: `.impeccable/review/ptd-import/review.md` and `.impeccable/review/ptd-import/{1440,390}-{settings,result,guide}.png`. The reviewer inspected all six captures; guide captures show the dialog scrolled. Mocked browser and backend checks are parent-reported evidence, not independent live PTD interoperability verification.

## Implemented surface behavior

- Accept a nonempty, unencrypted PTD backup ZIP or extracted cookies.json up to 8 MiB. Explain near file selection that this imports sites and Cookies only, not downloaders, tasks or plugin preferences.
- Offer updating existing Cookies while preserving other configuration, or skipping existing sites; allow automatic creation of recognized new sites. Matching and processing share the transactional WebDAV importer. Manual import does not depend on receiver settings or enablement and supports desktop IPC JSON requests.
- Associate file and policy labels with their controls. Group settings in a named fieldset, disable settings and the action while importing, and show progress in the action. Keep errors beside the import action with an alert role.
- After import, show explicit created, updated, unchanged and skipped counts in a status region, with expandable skipped reasons. Refresh the site list after success. A new file selection clears previous feedback.
- Keep the PTD export guide after the action and results. It names the unencrypted backup setting, local export and site Cookies selection, plus preserving and restoring an existing encryption key. Explain repeated-import behavior and skipped ambiguous, expired or inapplicable Cookies without claiming full PTD restoration.

## Boundaries

The file-to-action-to-result-to-guide order and local 16px panel heading belong to this surface. Preserve shared dialog scrolling, mobile dock clearance and existing selector semantics. No design-system token or sidecar change is required. Do not broaden this feature into whole-configuration restoration, receiver configuration, management authentication or a new backup system.
