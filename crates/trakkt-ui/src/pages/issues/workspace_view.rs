// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use super::issue_list::IssueListInner;

/// Workspace view page — loads a saved view by ID and renders cross-team issues.
#[component]
pub fn WorkspaceViewPage() -> impl IntoView {
    let params = use_params_map();
    let view_id = Signal::derive(move || params.read().get("view_id").unwrap_or_default());
    view! { <IssueListInner initial_view_id=view_id/> }
}
