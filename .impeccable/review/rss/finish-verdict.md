Scope: bounded verdict on material fix 1 from `finish-review.md`. This ship verdict covers the scored fix only, not the whole surface. The original five-section full review is preserved unchanged.

Evidence reviewed: all 24 screenshots named in `fix-verification.md` were opened at original resolution, including every 1440px and 390px failure capture and all 14 refreshed context captures. All are valid: their content matches the named state, full-page views begin at the top, and failure views intentionally show the submission position. Also read: the original review, verification record, surface contract, PRODUCT.md, DESIGN.md, craft floor, saved `detect.json` (`[]`), and approved design §§3–4; inspected the four changed RSS components. Unread: the remainder of the design document beyond sampled headings, unrelated frontend and backend implementation, browser test source and raw test logs. No context, detector, browser or tests were rerun. The verification record uses synthetic Web and desktop IPC API responses; it does not establish live PT/qBittorrent or native execution.

## verdict

1. resolved — DESIGN.md Local Feedback Rule / approved §4.4: `feed-validation-1440.png`, `feed-validation-390.png`, `feed-save-error-1440.png` and `feed-save-error-390.png` show the source error immediately above the visible save actions, with the entered fields retained and the URL validation message also beside its field; `rule-validation-1440.png`, `rule-validation-390.png`, `rule-save-error-1440.png` and `rule-save-error-390.png` show the rule error and recovery/save controls together at the form bottom, above the mobile dock, with target/options retained; `backfill-error-1440.png` and `backfill-error-390.png` show the failed submission beside the visible confirmation action while preserving the selected rule and preview. The inspected `Field` implementation supplies field error IDs, `aria-invalid` and `aria-describedby`; the verification record corroborates the feed URL/rule name associations, complete draft retention and stable retry request IDs. Failure handlers preserve state, and the rule feedback scroll avoids taking focus from an active form control.

## remaining

clear. No regression introduced by this fix batch is visible in the reviewed evidence. No additional whole-surface findings were sought.

disposition: ship
