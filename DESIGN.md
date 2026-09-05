---
name: 云母
description: A quiet collection desk with a two-tailed companion.
colors:
  background: "#f6f3eb"
  foreground: "#292822"
  card: "#fffdf8"
  border: "#ded9cc"
  primary: "#b44332"
  primary-foreground: "#ffffff"
  primary-hover: "#963727"
  primary-active: "#7f2f23"
  secondary: "#efe8db"
  secondary-foreground: "#5b4638"
  muted: "#746d60"
  accent: "#efeadd"
  destructive: "#b63232"
  surface-container: "#f0ece2"
  night: "#252620"
  jade: "#477564"
  sidebar: "#292b25"
  sidebar-foreground: "#f7f1e2"
  sidebar-muted: "#bfbbae"
  sidebar-active: "#eee6d3"
  sidebar-active-foreground: "#302f28"
  sidebar-hover: "#383a32"
  sidebar-focus: "#f2bc90"
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

**Creative North Star: "A companion’s collection desk"**

A companion’s collection desk: cream surfaces, charcoal navigation and restrained vermilion actions make a dense Chinese PT workspace feel calm and familiar. The flat cat emblem and resting two-tail illustration supply the character; borders, clear text and compact controls carry the work.

This is a record of the implemented light interface, grounded in the application shell, shared controls and operational pages. The existing Chinese system-font stack is a practical implementation fact; there is no bespoke display face or bundled font.

**Key Characteristics:**
- Warm paper-like surfaces with a dark navigation rail.
- Flat bordered containers, compact controls and visible task states.
- A small cat identity and a dedicated subscription empty-state illustration.

## Colors

Warm cream and charcoal form the neutral field; vermilion is the primary action and selection accent. Frontmatter values describe reused tokens; `input` and `surface` in Tailwind alias the card color, and `ring` aliases primary.

### Primary

- **Vermilion** (`primary`): primary actions, selected text, caret and selection color; darker hover and active variants give buttons feedback.
- **White** (`primary-foreground`): text on solid primary and destructive buttons.

### Secondary

- **Warm parchment** (`secondary`, `secondary-foreground`): quieter filled actions.
- **Jade** (`jade`): existing positive-state details. Semantic destructive red has a separate token; some pages retain Tailwind emerald, amber, red and slate status colors. These are functional state exceptions, not additional brand accents.

### Neutral

- **Cream workspace / ivory card** (`background`, `card`): the two principal light surfaces.
- **Charcoal ink / warm gray** (`foreground`, `muted`): reading text and supporting text.
- **Paper edge / inset paper** (`border`, `surface-container`, `accent`): dividers, table headings, tab tracks and hover feedback.
- **Charcoal rail** (`sidebar`): navigation background with cream foreground and lighter gray supporting text. Active items use `sidebar-active` and `sidebar-active-foreground`; hover uses `sidebar-hover`, focus uses `sidebar-focus`.
- **Night** (`night`): translucent modal scrims.

**The Task Accent Rule.** Use vermilion to identify actions and selected states; retain neutral surfaces for sustained reading.

## Typography

The body font stack in frontmatter applies across the interface. Font availability follows the device; the project does not download Noto Sans SC. Labels and page headings are sans serif. Monospace is used for the log console; tables, times and explicit numeric fields use tabular numerals.

Page headings use the headline role from the small breakpoint upward; below it they are 18px semibold with a snug line height. Card titles use the title role and slightly tightened tracking. Body and control text are generally 14px; explanatory paragraphs often increase line height to 24px. Labels and metadata are generally 12px, with existing 10–11px details in dense operational UI. The brand wordmark is 24px semibold with 0.12em tracking. These are functional sizes, not a display-font system to amplify on new screens.

## Layout

At 1024px and above, the shell is a viewport-height grid with a fixed 248px navigation column and a `minmax(0, 1fr)` content column. Navigation and content scroll independently. The workspace has 32px horizontal and 28px vertical padding, increasing horizontal padding to 40px at 1280px. Between 640px and 1023px, workspace padding is 24px. Below 640px it is 16px vertically and 12px horizontally.

Below 1024px, the rail becomes a modal menu (85vw, maximum 320px), with safe-area-aware outer padding and its own scroll region. A fixed dock offers four destinations plus the full menu. The dock is 92% wide, capped at 440px, and sits at `max(24px, env(safe-area-inset-bottom))`. Content bottom padding is `calc(110px + env(safe-area-inset-bottom, 0px))` throughout this mobile/tablet range. Preserve that clearance so final actions can scroll above the dock.

Page descriptions appear from 1024px and the clock from 1280px. Media task tabs use two columns below 1024px and four above. Forms commonly gain two columns at 640px, while wide tables retain horizontal scrolling instead of squeezing all columns. Shared card header/content padding is 24px; some page and dialog bodies use 16px, increasing to 24px at 640px. Main groups use 16–24px gaps; smaller control groups use 8–12px.

## Elevation & Depth

**The Bordered Surface Rule.** Standard cards have no shadow; light surface changes and thin warm borders separate content.

This is not a universal no-shadow system. Selected task tabs and some existing statistics/settings panels retain `shadow-sm`. The dock and select popovers use `shadow-lg`; dialogs use `shadow-xl`. Exact shadow values are recorded in the sidecar. The mobile menu backdrop retains a small blur over a black 40% scrim; standard dialogs use a night 45% scrim. Do not generalize that overlay blur to reading surfaces.

Motion is limited: shared buttons transition colors in 150ms; the brand icon rotates −6 degrees on hover/focus over 180ms ease-out. Other Tailwind transition utilities generally use their 150ms defaults, and dialog close controls use 200ms. Animation-like class names on menu/select wrappers have no configured animation plugin and are not a guaranteed entrance animation. Reduced-motion CSS shortens animations/transitions to 0.01ms and turns CSS smooth scrolling off. Existing programmatic smooth log scrolling is a separate behavior, not covered by that CSS guarantee.

## Shapes

Shared buttons, input fields and navigation items have softly squared 8px corners. Table frames, tab tracks and many inline panels use 12px corners; cards, dock and dialogs use 16px. The configured `3xl` also resolves to 16px. Status pills remain fully rounded. Borders are generally 1px. Lucide outline icons are normally 16px in controls and 20px in navigation. A small rotated square (6px) is the active rail marker.

## Components

### Buttons

Compact, firm controls: default height 40px, 8px corners, 8px by 20px padding and 14px semibold text. Primary uses vermilion/white, secondary uses parchment/brown, outline uses ivory/ink with a warm border, and destructive uses red/white. Outline hover warms the background and accents the border; secondary hover uses accent. Disabled buttons have 50% opacity and ignore pointer interaction. Page-level compact 36px and touch-oriented 44px height overrides already exist.

Shared buttons and inputs use an opaque 2px primary focus ring, separated from the component by a 2px card-colored offset. The global fallback is a 2px vermilion outline with 3px offset. Rail items override ring color to pale peach and include a 2px offset. Some local controls, including media task tabs, still use translucent or unoffset rings; those are recorded exceptions, not a replacement focus standard.

### Inputs / Fields

Fields are 44px high with 8px corners, an ivory fill, warm border, 16px horizontal padding and muted placeholders. Focus adds the primary border/ring; disabled fields fade to 50%. Select triggers match the field geometry and have explicit invalid-border styling. Their portaled menu is viewport-constrained, with a 6px gap, 8px viewport margin and computed maximum height up to 240px. Selected options use vermilion/white with a checkmark. Preserve keyboard navigation and focus restoration when extending these components.

### Cards / Containers

Cards are bordered ivory surfaces with 16px corners, clipped overflow and no default shadow. Headers have a subtle divider, 18px bold titles and 14px supporting text. Tables use a 12px frame, 44px header row, 12px semibold headings and 16px cell padding; hovered rows tint cells with accent at 60% opacity.

### Chips

Media status pills use a thin border, 12px semibold text and 4px by 10px padding. Neutral is inset paper/muted; positive uses a primary tint with ink text; negative uses a destructive tint and destructive text. Operational pages also use emerald/amber/red status variants. Preserve textual labels so hue does not carry status alone.

### Navigation

The rail groups destinations under collapsible 12px labels. Destination rows are at least 44px high, with a 20px icon and 14px medium label. Current-page cream fill and vermilion diamond complement `aria-current`. Collapsed groups show their current destination in text. Dock destinations are at least 56px high, with icons above 12px labels and solid vermilion selection. Mobile menu focus is trapped, background content becomes inert and focus returns after closing.

### Dialogs

Dialogs are bottom-aligned on phones and centered from 640px, capped at 90dvh and 1024px wide. On phones only the top corners are rounded; larger layouts round all corners. Headers and optional footers remain outside the scrolling body. Retain existing Escape/confirmation behavior and focus management.

### Brand and empty states

`frontend/public/yunmu-icon.svg` is the authored flat cat emblem, used for the brand entrance and favicon. The shell displays it at 48px. `frontend/public/kirara-rest.svg` is the authored resting two-tail illustration, displayed at 156px by 114px for the empty subscription state, with empty alt text. Its cream fur/charcoal markings/vermilion eyes are artwork colors, independent of UI tokens. Other empty states use simple contextual icons. Provenance is recorded in `frontend/public/ASSETS.md`. No shipping raster was created for this redesign. Media poster content is separate from brand art.

The dark slate log console, local status colors, isolated peach trophy accent, unresolved legacy utility names (for example `surface-container-highest`), and compact local focus/detail styles are incumbent exceptions. Do not promote these into general visual rules without deliberate implementation work.

## Do's and Don'ts

### Do:
- Do reuse the semantic palette and shared controls for new work.
- Do keep active navigation identifiable through the cream fill and diamond as well as color.
- Do preserve readable Chinese labels, numeric alignment and keyboard focus.
- Do reserve bottom clearance throughout the mobile and tablet layout.
- Do keep asset provenance with the authored SVG files.

### Don't:
- Don’t turn the cream workspace into a glass or gradient surface.
- Don’t spread the empty-state mascot into every operational panel.
- Don’t use color alone to replace status labels or destructive-action explanations.
- Don’t assume all local focus treatments or legacy utility names are established system rules.
