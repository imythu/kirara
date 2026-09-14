# RSS redesign documentation handoff

Updated persistence for the user-selected integrated subscription workflow. This is an authorized merge of RSS-specific truth into the current system, not a replacement of its established identity.

- `PRODUCT.md`: RSS capability now describes one source/conditions/destination subscription, atomic source/exclusive-rule save, whole-subscription control, bounded unsaved preview and secondary advanced management; existing baseline and delivery constraints remain.
- `DESIGN.md`: refreshed only the RSS component paragraph for the three-step form, two primary tasks, disclosed settings and desktop/mobile summary placement. Existing lavender/plum tokens, typography, art and global rules remain authoritative.
- `.impeccable/design.json`: regenerated the existing extension sidecar with a fresh timestamp; no token, snippet or global narrative change was needed because the updated prose describes a local RSS composition.
- `.impeccable/surfaces/frontend-src-pages-rss-page-tsx.md`: replaced stale cream/vermilion and separate-task instructions with the delivered direction contract, integrated workflow, responsive behavior and current evidence links.

The parent already updated `doc/rss-downloader.md` and `doc/rss-openapi.yaml`; those files were not changed in this pass. No shipping raster was introduced, so no new asset provenance record is required. Review screenshots use synthetic fixtures and are evidence, not product assets.

The finish review's only material fix was documentation persistence. The updated files resolve that finding without changing its original `fix` record or claiming a fresh independent visual verdict. The reviewer examined six captures and sampled implementation, and reported no material UI/function finding.

Validation supplied by the parent: 18 database RSS tests, 10 service RSS tests, RSS TypeScript checks, production build, and the new browser flow at 1440px and 390px passed. This pass read the current editor/page and review, checked the documentation diff and parsed the sidecar JSON. It did not rerun application tests, inspect live tracker/downloaders, or independently validate the whole application. Existing unrelated documentation drift was outside scope.
