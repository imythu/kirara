---
version: 1
slug: "frontend-src-pages-rss-page-tsx"
primary_target: "frontend/src/pages/rss-page.tsx"
related_targets: ["frontend/src/components/rss", "frontend/src/App.tsx"]
---

# RSS 下载

Mode: Operate. Scope: RSS sources, rule editor and preview, feed items, download history and recovery. The user approved implementation of `doc/rss-downloader-design.md`; its flows and existing 云母 design system settle the structure. This is a code-led addition to the current workspace, with no new identity or concept selection.

## Direction contract

THESIS: Explain every RSS decision beside the resource and the action that resolves it.
OWN-WORLD: Inherit the cream workspace, charcoal navigation, vermilion actions, Chinese system typography and shared form controls.
STORY: Validate a source, preview a rule, enable new-item downloads, inspect and recover a real delivery.
FIRST VIEWPORT: Existing rail and page heading; three task tabs, compact status filters and a readable list, with add actions beside the tabs. Source detail reveals items; the rule editor pairs settings with evidence and switches panels on phones.
FORM: The user-approved structure in the RSS design, extending the collection desk. No seed is needed for the specified structure.
FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Built surface

- At `#/rss`, 订阅源 / 下载规则 / 下载记录 share compact bordered lists. The hash retains the active tab, search, applicable filters, page and opened source, rule or task.
- Source items expand in place to show rule decisions, actual/expected values, attribute provenance and explicit unknowns. Testing and the first-fetch baseline do not create downloads; historical backfill requires selected items, a rule and a preview.
- Rule configuration and read-only preview sit side by side from 1280px; narrower views switch between them while retaining the draft. Source, matching, destination and execution sections lead to the shared save region. Changed conditions mark prior preview results stale.
- Download records distinguish delivery status from sampled downloader state, progress and collection time. Task detail preserves the queued rule/options snapshot and offers recovery actions appropriate to its state.
- Feed and backfill submission failures sit above their dialog footer actions; rule failures sit beside save/recovery controls. Specific validation messages accompany their fields with `aria-invalid` and `aria-describedby`. Failed requests retain drafts and selections; unchanged retries reuse their request IDs.

This surface applies the existing Task Accent, Bordered Surface and Local Feedback rules from [DESIGN.md](../../DESIGN.md). It introduces no global tokens or identity rules. Built source: [page](../../frontend/src/pages/rss-page.tsx) and [RSS components](../../frontend/src/components/rss).

## Finish evidence

The [full review](../review/rss/finish-review.md) identified one form-feedback fix. The [bounded verdict](../review/rss/finish-verdict.md) scores that fix resolved with disposition `ship`; it is not a new whole-surface review. [Verification](../review/rss/fix-verification.md) records passed RSS type checks, production build and synthetic 1440px/390px browser flows, including desktop IPC coverage. No shipping raster was added; screenshots are test evidence. See the [documentation handoff](../review/rss/documentation-handoff.md) for the exact scope and limits.
