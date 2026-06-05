# Getting Started

This chapter walks you through the first few minutes with Trakkt: creating a workspace, inviting people, setting up a team, and filing your first issue.

## Creating a workspace

How you create a workspace depends on how Trakkt is running.

### SaaS mode

Sign up at the hosted Trakkt instance, pick a name for your workspace, and you are in. Your workspace is an isolated tenant -- its teams, issues, and members are completely separate from every other workspace.

### Personal mode (self-hosted with SQLite)

If you run Trakkt with `TRAKKT_MODE=personal`, it uses an embedded SQLite database and skips authentication entirely. On first boot the server automatically provisions a user and a workspace for you. This mode is ideal for solo use on your own machine -- no database server required.

See the [Installation](../getting-started/installation.md) guide for the full setup instructions.

## Inviting team members

> This section applies to SaaS mode. Personal mode has a single implicit user.

Open **Workspace Settings** from the sidebar and navigate to the **Members** section. From there you can invite new members by email. Each invite assigns a role:

| Role | Permissions |
|------|-------------|
| **Owner** | Full control, including workspace deletion and ownership transfer. |
| **Admin** | Manage teams, members, labels, projects, and all issues. |
| **Member** | Create and edit issues, comment, and manage their own work. |

A workspace has exactly one owner. Admins can do everything members can, plus manage workspace-level settings.

## Creating your first team

Teams are the organizational backbone of Trakkt. Every issue belongs to a team, and each team has its own identifier prefix.

1. Open **Workspace Settings** and go to the **Teams** section.
2. Click **Create Team**.
3. Fill in the details:
   - **Name** -- a human-readable name like "Engineering" or "Design".
   - **Key** -- a 2-5 character uppercase prefix (e.g. `ENG`, `DES`). This becomes the prefix for every issue in the team, so `ENG-1`, `ENG-2`, and so on.
   - **Icon and color** -- pick an icon and a color to visually distinguish the team in the sidebar and elsewhere.
4. Click **Create**.

Your new team appears in the sidebar. You can now start creating issues inside it.

> **Tip:** You can always change the team name, icon, and color later in [Team Settings](team-settings.md). The key cannot be changed after creation because it is baked into every issue identifier.

## Creating your first issue

1. Navigate to your team in the sidebar.
2. Press `c` on your keyboard, or click the **New Issue** button in the toolbar.
3. Enter a title -- a short summary of the work.
4. Optionally add a description using markdown formatting.
5. Set the priority, assignee, labels, or any other fields you need. All fields except the title are optional.
6. Save the issue.

Your issue is created in the **Backlog** status by default. From here you can move it through the workflow as work progresses. Read the [Issues](issues.md) chapter for the full rundown on statuses, priorities, and everything else an issue can hold.
