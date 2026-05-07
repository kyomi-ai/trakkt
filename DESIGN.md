# Design System - Tane

**Last Updated:** 2026-04-30
**Status:** Official. All new UI work MUST follow these guidelines.

## Product Context

- **What this is:** Open-source issue tracker. Fast, opinionated, local-first. Ships as a single binary.
- **Who it's for:** Software teams who want Linear's UX without the lock-in or the price tag.
- **Space/industry:** Project management, issue tracking (competitors: Linear, Plane, Huly, GitHub Issues)
- **Project type:** Web application (issue list, board view, issue detail, settings)
- **Core experience:** Issues with markdown descriptions, organized by status and priority. Kanban board and list views. Keyboard-first navigation. AI agents interact via built-in MCP server.

## Aesthetic Direction

- **Direction:** Refined Warmth. Editorial precision with organic energy. Forked from Kyomi's design language.
- **Decoration level:** Intentional. Typography does most of the work.
- **Mood:** Sophisticated, focused, fast. Not cold enterprise, not playful startup. This tool respects your time.
- **Reference sites:** Linear (speed, opinions), GitHub Issues (simplicity), Notion (typography).

## Brand

- **Logo:** TBD — geometric mark with teal accent.
- **Domain:** tane.dev

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

## Color

- **Approach:** Restrained + warm. One strong accent, careful neutrals. Color is earned, not spent.

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
| `--surface-alt` | #F5F3EF | Alternate surface, code backgrounds |
| `--border` | #E8E5DE | Default borders |
| `--border-strong` | #D4D0C8 | Emphasized borders |
| `--text` | #1C1917 | Primary text |
| `--text-secondary` | #6B6660 | Secondary text, descriptions |
| `--text-muted` | #9C9790 | Muted text, placeholders, captions |

### Semantic Colors

| Token | Hex | Usage |
|-------|-----|-------|
| `--success` | #15803D | Connected, healthy, positive change |
| `--warning` | #CA8A04 | Attention needed |
| `--error` | #DC2626 | Failed, destructive |
| `--info` | #2563EB | Informational |

Each semantic color has background and border variants for alerts:
- Success: bg #F0FDF4, border #BBF7D0
- Warning: bg #FEFCE8, border #FDE68A
- Error: bg #FEF2F2, border #FECDD3
- Info: bg #EFF6FF, border #BFDBFE

### Priority Colors

| Priority | Color | Usage |
|----------|-------|-------|
| Urgent | #DC2626 (error red) | Urgent priority indicator |
| High | #EA580C (orange-600) | High priority indicator |
| Medium | #CA8A04 (warning yellow) | Medium priority indicator |
| Low | #6B6660 (text-secondary) | Low priority indicator |
| None | #9C9790 (text-muted) | No priority set |

### Status Colors

| Status | Color | Usage |
|--------|-------|-------|
| Backlog | #9C9790 (text-muted) | Not yet planned |
| Todo | #2563EB (info blue) | Planned, not started |
| In Progress | #0D9488 (accent teal) | Currently being worked on |
| Done | #15803D (success green) | Completed |
| Cancelled | #6B6660 (text-secondary) | Will not be done |

### Dark Mode

- **Base:** `--bg: #12100F` (warm stone), `--surface: #24201E`, `--surface-alt: #2C241E`
- **Borders:** `--border: #2E2925`, `--border-strong: #3B3530`
- **Text:** `--text: #F5F3EF`, `--text-secondary: #A8A29E`, `--text-muted: #78716C`
- **Accent:** Same #0D9488
- **Semantic colors:** Use transparent backgrounds (e.g., `rgba(21, 128, 61, 0.12)` for success)
- **Strategy:** Swap CSS custom properties via `.dark` class on `<html>` (Tailwind v4 `@custom-variant dark`)

## Icons

- **System:** Phosphor Icons (https://phosphoricons.com)
- **Why Phosphor:** Six weights allow shape-level state changes (Regular → Fill on active). Filled geometry pairs with Instrument Serif's editorial warmth.
- **Sizes:** 20px for navigation, 16px for inline/alerts/settings tabs, 14px for buttons, 12px for badges
- **Color:** `currentColor` (inherits from parent text color, adapts to theme automatically)
- **Leptos crate:** `phosphor-leptos`

### Weight convention

**`Regular` is the default weight for every `<Icon>` callsite.**

| Surface | Weight | Rationale |
|---|---|---|
| **Sidebar nav** | `Light` → `Fill` | Active row becomes a solid glyph — shape-level state change |
| **Settings tab strip** | `Light` → `Fill` | Same pattern as sidebar nav for consistency |
| **Small icon-in-pill** (12–14px) | `Bold` | At that size, Regular loses legibility |
| **Empty states** (64px+) | `Duotone` | Two-tone teal wash for onboarding, empty states |

- **No emojis.** Never use Unicode emojis as icons in the UI.
- **No icon mixing.** All icons must come from `phosphor_leptos::*`.

## Spacing

- **Base unit:** 4px
- **Density:** Comfortable
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
| Cards | `p-6` (24px) | - |
| Modal Header | `px-6 py-4` | - |
| Modal Content | `p-6` | - |
| Modal Footer | `px-6 py-4` | `gap-2` |
| Buttons | `px-5 py-2.5` (default), `px-3.5 py-2` (small) | `gap-1.5` |
| Input Fields | `px-3.5 py-2.5` | - |
| Section Spacing | - | `gap-4` or `gap-6` |
| Issue rows | `px-4 py-3` | `gap-3` |

## Layout

- **Grid:** 12 columns on desktop (lg+), 1 column on mobile
- **Sidebar:** 20rem (300px at 15px root) expanded, 4rem (60px) collapsed, warm-dark-deep (#1C1917) background
- **Content area:** Fills remaining width, scrollable, `bg-background` (#FAFAF8)

### Page Layout Pattern

The content area is one continuous warm surface. No visual separation between header and content.

```
┌──────────┬──────────────────────────────────────────┐
│          │ Page Title     [filters]   [primary btn] │  ← bg-background, no border
│  DARK    │                                          │
│  SIDEBAR │ [search / toolbar]                       │  ← bg-background, no border
│          │                                          │
│          │ Content: issue list / board / detail      │  ← bg-background
│          │                                          │
└──────────┴──────────────────────────────────────────┘
```

**Rules:**
- Page wrapper and all content zones: `bg-background`. NOT `bg-muted`, NOT `bg-card`.
- `bg-muted` is for alternate surfaces only (input bars, skeleton placeholders).
- `bg-card` is for elevated surfaces (cards, modals, popovers, kanban cards).
- No `border-b` between header and content area.
- The only hard border is between the sidebar and the content area.

### Issue List Page

```
bg-background (flex flex-col h-full)
├── Row 1: page-header (h-16 px-4 md:px-6 flex items-center justify-between)
│   ├── Left: page title "Issues" (text-3xl font-display)
│   └── Right: [+ New Issue] button
├── Row 2: toolbar (bg-background px-4 md:px-6 py-3)
│   ├── SearchInput (flex-1)
│   └── Filter dropdowns (Status, Priority, Assignee, Label)
├── Content area (flex-1 overflow-y-auto)
│   └── Issue rows (hover:bg-surface-alt, border-b border-border)
└── Keyboard nav: j/k to move, Enter to open, x to select
```

### Board Page (Kanban)

```
bg-background (flex flex-col h-full)
├── Row 1: page-header (h-16, same as list)
├── Content area (flex-1 overflow-x-auto px-4 md:px-6 py-4)
│   └── Columns (flex gap-4, each min-w-[280px] max-w-[320px])
│       ├── Column header (status name + count, sticky top)
│       └── Issue cards (bg-card, border border-border, rounded-md, p-4)
│           ├── Issue number (Geist Mono, text-xs, text-muted)
│           ├── Title (DM Sans, text-sm, font-medium)
│           ├── Labels (flex gap-1, colored pills)
│           └── Footer: priority icon + assignee avatar
└── Drag and drop between columns to change status
```

### Issue Detail Page

```
bg-background (flex flex-col h-full)
├── Header (h-16, back button + issue number)
├── Content (flex-1 overflow-y-auto p-4 md:p-6, max-w-[860px])
│   ├── Title (text-2xl font-display, inline-editable)
│   ├── Metadata bar (status, priority, assignee, labels, due date)
│   ├── Description (markdown, rendered)
│   ├── Divider
│   └── Comments thread
│       ├── Comment (avatar + name + timestamp + markdown body)
│       │   └── Threaded replies (indented one level)
│       └── New comment textarea
└── Metadata footer (created/updated timestamps)
```

### Back Navigation

Detail pages navigate back using a ghost icon button, leftmost in header.

### Content Header Spec

- Height: `h-16` (64px)
- Padding: `px-4 md:px-6`
- CSS class: `page-header` (sets `bg-background`, `border-bottom: none`)

### Border Radius

| Token | Value | Usage |
|-------|-------|-------|
| `--radius-sm` | 4px | Inputs, chips, label badges |
| `--radius-md` | 8px | Buttons, cards, dropdowns, kanban cards |
| `--radius-lg` | 12px | Modals, dialogs |
| `rounded-full` | 9999px | Avatars, status dots, priority dots |

### Shadows

| Token | Value | Usage |
|-------|-------|-------|
| `shadow-sm` | `0 1px 2px rgba(28,25,23,0.05)` | Buttons, inputs |
| `shadow-md` | `0 4px 12px rgba(28,25,23,0.08)` | Cards, dropdowns, kanban cards |
| `shadow-lg` | `0 8px 24px rgba(28,25,23,0.12)` | Modals, sheets |

### Scrollbars

All scrollbars use `scrollbar-width: thin` and must match their container's background.

| Context | Thumb | Track |
|---------|-------|-------|
| Light mode | `--color-border` (#E8E5DE) | transparent |
| Dark mode | `#3B3530` | transparent |
| Sidebar | `rgba(255,255,255,0.15)` | transparent |

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
- **Reduced motion:** All animations disabled when `prefers-reduced-motion: reduce` is active

## Accessibility

- **Focus states:** All interactive elements MUST have `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` (ring color = teal)
- **Disabled states:** `disabled:opacity-50 disabled:cursor-not-allowed disabled:pointer-events-none`
- **Modal backdrop:** 50% black (`bg-black/50`), no blur
- **Color contrast:** WCAG AA (4.5:1 normal text, 3:1 large text)
- **Keyboard navigation:** Full keyboard support is a v1 requirement, not an afterthought

## Component Patterns

### MANDATORY: Use Components, Not Raw HTML

1. **Use the Leptos component, never raw HTML.** Write `<Button>` not `<button>`.
2. **Never inline Tailwind classes for styled components.** Pass `variant`, `size`, and optional `class` for layout.
3. **Styles live in the component definition, not in the caller.**
4. **If no component exists, create one** before duplicating styles.

### Available Components

| Component | Variants | Usage |
|-----------|----------|-------|
| Button | default, secondary, outline, ghost, destructive | All interactive actions |
| Card | CardHeader, CardTitle, CardContent | Kanban cards, settings sections |
| Alert | default, warning, error, success, info | Inline status messages |
| StatusBadge | backlog, todo, in_progress, done, cancelled | Issue status indicators |
| PriorityIcon | urgent, high, medium, low, none | Priority indicators |
| LabelBadge | (dynamic color) | Issue label pills |
| Modal | sm, md, lg | Center overlays |
| ConfirmDialog | default, destructive | Yes/no confirmations |
| Toast | success, error, warning, info | Brief auto-dismiss notifications |
| Skeleton | - | Loading placeholders |
| SearchInput | - | Search bars with icon, clear button |
| CommandPalette | - | Cmd+K quick actions |

### Button Variants

All buttons: DM Sans 14px weight 600, `rounded-md` (8px), `px-5 py-2.5`, `gap-1.5`, `transition-colors duration-200`.

| Variant | Background | Text | Border | Hover |
|---------|-----------|------|--------|-------|
| Primary | `--accent` (#0D9488) | white | none | `--accent-hover` (#0F766E) |
| Secondary | `--secondary` (#F5F3EF) | `--foreground` (#1C1917) | `1px solid --border` | border → `--border-strong` |
| Ghost | transparent | `--accent` | none | `--accent-light` (#CCFBF1) |
| GhostMuted | transparent | `--text-muted` | none | text → `--text`, bg `--accent-light` |
| Destructive | `--error` (#DC2626) | white | none | darken 10% |
| Outline | transparent | `--text` | `1px solid --border` | `--surface-alt` |

Small buttons: 13px font, `px-3.5 py-2`.

### Issue Row Pattern

```
┌─────────────────────────────────────────────────────────────┐
│ [●] TRK-42  Fix login redirect loop    [bug] [auth]  ■ @j  │
│     status   number  title              labels     pri asgn │
└─────────────────────────────────────────────────────────────┘
```

- Status dot: `rounded-full w-2 h-2`, colored by status
- Issue number: Geist Mono, `text-xs text-muted-foreground`
- Title: DM Sans, `text-sm font-medium`
- Labels: colored pills, `text-xs px-1.5 py-0.5 rounded-sm`
- Priority: small colored square
- Assignee: avatar circle or initials, `w-5 h-5 rounded-full`
- Row hover: `bg-surface-alt`
- Row border: `border-b border-border`

### Kanban Card Pattern

```
┌─────────────────────┐
│ TRK-42              │  ← Geist Mono, text-xs, text-muted
│ Fix login redirect  │  ← DM Sans, text-sm, font-medium
│ loop                │
│                     │
│ [bug] [auth]        │  ← label pills
│ ■ Urgent    @jason  │  ← priority + assignee
└─────────────────────┘
```

- Card: `bg-card border border-border rounded-md p-4 shadow-sm`
- Drag handle: subtle grip dots on hover
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
| **Data loading** | Content-shaped `<Skeleton>` rectangles |
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
| `1-5` | Set priority (on issue detail) |
| `s` | Change status (opens status picker) |
| `l` | Add label (opens label picker) |
| `a` | Set assignee (opens assignee picker) |

## Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-30 | Fork Kyomi design system | Proven design language. Same typography, spacing, components. Different accent color for brand separation. |
| 2026-04-30 | Teal #0D9488 as primary accent | Distinct from Kyomi's amber. Teal signals calm focus and clarity — appropriate for a tool about organizing work. |
| 2026-04-30 | Warm grays (not cool) | Inherited from Kyomi. Coheres with serif typography and warm dark sidebar. |
| 2026-04-30 | Keyboard navigation as v1 requirement | Linear's keyboard shortcuts are a core part of why it feels fast. This is table stakes, not a nice-to-have. |
