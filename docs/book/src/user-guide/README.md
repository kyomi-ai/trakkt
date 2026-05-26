# User Guide

Trakkt is an issue tracker designed around speed and keyboard-first interaction. This guide covers the core concepts.

## Issues

Issues are the primary unit of work. Each issue belongs to a team and has:

- **Identifier** -- a team-scoped key like `TRA-35`, formed from the team prefix and a sequential number.
- **Title** -- a short summary of the work.
- **Description** -- rich markdown content with full formatting support.
- **Status** -- workflow state: Backlog, Todo, In Progress, Done, or Cancelled.
- **Priority** -- urgency level: None, Low, Medium, High, or Urgent.
- **Assignee** -- the team member responsible for the issue.
- **Labels** -- user-defined tags for categorization (workspace-wide or team-scoped).
- **Project and milestone** -- optional grouping for roadmap planning.
- **Relations** -- links between issues: blocks/blocked-by, parent/child, duplicate, and relates-to.

## Teams

Teams are organizational units within a workspace. Each team has a short key (e.g. `TRA`, `ENG`) that prefixes its issue identifiers. Teams can have their own:

- Custom statuses and workflow stages
- Estimation scales
- Auto-archive settings
- Scoped labels

## Projects and Milestones

Projects group related work across teams. Each project contains milestones that track progress toward specific goals. Issues can be assigned to a milestone to indicate which release or deadline they target.

## Labels

Labels are colored tags for categorizing issues. They can be workspace-wide (available to all teams) or scoped to a specific team. Use them for things like `bug`, `feature`, `docs`, or any taxonomy that makes sense for your workflow.

## Views

Trakkt supports multiple ways to view your issues:

- **List view** -- a table with sortable columns, inline status/priority changes, and filtering.
- **Board view** -- a Kanban board grouped by status, with drag-and-drop between columns.

Both views update in real-time via WebSocket sync.

## Keyboard Shortcuts

Trakkt is keyboard-first. Key shortcuts include:

- `j` / `k` -- navigate up/down in issue lists
- `Enter` -- open the selected issue
- `Cmd+K` / `Ctrl+K` -- open the command palette
- Single-key shortcuts for changing status and priority inline

## Workspaces

A workspace is the top-level container. In SaaS mode, each workspace is an isolated tenant with its own teams, issues, and members. Workspace features include invites, roles (owner, admin, member), and ownership transfer.
