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

When connected, Trakkt can automatically link PRs and commits that reference issue identifiers (e.g., `TRA-35`) in their title or description.

## Billing

The Billing page is available in SaaS mode and lets workspace owners manage their subscription through Stripe.

- **Subscribe** -- choose a plan and enter payment details.
- **Manage subscription** -- view your current plan, upcoming invoices, and payment method. Click **Manage Billing** to open the Stripe billing portal.
- **Cancel or reactivate** -- cancel your subscription (effective at period end) or reactivate a pending cancellation.

> **Note:** Billing is only available in SaaS mode. Self-hosted instances do not require a subscription.

## Members and roles

Member management — including inviting users, changing roles, and removing members — is found under **Settings > Team**. See [Team Settings](team-settings.md) for details.
