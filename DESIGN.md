# Design System - Trakkt

**Last Updated:** 2026-05-08
**Status:** Official. All new UI work MUST follow these guidelines.

## Product Context

- **What this is:** Open-source issue tracker. Fast, opinionated, local-first. Ships as a single binary.
- **Who it's for:** Software teams who want Linear's UX without the lock-in or the price tag.
- **Space/industry:** Project management, issue tracking (competitors: Linear, Plane, Huly, GitHub Issues)
- **Project type:** Web application (issue list, board view, issue detail, settings)
- **Core experience:** Issues with markdown descriptions, organized by status and priority. Kanban board and list views. Keyboard-first navigation. AI agents interact via built-in MCP server.

## Speed as Design

**"Faster than anything else."** This is the one thing someone should remember after seeing Trakkt for the first time. Every design decision serves perceived performance.

| Principle | Rule |
|-----------|------|
| **Optimistic UI** | Show the result before the server confirms. Status changes, issue creation, comment posting update immediately. Roll back only on error. |
| **Non-blocking animations** | Content appears immediately. Transitions layer on top. Max 200ms for UI state changes. No entrance animations that gate content visibility. |
| **No spinners under 100ms** | If an operation completes in under 100ms, show nothing. Loading indicators only appear after a 100ms delay to avoid flash-of-spinner. |
| **Content before chrome** | Issue data renders before sidebar animations complete. Priority: data first, navigation second, decorative elements last. |
| **Keyboard-first** | Every action reachable via keyboard. j/k navigation, single-key shortcuts, Cmd+K command palette. The mouse is optional. |
| **Zero layout shift** | Reserve space for async content with skeleton placeholders. Sidebar state persists across navigation. No content jumping after load. |
| **Instant page transitions** | No fade-between-pages. Route changes render the new page immediately. SSR ensures first paint has data. |

## Icons for System, Color for Users

System state (status, priority) is communicated through **icon shape**, not color. Color is reserved for **labels and teams** -- things the user creates and assigns.

This keeps the visual language clean: shape tells you system state, color tells you user taxonomy. They never compete.

**Shape rule:** Status icons are **round** (circles). Priority icons are **square** (rectangles with small radius). This creates instant visual distinction between the two icon families at any size.

### Status Icons (circle variants)

| Status | Icon | Color | Description |
|--------|------|-------|-------------|
| Backlog | Dashed circle | `--text-muted` | Not yet planned |
| Todo | Empty circle | `--text-secondary` | Planned, not started |
| In Progress | Half-filled circle | `--accent` (teal) | Currently being worked on |
| Done | Filled circle + checkmark | `--accent` (teal) | Completed |
| Cancelled | Circle + X | `--text-muted` | Will not be done |

Status icons at 14px. The shape is the primary communicator; teal accent on In Progress and Done is a subtle reinforcement, not the main signal.

### Priority Icons (3 bars + urgent exclamation)

| Priority | Icon | Color |
|----------|------|-------|
| Urgent | Red filled rounded square with white exclamation mark | `#DC2626` (red) |
| High | 3 bars filled | `--text` |
| Medium | 2 bars filled | `--text` |
| Low | 1 bar filled | `--text` |
| None | Horizontal dash line | `--text-muted` |

Priority icons at 14px. Three ascending bars for low/medium/high (fill count = severity). Urgent breaks out entirely with a red exclamation circle -- it's a different class of priority, not just "more bars." None is a horizontal dash.

### Labels (user-assigned color)

Labels are where color lives. Users pick from a preset palette when creating labels. Label pills render with a tinted background and saturated text.

Preset label palette:

| Color | Hex | Example |
|-------|-----|---------|
| Red | #DC2626 | bg: #FEF2F2, text: #DC2626 |
| Blue | #2563EB | bg: #EFF6FF, text: #2563EB |
| Teal | #0D9488 | bg: #CCFBF1, text: #0D9488 |
| Yellow | #CA8A04 | bg: #FEFCE8, text: #CA8A04 |
| Gray | #6B6660 | bg: #F5F3EF, text: #6B6660 |
| Pink | #DB2777 | bg: #FDF2F8, text: #DB2777 |
| Green | #15803D | bg: #F0FDF4, text: #15803D |
| Orange | #EA580C | bg: #FFF7ED, text: #EA580C |

## Aesthetic Direction

- **Direction:** Refined Warmth with Linear density. Editorial precision with organic energy, packed tight.
- **Decoration level:** Intentional. Typography and icons do the work. No decorative elements.
- **Mood:** Fast, focused, information-dense. This tool respects your time.
- **Reference sites:** Linear (speed, density, dropdowns), GitHub Issues (simplicity), Notion (typography).

## Brand

- **Logo:** TBD -- geometric mark with teal accent.
- **Domain:** trakkt.app

## Typography

- **Display/Hero:** Instrument Serif (Google Fonts) - literary, warm, unexpected in dev tools. Signals editorial depth. Use for page headings, hero text, section titles.
- **Body:** DM Sans (Google Fonts) - clean geometric sans with warmth, great at small sizes. Use for all body text, labels, nav items, buttons.
- **UI/Labels:** DM Sans (same as body), weight 500-600 for labels
- **Data/Tables:** Geist Mono (Google Fonts), `font-variant-numeric: tabular-nums` - crisp number alignment. Use for issue numbers, timestamps, metadata.
- **Code:** Geist Mono
- **Loading:**
  ```html
  <link href="https://fonts.googleapis.com/css2?family=DM+Sans:ital,opsz,wght@0,9..40,300;0,9..40,400;0,9..40,500;0,9..40,600;0,9..40,700;1,9..40,400&family=Instrument+Serif:ital@0;1&family=Geist+Mono:wght@400;500&display=swap" rel="stylesheet">
  ```
- **Scale:**

  | Token | Size | Tailwind class | Usage |
  |-------|------|---------------|-------|
  | xs | 12px | `text-xs` | Badges, timestamps, metadata |
  | sm | 14px | `text-sm` | Labels, nav items, body small, table cells |
  | base | 16px | `text-base` | Body text, paragraphs, descriptions |
  | lg | 20px | `text-xl` | Subheadings, card titles |
  | xl | 24px | `text-2xl` | Section titles (Instrument Serif) |
  | 2xl | 30px | `text-3xl` | Page titles (Instrument Serif) |
  | 3xl | 36px | `text-4xl` | Hero headings (Instrument Serif) |

- **Tailwind mapping note:** The root font size is 15px (not the browser default 16px). Tailwind's rem-based classes produce smaller pixel values than their names suggest. Use the Tailwind class column above to hit the intended pixel sizes.
- **Rem override principle:** Any Tailwind default that uses rem and must hit a specific pixel value needs its `@theme` variable overridden in `main.css` to compensate for the 15px root.
- **Weight guidelines:** 400 for body, 500 for labels and emphasis, 600 for headings and buttons, 700 for logo text only
- **`font-display: swap`** on all Google Fonts links. Text renders immediately in the system fallback, then swaps when the custom font loads. No invisible text while fonts download.

## Color

- **Approach:** Restrained + warm. One strong accent, careful neutrals. Color is earned, not spent.
- **Rule:** Color communicates user-created taxonomy (labels, teams). System states use icon shape, not color.

### Primary Palette

| Token | Hex | Usage |
|-------|-----|-------|
| `--accent` | #0D9488 | Primary actions, links, active states, focus rings, brand |
| `--accent-hover` | #0F766E | Hover state for primary actions |
| `--accent-light` | #CCFBF1 | Light accent backgrounds, selected states |
| `--warm-dark` | #2C241E | Sidebar active item background, dark surface highlights |
| `--warm-dark-deep` | #1C1917 | Sidebar background (light & dark mode) |

### Neutral Palette (Warm Grays)

| Token | Hex | Usage |
|-------|-----|-------|
| `--bg` | #FAFAF8 | Page background |
| `--surface` | #FFFFFF | Card/panel backgrounds |
| `--surface-alt` | #F5F3EF | Alternate surface, code backgrounds, hover states |
| `--border` | #E8E5DE | Default borders |
| `--border-strong` | #D4D0C8 | Emphasized borders |
| `--text` | #1C1917 | Primary text |
| `--text-secondary` | #6B6660 | Secondary text, descriptions |
| `--text-muted` | #9C9790 | Muted text, placeholders, captions |

### Semantic Colors (alerts and system feedback only)

| Token | Hex | Usage |
|-------|-----|-------|
| `--success` | #15803D | Connected, healthy, positive change |
| `--warning` | #CA8A04 | Attention needed |
| `--error` | #DC2626 | Failed, destructive, urgent priority |
| `--info` | #2563EB | Informational |

Each semantic color has background and border variants for alerts:
- Success: bg #F0FDF4, border #BBF7D0
- Warning: bg #FEFCE8, border #FDE68A
- Error: bg #FEF2F2, border #FECDD3
- Info: bg #EFF6FF, border #BFDBFE

These are for system-level feedback (toast notifications, alert banners). Never used for status or priority indicators.

### CSS Token Migration

The CSS (`main.css`) currently uses Kyomi's palette and must be updated to match this design system:

| CSS Token | Current (Kyomi) | Target (Trakkt) | Notes |
|-----------|----------------|-----------------|-------|
| `--color-primary` | #4f46e5 (indigo) | #0D9488 (teal) | Brand accent |
| `--color-primary-foreground` | #ffffff | #ffffff | No change |
| `--color-accent` | #FEF3C7 (amber) | #CCFBF1 (teal light) | Light accent bg |
| `--color-accent-foreground` | #1C1917 | #0D9488 | Accent text |
| `--color-ring` | #4f46e5 (indigo) | #0D9488 (teal) | Focus ring |

Also rename CSS class `prose-tane` to `prose-trakkt` and update all comments referencing "Tane" or "amber".

### Dark Mode

- **Base:** `--bg: #12100F` (warm stone), `--surface: #1A1816`, `--surface-alt: #24201E`
- **Borders:** `--border: #2E2925`, `--border-strong: #3B3530`
- **Text:** `--text: #F5F3EF`, `--text-secondary: #A8A29E`, `--text-muted: #78716C`
- **Accent:** Same #0D9488
- **Semantic colors:** Use transparent backgrounds (e.g., `rgba(21, 128, 61, 0.12)` for success)
- **Strategy:** Swap CSS custom properties via `.dark` class on `<html>` (Tailwind v4 `@custom-variant dark`)

## Icons

- **System:** Phosphor Icons (https://phosphoricons.com)
- **Why Phosphor:** Six weights allow shape-level state changes (Regular -> Fill on active). Filled geometry pairs with Instrument Serif's editorial warmth.
- **Sizes:** 20px for navigation, 16px for inline/alerts/settings tabs, 14px for status/priority icons, 12px for badges
- **Color:** `currentColor` (inherits from parent text color, adapts to theme automatically)
- **Leptos crate:** `phosphor-leptos`
- **Exception:** Status and priority icons are custom SVGs (not Phosphor) for precise control over circle variants and bar chart fills.

### Weight convention

**`Regular` is the default weight for every `<Icon>` callsite.**

| Surface | Weight | Rationale |
|---|---|---|
| **Sidebar nav** | `Light` -> `Fill` | Active row becomes a solid glyph -- shape-level state change |
| **Settings tab strip** | `Light` -> `Fill` | Same pattern as sidebar nav for consistency |
| **Small icon-in-pill** (12-14px) | `Bold` | At that size, Regular loses legibility |
| **Empty states** (64px+) | `Duotone` | Two-tone teal wash for onboarding, empty states |

- **No emojis.** Never use Unicode emojis as icons in the UI.
- **No icon mixing.** All icons must come from `phosphor_leptos::*` (except custom status/priority SVGs).

## Spacing

- **Base unit:** 4px
- **Density:** Comfortable-to-dense (Linear-inspired information density)
- **Scale:**

  | Token | Value | Tailwind |
  |-------|-------|----------|
  | 2xs | 2px | `0.5` |
  | xs | 4px | `1` |
  | sm | 8px | `2` |
  | md | 16px | `4` |
  | lg | 24px | `6` |
  | xl | 32px | `8` |
  | 2xl | 48px | `12` |
  | 3xl | 64px | `16` |

### Component Spacing

| Context | Padding | Gap |
|---------|---------|-----|
| Cards | `p-4` (16px) | - |
| Modal Header | `px-5 py-3` | - |
| Modal Content | `p-5` | - |
| Modal Footer | `px-5 py-3` | `gap-2` |
| Buttons (default) | `px-3.5 py-[7px]` | `gap-1.5` |
| Buttons (small) | `px-2.5 py-[5px]` | `gap-1` |
| Input Fields | `px-2.5 py-[5px]` | - |
| Section Spacing | - | `gap-3` or `gap-4` |
| Issue rows | `px-3 py-[6px]` height 36px | `gap-2.5` |
| Dropdown items | `px-2.5 py-[5px]` margin 1px 4px | `gap-2` |

## Layout

- **Grid:** 12 columns on desktop (lg+), 1 column on mobile
- **Sidebar:** 220px expanded, 48px collapsed, warm-dark-deep (#1C1917) background
- **Content area:** Fills remaining width, scrollable, `bg-background` (#FAFAF8)

### Page Layout Pattern

The content area is one continuous warm surface. No visual separation between header and content.

```
+----------+------------------------------------------+
|          | Issues        [List|Board]  [+ New Issue] |  <- bg-background, no border
|  DARK    |                                           |
|  SIDEBAR | [search]  [Status] [Priority] [Label]     |  <- filter triggers
|          |                                           |
|          | [P] [S] TRK-42 Title      [bug]  May 8 @j|  <- issue rows
|          | [P] [S] TRK-41 Title   [feature] May 7 @m|
|          |                                           |
+----------+------------------------------------------+
```

**Rules:**
- Page wrapper and all content zones: `bg-background`. NOT `bg-muted`, NOT `bg-card`.
- `bg-muted` is for alternate surfaces only (input bars, skeleton placeholders).
- `bg-card` is for elevated surfaces (cards, modals, popovers, kanban cards).
- No `border-b` between header and content area.
- The only hard border is between the sidebar and the content area.

### Issue Row Order

Left to right: **Priority icon | Status icon | Issue ID | Title | Labels | Estimate (optional) | Date | Assignee avatar**

Priority is first because it's the most important signal for triage scanning. Status is second because it's the most-changed field. Title and labels take the middle. Date and assignee are right-aligned metadata.

### Issue List Page

```
bg-background (flex flex-col h-full)
+-- Row 1: page-header (h-14 px-5 flex items-center justify-between)
|   +-- Left: page title "Issues" (DM Sans, font-semibold, text-sm)
|   +-- Right: [List|Board] toggle + [+ New Issue] button
+-- Row 2: toolbar (bg-background px-5 py-2)
|   +-- SearchInput (flex-1)
|   +-- Filter triggers (Status, Priority, Label, Assignee)
+-- Content area (flex-1 overflow-y-auto)
|   +-- Issue rows (36px height, hover:bg-surface-alt, border-b border-border)
+-- Keyboard nav: j/k to move, Enter to open, x to select
```

### Board Page (Kanban)

```
bg-background (flex flex-col h-full)
+-- Row 1: page-header (h-14, same as list)
+-- Content area (flex-1 overflow-x-auto px-5 py-3)
    +-- Columns (flex gap-3, each min-w-[260px] max-w-[300px])
        +-- Column header (status icon + name + count, sticky top)
        +-- Issue cards (bg-card, border border-border, rounded-md, p-3)
            +-- Issue number (Geist Mono, text-xs, text-muted)
            +-- Title (DM Sans, text-sm, font-medium)
            +-- Labels (flex gap-1, colored pills)
            +-- Footer: priority icon + assignee avatar
+-- Drag and drop between columns to change status
```

### Issue Detail Page

```
bg-background (flex flex-col h-full)
+-- Header (h-14, back button + issue number)
+-- Content (flex-1 overflow-y-auto p-5, max-w-[860px])
    +-- Title (text-2xl font-display, inline-editable)
    +-- Metadata bar (status, priority, assignee, labels, due date) -- all as dropdown triggers
    +-- Description (markdown, rendered via prose-trakkt)
    +-- Divider
    +-- Comments thread
        +-- Comment (avatar + name + timestamp + markdown body)
        +-- New comment textarea
+-- Metadata footer (created/updated timestamps)
```

### Back Navigation

Detail pages navigate back using a ghost icon button, leftmost in header.

### Content Header Spec

- Height: `h-14` (56px)
- Padding: `px-5`
- CSS class: `page-header` (sets `bg-background`, `border-bottom: none`)

### Border Radius

| Token | Value | Usage |
|-------|-------|-------|
| `--radius-sm` | 4px | Inputs, chips, label badges, dropdown triggers |
| `--radius-md` | 6px | Buttons, cards, dropdowns, kanban cards |
| `--radius-lg` | 12px | Modals, dialogs |
| `rounded-full` | 9999px | Avatars, status dots |

### Shadows

| Token | Value | Usage |
|-------|-------|-------|
| `shadow-sm` | `0 1px 2px rgba(28,25,23,0.04)` | Kanban cards at rest |
| `shadow-md` | `0 2px 8px rgba(28,25,23,0.08)` | Kanban cards on hover |
| `shadow-lg` | `0 4px 16px rgba(28,25,23,0.12)` | Modals, dropdown menus |

### Scrollbars

All scrollbars use `scrollbar-width: thin` and must match their container's background.

| Context | Thumb | Track |
|---------|-------|-------|
| Light mode | `--color-border` (#E8E5DE) | transparent |
| Dark mode | `#3B3530` | transparent |
| Sidebar | `rgba(255,255,255,0.15)` | transparent |

## Dropdowns

Linear-style searchable dropdown menus. These are a core interaction pattern -- every metadata field on an issue opens one.

### Trigger (collapsed state)

Compact button showing the current value with its icon and a chevron. Empty triggers show the field name only.

```
[icon] Value [v]     <- has value: icon + label + chevron
Field name [v]       <- no value: just the field name + chevron
```

- Border: `1px solid --border`, `--radius-sm` (4px)
- Font: DM Sans, 12px
- Color: `--text-secondary` default, `--text` when has value
- Hover: `border-color: --border-strong`, `color: --text`
- Padding: 4px 8px

### Menu (expanded state)

```
+---------------------------+
| [Search field...]         |  <- optional search input
+---------------------------+
| [icon] Backlog            |  <- items with icons
| [icon] Todo               |
| [icon] In Progress    [v] |  <- selected item has checkmark
| [icon] Done               |
| [icon] Cancelled          |
+---------------------------+
| [^][v] navigate  [ret] ok |  <- keyboard hints footer
+---------------------------+
```

- Width: 220px
- Background: `--surface`
- Border: `1px solid --border`, `--radius-md` (6px)
- Shadow: `shadow-lg`
- Search input: 12px, no border, transparent bg
- Items: 13px, 5px 10px padding, 3px radius, 1px 4px margin
- Item hover: `bg-surface-alt`
- Selected item: `bg-accent-light`, `color: --accent`, checkmark right-aligned
- Keyboard shortcuts: shown right-aligned in `Geist Mono 10px --text-muted` (e.g., `1` `2` `3` `4` for priority)
- Footer: keyboard hints in 10px with `<kbd>` styled keys
- Dividers: 1px `--border`, 4px vertical margin

### Priority dropdown extras

Number keys `1-4` as quick-set shortcuts, `0` for no priority. Shown right-aligned on each item.

### Label dropdown extras

"+ Create new label" item at the bottom, separated by a divider.

## Motion

- **Easing:**
  - Enter: `cubic-bezier(0.16, 1, 0.3, 1)` (ease-out)
  - Exit: `cubic-bezier(0.7, 0, 0.84, 0)` (ease-in)
  - Move: `cubic-bezier(0.45, 0, 0.55, 1)` (ease-in-out)
- **Duration:**
  - `--duration-fast`: 100ms (color changes, hover states)
  - `--duration-normal`: 200ms (position/size changes, mount animations)
  - `--duration-slow`: 300ms (panel slides, kanban card drag)
- **Rule:** Every element with `hover:bg-*` or `hover:text-*` MUST have `transition-colors`
- **Speed rule:** Animations must be non-blocking. Content appears immediately; animation is layered on top. Never delay content visibility for a transition to complete.
- **Reduced motion:** All animations disabled when `prefers-reduced-motion: reduce` is active

## Accessibility

- **Focus states:** All interactive elements MUST have `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` (ring color = teal)
- **Disabled states:** `disabled:opacity-50 disabled:cursor-not-allowed disabled:pointer-events-none`
- **Modal backdrop:** 50% black (`bg-black/50`), no blur
- **Color contrast:** WCAG AA (4.5:1 normal text, 3:1 large text)
- **Keyboard navigation:** Full keyboard support is a v1 requirement, not an afterthought
- **Icon accessibility:** Status and priority icons must have `aria-label` describing the state

## Component Patterns

### MANDATORY: Use Components, Not Raw HTML

1. **Use the Leptos component, never raw HTML.** Write `<Button>` not `<button>`.
2. **Never inline Tailwind classes for styled components.** Pass `variant`, `size`, and optional `class` for layout.
3. **Styles live in the component definition, not in the caller.**
4. **If no component exists, create one** before duplicating styles.

### Available Components

| Component | Variants | Usage |
|-----------|----------|-------|
| Button | default, secondary, outline, ghost, ghost-muted, ghost-destructive, destructive, active, pill, pill-active | All interactive actions |
| Card | CardHeader, CardTitle, CardContent | Kanban cards, settings sections |
| Alert | default, warning, error, success, info | Inline status messages |
| StatusIcon | backlog, todo, in_progress, done, cancelled | Issue status (custom SVG circles) |
| PriorityIcon | urgent, high, medium, low, none | Issue priority (bar chart fills) |
| IssueStatusBadge | (wraps StatusIcon) | Status display with label |
| LabelBadge | (dynamic color from preset palette) | Issue label pills |
| Modal | sm, md, lg | Center overlays |
| ConfirmDialog | default, destructive | Yes/no confirmations |
| Toast | success, error, warning, info | Brief auto-dismiss notifications |
| Skeleton | - | Loading placeholders |
| SearchInput | - | Search bars with icon, clear button |
| CommandPalette | - | Cmd+K quick actions |
| Input | - | Text inputs with label |
| Select | - | Custom dropdown replacement for native select |
| Checkbox | - | Toggle checkboxes |
| Switch | - | Toggle switches |
| Avatar | sm (18px), md (28px), lg (36px) | User avatars with initials fallback |
| Badge | - | Compact info badges |
| Tooltip | - | CSS-only hover tooltips |
| Popover | - | Positioned floating content |
| Spinner | - | Loading indicator |
| NavigationProgress | - | Top-of-page loading bar |
| ActionStatus | - | Inline action feedback |
| EmptyState | - | Empty content placeholders with Duotone icon |
| Layout | - | App shell with sidebar |

### Button Variants

All buttons: DM Sans 13px weight 600, `rounded-md` (6px), `gap-1.5`, `transition-colors duration-200`.

| Variant | Background | Text | Border | Hover |
|---------|-----------|------|--------|-------|
| Primary | `--accent` (#0D9488) | white | none | `--accent-hover` (#0F766E) |
| Secondary | `--surface-alt` (#F5F3EF) | `--text` (#1C1917) | `1px solid --border` | border -> `--border-strong` |
| Ghost | transparent | `--text-secondary` | none | `bg-surface-alt`, `color: --text` |
| GhostMuted | transparent | `--text-muted` | none | text -> `--text`, bg `--surface-alt` |
| Destructive | `--error` (#DC2626) | white | none | darken 10% |
| Outline | transparent | `--text` | `1px solid --border` | `--surface-alt` |

Default size: `px-3.5 py-[7px]`. Small: `px-2.5 py-[5px]`, 12px font.

### Issue Row Pattern

```
+---------------------------------------------------------------+
| [P] [S] TRK-42  Fix login redirect loop  [bug] [auth] May 8 @j|
| pri sta  number  title                   labels       date asgn|
+---------------------------------------------------------------+
```

- Priority: bar chart icon, 14px
- Status: circle variant SVG, 14px
- Issue number: Geist Mono, `text-xs text-muted`
- Title: DM Sans, `text-sm font-medium`, ellipsis overflow
- Labels: colored pills, `text-xs px-2 py-0.5 rounded-full`
- Date: Geist Mono, `text-xs text-muted`
- Assignee: avatar circle, `w-[18px] h-[18px] rounded-full`
- Row height: 36px
- Row hover: `bg-surface-alt`
- Row border: `border-b border-border`
- Cancelled issues: title gets `text-muted line-through`

### Kanban Card Pattern

```
+---------------------+
| TRK-42              |  <- Geist Mono, text-xs, text-muted
| Fix login redirect  |  <- DM Sans, text-sm, font-medium
| loop                |
|                     |
| [bug] [auth]        |  <- label pills
| [P] Urgent    @jason|  <- priority icon + assignee
+---------------------+
```

- Card: `bg-card border border-border rounded-md p-3 shadow-sm`
- Card hover: `shadow-md`

### Empty State Pattern

```rust
<div class="w-full text-center py-16">
    <div class="w-24 h-24 mx-auto text-muted-foreground mb-6 flex items-center justify-center">
        <Icon icon=CLIPBOARD_TEXT width="64" height="64" />
    </div>
    <h3 class="text-xl font-semibold text-foreground mb-2">
        "No issues yet"
    </h3>
    <p class="text-muted-foreground mb-6">
        "Create your first issue to get started"
    </p>
    <Button>"New Issue"</Button>
</div>
```

### Loading State Pattern

| Context | Pattern |
|---------|---------|
| **Data loading** | Content-shaped `<Skeleton>` rectangles (appear after 100ms delay) |
| **Inline actions** | `<Spinner>` next to button text |

### Overlay Decision Tree

- Issue create/edit form: **Modal**
- Simple yes/no (delete issue): **ConfirmDialog**
- Brief notification (issue created): **Toast**

## Keyboard Navigation (v1 requirement)

| Key | Action |
|-----|--------|
| `j` / `k` | Move selection down / up in issue list |
| `Enter` | Open selected issue |
| `x` | Toggle selection (for bulk actions) |
| `Cmd+K` | Open command palette |
| `c` | Create new issue (from list view) |
| `Escape` | Close modal / go back |
| `1-4` | Set priority (on issue detail, or in priority dropdown) |
| `0` | Clear priority |
| `s` | Change status (opens status picker) |
| `l` | Add label (opens label picker) |
| `a` | Set assignee (opens assignee picker) |

## Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-30 | Fork Kyomi design system | Proven design language. Same typography, spacing, components. Different accent color for brand separation. |
| 2026-04-30 | Teal #0D9488 as primary accent | Distinct from Kyomi's amber. Teal signals calm focus and clarity -- appropriate for a tool about organizing work. |
| 2026-04-30 | Warm grays (not cool) | Inherited from Kyomi. Coheres with serif typography and warm dark sidebar. |
| 2026-04-30 | Keyboard navigation as v1 requirement | Linear's keyboard shortcuts are a core part of why it feels fast. This is table stakes, not a nice-to-have. |
| 2026-05-08 | "Faster than anything else" as north star | Speed is the defining experience. Every design decision serves perceived performance. |
| 2026-05-08 | Icons for system state, color for users | Priority uses bar-chart icons (no color except urgent red). Status uses circle variants. Color reserved for labels and teams. Reduces visual noise and prevents system colors from competing with user-assigned label colors. |
| 2026-05-08 | Linear-density issue rows (36px) | Tighter rows maximize information per screen. Priority-first row order enables fast triage scanning. |
| 2026-05-08 | Linear-style searchable dropdowns | Every metadata field opens a searchable, keyboard-navigable dropdown with icon + checkmark + keyboard hints. Core interaction pattern. |
| 2026-05-08 | Rename Tane -> Trakkt, domain -> trakkt.app | Project identity update. CSS class `prose-tane` to be renamed to `prose-trakkt`. |
| 2026-05-08 | CSS migration from Kyomi palette | main.css still uses Kyomi's indigo/amber tokens. Must be updated to teal per the migration table in this document. |
