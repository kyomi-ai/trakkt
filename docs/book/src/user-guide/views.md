# Views

Trakkt gives you two ways to look at your issues: a list view for detailed scanning and a board view for visual workflow management. Both support sorting, filtering, and real-time updates.

## List view

The list view displays issues as rows in a table. Each row shows the issue's priority, status, identifier, title, labels, date, and assignee.

You can change an issue's status or priority inline -- click the status or priority indicator on any row to open a dropdown and select a new value without leaving the list.

### Sorting

Click a column header to sort by that field. Click again to reverse the direction. Available sort fields include:

- Priority
- Status
- Created date
- Updated date
- Assignee
- Due date

### Keyboard navigation

In list view, use `j` and `k` to move the selection up and down, and `Enter` to open the selected issue. See [Keyboard Shortcuts](keyboard-shortcuts.md) for the full list.

## Board view

The board view displays issues as cards arranged in columns grouped by status. Each column corresponds to a status in your workflow -- for example, Backlog, Todo, In Progress, In Review, Done, and Cancelled.

### Drag and drop

Drag an issue card from one column to another to change its status. This is the quickest way to move issues through your workflow visually.

### Hiding columns

You can hide columns you do not need. For example, if you rarely look at your Backlog on the board, hide that column to reduce visual noise. Hidden columns can be shown again at any time.

## Filtering

Both list and board views support filtering to narrow down what you see. You can filter by:

- **Status** -- show only issues in specific status categories (e.g. only started issues).
- **Priority** -- show only issues at certain priority levels.
- **Labels** -- show only issues with specific labels applied.
- **Project** -- show only issues assigned to a particular project.

Filters can be combined. For example, you might filter to see only high-priority started issues with the "bug" label.

## Saved views

If you find yourself applying the same filters repeatedly, save them as a named view.

### Creating a saved view

1. Set up your desired filters, sort order, and display mode (list or board).
2. Click **Save View** in the toolbar.
3. Give the view a name and optionally pick an icon.

Saved views appear in the sidebar for quick access.

### Shared views

Trakkt supports shared views that are visible to all members of the workspace. These are useful for team-wide dashboards like "Active Bugs" or "Sprint Board". Any workspace member can use a shared view, but only the creator can edit or delete it.

## Real-time updates

Both views update in real time via WebSocket sync. When a teammate changes an issue's status, adds a comment, or makes any other update, your view reflects the change immediately -- no manual refresh needed.
