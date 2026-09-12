---
name: 云母
description: A moonlit cel-animation collection desk with a prominent Inuyasha cast.
colors:
  background: "#f5f3fa"
  foreground: "#292438"
  card: "#fefcff"
  border: "#ddd6e9"
  primary: "#7450a3"
  primary-foreground: "#ffffff"
  primary-hover: "#603c8e"
  primary-active: "#4e2f77"
  secondary: "#ece5f5"
  secondary-foreground: "#544067"
  muted: "#746780"
  accent: "#eee8f6"
  destructive: "#b63232"
  surface-container: "#ede8f3"
  night: "#292135"
  jade: "#397968"
  sidebar: "#30263e"
  sidebar-foreground: "#f6f0ff"
  sidebar-muted: "#c6b8d5"
  sidebar-active: "#7450a3"
  sidebar-active-foreground: "#ffffff"
  sidebar-hover: "#483752"
  sidebar-focus: "#d7b6ff"
typography:
  headline:
    fontFamily: "\"Noto Sans SC\", \"PingFang SC\", \"Microsoft YaHei\", ui-sans-serif, system-ui, sans-serif"
    fontSize: "24px"
    fontWeight: 600
    lineHeight: "32px"
  title:
    fontFamily: "\"Noto Sans SC\", \"PingFang SC\", \"Microsoft YaHei\", ui-sans-serif, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 700
    lineHeight: "28px"
  body:
    fontFamily: "\"Noto Sans SC\", \"PingFang SC\", \"Microsoft YaHei\", ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: "20px"
  label:
    fontFamily: "\"Noto Sans SC\", \"PingFang SC\", \"Microsoft YaHei\", ui-sans-serif, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: "16px"
rounded:
  lg: "8px"
  xl: "12px"
  2xl: "16px"
  full: "9999px"
spacing:
  1: "4px"
  2: "8px"
  3: "12px"
  4: "16px"
  5: "20px"
  6: "24px"
  8: "32px"
  10: "40px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  button-primary-hover:
    backgroundColor: "{colors.primary-hover}"
    textColor: "{colors.primary-foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  button-primary-active:
    backgroundColor: "{colors.primary-active}"
    textColor: "{colors.primary-foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  button-secondary:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.secondary-foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  button-outline:
    backgroundColor: "{colors.card}"
    textColor: "{colors.foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  button-destructive:
    backgroundColor: "{colors.destructive}"
    textColor: "{colors.primary-foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 20px"
    height: "40px"
  input:
    backgroundColor: "{colors.card}"
    textColor: "{colors.foreground}"
    rounded: "{rounded.lg}"
    padding: "8px 16px"
    height: "44px"
  card:
    backgroundColor: "{colors.card}"
    textColor: "{colors.foreground}"
    rounded: "{rounded.2xl}"
---

# Design System: 云母

## Overview

**Creative North Star: "A moonlit cel-animation collection desk"**

A moonlit cel-animation collection desk: pale lavender surfaces, a dark plum navigation rail and violet actions frame a dense Chinese PT workspace. Generated chibi Kirara and the Inuyasha cast make the anime identity prominent; borders, clear text and compact controls carry the work.

This is a record of the implemented light interface, grounded in the application shell, shared controls and operational pages. The existing Chinese system-font stack is a practical implementation fact; there is no bespoke display face or bundled font.

**Key Characteristics:**
- Pale lavender surfaces with a dark plum navigation rail.
- Flat bordered containers, compact controls and visible task states.
- A generated Kirara identity, transparent companion art and a dedicated sidebar landscape.

## Colors

Pale lavender and plum ink form the neutral field; moonlit violet is the primary action and selection accent. Frontmatter values describe reused tokens; `input` and `surface` in Tailwind alias the card color, and `ring` aliases primary.

### Primary

- **Moonlit violet** (`primary`): primary actions, selected text, caret and selection color; darker hover and active variants give buttons feedback.
- **White** (`primary-foreground`): text on solid primary and destructive buttons.

### Secondary

- **Soft lavender** (`secondary`, `secondary-foreground`): quieter filled actions.
- **Jade** (`jade`): existing positive-state details. Semantic destructive red has a separate token; some pages retain Tailwind emerald, amber, red and slate status colors. These are functional state exceptions, not additional brand accents.

### Neutral

- **Lavender workspace / near-white card** (`background`, `card`): the two principal light surfaces.
- **Plum ink / muted mauve** (`foreground`, `muted`): reading text and supporting text.
- **Lavender edge / inset lavender** (`border`, `surface-container`, `accent`): dividers, table headings, tab tracks and hover feedback.
- **Dark plum rail** (`sidebar`): navigation background with pale lavender foreground and muted lilac supporting text. Active items use `sidebar-active` and `sidebar-active-foreground`; hover uses `sidebar-hover`, focus uses `sidebar-focus`. The rail search uses an inset plum surface (`#3d304b`) and muted border (`#51425f`), retaining clear contrast against the rail. Destination text uses `#e7ddef`; the selected diamond uses `#eee0ff`.
- **Night** (`night`): translucent modal scrims.

**The Task Accent Rule.** Use violet to identify actions and selected states; retain neutral surfaces for sustained reading.

## Typography

The body font stack in frontmatter applies across the interface. Font availability follows the device; the project does not download Noto Sans SC. Labels and page headings are sans serif. Monospace is used for the log console; tables, times and explicit numeric fields use tabular numerals.

Page headings use the headline role from the small breakpoint upward; below it they are 18px semibold with a snug line height. Card titles use the title role and slightly tightened tracking. Body and control text are generally 14px; explanatory paragraphs often increase line height to 24px. Labels and metadata are generally 12px, with existing 10–11px details in dense operational UI. The brand wordmark is 24px bold with 0.12em tracking, using the device serif stack "Noto Serif SC", "Songti SC", "SimSun", serif; it is a brand-only treatment with no bundled font. These are functional sizes, not a display-font system to amplify on new screens.

## Layout

At 1024px and above, the shell is a viewport-height grid with a fixed 256px navigation column and a `minmax(0, 1fr)` content column. Navigation and content scroll independently. The workspace has 32px horizontal and 28px vertical padding, increasing horizontal padding to 40px at 1280px. Between 640px and 1023px, workspace padding is 24px. Below 640px it is 16px vertically and 12px horizontally.

Below 1024px, the rail becomes a modal menu (85vw, maximum 320px), with safe-area-aware outer padding and its own scroll region. A fixed dock offers four destinations plus the full menu. The dock is 92% wide, capped at 440px, and sits at `max(24px, env(safe-area-inset-bottom))`. Content bottom padding is `calc(110px + env(safe-area-inset-bottom, 0px))` throughout this mobile/tablet range. Preserve that clearance so final actions can scroll above the dock.

Page descriptions appear from 1024px and the clock from 1280px. Transparent companion art appears beside the header at 1280px, occupying 120px by 80px. The sidebar landscape uses a 136px-high crop and disappears at viewport heights of 850px or less, leaving more space for navigation. Media task tabs use two columns below 1024px and four above. Forms commonly gain two columns at 640px, while wide tables retain horizontal scrolling instead of squeezing all columns. Shared card header/content padding is 24px; some page and dialog bodies use 16px, increasing to 24px at 640px. Main groups use 16–24px gaps; smaller control groups use 8–12px.

## Elevation & Depth

**The Bordered Surface Rule.** Standard cards have no shadow; light surface changes and thin lavender borders separate content.

Selected rail destinations use a subtle plum shadow; its exact value is in the sidecar. This is not a universal no-shadow system. Selected task tabs and some existing statistics/settings panels retain `shadow-sm`. The dock and select popovers use `shadow-lg`; dialogs use `shadow-xl`. Exact shadow values are recorded in the sidecar. The mobile menu backdrop retains a small blur over a black 40% scrim; standard dialogs use a night 45% scrim. Do not generalize that overlay blur to reading surfaces.

Motion is limited: shared buttons transition colors in 150ms; the brand icon rotates −6 degrees on hover/focus over 180ms ease-out. Other Tailwind transition utilities generally use their 150ms defaults, and dialog close controls use 200ms. Animation-like class names on menu/select wrappers have no configured animation plugin and are not a guaranteed entrance animation. Reduced-motion CSS shortens animations/transitions to 0.01ms and turns CSS smooth scrolling off. Existing programmatic smooth log scrolling is a separate behavior, not covered by that CSS guarantee.

## Shapes

Shared buttons, input fields and navigation items have softly squared 8px corners. Table frames, tab tracks and many inline panels use 12px corners; cards, dock and dialogs use 16px. The configured `3xl` also resolves to 16px. Status pills remain fully rounded. Borders are generally 1px. Lucide outline icons are normally 16px in controls and 20px in navigation, where their stroke width is 1.7px. A small rotated square (6px) is the active rail marker.

## Components

### Buttons

Compact, firm controls: default height 40px, 8px corners, 8px by 20px padding and 14px semibold text. Primary uses violet/white, secondary uses lavender/plum, outline uses near-white/plum with a lavender border, and destructive uses red/white. Outline hover tints the background and accents the border; secondary hover uses accent. Disabled buttons have 50% opacity and ignore pointer interaction. Page-level compact 36px and touch-oriented 44px height overrides already exist.

Shared buttons and inputs use an opaque 2px primary focus ring, separated from the component by a 2px card-colored offset. The global fallback is a 2px violet outline with 3px offset. Rail items use pale lilac ring color with a dark plum offset and include a 2px offset. Some local controls, including media task tabs, still use translucent or unoffset rings; those are recorded exceptions, not a replacement focus standard.

### Inputs / Fields

Fields are 44px high with 8px corners, a near-white fill, lavender border, 16px horizontal padding and muted placeholders. Focus adds the primary border/ring; disabled fields fade to 50%. Select triggers match the field geometry and have explicit invalid-border styling. Their portaled menu is viewport-constrained, with a 6px gap, 8px viewport margin and computed maximum height up to 240px. Selected options use violet/white with a checkmark. Preserve keyboard navigation and focus restoration when extending these components.

### Cards / Containers

Cards are bordered near-white surfaces with 16px corners, clipped overflow and no default shadow. Headers have a subtle divider, 18px bold titles and 14px supporting text. Tables use a 12px frame, 44px header row, 12px semibold headings and 16px cell padding; hovered rows tint cells with accent at 60% opacity.

### Chips

Media status pills use a thin border, 12px semibold text and 4px by 10px padding. Neutral is inset paper/muted; positive uses a primary tint with ink text; negative uses a destructive tint and destructive text. Operational pages also use emerald/amber/red status variants. Preserve textual labels so hue does not carry status alone.

### Navigation

The rail groups destinations under collapsible 12px labels. Destination rows are at least 44px high, with a 20px icon and 14px medium label. Current-page violet fill, white text and a pale lavender diamond complement `aria-current`. Collapsed groups show their current destination in text. Dock destinations are at least 56px high, with icons above 12px labels and solid violet selection. A 44px dark plum search field with pale text, muted lilac placeholder and pale focus outline filters destinations by label and description, opens matching groups while searching, and announces “没有匹配的菜单” with a status role when empty. Clearing the query restores the saved collapse state. Mobile menu focus is trapped, background content becomes inert and focus returns after closing.

### Dialogs

Dialogs are bottom-aligned on phones and centered from 640px, capped at 90dvh and 1024px wide. On phones only the top corners are rounded; larger layouts round all corners. Headers and optional footers remain outside the scrolling body. Retain existing Escape/confirmation behavior and focus management.

### Automation configuration and results

Keep operational settings in the existing dialog and shared controls. A compact group of primary/outline buttons identifies the selected mode with `aria-pressed`; labels describe each operation. Use thin dividers to separate configuration, connection instructions and result records. At narrow widths, stack fields and let actions wrap; preserve the path from configuration to its save action before connection instructions and history.

RSS configuration places one-sentence explanations beside fields and uses inline disclosures for longer help. Optional filters and execution settings start collapsed for new rules and open when already configured; editing values does not change their expanded state. Saved RSS addresses are displayed as masked text, not editable inputs.

**The Local Feedback Rule.** Keep an operation’s failure message beside its affected record and retry action; keep configuration feedback beside the save controls. Use explicit text for service and processing states, and expandable detail for longer result explanations.

### Brand and empty states

`frontend/public/art/kirara-icon.png` is the generated chibi Kirara mark, displayed at 56px in the brand entrance and used as the web favicon. The same source supplies the PNG, ICO and ICNS desktop icons in `src-tauri/icons/`. `frontend/public/art/companions.webp` depicts Inuyasha, Kagome and Kirara with transparency: it occupies 260px by 174px in the empty subscription state, constrained to available width, and also appears in the wide desktop header. `frontend/public/art/journey.webp` depicts Sesshomaru, Rin and Jaken in the sidebar landscape. Decorative character art is rendered with CSS background-image on aria-hidden elements; CSS owns contain/cover sizing and cropping. The brand entrance retains its text accessible name. Other empty states retain contextual icons, and media posters remain separate content.

**The Companion Placement Rule.** Place character art in brand, header, empty-state and sidebar spaces; keep operational labels, controls and results unobstructed. Hide the sidebar scene in short viewports to preserve menu access.

These raster assets were created with the built-in `image_gen`; exact prompts and provenance are persisted in `frontend/public/ASSETS.md`, adjacent JSON files and generated PNG metadata. Keep those records with derived web and desktop assets. Historical `yunmu-icon.svg` and `kirara-rest.svg` remain in the repository but are no longer referenced by the application. Artwork colors are independent of UI semantic tokens.

The primary palette also carries into overview/statistics charts and the site PNG export.

The dark slate log console, local status colors, isolated peach trophy accent, unresolved legacy utility names (for example `surface-container-highest`), and compact local focus/detail styles are incumbent exceptions. Do not promote these into general visual rules without deliberate implementation work.

## Do's and Don'ts

### Do:
- Do reuse the semantic palette and shared controls for new work.
- Do keep active navigation identifiable through the selected fill, pale text and diamond as well as color.
- Do preserve readable Chinese labels, numeric alignment and keyboard focus.
- Do reserve bottom clearance throughout the mobile and tablet layout.
- Do keep generation prompts and asset provenance with shipping raster files and desktop derivatives.
- Do keep configuration feedback by save controls and retry errors beside the affected result.

### Don't:
- Don’t turn the lavender workspace into a glass or gradient surface.
- Don’t let character art obscure operational controls or reduce menu access in short viewports.
- Don’t use color alone to replace status labels or destructive-action explanations.
- Don’t assume all local focus treatments or legacy utility names are established system rules.
