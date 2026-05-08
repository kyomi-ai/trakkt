// SPDX-License-Identifier: AGPL-3.0-or-later
#![recursion_limit = "512"]

//! trakkt-ui — Leptos frontend for Trakkt.

pub mod app;
pub mod cache;
pub mod components;
pub mod pages;
pub mod server_fns;
pub mod types;
pub mod utils;

pub use app::App;

/// Register all server functions with the Leptos runtime.
///
/// Must be called once at server startup before building the Axum router.
#[cfg(feature = "ssr")]
pub fn register_server_functions() {
    use leptos::server_fn::axum::register_explicit;

    // Auth
    use server_fns::auth::*;
    register_explicit::<GetAuthConfig>();
    register_explicit::<LoginWithPassword>();
    register_explicit::<SignupStart>();
    register_explicit::<SignupComplete>();
    register_explicit::<GoogleOauthCallback>();
    register_explicit::<ResendVerification>();
    register_explicit::<RecoveryStart>();
    register_explicit::<RecoveryVerify>();
    register_explicit::<RecoverySetPassword>();
    register_explicit::<PasskeyLoginStart>();
    register_explicit::<PasskeyLoginComplete>();
    register_explicit::<PasskeyRegisterStart>();
    register_explicit::<PasskeyRegisterComplete>();
    register_explicit::<PasskeySignupComplete>();
    register_explicit::<PasskeyRecoveryVerify>();

    // Context
    use server_fns::context::*;
    register_explicit::<GetUserContext>();

    // Profile
    use server_fns::profile::*;
    register_explicit::<GetProfile>();
    register_explicit::<GetPendingInvitations>();
    register_explicit::<UpdateProfileName>();
    register_explicit::<UpdateTheme>();
    register_explicit::<UpdateLandingPage>();
    register_explicit::<UpdateDefaultDashboard>();
    register_explicit::<UpdateQueryRetention>();
    register_explicit::<UpdateChartPalette>();
    register_explicit::<AcceptInvitation>();
    register_explicit::<DeclineInvitation>();

    // Security
    use server_fns::security::*;
    register_explicit::<HasPassword>();
    register_explicit::<SetPassword>();
    register_explicit::<ChangePassword>();
    register_explicit::<GetTotpStatus>();
    register_explicit::<SetupTotp>();
    register_explicit::<EnableTotp>();
    register_explicit::<DisableTotp>();
    register_explicit::<GetSessions>();
    register_explicit::<RevokeSession>();
    register_explicit::<Logout>();
    register_explicit::<LogoutAllSessions>();
    register_explicit::<ListPasskeys>();
    register_explicit::<StartPasskeyRegistration>();
    register_explicit::<CompletePasskeyRegistration>();
    register_explicit::<DeletePasskey>();
    register_explicit::<RenamePasskey>();

    // Sidebar
    use server_fns::sidebar::*;
    register_explicit::<GetRecentSessions>();
    register_explicit::<GetSidebarUser>();
    register_explicit::<ListUserWorkspaces>();
    register_explicit::<SwitchWorkspace>();

    // Team
    use server_fns::team::*;
    register_explicit::<ListWorkspaceMembers>();
    register_explicit::<UpdateMemberRole>();
    register_explicit::<RemoveMember>();
    register_explicit::<ListWorkspaceInvitations>();
    register_explicit::<InviteMember>();
    register_explicit::<CancelInvitation>();
    register_explicit::<ListOwnershipTransfers>();
    register_explicit::<CancelOwnershipTransfer>();
    register_explicit::<InitiateOwnershipTransfer>();

    // Ownership
    use server_fns::ownership::*;
    register_explicit::<GetOwnershipTransfer>();
    register_explicit::<AcceptOwnershipTransfer>();
    register_explicit::<DeclineOwnershipTransfer>();

    // Workspace
    use server_fns::workspace::*;
    register_explicit::<GetWorkspaceSettings>();
    register_explicit::<UpdateWorkspaceName>();
    register_explicit::<UpdateWorkspaceModel>();
    register_explicit::<UpdateWorkspaceChartmlConfig>();

    // Issues
    use server_fns::issues::*;
    register_explicit::<ListIssues>();
    register_explicit::<GetIssue>();
    register_explicit::<CreateIssue>();
    register_explicit::<UpdateIssue>();
    register_explicit::<DeleteIssue>();
    register_explicit::<SetIssueLabels>();

    // Comments
    use server_fns::comments::*;
    register_explicit::<ListComments>();
    register_explicit::<CreateComment>();
    register_explicit::<UpdateComment>();
    register_explicit::<DeleteComment>();

    // Labels
    use server_fns::labels::*;
    register_explicit::<ListLabels>();
    register_explicit::<CreateLabel>();
    register_explicit::<UpdateLabel>();
    register_explicit::<DeleteLabel>();

    // Notifications
    use server_fns::notifications::*;
    register_explicit::<ListNotifications>();
    register_explicit::<MarkNotificationRead>();
    register_explicit::<MarkAllNotificationsRead>();
    register_explicit::<CountUnreadNotifications>();

    // Teams (issue tracker)
    use server_fns::teams::*;
    register_explicit::<ListTeams>();
    register_explicit::<CreateTeam>();
    register_explicit::<GetDefaultTeam>();
}
