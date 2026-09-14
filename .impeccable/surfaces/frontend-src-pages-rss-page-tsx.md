---
version: 1
slug: "frontend-src-pages-rss-page-tsx"
primary_target: "frontend/src/pages/rss-page.tsx"
related_targets: ["frontend/src/components/rss", "frontend/src/App.tsx"]
---

# RSS 下载

Mode: Operate. Scope: integrated subscriptions, read-only preview, feed items, download history and recovery. The user explicitly chose one subscription containing its source, filters and download settings and authorized a structural refactor. This code-led usability redesign retains the existing identity; no approved comp or new raster is required.

## Direction contract

THESIS: Reduce the separate source/rule mental model to one subscription that can be configured and saved together.
OWN-WORLD: Inherit the actual lavender workspace, dark plum navigation, violet actions, Chinese system typography and shared bordered controls from DESIGN.md.
STORY: Supply an RSS source, choose download conditions and destination, preview the selection, then save and enable one subscription.
FIRST VIEWPORT: Two primary tabs, 我的订阅 and 下载记录; an adjacent 添加订阅 action, compact state filters, and list rows exposing conditions, destination, status and recovery. The editor progresses through RSS 来源 → 下载条件 → 预览与确认.
FORM: The user-selected integrated workflow is the structure authority. Advanced shared and multiple-rule configurations remain available through a secondary 高级规则管理 link.
FINISH: Preserve the bounded finish review, documentation handoff and desktop/mobile evidence; no new identity or shipping raster is introduced.

## Built surface

- At `#/rss`, 我的订阅 and 下载记录 are the primary tasks. The hash retains active task, search, filters, pagination and opened records. List rows pair source names with download conditions and downloader/path, with explicit running, paused, incomplete and failure states.
- The integrated editor saves the source and its exclusive rule atomically. The source switch controls the whole subscription while its rule remains ready. Existing shared or multiple-rule configurations use advanced management instead of being silently rewritten.
- The three sequential steps retain the draft when moving backward. Connection frequency, extra filters and execution settings disclose progressively; configured advanced values open their corresponding sections. Saved RSS addresses are masked text and cannot be replaced; another address requires a new subscription.
- Preview reads the current source, including an unsaved source, and evaluates at most 20 sample items without saving or downloading. Results explain matches, rejections and unknowns; changed source/filter inputs mark old results stale. The confirmation step explains that the first fetch establishes a baseline and only later discoveries download automatically. Existing resources require explicit selection and backfill preview.
- At 1280px and above, a 280px summary sits alongside the editor. Below that width it appears only beneath the preview/confirmation step. Narrow list filters use two columns with search spanning both; conditions/destination stack, actions wrap, and existing mobile dock clearance remains.
- Validation identifies affected fields and retains input. Save/preview failures appear by the form actions, and record failures remain beside recovery controls. Unsaved changes prompt before leaving through the editor controls. Unchanged save retries reuse their request ID.
- Download records distinguish delivery state from sampled downloader state and progress. Detail preserves the queued rule/options snapshot; retry, reconciliation and cancellation follow actual task state. History and downloader files survive subscription archival.

This surface applies the existing Task Accent, Bordered Surface and Local Feedback rules from [DESIGN.md](../../DESIGN.md). It introduces no global tokens or identity rules. Sources: [page](../../frontend/src/pages/rss-page.tsx), [subscription editor](../../frontend/src/components/rss/subscription-editor.tsx) and [RSS components](../../frontend/src/components/rss). Product behavior and API details live in [the user guide](../../doc/rss-downloader.md) and [OpenAPI](../../doc/rss-openapi.yaml).

## Finish evidence

The [redesign review](../review/rss-redesign/finish-review.md) found no material UI or functional issue in its bounded sample; its `fix` disposition identified stale documentation. This brief, PRODUCT.md's RSS capability and DESIGN.md's local RSS prose resolve that persistence finding. The review itself remains the original independent record; this documentation pass is not a new visual verdict.

Six synthetic-fixture captures cover the list, conditions and preview at desktop/mobile widths: [desktop](../review/rss-redesign/desktop.png), [mobile](../review/rss-redesign/mobile.png), [conditions 1440](../review/rss-redesign/conditions-1440.png), [conditions 390](../review/rss-redesign/conditions-390.png), [preview 1440](../review/rss-redesign/preview-1440.png), [preview 390](../review/rss-redesign/preview-390.png). Parent-reported validation passed 18 database RSS tests, 10 service RSS tests, RSS TypeScript checks, production build and the new browser flow at both widths. The documentation pass did not rerun those checks or claim real tracker/downloader integration. See [documentation handoff](../review/rss-redesign/documentation-handoff.md).

Earlier [initial RSS evidence](../review/rss/documentation-handoff.md) and [configuration clarity evidence](../review/rss-clarity/verification.md) are historical; the integrated flow above supersedes their separate source/rule composition.

The [bounded verdict](../review/rss-redesign/finish-verdict.md) scores the sole documentation finding resolved with disposition `ship`. [Final verification](../review/rss-redesign/verification.md) adds original browser regressions and the local simulated-tracker/downloader HTTP integration test; it does not claim validation against user production services.
