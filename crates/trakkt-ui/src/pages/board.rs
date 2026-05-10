// SPDX-License-Identifier: AGPL-3.0-or-later

//! Board page — Kanban view of issues grouped by status.
//!
//! Layout follows DESIGN.md "Board Page (Kanban)" spec:
//! - Page header: "Board" title (Instrument Serif)
//! - Content: 5 status columns (Backlog, Todo, In Progress, Done, Cancelled)
//! - Each column: header with status name + count, scrollable card list
//! - Cards: issue number, title, labels, priority + assignee footer
//!
//! Drag and drop:
//! - HTML5 drag API (dragstart, dragover, dragenter, dragleave, drop)
//! - Optimistic UI: card moves immediately, reverts on server error
//! - Same-column drops are no-ops

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use wasm_bindgen::JsCast;

use crate::components::{Alert, AlertVariant, Avatar, Button, ButtonSize, ButtonVariant, Checkbox, IssueStatusBadge, IssueStatusVariant, LabelBadge, PriorityIndicator, SearchInput, Skeleton};
use crate::pages::issues::filters::PriorityFilterDropdown;
use crate::server_fns::issues::{list_issues, update_issue};
use crate::server_fns::statuses::list_statuses;
use trakkt_types::models::{IssueWithDetails, Status};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Sort issues within a column: priority ASC (urgent=1 first, none=0 last),
/// then created_at DESC (newest first).
fn sort_column_issues(issues: &mut [IssueWithDetails]) {
    issues.sort_by(|a, b| {
        // Priority 0 (none) should sort last, so map 0 → i32::MAX.
        let pa = if a.priority == 0 { i32::MAX } else { a.priority };
        let pb = if b.priority == 0 { i32::MAX } else { b.priority };
        pa.cmp(&pb).then_with(|| b.created_at.cmp(&a.created_at))
    });
}

/// Group a flat list of issues into per-status columns, sorted.
///
/// Columns are ordered by the `statuses` list (server returns them ordered by
/// category then position). Issues are grouped by `status_id`.
fn group_by_status(statuses: &[Status], all: &[IssueWithDetails]) -> Vec<(Status, Vec<IssueWithDetails>)> {
    statuses
        .iter()
        .map(|status| {
            let mut col_issues: Vec<IssueWithDetails> = all
                .iter()
                .filter(|i| i.status_id == status.status_id)
                .cloned()
                .collect();
            sort_column_issues(&mut col_issues);
            (status.clone(), col_issues)
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// localStorage helpers for column visibility
// ─────────────────────────────────────────────────────────────────────────────

/// Build the localStorage key for hidden columns.
/// Team-scoped boards use `trakkt-board-hidden-{team_key}`, workspace board
/// uses `trakkt-board-hidden-global`.
fn storage_key(team_key: &Option<String>) -> String {
    match team_key {
        Some(key) => format!("trakkt-board-hidden-{key}"),
        None => "trakkt-board-hidden-global".to_string(),
    }
}

/// Read the set of hidden status IDs from localStorage.
/// Returns `None` if the key doesn't exist (first visit).
#[cfg(target_arch = "wasm32")]
fn read_hidden_from_storage(key: &str) -> Option<HashSet<String>> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let json = storage.get_item(key).ok()??;
    let ids: Vec<String> = serde_json::from_str(&json).ok()?;
    Some(ids.into_iter().collect())
}

/// Write the set of hidden status IDs to localStorage.
#[cfg(target_arch = "wasm32")]
fn write_hidden_to_storage(key: &str, hidden: &HashSet<String>) {
    let Ok(json) = serde_json::to_string(&hidden.iter().collect::<Vec<_>>()) else {
        return;
    };
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
    {
        let _ = storage.set_item(key, &json);
    }
}

/// Compute the default hidden set: hide all statuses with category "cancelled".
#[cfg(target_arch = "wasm32")]
fn default_hidden(statuses: &[Status]) -> HashSet<String> {
    statuses
        .iter()
        .filter(|s| s.category == "cancelled")
        .map(|s| s.status_id.clone())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Page
// ─────────────────────────────────────────────────────────────────────────────

/// Kanban board page — issues arranged in status columns with drag-and-drop.
///
/// When mounted at `/teams/:key/board`, reads the `:key` route param to scope
/// the board to a single team. When mounted at `/board` (no `:key` param),
/// all workspace issues and statuses are shown.
#[component]
pub fn BoardPage() -> impl IntoView {
    // If mounted under `/teams/:key/board`, read the team key from route params.
    // At `/board` there is no `:key` param, so this yields None — global board.
    let team_key = {
        let params = leptos_router::hooks::use_params_map();
        params.read().get("key")
    };

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // ── Error state for server function failures ──────────────────────────
    let issues_error = RwSignal::new(Option::<String>::None);
    let statuses_error = RwSignal::new(Option::<String>::None);

    // ── Resolve team from SyncStore ────────────────────────────────────────
    let team_key_for_memo = team_key.clone();
    let team = Memo::new(move |_| {
        let key = team_key_for_memo.as_ref()?;
        let store = sync_store?;
        store.teams().get().into_iter().find(|t| t.key.to_lowercase() == *key)
    });

    let server_issues = Resource::new(
        || (),
        move |_| async move { list_issues(None, None, None, None, None, None, None, None).await },
    );

    let all_issues = Memo::new(move |_| {
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if !items.is_empty() || store.initialized().get() {
                return items;
            }
        }
        match server_issues.get() {
            Some(Ok(items)) => {
                issues_error.set(None);
                items
            }
            Some(Err(e)) => {
                issues_error.set(Some(format!("Failed to load issues: {e}")));
                Vec::new()
            }
            None => Vec::new(),
        }
    });

    // Filter issues by team when a team is resolved.
    let issues = Memo::new(move |_| {
        let all = all_issues.get();
        match team.get() {
            Some(ref t) => all.into_iter().filter(|i| i.team_id == t.team_id).collect(),
            None => all,
        }
    });

    // Statuses are workspace config — fetched once via server function.
    // They don't change frequently enough to need SyncStore.
    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    let all_statuses = Memo::new(move |_| {
        match statuses_resource.get() {
            Some(Ok(items)) => {
                statuses_error.set(None);
                items
            }
            Some(Err(e)) => {
                statuses_error.set(Some(format!("Failed to load statuses: {e}")));
                Vec::new()
            }
            None => Vec::new(),
        }
    });

    // Filter statuses by team: show global (team_id=None) + team-specific.
    let statuses = Memo::new(move |_| {
        let all = all_statuses.get();
        match team.get() {
            Some(ref t) => all.into_iter().filter(|s| {
                s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id)
            }).collect(),
            None => all,
        }
    });

    // ── Hidden columns state ─────────────────────────────────────────────
    // Tracks which status columns are hidden. Initialized from localStorage
    // or defaults to hiding "cancelled" category on first visit.
    let hidden_statuses: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let key_for_storage = storage_key(&team_key);

    // Initialize hidden set once statuses are loaded.
    {
        let key = key_for_storage.clone();
        let initialized = StoredValue::new(false);
        Effect::new(move |_| {
            let s = statuses.get();
            if s.is_empty() || initialized.get_value() {
                return;
            }
            initialized.set_value(true);
            #[cfg(not(target_arch = "wasm32"))]
            let _ = (&key, &s);
            #[cfg(target_arch = "wasm32")]
            {
                let initial = match read_hidden_from_storage(&key) {
                    Some(stored) => stored,
                    None => {
                        let defaults = default_hidden(&s);
                        write_hidden_to_storage(&key, &defaults);
                        defaults
                    }
                };
                hidden_statuses.set(initial);
            }
        });
    }

    // ── Filter state ─────────────────────────────────────────────────────────
    let (search, set_search) = signal(String::new());
    let (priority_filter, set_priority_filter) = signal(String::new());

    // Client-side filtered issues: search + priority applied before grouping.
    let filtered_issues = Memo::new(move |_| {
        let raw = issues.get();
        let search_val = search.get().to_lowercase();
        let priority_val = priority_filter.get();

        raw.into_iter()
            .filter(|issue| {
                if !priority_val.is_empty() {
                    if let Ok(p) = priority_val.parse::<i32>() {
                        if issue.priority != p {
                            return false;
                        }
                    }
                }
                if !search_val.is_empty() && !issue.title.to_lowercase().contains(&search_val) {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>()
    });

    // ── Drag state ──────────────────────────────────────────────────────────
    let (dragging, set_dragging) = signal(Option::<String>::None);
    let (drag_target, set_drag_target) = signal(Option::<String>::None);

    // ── Drop handler ────────────────────────────────────────────────────────
    let handle_drop = move |issue_id: String, target_status_id: String| {
        let current_issues = issues.get_untracked();
        let issue = match current_issues.iter().find(|i| i.issue_id == issue_id) {
            Some(i) => i.clone(),
            None => return,
        };

        if issue.status_id == target_status_id {
            return;
        }

        let old_status_id = issue.status_id.clone();
        let old_status_name = issue.status_name.clone();
        let old_status_category = issue.status_category.clone();
        let issue_number = issue.number;

        let current_statuses = statuses.get_untracked();
        let target_status = current_statuses.iter().find(|s| s.status_id == target_status_id);
        let target_name = target_status.map(|s| s.name.clone()).unwrap_or_default();
        let target_category = target_status.map(|s| s.category.clone()).unwrap_or_default();

        // Optimistic update via SyncStore.
        if let Some(store) = sync_store {
            let mut updated = issue.clone();
            updated.status_id = target_status_id.clone();
            updated.status_name = target_name;
            updated.status_category = target_category;
            store.upsert_issue(updated);
        }

        let target_id_for_server = target_status_id.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = update_issue(
                issue_number,
                None, None,
                Some(target_id_for_server),
                None, None, None, None, None,
            ).await {
                tracing::warn!("Failed to update issue status: {e}");
                // Revert optimistic update on failure.
                if let Some(store) = sync_store {
                    let mut reverted = issue;
                    reverted.status_id = old_status_id;
                    reverted.status_name = old_status_name;
                    reverted.status_category = old_status_category;
                    store.upsert_issue(reverted);
                }
            }
        });
    };

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">
                    {move || match team.get() {
                        Some(t) => format!("{} Board", t.name),
                        None => "Board".to_string(),
                    }}
                </h1>
                <BoardDisplayOptions
                    statuses=statuses
                    hidden=hidden_statuses
                    storage_key=key_for_storage.clone()
                />
            </div>

            // ── Toolbar ─────────────────────────────────────────────────────
            <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |v: String| set_search.set(v))
                    placeholder="Filter cards..."
                    class="flex-1 max-w-sm"
                />
                <PriorityFilterDropdown
                    value=priority_filter
                    on_change=Callback::new(move |v: String| set_priority_filter.set(v))
                />
            </div>

            // ── Error alert ─────────────────────────────────────────────────
            <Show when=move || issues_error.get().is_some() || statuses_error.get().is_some()>
                <div class="mx-4 mt-4 space-y-2">
                    {move || issues_error.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error>{msg}</Alert>
                    })}
                    {move || statuses_error.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error>{msg}</Alert>
                    })}
                </div>
            </Show>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-x-auto px-4 md:px-6 py-4" style="scrollbar-width: thin;">
                {move || {
                    let s = statuses.get();
                    if s.is_empty() {
                        view! { <BoardSkeleton/> }.into_any()
                    } else {
                        let grouped_all = Memo::new(move |_| group_by_status(&statuses.get(), &filtered_issues.get()));
                        let grouped = move || {
                            let hidden = hidden_statuses.get();
                            grouped_all.get()
                                .into_iter()
                                .filter(|(status, _)| !hidden.contains(&status.status_id))
                                .collect::<Vec<_>>()
                        };
                        view! {
                            <div class="flex gap-4 h-full">
                                {move || grouped().into_iter().map(|(status, issues)| {
                                    let status_id = status.status_id.clone();
                                    let status_name = status.name.clone();
                                    let status_variant = IssueStatusVariant::parse(&status.category);
                                    let count = issues.len();
                                    view! {
                                        <BoardColumn
                                            status_id=status_id
                                            label=status_name
                                            status_variant=status_variant
                                            count=count
                                            issues=issues
                                            dragging=dragging
                                            set_dragging=set_dragging
                                            drag_target=drag_target
                                            set_drag_target=set_drag_target
                                            on_drop=handle_drop
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Display Options
// ─────────────────────────────────────────────────────────────────────────────

/// Popover trigger + content for toggling column visibility.
///
/// Shows a "Display" button with sliders icon. When clicked, a popover lists
/// every status with a checkbox. Toggling a checkbox hides/shows that column
/// and persists the choice to localStorage.
#[component]
fn BoardDisplayOptions(
    /// All statuses available for this board.
    statuses: Memo<Vec<Status>>,
    /// The set of hidden status IDs (read/write).
    hidden: RwSignal<HashSet<String>>,
    /// localStorage key for persistence.
    storage_key: String,
) -> impl IntoView {
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (open, set_open) = signal(false);

    let _key = StoredValue::new(storage_key);
    let hidden_count = Memo::new(move |_| hidden.get().len());

    view! {
        <div class="flex items-center gap-2">
            <div node_ref=trigger_ref>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click=move |_| set_open.update(|o| *o = !*o)
                >
                    <Icon icon=phosphor_leptos::SLIDERS_HORIZONTAL size="14px"/>
                    "Display"
                    {move || {
                        let n = hidden_count.get();
                        if n > 0 {
                            view! {
                                <span class="text-xs text-muted-foreground ml-0.5">
                                    {format!("({n} hidden)")}
                                </span>
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                </Button>
            </div>

            <crate::components::popover::Popover
                trigger_ref=trigger_ref
                open=Signal::derive(move || open.get())
                on_close=Callback::new(move |()| set_open.set(false))
                placement=crate::components::popover::Placement::BOTTOM_END
                class="bg-popover border border-border rounded-lg shadow-lg p-3 min-w-[200px]"
            >
                <div class="flex flex-col gap-1">
                    <div class="text-xs font-semibold text-muted-foreground mb-1 px-1">
                        "Status columns"
                    </div>
                    {move || statuses.get().into_iter().map(|status| {
                        let status_id = status.status_id.clone();
                        let status_id_for_toggle = status_id.clone();
                        let status_name = status.name.clone();
                        let status_variant = IssueStatusVariant::parse(&status.category);
                        let is_visible = {
                            let sid = status_id.clone();
                            Signal::derive(move || !hidden.get().contains(&sid))
                        };
                        view! {
                            <div class="flex items-center gap-2 px-1 py-1 rounded hover:bg-accent cursor-pointer">
                                <Checkbox
                                    checked=is_visible
                                    on_change=Callback::new(move |checked: bool| {
                                        hidden.update(|set| {
                                            if checked {
                                                set.remove(&status_id_for_toggle);
                                            } else {
                                                set.insert(status_id_for_toggle.clone());
                                            }
                                        });
                                        #[cfg(target_arch = "wasm32")]
                                        _key.with_value(|k| {
                                            write_hidden_to_storage(k, &hidden.get_untracked());
                                        });
                                    })
                                />
                                <IssueStatusBadge status=status_variant/>
                                <span class="text-sm text-foreground">{status_name}</span>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </crate::components::popover::Popover>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Column
// ─────────────────────────────────────────────────────────────────────────────

/// A single Kanban column for one status.
///
/// DESIGN.md: `min-w-[280px] max-w-[320px] flex flex-col`
/// - Column header: sticky top, status name + issue count
/// - Cards container: scrollable, flex-col gap-2
#[component]
fn BoardColumn(
    /// The status_id for this column (used for drag-drop targeting).
    status_id: String,
    /// Human-readable label (e.g. "In Progress").
    label: String,
    /// Status variant for the SVG icon in the column header.
    status_variant: IssueStatusVariant,
    /// Number of issues in this column.
    count: usize,
    /// Issues to render in this column.
    issues: Vec<IssueWithDetails>,
    /// Signal: which issue_id is currently being dragged.
    dragging: ReadSignal<Option<String>>,
    /// Setter for the dragging signal.
    set_dragging: WriteSignal<Option<String>>,
    /// Signal: which status_id is the current drag target.
    drag_target: ReadSignal<Option<String>>,
    /// Setter for the drag target signal.
    set_drag_target: WriteSignal<Option<String>>,
    /// Callback when an issue is dropped on this column.
    #[prop(into)]
    on_drop: Callback<(String, String)>,
) -> impl IntoView {
    let status_id_for_over = status_id.clone();
    let status_id_for_drop = status_id.clone();
    let status_id_for_class = status_id.clone();

    // Determine if this column is the active drop target.
    let is_drop_target = move || {
        drag_target.get().as_deref() == Some(status_id_for_class.as_str())
    };

    let column_class = move || {
        let base = "min-w-[280px] max-w-[320px] flex flex-col rounded-lg transition-colors duration-200";
        if is_drop_target() {
            format!("{base} border-2 border-primary bg-primary/5")
        } else {
            format!("{base} border-2 border-transparent")
        }
    };

    view! {
        <div
            class=column_class
            on:dragover=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                set_drag_target.set(Some(status_id_for_over.clone()));
            }
            on:dragleave=move |ev: web_sys::DragEvent| {
                let should_clear = match (ev.current_target(), ev.related_target()) {
                    (Some(target), Some(related)) => {
                        match (
                            target.dyn_into::<web_sys::Node>(),
                            related.dyn_into::<web_sys::Node>(),
                        ) {
                            (Ok(container), Ok(rel_node)) => !container.contains(Some(&rel_node)),
                            _ => true,
                        }
                    }
                    _ => true,
                };
                if should_clear {
                    set_drag_target.set(None);
                }
            }
            on:drop={
                let status_id_drop = status_id_for_drop.clone();
                move |ev: web_sys::DragEvent| {
                    ev.prevent_default();
                    if let Some(dt) = ev.data_transfer()
                        && let Ok(issue_id) = dt.get_data("text/plain")
                        && !issue_id.is_empty()
                    {
                        on_drop.run((issue_id, status_id_drop.clone()));
                    }
                    set_drag_target.set(None);
                }
            }
        >
            // ── Column header ───────────────────────────────────────────────
            <div class="sticky top-0 bg-background pb-3 pt-1 px-2">
                <div class="flex items-center gap-2">
                    <IssueStatusBadge status=status_variant/>
                    <span class="font-medium text-sm text-foreground">{label}</span>
                    <span class="text-muted-foreground text-xs">{count}</span>
                </div>
            </div>

            // ── Cards container ─────────────────────────────────────────────
            <div class="flex flex-col gap-2 flex-1 overflow-y-auto px-1 pb-2" style="scrollbar-width: thin;">
                {if issues.is_empty() {
                    view! {
                        <div class="flex items-center justify-center py-8 text-muted-foreground text-sm">
                            "No issues"
                        </div>
                    }.into_any()
                } else {
                    issues.into_iter().map(|issue| {
                        view! {
                            <BoardCard
                                issue=issue
                                dragging=dragging
                                set_dragging=set_dragging
                            />
                        }
                    }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Card
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue card on the Kanban board.
///
/// DESIGN.md Kanban Card Pattern:
/// ```text
/// ┌─────────────────────┐
/// │ TRK-42              │  ← Geist Mono, text-xs, text-muted
/// │ Fix login redirect  │  ← DM Sans, text-sm, font-medium
/// │ loop                │
/// │                     │
/// │ [bug] [auth]        │  ← label pills
/// │ ■ Urgent    @jason  │  ← priority + assignee
/// └─────────────────────┘
/// ```
///
/// - `bg-card border border-border rounded-md p-4 shadow-sm`
/// - Hover: `shadow-md` with `transition-shadow`
/// - Dragging: reduced opacity
/// - Click: navigate to `/issues/:number`
#[component]
fn BoardCard(
    issue: IssueWithDetails,
    /// Signal: which issue_id is currently being dragged.
    dragging: ReadSignal<Option<String>>,
    /// Setter for the dragging signal.
    set_dragging: WriteSignal<Option<String>>,
) -> impl IntoView {
    let issue_id = issue.issue_id.clone();
    let issue_id_for_drag = issue_id.clone();
    let issue_id_for_opacity = issue_id.clone();
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let number = issue.number;
    let title = issue.title.clone();
    let labels = issue.labels.clone();
    let priority = issue.priority;
    let assignee_name = issue.assignee_name.clone();

    let (did_drag, set_did_drag) = signal(false);

    // Track if THIS card is being dragged for opacity.
    let is_dragging = move || dragging.get().as_deref() == Some(issue_id_for_opacity.as_str());

    let card_class = move || {
        let base = "bg-card border border-border rounded-md p-4 shadow-sm hover:shadow-md transition-shadow cursor-grab focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
        if is_dragging() {
            format!("{base} opacity-50")
        } else {
            base.to_string()
        }
    };

    let navigate_to_issue = move |_: web_sys::MouseEvent| {
        if did_drag.get_untracked() {
            set_did_drag.set(false);
            return;
        }
        let nav = use_navigate();
        nav(&format!("/issues/{number}"), Default::default());
    };

    view! {
        <div
            class=card_class
            draggable="true"
            tabindex="0"
            on:dragstart={
                let issue_id_ds = issue_id_for_drag.clone();
                move |ev: web_sys::DragEvent| {
                    if let Some(dt) = ev.data_transfer() {
                        let _ = dt.set_data("text/plain", &issue_id_ds);
                        // Set drag effect.
                        dt.set_effect_allowed("move");
                    }
                    set_dragging.set(Some(issue_id_ds.clone()));
                }
            }
            on:dragend=move |_: web_sys::DragEvent| {
                set_dragging.set(None);
                set_did_drag.set(true);
            }
            on:click=navigate_to_issue
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" {
                    let nav = use_navigate();
                    nav(&format!("/issues/{number}"), Default::default());
                }
            }
        >
            // Issue number
            <div class="font-mono text-xs text-muted-foreground mb-1">
                {issue_key}
            </div>

            // Title (2-line clamp)
            <div class="text-sm font-medium text-foreground line-clamp-2">
                {title}
            </div>

            // Labels
            {if !labels.is_empty() {
                view! {
                    <div class="flex gap-1 flex-wrap mt-2">
                        {labels.iter().map(|label| {
                            view! {
                                <LabelBadge
                                    name=label.name.clone()
                                    color=label.color.clone()
                                />
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

            // Footer: priority + assignee
            <div class="flex items-center justify-between mt-3">
                <PriorityIndicator priority=priority/>
                {if let Some(ref name) = assignee_name {
                    view! {
                        <Avatar name=name.clone()/>
                    }.into_any()
                } else {
                    view! {
                        <span class="w-5 h-5 shrink-0"></span>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading Skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Five skeleton columns with placeholder card shapes for the loading state.
///
/// DESIGN.md Loading State Pattern: "Content-shaped Skeleton rectangles".
#[component]
fn BoardSkeleton() -> impl IntoView {
    let columns = (0..5).map(|_| {
        let cards = (0..3).map(|_| {
            view! {
                <div class="bg-card border border-border rounded-md p-4">
                    // Issue number placeholder
                    <Skeleton class="w-14 h-3 mb-2"/>
                    // Title placeholder (2 lines)
                    <Skeleton class="w-full h-4 mb-1"/>
                    <Skeleton class="w-3/4 h-4 mb-3"/>
                    // Labels placeholder
                    <div class="flex gap-1 mb-3">
                        <Skeleton class="w-10 h-5 rounded-sm"/>
                        <Skeleton class="w-12 h-5 rounded-sm"/>
                    </div>
                    // Footer: priority + avatar
                    <div class="flex items-center justify-between">
                        <Skeleton class="w-3.5 h-3.5 rounded-[2px]"/>
                        <Skeleton class="w-5 h-5 rounded-full"/>
                    </div>
                </div>
            }
        }).collect_view();

        view! {
            <div class="min-w-[280px] max-w-[320px] flex flex-col">
                // Column header skeleton
                <div class="pb-3 pt-1 px-2">
                    <div class="flex items-center gap-2">
                        <Skeleton class="w-3.5 h-3.5 rounded-full"/>
                        <Skeleton class="w-20 h-4"/>
                        <Skeleton class="w-4 h-3"/>
                    </div>
                </div>
                // Card skeletons
                <div class="flex flex-col gap-2 px-1">
                    {cards}
                </div>
            </div>
        }
    }).collect_view();

    view! {
        <div class="flex gap-4 h-full">
            {columns}
        </div>
    }
}
