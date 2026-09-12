# Surface brief: 求药 / 发药

## Scope

`frontend/src/pages/invite-profile-page.tsx` · route `#/invite-profile`

Mode: **Operate**. Visitors complete a short exchange: copy own site+UID, or paste a peer list and verify public profiles.

## Audience & job

PT users arranging invites. They already know site names and UIDs. They need a reliable copy format and a sequential lookup that never loses a row's outcome.

Constraints: Chinese UI; reuse established 云母 violet/lavender world; no new auth; lookup uses configured site cookies via `POST /api/invite-profile/lookup`.

## Direction contract

### THESIS

A two-desk exchange bench — 出示 and 核验 — not a generic form stack. Refuses icon-card scaffolds and long format lectures.

### OWN-WORLD

Pale lavender workspace, near-white bordered cards (no shadow), moonlit violet actions, jade/destructive status pills with text labels, mono only for IDs and payload text.

### STORY

I select sites I can prove, copy one pasteable payload, then either wait for the invite or verify theirs line by line with local error beside each row.

### FIRST VIEWPORT

Title + one-line purpose. Two equal cards: left dense site table with filter and sticky-ish copy action; right paste field, primary query, progress chips, ledger rows.

### FORM

Form #1 of a local extension: exchange bench with progressive ledger. Seed key: local-extend-operate.

### FINISH

unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
