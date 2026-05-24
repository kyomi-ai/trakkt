# Agent Workflows

This page shows practical patterns for common agent tasks. Each workflow is a sequence of MCP tool calls that accomplish a real objective.

## Getting Started

The first thing any agent should do is discover the workspace structure. This establishes context for all subsequent operations.

```
1. list_teams
   → Learn team keys (e.g., ENG, DES, OPS) and team IDs

2. list_statuses
   → Learn available workflow states (backlog, in progress, done, etc.)

3. list_labels
   → Learn available labels (bug, feature, etc.)

4. list_issues(team_key: "ENG", limit: 10)
   → See recent issues to understand naming conventions and priorities
```

After this discovery phase, the agent has the vocabulary it needs to create and update issues correctly.

## Filing an Issue

Create an issue with full context by combining discovery with creation:

```
1. list_teams
   → Find the right team key for this issue

2. list_labels
   → Find relevant label IDs

3. create_issue(
     team_key: "ENG",
     title: "Login page crashes on Safari 17",
     description: "## Steps to reproduce\n1. Open login page in Safari 17\n2. Enter credentials\n3. Click submit\n\n## Expected\nRedirect to dashboard\n\n## Actual\nPage crashes with white screen",
     priority: 2,
     labels: ["<bug-label-id>"]
   )
   → Returns the new issue identifier (e.g., ENG-156)
```

## Triage Workflow

Review and categorize incoming issues:

```
1. list_issues(team_key: "ENG", status_category: "backlog")
   → Get all untriaged backlog issues

2. get_issue(issue_identifier: "ENG-150")
   → Read full description, comments, and context

3. update_issue(
     issue_identifier: "ENG-150",
     priority: 3,
     labels: ["<bug-label-id>"]
   )
   → Set priority and categorize

4. add_comment(
     issue_identifier: "ENG-150",
     body: "Triaged: medium priority bug. Reproducible on Safari 17+. Assigning to frontend team."
   )
   → Leave a triage note
```

Repeat steps 2-4 for each issue in the backlog.

## Backlog Worker

An autonomous agent that picks up issues and works on them:

```
1. list_issues(team_key: "ENG", status_category: "backlog", limit: 5)
   → Get the highest-priority unstarted issues

2. get_issue(issue_identifier: "ENG-145")
   → Read the full issue for implementation context

3. list_statuses
   → Find the "In Progress" status ID

4. update_issue(
     issue_identifier: "ENG-145",
     status_id: "<in-progress-status-id>",
     assignee: "<agent-user-id>"
   )
   → Claim the issue

5. add_comment(
     issue_identifier: "ENG-145",
     body: "Starting implementation. Will update with progress."
   )
   → Signal work has begun

   ... agent does the work ...

6. add_comment(
     issue_identifier: "ENG-145",
     body: "Implementation complete. PR #42 created."
   )
   → Post completion update

7. update_issue(
     issue_identifier: "ENG-145",
     status_id: "<done-status-id>"
   )
   → Mark as complete
```

## Git Integration

Link code changes back to issues using the GitHub tools:

```
1. lookup_branch(branch: "fix/login-safari-crash")
   → Find which issues are linked to this branch
   → Returns: ENG-150 "Login page crashes on Safari 17"

2. get_issue(issue_identifier: "ENG-150")
   → Read the linked issue for context

3. update_issue(
     issue_identifier: "ENG-150",
     status_id: "<in-review-status-id>"
   )
   → Move to review state since code is ready
```

Or look up issues from a commit:

```
1. lookup_commit(sha: "a1b2c3d")
   → Find which issues are linked to this commit (prefix matching, 7+ chars)

2. add_comment(
     issue_identifier: "ENG-150",
     body: "Fix deployed in commit a1b2c3d. Verified on staging."
   )
   → Post deployment confirmation
```

## Project Planning

Set up a project with milestones and link issues:

```
1. create_project(
     name: "Q3 Platform Redesign",
     description: "Modernize the platform UI and improve performance"
   )
   → Returns the project ID

2. create_milestone(
     project_id: "<project-id>",
     name: "Phase 1 - Design",
     target_date: "2026-07-15"
   )
   → Returns milestone ID

3. create_milestone(
     project_id: "<project-id>",
     name: "Phase 2 - Implementation",
     target_date: "2026-08-30"
   )
   → Returns milestone ID

4. update_issue(
     issue_identifier: "ENG-160",
     project_id: "<project-id>",
     milestone_id: "<phase-1-milestone-id>"
   )
   → Link existing issues to the project and milestone
```

## Monitoring Activity

Keep track of what is happening across the workspace:

```
1. list_workspace_activities
   → See the latest activity across all teams (status changes, comments, assignments)

2. list_issue_activities(issue_identifier: "ENG-150")
   → See the full history of a specific issue
```

## Tips for Agent Developers

- **Always call `list_teams` first.** Most operations require a `team_key` or `team_id`. Discover these before trying to create or list issues.
- **Use `team_key` over `team_id`.** Keys like `"ENG"` are human-readable and stable. IDs are UUIDs that are harder to work with.
- **Use issue identifiers.** Tools accept identifiers like `"ENG-42"` rather than raw UUIDs. These are the same identifiers shown in the Trakkt UI.
- **Filter aggressively.** `list_issues` supports `team_key`, `priority`, `status_category`, `label`, and `assignee` filters. Narrow your queries to avoid processing large result sets.
- **Exclude closed issues by default.** `list_issues` already excludes completed and cancelled issues unless you pass `include_closed=true`. This is usually what you want.
- **Check `list_statuses` for status IDs.** Do not hardcode status IDs. They vary between workspaces and teams.
