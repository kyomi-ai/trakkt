# Status and Priority Icons

**Date:** 2026-05-08
**Goal:** Replace the plain colored dots/squares with expressive SVG icons for status and priority that communicate meaning at a glance.

## Context

Currently, status shows as a tiny colored dot (8x8px) and priority as a colored square (10x10px). These are nearly invisible and convey nothing at a glance. We want distinctive SVG icons where you can read an issue list without looking at text labels. The icons should be Trakkt's own — inspired by the general pattern of status/priority iconography but not copied from any specific product.

## Icon Design Direction

### Status Icons (16x16, stroke-based SVGs)
Each status should have a **unique shape**, not just a color change. Use circles as the base motif (representing completeness/progress) with variations:

| Status | Icon Concept | Description |
|--------|-------------|-------------|
| Backlog | Dotted/dashed circle | Segments or dashes forming an incomplete circle — "queued, not planned yet" |
| Todo | Hollow circle | Clean empty circle with solid stroke — "defined, ready to start" |
| In Progress | Partially filled circle | Circle with an arc or pie-slice fill — "actively being worked on" |
| Done | Circle with checkmark | Solid or outlined circle with an interior check — "finished" |
| Cancelled | Circle with slash | Circle with a diagonal strikethrough — "abandoned" |

### Priority Icons (16x16)
Use a **signal bars** motif (ascending bars representing urgency), or an alternative approach the implementing agent thinks looks better. The key requirement: each level must be visually distinct at 16x16 without relying solely on color.

| Priority | Icon Concept | Description |
|----------|-------------|-------------|
| No priority (0) | Muted dots or empty bars | Placeholder indicating "not set" |
| Urgent (1) | All bars filled, red | Maximum intensity — immediately obvious |
| High (2) | 3 of 4 bars filled, orange | High but not critical |
| Medium (3) | 2 of 4 bars filled, yellow | Normal work |
| Low (4) | 1 of 4 bars filled, gray | Can wait |

## Tasks

### Task 1: Create SVG status icons as Leptos components

**File:** `crates/trakkt-ui/src/components/issue_status_badge.rs`

Replace the colored dot with inline SVG icons. Each status gets a unique shape — not just a color change.

**Implementation approach:** Render SVG directly in the `view!` macro. Each icon is a 16x16 SVG with `viewBox="0 0 16 16"`, using `currentColor` for strokes so it inherits the status color from a parent CSS class.

```rust
// Example for "In Progress" — half-filled circle
view! {
    <svg class="w-4 h-4 shrink-0" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
        <circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-width="1.5"/>
        <path d="M8 1.5 A6.5 6.5 0 0 1 8 14.5" fill="currentColor"/>
    </svg>
}
```

**Status SVG specifications:**

- **Backlog:** Circle with 4 dashed segments (stroke-dasharray). Color: `text-[#9C9790]`.
- **Todo:** Clean hollow circle, 1.5px stroke. Color: `text-[#2563EB]`.
- **In Progress:** Circle with left half filled (arc path). Color: `text-[#0D9488]`. Optionally add a slow rotation animation.
- **Done:** Filled circle with a white checkmark path inside. Color: `text-[#15803D]`.
- **Cancelled:** Hollow circle with a diagonal line through it (line from top-right to bottom-left). Color: `text-[#6B6660]`.

**Props remain the same:** `status: IssueStatusVariant` and `show_label: bool`. When `show_label` is false (issue list rows), render only the SVG icon. When true (detail page), render icon + text.

**Size:** 16x16 in the issue list (`w-4 h-4`), matching Phosphor icon sizing. This is double the current 8x8 dot — much more visible.

**Update `IssueStatusVariant`:**
- Keep `parse()`, `label()`, `dot_color()` (for backward compat with board column headers)
- Add `text_color(&self) -> &'static str` that returns `text-[#HEXCOLOR]` for use as a CSS class on the SVG wrapper

### Task 2: Create SVG priority icons as Leptos components

**File:** `crates/trakkt-ui/src/components/priority_indicator.rs`

Replace the colored square with a bar-chart icon. The number of filled bars indicates priority level.

**Implementation approach:** 4 vertical bars at increasing heights. Filled bars use the priority color, empty bars use a muted gray.

```rust
// Example for "High" (3 bars filled)
view! {
    <svg class="w-4 h-4 shrink-0" viewBox="0 0 16 16" fill="none">
        <rect x="1" y="10" width="2.5" height="4" rx="0.5" fill="currentColor"/>
        <rect x="5" y="7" width="2.5" height="7" rx="0.5" fill="currentColor"/>
        <rect x="9" y="4" width="2.5" height="10" rx="0.5" fill="currentColor"/>
        <rect x="13" y="1" width="2.5" height="13" rx="0.5" fill="#E5E3DE"/>
    </svg>
}
```

**Priority SVG specifications:**

- **No priority (0):** 3 horizontal dots or 4 empty bars in muted gray. Color: `text-[#9C9790]`.
- **Urgent (1):** 4 bars all filled. Color: `text-[#DC2626]` (red). Optionally: add `!` exclamation overlay or pulsing glow.
- **High (2):** 3 bars filled, 1 empty. Color: `text-[#EA580C]` (orange).
- **Medium (3):** 2 bars filled, 2 empty. Color: `text-[#CA8A04]` (yellow).
- **Low (4):** 1 bar filled, 3 empty. Color: `text-[#6B6660]` (gray).

**Keep `priority_meta()` function** for the color/label mapping, but change the rendering from a square to the bar SVG.

### Task 3: Update all consumers

Both components are used in multiple places. Update the rendering in each:

1. **Issue list rows** (`crates/trakkt-ui/src/pages/issues/issue_list.rs`)
   - `<IssueStatusBadge status=status/>` — already used, will auto-update
   - `<PriorityIndicator priority=issue.priority/>` — already used, will auto-update
   - Verify spacing still looks good with 16x16 icons (currently `gap-3` between row elements)

2. **Issue detail metadata bar** (`crates/trakkt-ui/src/pages/issues/issue_detail.rs`)
   - Status and priority dropdowns — the selected value should show the icon next to the text
   - `<IssueStatusBadge status=... show_label=true/>` — renders icon + text

3. **Board view column headers** (`crates/trakkt-ui/src/pages/board.rs`)
   - Column headers use `col.variant.dot_color()` to render a colored dot
   - Replace with the status icon SVG component

4. **Board view cards** (`crates/trakkt-ui/src/pages/board.rs`)
   - Priority indicator on each card — already uses `<PriorityIndicator/>`

5. **Command palette results** (`crates/trakkt-ui/src/components/command_palette.rs`)
   - If issue results show status/priority, update those too

### Task 4: Visual verification

After implementing, rebuild (`trunk build`), restart the server, and verify in the browser:

- [ ] Issue list: each row shows a distinct status icon (not just a dot) — can you tell the status without reading text?
- [ ] Issue list: priority bars are visible and distinct at each level
- [ ] Issue detail: metadata bar shows icon + text for status and priority
- [ ] Board: column headers show the status icon next to the column name
- [ ] Board: cards show the priority bar icon
- [ ] Dark mode: icons remain visible (use `currentColor` so they inherit theme colors)
- [ ] All 5 status variants render correctly
- [ ] All 5 priority levels render correctly (including "no priority")
- [ ] Icons are crisp at 16x16 — no anti-aliasing blur

## Architecture references

- `crates/trakkt-ui/src/components/issue_status_badge.rs` — current status component (replace SVG, keep API)
- `crates/trakkt-ui/src/components/priority_indicator.rs` — current priority component (replace SVG, keep API)
- `crates/trakkt-ui/src/components/mod.rs` — component re-exports
- `DESIGN.md` — status colors (#9C9790, #2563EB, #0D9488, #15803D, #6B6660) and priority colors (#DC2626, #EA580C, #CA8A04, #6B6660, #9C9790)

## What NOT to change

- The `IssueStatusVariant` enum and its `parse()` method — used everywhere
- The `PriorityIndicator` component API (just `priority: i32`)
- The status/priority color values — these are from DESIGN.md
- Any service layer or server function code — this is purely a UI change
