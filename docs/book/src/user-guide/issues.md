# Issues

Issues are the primary unit of work in Trakkt. Every issue belongs to a team and is identified by a key like `ENG-42` -- the team prefix plus a sequential number.

## Creating an issue

Navigate to a team page and press `c` or click **New Issue**. The only required field is a title. Everything else -- description, status, priority, assignee, labels, due date, estimate, project, and milestone -- is optional and can be set at creation time or added later.

### Description

The description field supports full markdown formatting: headings, lists, code blocks, links, images, and more. Use it to provide context, acceptance criteria, or any detail that helps the person working on the issue.

## Status workflow

Every issue has a status that tracks where it sits in your workflow. Statuses are grouped into five categories:

| Category | Default status | Meaning |
|----------|---------------|---------|
| **Backlog** | Backlog | Captured but not yet planned for work. |
| **Unstarted** | Todo | Planned and ready to be picked up. |
| **Started** | In Progress, In Review | Actively being worked on. |
| **Completed** | Done | Work is finished. |
| **Cancelled** | Cancelled | Work was abandoned or no longer needed. |

A typical issue moves from **Backlog** through **Todo**, into **In Progress** or **In Review**, and finally lands on **Done**. Issues that become irrelevant can be moved to **Cancelled** at any point.

You can change an issue's status from the issue detail page, or inline from any list or board view.

> **Note:** Completed and cancelled issues are hidden from the default views. Use the filter controls to include them when needed. These issues are also candidates for [auto-archiving](#archiving).

## Priority

Priority signals how urgent an issue is. There are five levels:

| Level | When to use |
|-------|------------|
| **Urgent** | Needs immediate attention -- something is broken or blocking others. |
| **High** | Important work that should be picked up soon. |
| **Medium** | Standard priority for most planned work. |
| **Low** | Nice to have, no time pressure. |
| **No priority** | Not yet triaged, or priority is not applicable. |

Priority can be set from the issue detail page or changed inline from list and board views.

## Assignee

Each issue can have a single assignee -- the team member responsible for the work. Assign someone from the issue detail page by selecting a workspace member from the dropdown. You can also reassign or unassign at any time.

Assigned issues appear on the assignee's [My Issues](notifications.md#my-issues) page.

## Labels

Labels are colored tags for categorizing issues. They come in two scopes:

- **Workspace labels** -- available to all teams. Good for cross-cutting concerns like `bug`, `feature`, or `docs`.
- **Team labels** -- scoped to a single team. Useful for team-specific categories.

An issue can have multiple labels. You can add and remove labels from the issue detail page. To manage the available labels, see [Workspace Settings](workspace-settings.md).

## Due dates

Set a due date to signal when an issue should be completed. Due dates appear in list views and can be used for sorting. There is no automatic enforcement -- due dates are informational, not blocking.

## Estimates

Estimates let you assign a size or effort value to an issue. The available scales depend on your team's configuration (see [Team Settings](team-settings.md)):

| Scale | Values |
|-------|--------|
| **Exponential** | 1, 2, 4, 8, 16 Points |
| **Fibonacci** | 1, 2, 3, 5, 8 Points |
| **Linear** | 1, 2, 3, 4, 5 Points |
| **T-Shirt** | XS, S, M, L, XL |

Each scale also supports an extended range for larger items (for example, Exponential adds 32 and 64; T-Shirt adds XXL). Extended range and the "No estimate" option can be toggled per team.

If estimation is disabled for a team, the estimate field will not appear.

## Relations

Issues can be linked to other issues through relations. Trakkt supports four relation types:

### Blocks / Blocked by

Use when one issue cannot proceed until another is finished. Creating a "blocks" relation from issue A to issue B means A blocks B, and B is blocked by A. Trakkt validates that you cannot create a circular blocking chain.

### Parent / Child (sub-issues)

Use to break a large issue into smaller pieces. One issue becomes the parent, and the others become its children. An issue can have at most one parent, but a parent can have many children. Trakkt prevents circular parent chains.

### Duplicate

Marks an issue as a duplicate of another. An issue can only be a duplicate of one other issue, but multiple issues can point to the same original.

### Relates to

A lightweight link between two related issues with no directional semantics. Use it when issues are connected but do not have a blocking or hierarchical relationship.

You can add relations from the issue detail page. The related issues and their relation types are displayed in the issue sidebar.

## Comments

Comments let you discuss an issue with your team. They support full markdown formatting, just like descriptions.

Comments show the author, a timestamp, and whether the comment was made by a user, an API integration, or an AI agent.

## Attachments

You can upload files to an issue -- screenshots, documents, logs, or anything else that provides useful context. Attachments are listed on the issue detail page with their filename, type, and size. You can download or delete them at any time.

## Activity log

Every change to an issue is recorded in the activity log: status changes, priority updates, reassignments, label additions, and more. Each entry shows who made the change, what changed (old value and new value), and when it happened.

The activity log also indicates the source of each action -- whether it came from a user in the browser, an API call, or an AI agent.

## Archiving

Completed and cancelled issues are automatically archived after a configurable number of days. Archiving removes them from the main issue list and board views, keeping your active workspace clean.

- The default archive period is set at the workspace level in [Workspace Settings](workspace-settings.md).
- Individual teams can override this default in [Team Settings](team-settings.md).
- Archived issues are still accessible through the archived issues view and can be unarchived if needed.

Archiving is non-destructive -- no data is lost. It simply moves issues out of your day-to-day views.
