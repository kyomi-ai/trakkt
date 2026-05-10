// SPDX-License-Identifier: AGPL-3.0-or-later

//! View page — renders a saved view's filters as an issue list.
//!
//! Layout:
//! - Header: view name + actions dropdown (rename, delete)
//! - Content: issue list/board filtered by the view's persisted filters

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::{Icon, IconWeight};
use serde::{Deserialize, Serialize};

use crate::cache::store::SyncStore;
use crate::components::{Alert, AlertVariant, ConfirmDialog, EmptyState, INPUT_CLASS};
use crate::pages::issues::issue_row::IssueRow;
use crate::server_fns::views::{delete_view, list_views, update_view};
use trakkt_types::models::View;

// ─────────────────────────────────────────────────────────────────────────────
// Filter/display types for JSON deserialization
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct ViewFilters {
    #[serde(default)]
    statuses: Vec<String>,
    #[serde(default)]
    priorities: Vec<i32>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    search: String,
    #[serde(default)]
    team_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// View Page
// ─────────────────────────────────────────────────────────────────────────────

/// Saved view page — renders a filtered issue list/board based on persisted view config.
#[component]
pub fn ViewPage() -> impl IntoView {
    let params = use_params_map();
    let view_id = Memo::new(move |_| params.get().get("view_id").unwrap_or_default());
    let nav = use_navigate();

    // ── Data source: SyncStore with server function fallback ────────────────
    let sync_store = use_context::<SyncStore>();

    // Server function fallback for views.
    let server_views = Resource::new(|| (), move |_| async move { list_views().await });

    // Resolve the current view from SyncStore or fallback.
    let current_view: Memo<Option<View>> = Memo::new(move |_| {
        let id = view_id.get();
        if id.is_empty() {
            return None;
        }

        // Try SyncStore first.
        if let Some(store) = sync_store {
            let views = store.views().get();
            if !views.is_empty() || store.initialized().get() {
                return views.into_iter().find(|v| v.view_id == id);
            }
        }

        // Fallback to server function result.
        server_views
            .get()
            .and_then(|r| r.ok())
            .and_then(|views| views.into_iter().find(|v| v.view_id == id))
    });

    // Parse filters and display options from the view's JSON.
    let parsed_filters = Memo::new(move |_| {
        current_view
            .get()
            .and_then(|v| serde_json::from_str::<ViewFilters>(&v.filters).ok())
            .unwrap_or_default()
    });

    // ── Issue data source ──────────────────────────────────────────────────
    let server_issues = Resource::new(
        || (),
        move |_| async move {
            crate::server_fns::issues::list_issues(None, None, None, None, None, None, None, None)
                .await
        },
    );

    // Get all issues from SyncStore or fallback.
    let all_issues = Memo::new(move |_| {
        if let Some(store) = sync_store {
            let issues = store.issues().get();
            if !issues.is_empty() || store.initialized().get() {
                return issues;
            }
        }
        server_issues
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    });

    // Apply the view's filters to the issue list.
    let filtered_issues = Memo::new(move |_| {
        let issues = all_issues.get();
        let filters = parsed_filters.get();

        issues
            .into_iter()
            .filter(|issue| {
                // Filter by team_id.
                if !filters.team_id.is_empty() && issue.team_id != filters.team_id {
                    return false;
                }
                // Filter by statuses.
                if !filters.statuses.is_empty()
                    && !filters.statuses.contains(&issue.status_id)
                {
                    return false;
                }
                // Filter by priorities.
                if !filters.priorities.is_empty()
                    && !filters.priorities.contains(&issue.priority)
                {
                    return false;
                }
                // Filter by search.
                if !filters.search.is_empty()
                    && !issue.title.to_lowercase().contains(&filters.search.to_lowercase())
                {
                    return false;
                }
                // Filter by labels.
                if !filters.labels.is_empty() {
                    let issue_label_ids: Vec<&str> =
                        issue.labels.iter().map(|l| l.label_id.as_str()).collect();
                    if !filters.labels.iter().any(|lid| issue_label_ids.contains(&lid.as_str())) {
                        return false;
                    }
                }
                true
            })
            .collect::<Vec<_>>()
    });

    // ── Error state ─────────────────────────────────────────────────────────
    let error_msg = RwSignal::new(Option::<String>::None);

    // ── Rename state ───────────────────────────────────────────────────────
    let (renaming, set_renaming) = signal(false);
    let (rename_value, set_rename_value) = signal(String::new());
    let (rename_submitting, set_rename_submitting) = signal(false);

    // ── Delete state ───────────────────────────────────────────────────────
    let (confirm_delete_open, set_confirm_delete_open) = signal(false);

    // ── Actions dropdown ───────────────────────────────────────────────────
    let (actions_open, set_actions_open) = signal(false);

    let handle_rename_start = move || {
        if let Some(view) = current_view.get_untracked() {
            set_rename_value.set(view.name.clone());
            set_renaming.set(true);
            set_actions_open.set(false);
            error_msg.set(None);
        }
    };

    let handle_rename_submit = move || {
        let name = rename_value.get_untracked().trim().to_string();
        if name.is_empty() {
            set_renaming.set(false);
            return;
        }
        let Some(view) = current_view.get_untracked() else {
            set_renaming.set(false);
            return;
        };
        set_rename_submitting.set(true);
        let vid = view.view_id.clone();
        leptos::task::spawn_local(async move {
            match update_view(vid, Some(name), None, None, None, None, None).await {
                Ok(_) => error_msg.set(None),
                Err(e) => error_msg.set(Some(format!("Failed to rename: {e}"))),
            }
            set_rename_submitting.set(false);
            set_renaming.set(false);
        });
    };

    let handle_delete = {
        let nav = nav.clone();
        move || {
            let Some(view) = current_view.get_untracked() else { return };
            let vid = view.view_id.clone();
            let nav = nav.clone();
            leptos::task::spawn_local(async move {
                match delete_view(vid).await {
                    Ok(_) => nav("/my-issues", Default::default()),
                    Err(e) => error_msg.set(Some(format!("Failed to delete: {e}"))),
                }
            });
        }
    };

    let selected_index = Signal::derive(|| Option::<usize>::None);

    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                {move || {
                    if renaming.get() {
                        view! {
                            <form
                                class="flex items-center gap-2"
                                on:submit=move |ev: web_sys::SubmitEvent| {
                                    ev.prevent_default();
                                    handle_rename_submit();
                                }
                            >
                                <input
                                    type="text"
                                    autofocus=true
                                    class=INPUT_CLASS
                                    prop:value=move || rename_value.get()
                                    on:input=move |ev| set_rename_value.set(event_target_value(&ev))
                                    on:blur=move |_| handle_rename_submit()
                                    prop:disabled=move || rename_submitting.get()
                                />
                            </form>
                        }.into_any()
                    } else {
                        let name = current_view.get().map(|v| v.name.clone()).unwrap_or_else(|| "View".to_string());
                        view! {
                            <h1 class="text-sm font-semibold text-foreground flex items-center gap-2">
                                <Icon icon=phosphor_leptos::FUNNEL weight=IconWeight::Light size="16px"/>
                                {name}
                            </h1>
                        }.into_any()
                    }
                }}

                // Actions dropdown
                <div class="relative">
                    <button
                        class="p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
                        on:click=move |_| set_actions_open.update(|v| *v = !*v)
                        title="View actions"
                    >
                        <Icon icon=phosphor_leptos::DOTS_THREE weight=IconWeight::Bold size="16px"/>
                    </button>

                    <Show when=move || actions_open.get()>
                        <div class="absolute right-0 top-full mt-1 w-40 bg-popover border border-border rounded-lg shadow-lg py-1 z-50">
                            <button
                                class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                                on:click=move |_| handle_rename_start()
                            >
                                "Rename"
                            </button>
                            <button
                                class="w-full text-left px-4 py-2 text-sm text-destructive hover:bg-secondary transition-colors"
                                on:click=move |_| {
                                    set_actions_open.set(false);
                                    set_confirm_delete_open.set(true);
                                }
                            >
                                "Delete"
                            </button>
                        </div>
                    </Show>
                </div>
            </div>

            // ── Error alert ─────────────────────────────────────────────────
            <Show when=move || error_msg.get().is_some()>
                <div class="mx-5 mt-2">
                    <Alert variant=AlertVariant::Error>
                        {move || error_msg.get().unwrap_or_default()}
                    </Alert>
                </div>
            </Show>

            // ── Content area: issue list ────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                {move || {
                    let view = current_view.get();
                    if view.is_none() {
                        // View not found or still loading.
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::FUNNEL weight=IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        return view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="View not found"
                                    description="This saved view may have been deleted."
                                />
                            </div>
                        }.into_any();
                    }

                    let list = filtered_issues.get();

                    if list.is_empty() {
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::CLIPBOARD_TEXT weight=IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No matching issues"
                                    description="No issues match this view's filters."
                                />
                            </div>
                        }.into_any()
                    } else {
                        let rows = list.iter().enumerate().map(|(idx, issue)| {
                            view! { <IssueRow issue=issue.clone() index=idx selected_index=selected_index/> }
                        }).collect_view();
                        view! {
                            <div role="list">
                                {rows}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>

        // ── Delete confirmation dialog ──────────────────────────────────────
        <ConfirmDialog
            open=Signal::derive(move || confirm_delete_open.get())
            title="Delete view?"
            message="This saved view will be permanently deleted."
            confirm_text="Delete"
            on_confirm=Callback::new(move |()| {
                set_confirm_delete_open.set(false);
                handle_delete();
            })
            on_cancel=Callback::new(move |()| set_confirm_delete_open.set(false))
        />
    }
}
