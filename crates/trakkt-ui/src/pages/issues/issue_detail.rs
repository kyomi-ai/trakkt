// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue detail page — full view of a single issue with editing.
//!
//! Layout follows DESIGN.md "Issue Detail Page" spec:
//! - Header: back button + issue number (font-mono)
//! - Content: title (editable), metadata bar, description (kode WYSIWYG), comments
//! - Footer: created/updated timestamps
//!
//! Key interactions:
//! - Title: click to edit, Enter/blur to save
//! - Status/Priority: StyledSelect dropdowns with immediate save
//! - Description: kode WYSIWYG editor with debounced auto-save
//! - Comments: threaded display with new comment form

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::Icon;

use crate::components::{
    Alert, AlertVariant, Avatar, AvatarSize, Button, ButtonSize, ButtonVariant,
    DropdownItem, DropdownMenu, DropdownTrigger,
    IssueStatusBadge, IssueStatusVariant,
    LabelBadge, Modal, ModalSize, Skeleton, StyledSelect,
};
use crate::server_fns::comments::{create_comment, list_comments};
use crate::server_fns::issues::{get_issue, set_issue_labels, update_issue};
use crate::server_fns::labels::list_labels;
use crate::server_fns::statuses::list_statuses;
use trakkt_types::models::{Comment, IssueWithDetails};

// ─────────────────────────────────────────────────────────────────────────────
// Relative timestamp helper
// ─────────────────────────────────────────────────────────────────────────────

/// Convert an ISO timestamp string to a human-friendly relative format.
///
/// Returns: "just now", "2m ago", "1h ago", "3d ago", "May 5", "Dec 15, 2025"
///
/// Uses `js_sys::Date::now()` for current time (WASM-safe, no `wasmbind` chrono feature needed).
/// Falls back to the raw string if parsing fails.
fn relative_time(timestamp: &str) -> String {
    use chrono::NaiveDateTime;

    // Parse the timestamp (ISO 8601 format from the DB: "2026-05-07T12:34:56" or with timezone)
    let parsed = timestamp
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .or_else(|| {
            // Try parsing without timezone (SQLite format)
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .or_else(|| NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").ok())
                .map(|naive| naive.and_utc())
        });

    let ts = match parsed {
        Some(dt) => dt,
        None => return timestamp.to_string(),
    };

    // Get current time from JS (WASM-safe)
    let now_ms = js_sys::Date::now();
    let now_secs = (now_ms / 1000.0) as i64;
    let ts_secs = ts.timestamp();
    let diff_secs = now_secs - ts_secs;

    if diff_secs < 60 {
        return "just now".to_string();
    }

    let diff_mins = diff_secs / 60;
    if diff_mins < 60 {
        return format!("{diff_mins}m ago");
    }

    let diff_hours = diff_mins / 60;
    if diff_hours < 24 {
        return format!("{diff_hours}h ago");
    }

    let diff_days = diff_hours / 24;
    if diff_days < 7 {
        return format!("{diff_days}d ago");
    }

    // For older dates, use "May 5" or "Dec 15, 2025" format
    let now_year = {
        let d = js_sys::Date::new_0();
        d.get_full_year() as i32
    };

    let ts_year = ts.format("%Y").to_string().parse::<i32>().unwrap_or(0);
    if ts_year == now_year {
        ts.format("%b %-d").to_string()
    } else {
        ts.format("%b %-d, %Y").to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared kode theme builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a kode `Theme` matching Trakkt's design system (warm light palette).
///
/// Since kode's `Theme` is `#[non_exhaustive]`, we start from `Theme::light()`
/// and override the fields we need.
pub(crate) fn trakkt_kode_theme() -> kode_leptos::Theme {
    let mut t = kode_leptos::Theme::light();
    // Core colors — Trakkt warm palette
    t.bg = "#FAFAF8";
    t.fg = "#1C1917";
    t.fg_bright = "#1C1917";
    t.fg_dim = "#9C9790";
    t.cursor = "#1C1917";
    t.selection = "rgba(13, 148, 136, 0.15)";
    t.current_line = "rgba(0, 0, 0, 0.03)";
    t.gutter_fg = "#9C9790";
    t.gutter_border = "#E8E5DE";
    t.border = "#E8E5DE";
    t.accent = "#0D9488";
    t.bg_highlight = "#F5F3EF";
    t.bg_hover = "#E8E5DE";
    t.marker_error = "#DC2626";
    t.marker_warning = "#CA8A04";
    t.marker_info = "#2563EB";
    t.marker_hint = "#9C9790";
    t.code_fg = "#0D9488";
    t.link = "#0D9488";
    t.syntax = kode_leptos::SyntaxTheme::GithubLight;
    // Typography — DESIGN.md fonts
    t.content_font_family = Some("'DM Sans', sans-serif");
    t.heading_font_family = Some("'Instrument Serif', serif");
    t.code_font_family = Some("'Geist Mono', monospace");
    t.font_family = Some("'Geist Mono', monospace");
    // Content layout
    t.content_max_width = Some("100%");
    t.container_padding = Some("0");
    // Toolbar styling to match Trakkt
    t.toolbar_bg = Some("#FAFAF8");
    t.toolbar_border_color = Some("#E8E5DE");
    t.toolbar_button_border_radius = Some("6px");
    t.toolbar_button_hover_bg = Some("#F5F3EF");
    t.toolbar_button_selected_bg = Some("#0D9488");
    t.toolbar_button_selected_color = Some("#FFFFFF");
    // Heading styling
    t.heading_font_weight = Some("600");
    t.h1_border_width = Some("0");
    t.h2_border_width = Some("0");
    t
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue Detail Page
// ─────────────────────────────────────────────────────────────────────────────

/// Full issue detail page with editing, description, and comments.
#[component]
pub fn IssueDetailPage() -> impl IntoView {
    let params = use_params_map();
    let number = Memo::new(move |_| {
        params
            .get()
            .get("number")
            .and_then(|n| n.parse::<i32>().ok())
            .unwrap_or(0)
    });

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let (version, set_version) = signal(0u32);

    let server_issue = Resource::new(
        move || (number.get(), version.get()),
        move |(num, _)| async move { get_issue(num).await },
    );

    let issue_data = Signal::derive(move || {
        let num = number.get();
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if let Some(issue) = items.iter().find(|i| i.number == num) {
                return Some(Ok(Some(issue.clone())));
            }
            if store.initialized().get() {
                return Some(Ok(None));
            }
        }
        server_issue.get()
    });

    let comments_resource = Resource::new(
        move || (number.get(), version.get()),
        move |(num, _)| async move { list_comments(num).await },
    );

    let refetch = move || set_version.update(|v| *v += 1);

    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Header ─────────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::IconSm
                    aria_label="Back to issues"
                    on:click=move |_| {
                        let nav = use_navigate();
                        nav("/issues", Default::default());
                    }
                >
                    <Icon icon=phosphor_leptos::ARROW_LEFT size="20px"/>
                </Button>
                <span class="font-mono text-sm text-muted-foreground">
                    {move || format!("#{}", number.get())}
                </span>
            </div>

            // ── Content ────────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                {move || {
                    match issue_data.get() {
                        Some(Ok(Some(issue))) => {
                            let comments = comments_resource.get()
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
                            let refetch = refetch;
                            view! {
                                <IssueDetailContent
                                    initial_issue=issue
                                    comments=comments
                                    on_change=Callback::new(move |()| refetch())
                                />
                            }.into_any()
                        }
                        Some(Ok(None)) => {
                            view! { <IssueNotFound number=number.get()/> }.into_any()
                        }
                        Some(Err(_)) => {
                            view! {
                                <div class="max-w-[860px] mx-auto w-full text-center py-16">
                                    <p class="text-muted-foreground">"Failed to load issue. Please try again."</p>
                                </div>
                            }.into_any()
                        }
                        None => {
                            view! { <IssueDetailSkeleton/> }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue Detail Content (populated state)
// ─────────────────────────────────────────────────────────────────────────────

/// The main content of the issue detail page — rendered when issue data is loaded.
///
/// Reads from SyncStore reactively so external changes (MCP, other clients)
/// update the UI in realtime without a page refresh.
#[component]
fn IssueDetailContent(
    initial_issue: IssueWithDetails,
    comments: Vec<Comment>,
    on_change: Callback<()>,
) -> impl IntoView {
    let number = initial_issue.number;
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let initial = RwSignal::new(initial_issue);

    let issue = Signal::derive(move || {
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if let Some(found) = items.iter().find(|i| i.number == number) {
                return found.clone();
            }
        }
        initial.get()
    });

    // ── Sub-issues: derived from SyncStore ───────────────────────────
    let sub_issues = Memo::new(move |_| {
        let Some(store) = sync_store else { return vec![] };
        let i = issue.get();
        store.issues().get()
            .into_iter()
            .filter(|child| child.parent_issue_id.as_deref() == Some(i.issue_id.as_str()))
            .collect::<Vec<_>>()
    });

    // ── New sub-issue modal state ─────────────────────────────────────
    let (show_new_sub_issue, set_show_new_sub_issue) = signal(false);

    let on_sub_issue_created = {
        let on_change = on_change;
        Callback::new(move |()| {
            set_show_new_sub_issue.set(false);
            on_change.run(());
        })
    };

    view! {
        <div class="max-w-[860px] mx-auto w-full">
            // ── Parent breadcrumb ─────────────────────────────────────
            {move || {
                let i = issue.get();
                let parent_id = i.parent_issue_id.as_ref()?;
                let store = sync_store?;
                let parent = store.issues().get().into_iter().find(|p| p.issue_id == *parent_id)?;
                Some(view! {
                    <a
                        href=format!("/issues/{}", parent.number)
                        class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1 mb-1"
                    >
                        <Icon icon=phosphor_leptos::ARROW_BEND_UP_LEFT size="12px"/>
                        {format!("{}-{} {}", parent.team_key, parent.number, parent.title)}
                    </a>
                })
            }}

            // ── Title ──────────────────────────────────────────────────
            {move || {
                let i = issue.get();
                view! { <EditableTitle number=number title=i.title.clone() on_save=on_change/> }
            }}

            // ── Metadata bar ───────────────────────────────────────────
            {move || {
                let i = issue.get();
                view! { <MetadataBar issue=i on_change=on_change/> }
            }}

            // ── Description ────────────────────────────────────────────
            {move || {
                let i = issue.get();
                view! {
                    <DescriptionEditor
                        number=number
                        description=i.description.clone().unwrap_or_default()
                        on_save=on_change
                    />
                }
            }}

            // ── Sub-issues section ────────────────────────────────────
            <SubIssuesSection
                sub_issues=sub_issues
                on_add=Callback::new(move |()| set_show_new_sub_issue.set(true))
            />

            // ── Divider ────────────────────────────────────────────────
            <div class="border-t border-border my-6"></div>

            // ── Comments ───────────────────────────────────────────────
            <CommentsSection
                number=number
                comments=comments
                on_comment_added=on_change
            />

            // ── Footer: timestamps ────────────────────────────────────
            {move || {
                let i = issue.get();
                view! {
                    <div class="mt-6 pb-4">
                        <div class="flex items-center gap-4 text-xs text-muted-foreground">
                            <span>{format!("Created {}", relative_time(&i.created_at))}</span>
                            <span>{format!("Updated {}", relative_time(&i.updated_at))}</span>
                        </div>
                    </div>
                }
            }}
        </div>

        // ── New Sub-Issue modal ───────────────────────────────────────
        {move || {
            let i = issue.get();
            let team_id = i.team_id.clone();
            let parent_id = i.issue_id.clone();
            let parent_title = format!("{}-{} {}", i.team_key, i.number, i.title);
            view! {
                <NewSubIssueModal
                    show=Signal::derive(move || show_new_sub_issue.get())
                    on_close=Callback::new(move |()| set_show_new_sub_issue.set(false))
                    on_created=on_sub_issue_created
                    team_id=team_id
                    parent_issue_id=parent_id
                    parent_title=parent_title
                />
            }
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Editable Title
// ─────────────────────────────────────────────────────────────────────────────

/// Inline-editable title — click to edit, Enter or blur to save.
#[component]
fn EditableTitle(
    number: i32,
    title: String,
    on_save: Callback<()>,
) -> impl IntoView {
    let (editing, set_editing) = signal(false);
    let (current_title, set_current_title) = signal(title.clone());
    let (saving, set_saving) = signal(false);
    let original_title = title.clone();
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let save_title = move || {
        let new_title = current_title.get_untracked();
        if new_title.trim().is_empty() || saving.get_untracked() {
            return;
        }
        set_editing.set(false);
        set_saving.set(true);

        leptos::task::spawn_local(async move {
            let _ = update_issue(
                number,
                Some(new_title),
                None, // description
                None, // status_id
                None, // priority
                None, // assignee_id
                None, // due_date
                None, // project_id
                None, // milestone_id
                None, // parent_issue_id
            )
            .await;
            set_saving.set(false);
            on_save.run(());
        });
    };

    // Focus the input when entering edit mode
    Effect::new(move || {
        if editing.get() && let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    let title_for_display = title.clone();

    view! {
        <Show
            when=move || editing.get()
            fallback=move || {
                let t = title_for_display.clone();
                view! {
                    <h1
                        class="text-2xl font-display text-foreground cursor-pointer hover:text-foreground/80 transition-colors"
                        on:click=move |_| {
                            set_editing.set(true);
                        }
                        title="Click to edit title"
                    >
                        {t}
                    </h1>
                }
            }
        >
            {
                let orig = original_title.clone();
                view! {
                    <input
                        node_ref=input_ref
                        type="text"
                        class="text-2xl font-display text-foreground bg-transparent border-b-2 border-primary outline-none w-full py-1"
                        prop:value=move || current_title.get()
                        on:input=move |ev| set_current_title.set(event_target_value(&ev))
                        on:blur=move |_| save_title()
                        on:keydown={
                            let orig = orig.clone();
                            move |ev: web_sys::KeyboardEvent| {
                                if ev.key() == "Enter" {
                                    save_title();
                                } else if ev.key() == "Escape" {
                                    set_current_title.set(orig.clone());
                                    set_editing.set(false);
                                }
                            }
                        }
                    />
                }
            }
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Metadata Bar
// ─────────────────────────────────────────────────────────────────────────────

/// Metadata bar showing status, priority, assignee, labels, and due date.
#[component]
fn MetadataBar(
    issue: IssueWithDetails,
    on_change: Callback<()>,
) -> impl IntoView {
    let number = issue.number;
    let current_status_id = issue.status_id.clone();
    let current_status_category = issue.status_category.clone();
    let priority = issue.priority;

    // Fetch statuses dynamically for the status dropdown.
    let statuses_resource = LocalResource::new(move || list_statuses(None));

    // ── Status change handler ───────────────────────────────────────────
    let on_status_change = move |new_status_id: String| {
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                number,
                None, // title
                None, // description
                Some(new_status_id),
                None, // priority
                None, // assignee_id
                None, // due_date
                None, // project_id
                None, // milestone_id
                None, // parent_issue_id
            )
            .await;
            on_change.run(());
        });
    };

    // ── Priority change handler ─────────────────────────────────────────
    let on_priority_change = move |new_priority: String| {
        let prio = new_priority.parse::<i32>().unwrap_or(0);
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                number,
                None, // title
                None, // description
                None, // status_id
                Some(prio),
                None, // assignee_id
                None, // due_date
                None, // project_id
                None, // milestone_id
                None, // parent_issue_id
            )
            .await;
            on_change.run(());
        });
    };

    let status_variant = IssueStatusVariant::parse(&current_status_category);
    let (status_open, set_status_open) = signal(false);
    let status_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let statuses = RwSignal::new(Vec::<trakkt_types::models::Status>::new());

    Effect::new(move || {
        if let Some(Ok(loaded)) = statuses_resource.get() {
            statuses.set(loaded);
        }
    });

    let current_status_name = {
        let id = current_status_id.clone();
        move || {
            statuses.get().iter()
                .find(|s| s.status_id == id)
                .map(|s| s.name.clone())
        }
    };

    view! {
        <div class="flex flex-wrap items-center gap-4 mt-4">
            // ── Status ─────────────────────────────────────────────────
            <div class="flex items-center gap-2">
                <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Status"</span>
                <div node_ref=status_trigger_ref>
                    <DropdownTrigger
                        label="Status"
                        value=Signal::derive(move || current_status_name())
                        icon=Arc::new(move || {
                            view! { <IssueStatusBadge status=status_variant size=12/> }.into_any()
                        }) as ChildrenFn
                        on_click=Callback::new(move |()| set_status_open.update(|o| *o = !*o))
                    />
                </div>
                <DropdownMenu
                    trigger_ref=status_trigger_ref
                    open=Signal::derive(move || status_open.get())
                    on_close=Callback::new(move |()| set_status_open.set(false))
                    search_placeholder="Filter status..."
                >
                    {
                        let current_sid = current_status_id.clone();
                        move || statuses.get().into_iter().map({
                            let current_sid = current_sid.clone();
                            move |status| {
                                let status_id = status.status_id.clone();
                                let status_id_check = status.status_id.clone();
                                let label = status.name.clone();
                                let variant = IssueStatusVariant::parse(&status.category);
                                view! {
                                    <DropdownItem
                                        label=label
                                        selected=Signal::derive({
                                            let id = current_sid.clone();
                                            move || id == status_id_check
                                        })
                                on_select=Callback::new({
                                    let id = status_id.clone();
                                    move |()| {
                                        on_status_change(id.clone());
                                        set_status_open.set(false);
                                    }
                                })
                                icon=Arc::new(move || view! { <IssueStatusBadge status=variant size=14/> }.into_any()) as ChildrenFn
                            />
                        }
                    }}).collect_view()}
                </DropdownMenu>
            </div>

            // ── Priority ───────────────────────────────────────────────
            <div class="flex items-center gap-2">
                <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Priority"</span>
                <div class="w-32">
                    <StyledSelect
                        value=priority.to_string()
                        options=vec![
                            ("0", "None"),
                            ("1", "Urgent"),
                            ("2", "High"),
                            ("3", "Medium"),
                            ("4", "Low"),
                        ]
                        on_change=on_priority_change
                    />
                </div>
            </div>

            // ── Assignee (display only — picker is future work) ────────
            <div class="flex items-center gap-2">
                <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Assignee"</span>
                {if let Some(ref name) = issue.assignee_name {
                    view! {
                        <div class="flex items-center gap-1.5">
                            <Avatar name=name.clone() size=AvatarSize::Sm/>
                            <span class="text-sm text-foreground">{name.clone()}</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <span class="text-sm text-muted-foreground">"Unassigned"</span>
                    }.into_any()
                }}
            </div>

            // ── Labels ─────────────────────────────────────────────────
            <LabelPicker
                number=number
                team_id=issue.team_id.clone()
                current_labels=issue.labels.clone()
                on_change=on_change
            />

            // ── Due date (display only — date picker is future work) ───
            {issue.due_date.as_ref().map(|date| {
                view! {
                    <div class="flex items-center gap-2">
                        <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Due"</span>
                        <span class="text-sm text-foreground">{date.clone()}</span>
                    </div>
                }
            })}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Description Editor (kode WYSIWYG)
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Label Picker
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn LabelPicker(
    number: i32,
    /// The team_id of the issue, used to scope the label list.
    team_id: String,
    current_labels: Vec<trakkt_types::models::Label>,
    on_change: Callback<()>,
) -> impl IntoView {
    let (show_picker, set_show_picker) = signal(false);
    let current_ids = RwSignal::new(
        current_labels.iter().map(|l| l.label_id.clone()).collect::<Vec<_>>()
    );
    let current_display = RwSignal::new(current_labels);

    let team_id_for_fetch = Some(team_id);
    let all_labels = LocalResource::new(move || {
        let tid = team_id_for_fetch.clone();
        async move { list_labels(tid).await }
    });

    let toggle_label = move |label: trakkt_types::models::Label| {
        let mut ids = current_ids.get_untracked();
        let mut display = current_display.get_untracked();
        if ids.contains(&label.label_id) {
            ids.retain(|id| id != &label.label_id);
            display.retain(|l| l.label_id != label.label_id);
        } else {
            ids.push(label.label_id.clone());
            display.push(label);
        }
        current_ids.set(ids.clone());
        current_display.set(display);

        let label_ids_str = ids.join(",");
        leptos::task::spawn_local(async move {
            let _ = set_issue_labels(number, label_ids_str).await;
            on_change.run(());
        });
    };

    view! {
        <div class="flex items-center gap-2 relative">
            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Labels"</span>
            <div class="flex items-center gap-1">
                {move || {
                    let labels = current_display.get();
                    if labels.is_empty() {
                        view! {
                            <span class="text-sm text-muted-foreground">"None"</span>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex items-center gap-1">
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
                    }
                }}
                <button
                    class="w-5 h-5 rounded flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors text-xs"
                    on:click=move |_| set_show_picker.update(|v| *v = !*v)
                    title="Edit labels"
                >
                    "+"
                </button>
            </div>

            // Dropdown picker
            <Show when=move || show_picker.get()>
                <div class="absolute top-full left-0 mt-1 z-50 bg-popover border border-border rounded-lg shadow-lg py-1 min-w-[200px]">
                    <Suspense fallback=|| view! { <div class="px-3 py-2 text-sm text-muted-foreground">"Loading..."</div> }>
                        {move || all_labels.get().map(|result| {
                            match result {
                                Ok(labels) => {
                                    if labels.is_empty() {
                                        view! {
                                            <div class="px-3 py-2 text-sm text-muted-foreground">"No labels. Create one in Settings → Labels."</div>
                                        }.into_any()
                                    } else {
                                        let items = labels.clone();
                                        view! {
                                            <div>
                                                {items.into_iter().map(|label| {
                                                    let label_for_click = label.clone();
                                                    let label_id = label.label_id.clone();
                                                    let is_selected = move || current_ids.get().contains(&label_id);
                                                    view! {
                                                        <button
                                                            class="w-full text-left px-3 py-1.5 text-sm hover:bg-muted transition-colors flex items-center gap-2"
                                                            on:click=move |_| toggle_label(label_for_click.clone())
                                                        >
                                                            <span
                                                                class="w-3 h-3 rounded-sm shrink-0"
                                                                style=format!("background-color: {}", label.color)
                                                            />
                                                            <span class="flex-1">{label.name.clone()}</span>
                                                            {move || if is_selected() {
                                                                view! { <span class="text-primary text-xs">"✓"</span> }.into_any()
                                                            } else {
                                                                ().into_any()
                                                            }}
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }.into_any()
                                    }
                                },
                                Err(_) => view! {
                                    <div class="px-3 py-2 text-sm text-destructive-foreground">"Failed to load labels"</div>
                                }.into_any(),
                            }
                        })}
                    </Suspense>
                </div>
            </Show>
        </div>
    }
}

/// Description section using the kode WYSIWYG markdown editor.
///
/// Auto-saves on change with a 500ms debounce using a version counter:
/// each edit increments the counter; after the delay, save only fires if
/// the counter has not changed (i.e., no newer edits arrived).
#[component]
fn DescriptionEditor(
    number: i32,
    description: String,
    on_save: Callback<()>,
) -> impl IntoView {
    use kode_leptos::TreeWysiwygEditor;

    // Initial content — passed to the editor once, not updated reactively
    // (updating the content signal would cause re-render and focus loss).
    let initial_content = Signal::stored(description);
    let latest_text = RwSignal::new(String::new());
    let edit_version = RwSignal::new(0u32);

    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        latest_text.set(text);
        edit_version.update(|v| *v += 1);
        let snapshot = edit_version.get_untracked();

        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if edit_version.get_untracked() != snapshot {
                return;
            }
            let current_text = latest_text.get_untracked();
            let desc = if current_text.trim().is_empty() {
                Some(String::new())
            } else {
                Some(current_text)
            };
            let _ = update_issue(
                number,
                None, // title
                desc,
                None, // status_id
                None, // priority
                None, // assignee_id
                None, // due_date
                None, // project_id
                None, // milestone_id
                None, // parent_issue_id
            )
            .await;
            on_save.run(());
        });
    });

    let mut theme = trakkt_kode_theme();
    theme.content_padding = Some("1rem 1.25rem");
    let theme_signal = Signal::stored(theme);

    view! {
        <div class="mt-6">
            <h2 class="text-xs text-muted-foreground font-medium mb-3">"Description"</h2>
            <div class="border border-border rounded-md overflow-hidden" style="min-height: 200px;">
                <TreeWysiwygEditor
                    content=initial_content
                    on_change=on_change
                    theme=theme_signal
                />
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-issues Section
// ─────────────────────────────────────────────────────────────────────────────

/// Sub-issues section showing child issues with progress bar.
///
/// Displays: header with count + progress text, progress bar, compact issue list,
/// and an "Add sub-issue" button.
#[component]
fn SubIssuesSection(
    sub_issues: Memo<Vec<IssueWithDetails>>,
    on_add: Callback<()>,
) -> impl IntoView {
    // Only render the section if there are sub-issues OR we always show it for the add button.
    // We show it always so users can discover the "Add sub-issue" action.
    view! {
        <div class="mt-6">
            {move || {
                let items = sub_issues.get();
                let total = items.len();
                let completed = items.iter().filter(|i| {
                    i.status_category == "completed" || i.status_category == "cancelled"
                }).count();
                let percent = if total > 0 { (completed as f64 / total as f64) * 100.0 } else { 0.0 };

                view! {
                    // ── Header ─────────────────────────────────────────
                    <div class="flex items-center justify-between mb-2">
                        <div class="flex items-center gap-2">
                            <h2 class="text-xs text-muted-foreground font-medium">
                                {if total > 0 {
                                    format!("Sub-issues ({})", total)
                                } else {
                                    "Sub-issues".to_string()
                                }}
                            </h2>
                            {(total > 0).then(|| view! {
                                <span class="text-xs text-muted-foreground">
                                    {format!("{} of {} done", completed, total)}
                                </span>
                            })}
                        </div>
                        <button
                            class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                            on:click=move |_| on_add.run(())
                            title="Add sub-issue"
                        >
                            <Icon icon=phosphor_leptos::PLUS size="12px"/>
                            "Add"
                        </button>
                    </div>

                    // ── Progress bar ──────────────────────────────────
                    {(total > 0).then(|| view! {
                        <div class="h-1.5 w-full bg-secondary rounded-full overflow-hidden mb-2">
                            <div
                                class="h-full bg-primary rounded-full transition-all duration-300"
                                style=format!("width: {}%", percent)
                            />
                        </div>
                    })}

                    // ── Sub-issue rows ────────────────────────────────
                    {(total > 0).then(|| {
                        let rows = items.into_iter().map(|child| {
                            let child_status = IssueStatusVariant::parse(&child.status_category);
                            let child_key = format!("{}-{}", child.team_key, child.number);
                            let child_href = format!("/issues/{}", child.number);
                            let child_title = child.title.clone();
                            let assignee_name = child.assignee_name.clone();
                            view! {
                                <div class="flex items-center gap-2 px-3 py-1.5 hover:bg-secondary/50 rounded-md transition-colors">
                                    <IssueStatusBadge status=child_status size=12/>
                                    <span class="text-xs text-muted-foreground font-mono shrink-0">{child_key}</span>
                                    <a href=child_href class="text-sm text-foreground hover:underline truncate flex-1">
                                        {child_title}
                                    </a>
                                    {assignee_name.map(|name| view! {
                                        <Avatar name=name size=AvatarSize::Sm/>
                                    })}
                                </div>
                            }
                        }).collect_view();
                        view! {
                            <div class="space-y-0.5">
                                {rows}
                            </div>
                        }
                    })}
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// New Sub-Issue Modal
// ─────────────────────────────────────────────────────────────────────────────

/// Modal for creating a sub-issue with the parent pre-filled.
#[component]
fn NewSubIssueModal(
    show: Signal<bool>,
    on_close: Callback<()>,
    on_created: Callback<()>,
    team_id: String,
    parent_issue_id: String,
    parent_title: String,
) -> impl IntoView {
    let (title, set_title) = signal(String::new());
    let (priority, set_priority) = signal("0".to_string());
    let (submitting, set_submitting) = signal(false);
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Reset form when modal opens.
    Effect::new(move || {
        if show.get() {
            set_title.set(String::new());
            set_priority.set("0".to_string());
            set_error_msg.set(None);
            set_submitting.set(false);
        }
    });

    let team_id_stored = Signal::stored(team_id);
    let parent_id_stored = Signal::stored(parent_issue_id);

    let handle_submit = move || {
        let title_val = title.get_untracked();
        if title_val.trim().is_empty() {
            return;
        }

        let prio = priority.get_untracked().parse::<i32>().unwrap_or(0);
        let tid = team_id_stored.get();
        let pid = parent_id_stored.get();

        set_submitting.set(true);
        set_error_msg.set(None);

        leptos::task::spawn_local(async move {
            match crate::server_fns::issues::create_issue(
                title_val,
                None,   // description
                prio,
                None,   // assignee_id
                None,   // due_date
                String::new(), // label_ids
                None,   // project_id
                None,   // milestone_id
                Some(pid),
                Some(tid),
            ).await {
                Ok(_) => {
                    set_submitting.set(false);
                    on_created.run(());
                }
                Err(e) => {
                    set_submitting.set(false);
                    set_error_msg.set(Some(format!("Failed to create sub-issue: {e}")));
                }
            }
        });
    };

    let title_empty = Memo::new(move |_| title.get().trim().is_empty());

    let handle_submit_for_footer = handle_submit;
    let modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        let submit = handle_submit_for_footer;
        view! {
            <Button
                variant=ButtonVariant::Ghost
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </Button>
            <Button
                disabled=Signal::derive(move || submitting.get() || title_empty.get())
                on:click=move |_| submit()
            >
                {move || if submitting.get() { "Creating..." } else { "Create Sub-issue" }}
            </Button>
        }.into_any()
    });

    let parent_title_stored = Signal::stored(parent_title);

    view! {
        <Modal
            show=show
            on_close=on_close
            title="New Sub-issue"
            size=ModalSize::Sm
            footer=modal_footer
        >
            <form
                on:submit=move |ev: web_sys::SubmitEvent| {
                    ev.prevent_default();
                    handle_submit();
                }
                class="space-y-4"
            >
                // Error message
                <Show when=move || error_msg.get().is_some()>
                    <Alert variant=AlertVariant::Error>
                        <crate::components::AlertDescription>
                            {move || error_msg.get().unwrap_or_default()}
                        </crate::components::AlertDescription>
                    </Alert>
                </Show>

                // Parent (read-only display)
                <div class="space-y-2">
                    <label class="text-sm font-medium text-foreground">"Parent"</label>
                    <div class="flex items-center gap-1.5 text-sm text-muted-foreground px-3 py-2 border border-border rounded-md bg-muted/30">
                        <Icon icon=phosphor_leptos::ARROW_BEND_UP_LEFT size="14px"/>
                        {move || parent_title_stored.get()}
                    </div>
                </div>

                // Title
                <div class="space-y-2">
                    <label for="sub-issue-title" class="text-sm font-medium text-foreground">
                        "Title"
                    </label>
                    <input
                        id="sub-issue-title"
                        type="text"
                        required=true
                        autofocus=true
                        placeholder="Sub-issue title"
                        class=crate::components::INPUT_CLASS
                        prop:value=move || title.get()
                        on:input=move |ev| set_title.set(event_target_value(&ev))
                    />
                </div>

                // Priority
                <div class="space-y-2">
                    <label class="text-sm font-medium text-foreground">"Priority"</label>
                    <select
                        class="w-full h-9 px-3 rounded-md border border-border bg-background text-sm text-foreground"
                        prop:value=move || priority.get()
                        on:change=move |ev| set_priority.set(event_target_value(&ev))
                    >
                        <option value="0">"None"</option>
                        <option value="1">"Urgent"</option>
                        <option value="2">"High"</option>
                        <option value="3">"Medium"</option>
                        <option value="4">"Low"</option>
                    </select>
                </div>
            </form>
        </Modal>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Comments Section
// ─────────────────────────────────────────────────────────────────────────────

/// Comments section with threaded display and new comment form.
#[component]
fn CommentsSection(
    number: i32,
    comments: Vec<Comment>,
    on_comment_added: Callback<()>,
) -> impl IntoView {
    // Group comments into top-level and replies
    let top_level: Vec<Comment> = comments
        .iter()
        .filter(|c| c.parent_id.is_none())
        .cloned()
        .collect();

    let replies: Vec<Comment> = comments
        .iter()
        .filter(|c| c.parent_id.is_some())
        .cloned()
        .collect();

    let comment_count = comments.len();

    view! {
        <div>
            <h2 class="text-sm font-medium text-foreground mb-4">
                {format!("Comments ({comment_count})")}
            </h2>

            // ── Comment list ───────────────────────────────────────────
            <div class="space-y-4">
                {top_level.into_iter().map(|comment| {
                    let comment_id = comment.comment_id.clone();
                    let comment_replies: Vec<Comment> = replies
                        .iter()
                        .filter(|r| r.parent_id.as_deref() == Some(&comment_id))
                        .cloned()
                        .collect();
                    view! {
                        <CommentItem comment=comment/>
                        // ── Threaded replies ───────────────────────────
                        {if !comment_replies.is_empty() {
                            Some(view! {
                                <div class="ml-10 space-y-4">
                                    {comment_replies.into_iter().map(|reply| {
                                        view! { <CommentItem comment=reply/> }
                                    }).collect_view()}
                                </div>
                            })
                        } else {
                            None
                        }}
                    }
                }).collect_view()}
            </div>

            // ── New comment form ───────────────────────────────────────
            <NewCommentForm number=number on_created=on_comment_added/>
        </div>
    }
}

/// A single comment with avatar, author name, timestamp, and body.
#[component]
fn CommentItem(comment: Comment) -> impl IntoView {
    let author = comment
        .author_name
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let timestamp = relative_time(&comment.created_at);

    view! {
        <div class="flex gap-3">
            <Avatar name=author.clone() size=AvatarSize::Md/>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm font-medium text-foreground">{author}</span>
                    <span class="text-xs text-muted-foreground">{timestamp}</span>
                </div>
                <div class="mt-1 text-sm text-foreground">
                    // Render markdown body as plain text for v1 (not editable).
                    // A full markdown renderer can be added later.
                    <p class="whitespace-pre-wrap">{comment.body.clone()}</p>
                </div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// New Comment Form
// ─────────────────────────────────────────────────────────────────────────────

/// Form for adding a new comment with kode WYSIWYG editor.
#[component]
fn NewCommentForm(number: i32, on_created: Callback<()>) -> impl IntoView {
    use kode_leptos::TreeWysiwygEditor;

    let content = RwSignal::new(String::new());
    let (submitting, set_submitting) = signal(false);

    let is_empty = Memo::new(move |_| content.get().trim().is_empty());

    let handle_submit = move || {
        let body = content.get_untracked();
        if body.trim().is_empty() || submitting.get_untracked() {
            return;
        }

        set_submitting.set(true);
        leptos::task::spawn_local(async move {
            match create_comment(number, body, None).await {
                Ok(_) => {
                    content.set(String::new());
                    set_submitting.set(false);
                    on_created.run(());
                }
                Err(e) => {
                    tracing::warn!("Failed to create comment: {e}");
                    set_submitting.set(false);
                }
            }
        });
    };

    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        content.set(text);
    });

    let mut theme = trakkt_kode_theme();
    theme.content_padding = Some("0.75rem 1rem");
    let theme_signal = Signal::stored(theme);

    view! {
        <div class="mt-6">
            <div class="border border-border rounded-md overflow-hidden" style="min-height: 120px;">
                <TreeWysiwygEditor
                    content=content.read_only()
                    on_change=on_change
                    theme=theme_signal
                />
            </div>
            <div class="flex justify-end mt-3">
                <Button
                    disabled=Signal::derive(move || submitting.get() || is_empty.get())
                    size=ButtonSize::Sm
                    on:click=move |_| handle_submit()
                >
                    {move || if submitting.get() { "Commenting..." } else { "Comment" }}
                </Button>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading Skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Skeleton loading state matching the issue detail layout shape.
#[component]
fn IssueDetailSkeleton() -> impl IntoView {
    view! {
        <div class="max-w-[860px] mx-auto w-full">
            // Title
            <Skeleton class="h-8 w-2/3 mb-4"/>
            // Metadata bar
            <div class="flex flex-wrap gap-4 mt-4">
                <Skeleton class="h-11 w-36"/>
                <Skeleton class="h-11 w-32"/>
                <Skeleton class="h-5 w-24"/>
                <Skeleton class="h-5 w-20"/>
            </div>
            // Description
            <div class="mt-6">
                <Skeleton class="h-4 w-20 mb-3"/>
                <Skeleton class="h-48 w-full rounded-md"/>
            </div>
            // Divider
            <div class="border-t border-border my-6"></div>
            // Comments heading
            <Skeleton class="h-5 w-28 mb-4"/>
            // Comment placeholders
            <div class="space-y-4">
                {(0..2).map(|_| {
                    view! {
                        <div class="flex gap-3">
                            <Skeleton class="w-7 h-7 rounded-full shrink-0"/>
                            <div class="flex-1">
                                <div class="flex gap-2 mb-1">
                                    <Skeleton class="h-4 w-20"/>
                                    <Skeleton class="h-3 w-12"/>
                                </div>
                                <Skeleton class="h-12 w-full"/>
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Not Found State
// ─────────────────────────────────────────────────────────────────────────────

/// Displayed when the issue number does not resolve to an issue.
#[component]
fn IssueNotFound(number: i32) -> impl IntoView {
    view! {
        <div class="max-w-[860px] mx-auto w-full text-center py-16">
            <h2 class="text-xl font-semibold text-foreground mb-2">
                {format!("Issue #{number} not found")}
            </h2>
            <p class="text-muted-foreground mb-6">
                "This issue may have been deleted or you don\u{2019}t have access."
            </p>
            <Button
                variant=ButtonVariant::Secondary
                on:click=move |_| {
                    let nav = use_navigate();
                    nav("/issues", Default::default());
                }
            >
                <Icon icon=phosphor_leptos::ARROW_LEFT size="14px"/>
                "Back to Issues"
            </Button>
        </div>
    }
}
