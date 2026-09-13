---
version: 1
slug: "frontend-src-pages-scheduled-tasks-page"
primary_target: "frontend/src/pages/scheduled-tasks-page.tsx"
related_targets: ["frontend/src/App.tsx", "frontend/src/components/scheduled-http-panel.tsx"]
---

# 定时任务
Mode: Operate. Extend the existing 云母 automation workspace, code-led. Preserve the lavender/plum palette, shared controls, mobile navigation, and API/desktop transport.

## Direction contract
Purpose: configure recurring HTTP requests and inspect execution outcomes.
First viewport: compact task toolbar, explicit empty state or searchable task list with schedule, next execution, latest result and labeled actions. New/edit replaces the list with an inline workbench; request configuration occupies the wider left column and execution timing the right. At narrow widths the schedule follows the request configuration in document order.
Interaction: simple readable intervals by default, custom units and an optional five-field Cron mode with fixed UTC offset and three upcoming executions. Request method/address lead; query, headers, authentication, body and final request preview are selectable sections. Schedule mode labels are 指定间隔 and 高级CRON表达式. Advanced request options disclose progressively.
States: loading/error/retry, empty/search-empty, enabled/paused/running/success/failed/interrupted, save feedback, busy actions, execution-history and destructive-delete dialogs. Stored request configuration is preserved by default when editing schedules. An explicit 载入已保存配置 action loads the saved request for inspection and modification; authentication values start hidden and can be revealed deliberately.
Quality bar: no horizontal overflow at 390px; visible keyboard focus; field labels and local feedback; actual durable execution rather than mock success; secret-free history.

## Implemented surface

Source: `frontend/src/pages/scheduled-tasks-page.tsx`. This is a code-led local extension; no visual comps or new global tokens were introduced. The existing [DESIGN.md](../../DESIGN.md) remains the visual authority, particularly its Layout, shared Buttons / Inputs / Dialogs, Task Accent Rule and Bordered Surface Rule.

- The list uses one bordered near-white container with divided records, task/enabled counts and a name/host search. Each record carries a textual enabled state and result badge, frequency, next execution, last-result message and wrapping execution/pause/edit/history actions. Empty and search-empty messages offer distinct next steps.
- The inline editor replaces the list. At the large breakpoint its request column takes the remaining width beside a 320px schedule column; below it, the schedule follows request configuration. Inner padding increases from 16px to 24px at the small breakpoint. Method/address and name/type field groups also stack on narrow screens. The shell's existing mobile dock clearance applies to the save footer.
- Shared `Button`, `Input`, `Select` and `Dialog` components retain incumbent geometry and focus behavior. Containers use `colors.card`, `colors.border` and `rounded.2xl`; controls reuse primary/outline/destructive variants. Supporting copy uses `colors.muted`; request section selection uses `colors.secondary` and `colors.secondary-foreground`. Success uses the existing accent/foreground pair, failures use destructive text on a destructive tint, and neutral/running states use secondary colors. Every result has a text label. These are surface applications of existing tokens, not additions to the global system.
- Request subsections are wrapping, labeled pressed-state buttons. Query/header pairs have individually named controls; header values and secret authentication values are masked by default. Authentication supports none, Bearer, Basic, API Key in a header or query, and Cookie, with a labeled final authentication block and show/hide control. Advanced request settings use disclosure. Editing initially shows the safe request summary and a labeled load action, preserving the saved request until loaded for editing.
- Schedule preview appears beside its settings with live feedback and browser-local display times. Loading, preview, save, history and delete feedback have explicit status/error text. Saving disables the form and actions; opening the editor focuses its name field, and closing returns focus to the main create action. History and deletion use shared dialogs, with the deletion consequence explained before confirmation.

## Finish evidence and remaining limits

The independent [finish review](../review/scheduled-tasks/finish-review.md) records **ship**. Its evidence set is [desktop editor](../review/scheduled-tasks/desktop.png), [390px editor](../review/scheduled-tasks/mobile.png), [desktop list](../review/scheduled-tasks/list-desktop.png), [390px list](../review/scheduled-tasks/list-mobile.png) and [execution history](../review/scheduled-tasks/history.png). The reviewer found no material interface, usability or accessibility blocker and reported no visible narrow-screen clipping. [Detector output](../review/scheduled-tasks/detector.json) is an empty array.

The review records implementation-agent browser checks for creation, real HTTP execution, pause, preserved-request Cron editing, manual execution while paused, history, deletion, console cleanliness, overflow and mobile final-action clearance. These are attributed browser results; screenshot/source review alone does not establish scheduler or backend correctness. Final build and scheduling validation belong to the primary implementation report. Dark mode, exhaustive assistive-technology operation, large datasets and every invalid-input state were not separately exercised in the finish review.

Nonblocking follow-up: list action failures currently appear in the page-level notice. For long lists, move this feedback beside the affected record and retry action to more closely follow DESIGN.md's Local Feedback Rule. Save and delete errors already remain beside their relevant controls.


## HTTP configuration expansion (2026-09-13)

The user corrections extend the existing surface, with no new global design tokens. The execution mode selector uses **指定间隔** and **高级CRON表达式**. Authentication values can be deliberately inspected in their final header/query form, including UTF-8/Base64 Basic encoding. Saved credentials and file payloads can be reloaded through an explicit configuration action; the previous write-only contract is superseded.

The body selector supports none, JSON, plain text, XML, HTML, URL-encoded fields, multipart text/file fields, binary file and custom raw Content-Type. Repeated form names are supported; file controls state persistence and the 256 KiB content limit. Multipart rows use dividers and shared controls, stack on mobile, and keep per-file loading/error feedback local. GET/HEAD expose explanatory copy and disable body-type selection. A separate request-preview section generates the merged request without sending it, starts masked, and explains that multipart displays a summary and regenerates its boundary at execution.

The independent [HTTP expansion finish review](../review/scheduled-http-v2/finish-review.md) records **ship**, with [desktop authentication](../review/scheduled-http-v2/auth-desktop.png), [mobile authentication](../review/scheduled-http-v2/auth-mobile.png), [desktop multipart](../review/scheduled-http-v2/body-desktop.png), [mobile multipart](../review/scheduled-http-v2/body-mobile.png), and [request preview](../review/scheduled-http-v2/preview-desktop.png). The desktop evidence captures the first viewport of an internally scrolling workspace; mobile evidence captures the full document. The reviewer checked source and these images, while live interaction, wire tests and final build validation remain attributed to the implementation report.

## HTTP delivery modes (2026-09-13)

`HttpDeliveryEditor` in `frontend/src/components/scheduled-http-panel.tsx`, rendered by `frontend/src/pages/scheduled-tasks-page.tsx`, places a labeled shared select before advanced request options. HTTP 客户端 exposes 使用全局代理发送; 浏览器（Browserless） exposes 连接浏览器时使用全局代理. These are independent persisted booleans, defaulting to false. Supporting copy identifies the existing Browserless configuration location, /function support, and the distinction between the application's connection proxy and the browser service's externally configured target-site egress. README.md records the same behavior. The extension uses existing tokens and controls and adds no global design rule.

The independent [delivery finish review](../review/scheduled-delivery/finish-review.md) records **ship**, supported by [desktop](../review/scheduled-delivery/desktop.png), [mobile](../review/scheduled-delivery/mobile.png), source inspection and an empty [detector result](../review/scheduled-delivery/detector.json). It identifies a small incumbent request-introduction copy mismatch for the primary agent to correct; live control, overflow, backend and release results remain attributed to the implementation report.
