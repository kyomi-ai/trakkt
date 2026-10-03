# Team Settings

Each team has its own settings page where you can configure identity, estimation, and archiving behavior. Open team settings by clicking the gear icon next to a team name in the sidebar, or from the [Workspace Settings](workspace-settings.md) teams list.

## General

### Name

The team's display name, shown in the sidebar, issue lists, and everywhere the team is referenced. You can change it at any time.

### Key

The team key is a 2-5 character uppercase identifier (e.g. `ENG`, `OPS`, `DES`) that forms the prefix of every issue in the team. Issue `ENG-42` belongs to the team with key `ENG`.

The key is set at team creation and cannot be changed afterward, because it is embedded in every existing issue identifier.

### Icon and color

Pick an icon and a custom color to visually distinguish the team in the sidebar and other parts of the interface. Both can be changed at any time.

## Estimation

Estimation settings control how your team sizes issues. By default, estimation is disabled -- you need to pick a scale to enable it.

### Scale selection

Choose one of the four built-in scales:

| Scale | Values | Extended range |
|-------|--------|----------------|
| **Exponential** | 1, 2, 4, 8, 16 Points | 32, 64 Points |
| **Fibonacci** | 1, 2, 3, 5, 8 Points | 13, 21 Points |
| **Linear** | 1, 2, 3, 4, 5 Points | 6, 7, 8, 9, 10 Points |
| **T-Shirt** | XS, S, M, L, XL | XXL |

Select **Disabled** to turn off estimation entirely for this team.

### Options

Once a scale is selected, you can toggle these options:

- **Allow zero / "No estimate"** -- adds a "No estimate" option to the picker, so team members can explicitly mark an issue as unsized rather than leaving the field blank.
- **Extended range** -- enables the larger values shown in the Extended Range column above. Use this for teams that occasionally encounter very large work items.
- **Count unestimated issues** -- when enabled, issues without an estimate are included in velocity and capacity totals. When disabled, only estimated issues count toward those numbers. This is enabled by default.

## Auto-archive

Override the workspace-level auto-archive setting for this specific team. Set the number of days after an issue reaches a completed or cancelled status before it is automatically archived.

- If set, this value takes precedence over the workspace default for issues in this team.
- If left unset, the team inherits the workspace-level default.

See [Archiving](issues.md#archiving) for more about how auto-archiving works.

## Members

The Members section shows everyone who belongs to the workspace and their role.

### Inviting members

Click **Invite** to send an invitation by email. You choose the role at invite time. The invited person receives a link to join the workspace.

### Roles

| Role | What they can do |
|------|-----------------|
| **Owner** | Everything an admin can do, plus delete the workspace and transfer ownership. |
| **Admin** | Manage teams, members, labels, projects, and all issues across the workspace. |
| **Member** | Create and edit issues, comment, manage their own work, and use all views. |

### Changing roles

Admins and the owner can change another member's role. The owner role is unique -- there is exactly one owner per workspace, and transferring ownership requires explicit action.

### Removing members

Admins and the owner can remove members from the workspace. Removed members lose access immediately, but their historical activity (comments, issue changes) is preserved.

## Danger zone

The danger zone contains destructive actions:

- **Delete team** -- permanently deletes the team and all of its issues. This action cannot be undone. You will be asked to confirm before the deletion proceeds.

> **Warning:** Deleting a team removes every issue, comment, attachment, and activity record associated with it. Make sure you have moved or archived anything you need to keep before deleting.
