# Workspace Settings

Workspace settings control the shared configuration that applies to everyone in your workspace. Open them from the sidebar by clicking on your workspace name or navigating to the settings page.

## Workspace name

You can change your workspace's display name at any time. This is the name shown in the sidebar and anywhere else the workspace is referenced.

## Teams

The Teams section lists every team in the workspace. From here you can:

- **Create a new team** -- add teams as your organization grows. See [Getting Started](getting-started.md#creating-your-first-team) for a walkthrough.
- **Set the default team** -- choose which team is selected by default when creating issues from the command palette or other contexts where no team is pre-selected.
- **Open team settings** -- click a team to go to its [Team Settings](team-settings.md) page.

## Labels

The Labels page lets you manage workspace-wide labels that are available to all teams. From here you can:

- **Create a label** -- give it a name and pick a color from the preset palette (Red, Orange, Yellow, Green, Teal, Blue, Violet, Pink, Gray, Black) or enter a custom hex code.
- **Edit a label** -- change the name or color of any existing label.
- **Delete a label** -- remove a label from the workspace. Issues that have the label will lose it.

Labels provide a flexible taxonomy for categorizing issues. Common uses include `bug`, `feature`, `improvement`, `documentation`, or any categories that fit your workflow. See [Issues](issues.md#labels) for how labels appear on issues.

> **Note:** Teams can also have their own scoped labels. See [Team Settings](team-settings.md) for team-level label management.

## Integrations

The Integrations page lets you connect external services to your workspace.

### GitHub

Connect your GitHub organization to link pull requests, branches, and commits to Trakkt issues. The integration has three states:

- **Not configured** -- the GitHub App has not been set up. Self-hosted users need to configure the GitHub App environment variables first.
- **Not connected** -- the GitHub App is configured but your workspace is not connected. Click **Connect GitHub** to start the installation flow.
- **Connected** -- your GitHub organization is linked. You can see which repositories are connected and disconnect the integration if needed.

When connected, Trakkt links a pull request to an issue by **intent**, the way Linear does. A key such as `TRA-35` in a PR description is only a mention.

### How a PR links to an issue

| What the PR says | Link | What it does |
| --- | --- | --- |
| The key in the **branch name** or the **PR title** | Working link | Starts the issue when the PR opens, completes it when the PR merges |
| `Closes`, `Fixes`, `Resolves`, `Completes`, `Implements` (and their `-es`, `-ed`, `-ing` forms) before the key | Closing | Starts the issue; completes it when the PR merges |
| `Refs`, `References`, `Part of`, `Contributes to`, `Toward`, `Towards` before the key | Contributing | Starts the issue; never completes it |
| `Related to`, `Relates to` before the key, or a bare key in the description | Related | Shown on the issue and the PR; never changes status |
| `Skip` or `Ignore` before the key | None | No link and no automation, even if the branch name has the key |

Several keys can follow one word: `Fixes TRA-12, TRA-13 and TRA-14`. The word has to come directly before the key, so `Fixes the crash described in TRA-12` only mentions it. A full Trakkt issue URL works like a key. Words in PR comments never link. A magic word in a PR's title or description decides that key's link; a bare key in the title is a working link, a bare key in the description is related.

### What the automation does

- A **draft** PR does nothing until it is marked ready for review (a team can give the draft step its own rule). Reopening a PR starts the issue like opening it.
- When a PR merges, the issue is completed **only after the last open PR linked to it is done**. A PR closed without merging does not complete anything; if a sibling PR already merged, the issue is completed when the last open one closes.
- A PR that closes an issue and is **closed without merging** moves it to **Cancelled** when nothing else is linked. Under the rule below that is final.
- Issues are **never moved backwards**, and **Completed or Cancelled issues are never moved by GitHub**: reopening finished work is your decision.
- Editing a PR is not an event of its own: an issue starts at the next open, ready or reopen event, or when the PR merges.
- When automation changes an issue's status it comments on the PR. Turn that off in **Settings > Integrations > Team automation**; the switch covers the comment posted when you complete an issue by hand as well.

The default rules (PR opened or ready -> **Started**, last PR merged -> **Completed**, closing PR closed without merge -> **Cancelled**) apply workspace-wide, and workspace admins can toggle each one in the Integrations page. Under **Team automation** a team can replace any step for its own issues with a different status or turn it off. Your workspace must have statuses in the corresponding categories for transitions to apply.

The REST and MCP `list_issue_github_links` response carries a `link_kind` (`closing`, `contributing`, `branch_title` or `related`) on each link in place of the old `close_intent` flag.

## Billing

The Billing page is available in SaaS mode and lets workspace owners manage their subscription through Stripe.

- **Subscribe** -- choose a plan and enter payment details.
- **Manage subscription** -- view your current plan, upcoming invoices, and payment method. Click **Manage Billing** to open the Stripe billing portal.
- **Cancel or reactivate** -- cancel your subscription (effective at period end) or reactivate a pending cancellation.

> **Note:** Billing is only available in SaaS mode. Self-hosted instances do not require a subscription.

## Members and roles

Member management — including inviting users, changing roles, and removing members — is found under **Settings > Team**. See [Team Settings](team-settings.md) for details.
