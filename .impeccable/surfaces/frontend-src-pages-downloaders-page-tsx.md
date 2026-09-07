---
version: 1
slug: "frontend-src-pages-downloaders-page-tsx"
primary_target: "frontend/src/pages/downloaders-page.tsx"
---

# 下载器保存路径分析：分类与标签

Mode: Operate. Scope: category/tag filtering before save-path analysis, retaining expanded torrent metadata and directory search. Inherit DESIGN.md's cream, charcoal and vermilion palette, typography, bordered panels and shared controls.

## Direction contract

THESIS: Select categories and tags before calculating directory totals, then inspect the matching torrents in their save-path groups.
OWN-WORLD: Existing bordered cream panels, charcoal text, muted metadata labels and vermilion progress indicators.
STORY: Open save-path analysis, select categories/tags, start the calculation, then search and expand the resulting directory groups.
FORM: Searchable multi-select dropdowns and the start button sit above results, stacking on narrow screens. Expanded rows retain the labeled 分类 and 标签 middle column at lg; smaller screens stack identity, metadata and progress. Long metadata wraps within its column.
FINISH: Ship. Review captures: `1440-filtered.png`, `1440-multi-select.png`, `390-filtered.png` and `390-multi-select.png` under `.impeccable/review/downloaders-filters/`. The review found no material frontend issues; subsequent build and backend checks passed.

## Implemented surface behavior

- Category and tag dropdowns support multiple selections: OR within either group, AND across groups. An empty selection means all values; 未分类 and 无标签 are explicit options.
- Opening analysis preloads options used by the downloader's torrents. Loading and error states disable selection and calculation; failures offer 重试加载.
- Changing or clearing filters clears prior results and their timestamp. 开始统计 fetches fresh torrents, including incomplete torrents, applies the selected filters, then groups by save path.
- The shared Select accepts optional multiple selection while preserving existing single-selection usage.
- Category and tags use a definition list with explicit Chinese labels. Empty categories show 未分类; empty tags show 无标签. Comma-separated tags are trimmed, empty entries removed and visible values separated by a middle dot.
- Search matches directory path, torrent name, hash, category or tags, case-insensitively. A matching torrent keeps its entire filtered directory group visible.
- Search placeholder and accessible label mention paths, torrents, tags and categories.
- The read-only torrent API normalizes remaining bytes from qBittorrent's `amount_left`, falling back to `completed` or `progress` when needed. It returns completed selected-file bytes and an incomplete flag; metadata-only torrents can be incomplete with no known remaining bytes. Analysis never derives these from cumulative `downloaded` traffic.

## Scope and validation

The corrected scope is pre-analysis category/tag filtering, with existing metadata and search retained. Frontend build, desktop/mobile filter and shared-select browser regressions, downloader unit tests and the torrent-list/space-statistics integration test passed. Full TypeScript checking still reports errors in unrelated existing files. Global DESIGN.md and its sidecar remain unchanged.
