// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project-scoped Kanban board — groups issues by status **category** rather
//! than individual team-specific statuses.
//!
//! Projects span teams, so the board shows 5 "virtual" columns:
//! Backlog, Todo, In Progress, Done, Cancelled.
//!
//! Each issue is mapped to the correct column by looking up its status's
//! `category` field. Drag-and-drop across columns finds the first status
//! in the target category that belongs to the issue's team.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::Icon;
use wasm_bindgen::JsCast;

use crate::components::{
    Avatar, Button, ButtonSize, ButtonVariant, Checkbox, IssueStatusBadge,
    IssueStatusVariant, LabelBadge, PriorityIndicator, SearchInput, Skeleton,
    TeamKeyBadge,
};
use crate::components::toast::toast_error;
use crate::pages::issues::filters::{AssigneeFilterDropdown, LabelFilterDropdown, PriorityFilterDropdown};
use crate::server_fns::issues::update_issue;
use crate::types::IssueNavState;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// The 5 status categories used as virtual board columns, in display order.
const CATEGORY_COLUMNS: &[(&str, &str)] = &[
    ("backlog", "Backlog"),
    ("unstarted", "Todo"),
    ("started", "In Progress"),
    ("completed", "Done"),
    ("cancelled", "Cancelled"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Sort issues within a column: priority ASC (0 → last) then created_at DESC.
fn sort_column_issues(issues: &mut [IssueWithDetails]) {
    issues.sort_by(|a, b| {
        let pa = if a.priority == 0 { i32::MAX } else { a.priority };
        let pb = if b.priority == 0 { i32::MAX } else { b.priority };
        pa.cmp(&pb).then_with(|| b.created_at.cmp(&a.created_at))
    });
}

/// Map a status category string to an `IssueStatusVariant` for icon rendering.
fn category_to_variant(category: &str) -> IssueStatusVariant {
    match category {
        "backlog" => IssueStatusVariant::Backlog,
        "unstarted" => IssueStatusVariant::Unstarted,
        "started" => IssueStatusVariant::Started,
        "completed" => IssueStatusVariant::Completed,
        "cancelled" => IssueStatusVariant::Cancelled,
        _ => IssueStatusVariant::Backlog,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// localStorage helpers for hidden columns
// ─────────────────────────────────────────────────────────────────────────────

/// Build the localStorage key for hidden columns on a project board.
#[cfg(target_arch = "wasm32")]
fn storage_key(project_id: &str) -> String {
    format!("trakkt-board-hidden-project-{project_id}")
}

/// Read the set of hidden category names from localStorage.
#[cfg(target_arch = "wasm32")]
fn read_hidden_from_storage(key: &str) -> Option<HashSet<String>> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let json = storage.get_item(key).ok()??;
    let ids: Vec<String> = serde_json::from_str(&json).ok()?;
    Some(ids.into_iter().collect())
}

/// Write the set of hidden category names to localStorage.
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

/// Default hidden set: hide "cancelled" column.
#[cfg(target_arch = "wasm32")]
fn default_hidden() -> HashSet<String> {
    let mut set = HashSet::new();
    set.insert("cancelled".to_string());
    set
}

// ─────────────────────────────────────────────────────────────────────────────
// Board interaction state
// ─────────────────────────────────────────────────────────────────────────────

/// What the user has told the board to do: the four toolbar filters, and the
/// drag they are part-way through.
///
/// Created by `ProjectDetailPage` as part of `ProjectEditState` and passed in,
/// not created in [`ProjectBoardContent`]'s body, and that is load-bearing
/// rather than stylistic. `ProjectDetailPage`'s content closure reads six
/// resources; any one settling re-runs it, which reconstructs
/// `ProjectDetailContent`, which rebuilds the `{move || active_view…}` child
/// that builds this component. A dynamic child's `rebuild` is an unconditional
/// `build()` of a fresh `RenderEffect` followed by `unmount()` of the old one
/// (`tachys::reactive_graph`), so this component function runs again and every
/// signal declared in its body is replaced at its initial value.
///
/// Three of those six move on workspace-wide counters — `milestones_version`,
/// `project_updates_version`, `project_members_version` — so a colleague's
/// milestone, status update or membership change on *any* project re-runs it.
/// The two that are project-scoped are enough on their own: `project_issues` is
/// a `Memo` over `SyncStore::issues` filtered to this project, which this
/// board's own drop handler changes every time a card is moved, since it writes
/// the moved issue back to the store.
///
/// See `ProjectEditState` in `project_detail.rs` for the rest of the reasoning
/// and for where these are cleared when the router reuses the page for a
/// different project.
#[derive(Clone, Copy)]
pub struct BoardViewState {
    search: RwSignal<String>,
    priority_filter: RwSignal<Vec<String>>,
    label_filter: RwSignal<Vec<String>>,
    assignee_filter: RwSignal<Vec<String>>,
    /// Which issue_id is mid-drag, or `None`.
    ///
    /// Held here so a sync frame landing mid-drag does not silently drop the
    /// card's half-opacity and the target column's outline. It does not rescue
    /// the drag itself: the rebuild detaches the node being dragged, and no
    /// amount of surviving state puts it back. That half is TRA-10031.
    ///
    /// Cleared by `ProjectBoardColumn`'s `on:drop` as well as the card's
    /// `on:dragend`, because the drop is what triggers the rebuild that detaches
    /// the card — see the comment at that call.
    dragging: RwSignal<Option<String>>,
    /// Which category column the pointer is currently over, or `None`.
    drag_target: RwSignal<Option<String>>,
}

impl Default for BoardViewState {
    fn default() -> Self {
        Self {
            search: RwSignal::new(String::new()),
            priority_filter: RwSignal::new(Vec::new()),
            label_filter: RwSignal::new(Vec::new()),
            assignee_filter: RwSignal::new(Vec::new()),
            dragging: RwSignal::new(None),
            drag_target: RwSignal::new(None),
        }
    }
}

impl BoardViewState {
    /// Put every control back to the value it has on a board opened fresh.
    ///
    /// Called from `ProjectEditState::reset`, i.e. when the router moves this
    /// page to a different project. Without it a filter typed on one project
    /// would still be narrowing the board of the next one, with no visible
    /// cause — the same leak `ProjectEditState::reset` exists to prevent for
    /// the inline editors.
    pub fn reset(&self) {
        self.search.set(String::new());
        self.priority_filter.set(Vec::new());
        self.label_filter.set(Vec::new());
        self.assignee_filter.set(Vec::new());
        self.dragging.set(None);
        self.drag_target.set(None);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Board Content
// ─────────────────────────────────────────────────────────────────────────────

/// Project-scoped Kanban board. Groups issues by status category across teams.
///
/// Accepts a `project_id` prop and reads all data from the `SyncStore`.
#[component]
pub fn ProjectBoardContent(
    /// The project ID to filter issues by.
    project_id: Signal<String>,
    /// Filter selections and in-progress drag, owned by `ProjectDetailPage` so
    /// they survive this component being reconstructed. See [`BoardViewState`].
    state: BoardViewState,
) -> impl IntoView {
    let BoardViewState {
        search,
        priority_filter,
        label_filter,
        assignee_filter,
        dragging,
        drag_target,
    } = state;

    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // ── Data: issues for this project ────────────────────────────────────
    let project_issues = Memo::new(move |_| {
        let pid = project_id.get();
        if pid.is_empty() {
            return Vec::new();
        }
        let Some(store) = sync_store else {
            return Vec::new();
        };
        store
            .issues()
            .get()
            .into_iter()
            .filter(|issue| issue.project_id.as_deref() == Some(&pid))
            .collect::<Vec<_>>()
    });

    // ── Data: all statuses (needed for drag-drop target resolution) ──────
    let all_statuses = Memo::new(move |_| {
        sync_store
            .map(|store| store.statuses().get())
            .unwrap_or_default()
    });

    // ── Hidden columns state ─────────────────────────────────────────────
    //
    // Declared here and not in `BoardViewState`, unlike everything else on this
    // toolbar, and TRA-10032 left it that way deliberately.
    //
    // Column visibility is already durable in a stronger sense than hoisting
    // would make it: the toggle in `ProjectBoardDisplayOptions` writes the whole
    // set through to localStorage under a per-project key on every change, so
    // there is never an unsaved value to lose. The rebuild replaces this signal
    // with an empty set, and the effect below — whose `initialized` guard is
    // replaced with it — reads the key straight back. Nothing the user chose is
    // discarded; the residue is one frame in which a column that should be
    // hidden is shown, and the same rebuild is tearing down and re-creating
    // every column's DOM either way. That is the render churn TRA-10031 is
    // about, not state loss, and moving this signal would not shorten it by a
    // frame.
    //
    // It also has a per-project storage key doing the job `ProjectEditState::
    // reset` does for the hoisted state. Hoisting it would put two mechanisms
    // on one value and make their order on a project switch matter.
    let hidden_categories: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());

    // Initialize hidden set from localStorage.
    {
        let pid_for_init = project_id;
        let initialized = StoredValue::new(false);
        Effect::new(move |_| {
            let pid = pid_for_init.get();
            if pid.is_empty() || initialized.get_value() {
                return;
            }
            initialized.set_value(true);
            #[cfg(not(target_arch = "wasm32"))]
            let _ = &pid;
            #[cfg(target_arch = "wasm32")]
            {
                let key = storage_key(&pid);
                let initial = match read_hidden_from_storage(&key) {
                    Some(stored) => stored,
                    None => {
                        let defaults = default_hidden();
                        write_hidden_to_storage(&key, &defaults);
                        defaults
                    }
                };
                hidden_categories.set(initial);
            }
        });
    }

    // Filtered issues: search + priority + label + assignee applied before grouping.
    let filtered_issues = Memo::new(move |_| {
        let raw = project_issues.get();
        let search_val = search.get().to_lowercase();
        let priority_val = priority_filter.get();
        let label_val = label_filter.get();
        let assignee_val = assignee_filter.get();

        raw.into_iter()
            .filter(|issue| {
                if !priority_val.is_empty() {
                    let p_str = issue.priority.to_string();
                    if !priority_val.contains(&p_str) {
                        return false;
                    }
                }
                if !label_val.is_empty() {
                    let has_match = issue.labels.iter().any(|l| label_val.contains(&l.label_id));
                    if !has_match {
                        return false;
                    }
                }
                if !assignee_val.is_empty() {
                    match &issue.assignee_id {
                        Some(aid) if assignee_val.contains(aid) => {}
                        _ => return false,
                    }
                }
                if !search_val.is_empty() && !issue.title.to_lowercase().contains(&search_val) {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>()
    });

    // ── Drop handler ────────────────────────────────────────────────────
    // Args: (issue_id, target_category)
    let handle_drop = move |issue_id: String, target_category: String| {
        let current_issues = project_issues.get_untracked();
        let issue = match current_issues.iter().find(|i| i.issue_id == issue_id) {
            Some(i) => i.clone(),
            None => return,
        };

        // Same category → no-op.
        if issue.status_category == target_category {
            return;
        }

        let current_statuses = all_statuses.get_untracked();

        // Find the first status in the target category that belongs to the
        // issue's team.
        let target_status = current_statuses
            .iter()
            .filter(|s| s.category == target_category)
            .find(|s| s.team_id.as_deref() == Some(&issue.team_id));

        let Some(target_status) = target_status else {
            toast_error(format!(
                "Team {} has no status in the \"{}\" category",
                issue.team_key,
                CATEGORY_COLUMNS
                    .iter()
                    .find(|(cat, _)| *cat == target_category)
                    .map(|(_, label)| *label)
                    .unwrap_or(&target_category),
            ));
            return;
        };

        let target_status_id = target_status.status_id.clone();
        let target_status_name = target_status.name.clone();
        let target_status_category = target_status.category.clone();

        let old_status_id = issue.status_id.clone();
        let old_status_name = issue.status_name.clone();
        let old_status_category = issue.status_category.clone();

        // Optimistic update via SyncStore.
        if let Some(store) = sync_store {
            let mut updated = issue.clone();
            updated.status_id = target_status_id.clone();
            updated.status_name = target_status_name;
            updated.status_category = target_status_category;
            updated.sort_order = None;
            store.upsert_issue(updated);
        }

        let issue_team_key = issue.team_key.clone();
        let issue_number = issue.number;
        leptos::task::spawn_local(async move {
            if let Err(e) = update_issue(
                issue_team_key,
                issue_number,
                None, None,
                Some(target_status_id),
                None, None, None, None, None, None, None,
                Some("sort_order".to_string()),
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

    // ── Render ──────────────────────────────────────────────────────────
    view! {
        <div class="flex flex-col h-full">
            // ── Toolbar ─────────────────────────────────────────────────
            <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0 flex-wrap">
                <SearchInput
                    value=search
                    on_input=Callback::new(move |v: String| search.set(v))
                    placeholder="Filter cards..."
                    class="flex-1 max-w-sm"
                />
                <PriorityFilterDropdown
                    value=priority_filter
                    on_change=Callback::new(move |v: Vec<String>| priority_filter.set(v))
                />
                <LabelFilterDropdown
                    value=label_filter
                    on_change=Callback::new(move |v: Vec<String>| label_filter.set(v))
                />
                <AssigneeFilterDropdown
                    value=assignee_filter
                    on_change=Callback::new(move |v: Vec<String>| assignee_filter.set(v))
                />
                <ProjectBoardDisplayOptions
                    hidden=hidden_categories
                    project_id=project_id
                />
            </div>

            // ── Content area ────────────────────────────────────────────
            <div class="flex-1 overflow-x-auto px-4 md:px-6 py-4" style="scrollbar-width: thin;">
                {move || {
                    let store_ready = sync_store.map(|s| s.initialized().get()).unwrap_or(false);
                    if !store_ready {
                        view! { <ProjectBoardSkeleton/> }.into_any()
                    } else {
                        let hidden = hidden_categories.get();
                        let issues = filtered_issues.get();

                        let columns = CATEGORY_COLUMNS
                            .iter()
                            .filter(|(cat, _)| !hidden.contains(*cat))
                            .map(|(category, label)| {
                                let mut col_issues: Vec<IssueWithDetails> = issues
                                    .iter()
                                    .filter(|i| i.status_category == *category)
                                    .cloned()
                                    .collect();
                                sort_column_issues(&mut col_issues);
                                let count = col_issues.len();
                                let variant = category_to_variant(category);
                                let category_owned = category.to_string();

                                view! {
                                    <ProjectBoardColumn
                                        category=category_owned
                                        label=label.to_string()
                                        status_variant=variant
                                        count=count
                                        issues=col_issues
                                        dragging=dragging
                                        drag_target=drag_target
                                        on_drop=handle_drop
                                    />
                                }
                            })
                            .collect_view();

                        view! {
                            <div class="flex gap-4 h-full">
                                {columns}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Display Options
// ─────────────────────────────────────────────────────────────────────────────

/// Popover for toggling category column visibility on the project board.
#[component]
fn ProjectBoardDisplayOptions(
    hidden: RwSignal<HashSet<String>>,
    project_id: Signal<String>,
) -> impl IntoView {
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (open, set_open) = signal(false);
    // Suppress unused variable warning on SSR (project_id is only used in wasm32 cfg blocks).
    let _pid = project_id;

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
                        "Category columns"
                    </div>
                    {CATEGORY_COLUMNS.iter().map(|(category, label)| {
                        let cat = category.to_string();
                        let cat_for_toggle = cat.clone();
                        let variant = category_to_variant(category);
                        let is_visible = {
                            let cat_check = cat.clone();
                            Signal::derive(move || !hidden.get().contains(&cat_check))
                        };
                        view! {
                            <div class="flex items-center gap-2 px-1 py-1 rounded hover:bg-accent cursor-pointer">
                                <Checkbox
                                    checked=is_visible
                                    on_change=Callback::new(move |checked: bool| {
                                        hidden.update(|set| {
                                            if checked {
                                                set.remove(&cat_for_toggle);
                                            } else {
                                                set.insert(cat_for_toggle.clone());
                                            }
                                        });
                                        #[cfg(target_arch = "wasm32")]
                                        {
                                            let key = storage_key(&project_id.get_untracked());
                                            write_hidden_to_storage(&key, &hidden.get_untracked());
                                        }
                                    })
                                />
                                <IssueStatusBadge status=variant/>
                                <span class="text-sm text-foreground">{label.to_string()}</span>
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

/// A single Kanban column for one status category on the project board.
#[component]
fn ProjectBoardColumn(
    /// The category key (e.g. "started").
    category: String,
    /// Human-readable label (e.g. "In Progress").
    label: String,
    /// Status variant for the SVG icon in the column header.
    status_variant: IssueStatusVariant,
    /// Number of issues in this column.
    count: usize,
    /// Issues to render in this column.
    issues: Vec<IssueWithDetails>,
    /// Which issue_id is currently being dragged. Owned by `ProjectDetailPage`
    /// — see [`BoardViewState`].
    dragging: RwSignal<Option<String>>,
    /// Which category is the current drag target. Owned by `ProjectDetailPage`
    /// — see [`BoardViewState`].
    drag_target: RwSignal<Option<String>>,
    /// Callback when an issue is dropped on this column.
    /// Args: (issue_id, target_category).
    #[prop(into)]
    on_drop: Callback<(String, String)>,
) -> impl IntoView {
    let category_for_over = category.clone();
    let category_for_drop = category.clone();
    let category_for_class = category.clone();

    let is_drop_target = move || {
        drag_target.get().as_deref() == Some(category_for_class.as_str())
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
                drag_target.set(Some(category_for_over.clone()));
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
                    drag_target.set(None);
                }
            }
            on:drop={
                let category_drop = category_for_drop.clone();
                move |ev: web_sys::DragEvent| {
                    ev.prevent_default();
                    if let Some(dt) = ev.data_transfer()
                        && let Ok(issue_id) = dt.get_data("text/plain")
                        && !issue_id.is_empty()
                    {
                        on_drop.run((issue_id, category_drop.clone()));
                    }
                    // Both halves of the drag end here, not just the target
                    // highlight. `dragend` on the card says the same thing, but
                    // `on_drop` above writes the moved issue back to the
                    // `SyncStore`, which rebuilds this board and detaches the
                    // card that is about to receive it — so leaving the "which
                    // issue is being dragged" signal to `dragend` alone would
                    // make a moved card's half-opacity depend on an event
                    // reaching a node no longer in the document.
                    dragging.set(None);
                    drag_target.set(None);
                }
            }
        >
            // ── Column header ───────────────────────────────────────────
            <div class="sticky top-0 bg-background pb-3 pt-1 px-2">
                <div class="flex items-center gap-2">
                    <IssueStatusBadge status=status_variant/>
                    <span class="font-medium text-sm text-foreground">{label}</span>
                    <span class="text-muted-foreground text-xs">{count}</span>
                </div>
            </div>

            // ── Cards container ─────────────────────────────────────────
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
                            <ProjectBoardCard
                                issue=issue
                                dragging=dragging
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

/// A single issue card on the project Kanban board.
///
/// Follows the same card layout as the team board:
/// issue key, title, labels, priority + assignee footer.
#[component]
fn ProjectBoardCard(
    issue: IssueWithDetails,
    /// Which issue_id is currently being dragged. Owned by `ProjectDetailPage`
    /// — see [`BoardViewState`].
    dragging: RwSignal<Option<String>>,
) -> impl IntoView {
    let issue_id = issue.issue_id.clone();
    let issue_id_for_drag = issue_id.clone();
    let issue_id_for_opacity = issue_id.clone();
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let title = issue.title.clone();
    let labels = issue.labels.clone();
    let priority = issue.priority;
    let assignee_name = issue.assignee_name.clone();
    let team_key = issue.team_key.clone();

    let (did_drag, set_did_drag) = signal(false);

    let is_dragging = move || dragging.get().as_deref() == Some(issue_id_for_opacity.as_str());

    let card_class = move || {
        let base = "bg-card border border-border rounded-md p-4 shadow-sm hover:shadow-md transition-shadow cursor-grab focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
        if is_dragging() {
            format!("{base} opacity-50")
        } else {
            base.to_string()
        }
    };

    let issue_href = format!("/issues/{issue_key}");
    let issue_href_for_click = issue_href.clone();
    let issue_href_for_keydown = issue_href.clone();

    let location = use_location();

    // Look up team color from SyncStore.
    let team_color = {
        let sync_store = use_context::<crate::cache::store::SyncStore>();
        let team_id = issue.team_id.clone();
        sync_store.and_then(|store| {
            store.teams().get_untracked().into_iter()
                .find(|t| t.team_id == team_id)
                .and_then(|t| t.icon_color.clone())
        }).unwrap_or_else(|| crate::components::team_icon::DEFAULT_ICON_COLOR.to_string())
    };

    let navigate_to_issue = move |_: web_sys::MouseEvent| {
        if did_drag.get_untracked() {
            set_did_drag.set(false);
            return;
        }
        let path = location.pathname.get_untracked();
        let search = location.search.get_untracked();
        let nav_state = IssueNavState::from_current_path(&path, &search);
        let json = nav_state.to_json();
        let nav = use_navigate();
        nav(&issue_href_for_click, NavigateOptions {
            state: State::from(wasm_bindgen::JsValue::from_str(&json)),
            ..Default::default()
        });
    };

    let card_dom_id = format!("card-{issue_id}");

    view! {
        <div
            id=card_dom_id
            class=card_class
            draggable="true"
            tabindex="0"
            on:dragstart={
                let issue_id_ds = issue_id_for_drag.clone();
                move |ev: web_sys::DragEvent| {
                    if let Some(dt) = ev.data_transfer() {
                        let _ = dt.set_data("text/plain", &issue_id_ds);
                        dt.set_effect_allowed("move");
                    }
                    dragging.set(Some(issue_id_ds.clone()));
                }
            }
            on:dragend=move |_: web_sys::DragEvent| {
                dragging.set(None);
                set_did_drag.set(true);
            }
            on:click=navigate_to_issue
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" {
                    let path = location.pathname.get_untracked();
                    let search = location.search.get_untracked();
                    let nav_state = IssueNavState::from_current_path(&path, &search);
                    let json = nav_state.to_json();
                    let nav = use_navigate();
                    nav(&issue_href_for_keydown, NavigateOptions {
                        state: State::from(wasm_bindgen::JsValue::from_str(&json)),
                        ..Default::default()
                    });
                }
            }
        >
            // Issue key with team badge
            <div class="flex items-center gap-1.5 mb-1">
                <TeamKeyBadge team_key=team_key color=team_color/>
                <span class="font-mono text-xs text-muted-foreground">
                    {issue.number}
                </span>
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

/// Five skeleton columns with placeholder card shapes.
#[component]
fn ProjectBoardSkeleton() -> impl IntoView {
    let columns = CATEGORY_COLUMNS.iter().map(|_| {
        let cards = (0..3).map(|_| {
            view! {
                <div class="bg-card border border-border rounded-md p-4">
                    <Skeleton class="w-14 h-3 mb-2"/>
                    <Skeleton class="w-full h-4 mb-1"/>
                    <Skeleton class="w-3/4 h-4 mb-3"/>
                    <div class="flex gap-1 mb-3">
                        <Skeleton class="w-10 h-5 rounded-sm"/>
                        <Skeleton class="w-12 h-5 rounded-sm"/>
                    </div>
                    <div class="flex items-center justify-between">
                        <Skeleton class="w-3.5 h-3.5 rounded-[2px]"/>
                        <Skeleton class="w-5 h-5 rounded-full"/>
                    </div>
                </div>
            }
        }).collect_view();

        view! {
            <div class="min-w-[280px] max-w-[320px] flex flex-col">
                <div class="pb-3 pt-1 px-2">
                    <div class="flex items-center gap-2">
                        <Skeleton class="w-3.5 h-3.5 rounded-full"/>
                        <Skeleton class="w-20 h-4"/>
                        <Skeleton class="w-4 h-3"/>
                    </div>
                </div>
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
