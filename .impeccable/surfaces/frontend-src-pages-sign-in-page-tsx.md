---
version: 1
slug: "frontend-src-pages-sign-in-page-tsx"
primary_target: "frontend/src/pages/sign-in-page.tsx"
related_targets: ["frontend/src/components/sign-in-create-panel.tsx"]
---

# 自动签到任务创建

Mode: Operate. Scope: production integration of the approved single/batch creation demo, with one sign-in task per site. Inherit DESIGN.md's cream, charcoal and vermilion palette, shared controls and typography; introduce no global tokens or assets.

## Direction contract

THESIS: Choose eligible sites, review execution settings, and create independent tasks with an accurate outcome.
OWN-WORLD: Existing bordered cream panels, charcoal text, vermilion primary actions and jade availability feedback.
STORY: Open creation, select sites, review settings, submit, then inspect created tasks or retry failures.
FIRST VIEWPORT: Single and batch actions open an inline editor before the task/record tabs; focus moves to site search. Available sites sort first in a vertically scrolling list capped at 350px.
FORM: Site selection and execution settings occupy two columns at the shared lg breakpoint and stack below it. Dividers, shared inputs and a wrapping footer preserve the incumbent interface.
FINISH: Parent-reported reviewer disposition: ship, with no material findings. This documentation pass inspected implementation source; it did not repeat browser review.

## Implemented surface behavior

- Single creation uses radios and a site-derived editable task name. Batch creation uses checkboxes and select-all for available sites matching the current search; hidden selections remain in memory while the panel stays open.
- Existing tasks, including paused tasks, make their sites unavailable. Missing login credentials, missing browser configuration and unadapted sites have explicit reasons. Eligible sites inherit their real sign-in profiles; unadapted sites expose a manual action opening the existing form.
- The execution selector defaults to 8 and retains the existing 6/8/12/16/20/24 choices. Selection and setting changes update the preview and action count. Empty selection disables creation; single creation requires a name.
- Submission sends sequential real POST requests to `/api/sign-in-tasks`. Controls and dismissal are disabled during submission. There is no new backend batch endpoint.
- Completion reports actual successful and failed request counts, refreshes tasks, and keeps the panel open. Successful sites become unavailable; failures retain selection and show site-specific errors, so retry submits only remaining eligible selections. Selection and attempt feedback are panel memory, while created tasks persist on the server.
- Closing returns focus to the originating action. Labeled native selection controls, an indeterminate select-all state, live counts/status and inline error alerts communicate progress and outcomes.
- The one-task-per-site requirement also constrains the existing create/edit form. Parent-reported backend integration enforces unique `site_id` on create/update; legacy duplicate configurations are archived and their histories reassigned to the earliest task.

## Boundaries

Reference implementation: `frontend/src/components/sign-in-create-panel.tsx` and `frontend/src/pages/sign-in-page.tsx`. The approved static demo remains a separate artifact; its local colors and breakpoint are not production design authority. DESIGN.md and the global sidecar remain unchanged.

Do not canonize the inherited interval wording as rolling scheduling semantics: the existing hour-field cron expression resets daily, so the 16/20 choices differ from true rolling intervals. This is a parent-reported nonblocking inherited limitation, not a new scheduling contract.
