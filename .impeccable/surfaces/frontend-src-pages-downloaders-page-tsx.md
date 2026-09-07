---
version: 1
slug: "frontend-src-pages-downloaders-page-tsx"
primary_target: "frontend/src/pages/downloaders-page.tsx"
---

# 下载器保存路径分析：分类与标签

Mode: Operate. Scope: a small addition to expanded torrent rows and directory search. Inherit DESIGN.md's cream, charcoal and vermilion palette, typography and shared controls; introduce no new system choices.

## Direction contract

THESIS: Inspect torrent category and tags within a directory and find the relevant directory without losing its group context.
OWN-WORLD: Existing bordered cream panels, charcoal text, muted metadata labels and vermilion progress indicators.
STORY: Open save-path analysis, search directories or torrent metadata, then expand a matching directory to inspect its torrents.
FORM: Expanded rows place labeled 分类 and 标签 in a separate middle column at the shared lg breakpoint; smaller screens stack torrent identity, metadata and progress. Long metadata wraps within its column.
FINISH: Parent-reported finish reviewer disposition: Ship, with no material findings across `after-1440.png`, `after-390.png` and `after-390-detail.png` under `.impeccable/review/downloaders/`. This documentation pass inspected source and did not repeat browser review.

## Implemented surface behavior

- Category and tags use a definition list with explicit Chinese labels. Empty categories show 未分类; empty tags show 无标签. Comma-separated tags are trimmed, empty entries removed and visible values separated by a middle dot.
- Search matches the directory path, torrent name, hash, category or tags, case-insensitively. A matching torrent keeps its entire directory group visible, including the other torrents in that group.
- Search placeholder and accessible label mention paths, torrents, tags and categories.
- The read-only torrent listing is available for save-path analysis.

## Scope and validation

The user's wording “标签/分类的列表参数” was interpreted as displayed metadata plus search support; the optional clarification received no answer. This record does not imply separate category/tag filter controls or editing support.

Parent-reported frontend build, browser checks and backend integration passed. Full TypeScript checking has unrelated existing errors. Global DESIGN.md and its sidecar remain unchanged.
