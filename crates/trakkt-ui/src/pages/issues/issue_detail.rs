// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue detail page — full view of a single issue with editing.
//!
//! Layout follows Linear's two-column pattern:
//! - Header: back button + issue number (font-mono)
//! - Left column: title (editable), description (kode WYSIWYG), sub-issues, comments, timestamps
//! - Right column (280px sidebar): status, priority, assignee, labels, due date, watch, parent, team
//! - Responsive: sidebar stacks below main content on mobile
//!
//! Key interactions:
//! - Title: click to edit, Enter/blur to save
//! - Status/Priority: DropdownMenu pickers with immediate save
//! - Description: kode WYSIWYG editor with debounced auto-save
//! - Comments: threaded display with new comment form

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::Icon;

use crate::components::{
    Avatar, AvatarSize, Button, ButtonSize, ButtonVariant,
    DropdownItem, DropdownMenu, DropdownTrigger,
    IssueStatusBadge, IssueStatusVariant,
    LabelBadge, Modal, ModalSize, PriorityIndicator, SearchInput, Skeleton,
};
use crate::pages::issues::issue_list::NewIssueModal;
use crate::server_fns::comments::{create_comment, list_comments};
use crate::server_fns::issues::{get_issue, list_issues, set_issue_labels, update_issue};
use crate::server_fns::labels::list_labels;
use crate::server_fns::projects::list_milestones;
use crate::server_fns::relations::{add_relation, list_issue_relations, remove_relation};
use crate::server_fns::statuses::list_statuses;
use crate::server_fns::watchers::{is_watching, watch_issue, unwatch_issue};
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
    // Colors use CSS var() references so they follow Trakkt's light/dark
    // mode automatically. The actual values live in main.css :root block
    // which maps --kode-* vars to --color-* design tokens.
    t.bg = "var(--color-card)";
    t.fg = "var(--color-foreground)";
    t.fg_bright = "var(--color-foreground)";
    t.fg_dim = "var(--color-muted-foreground)";
    t.cursor = "var(--color-foreground)";
    t.selection = "rgba(13, 148, 136, 0.15)";
    t.current_line = "transparent";
    t.gutter_fg = "var(--color-muted-foreground)";
    t.gutter_border = "var(--color-border)";
    t.border = "var(--color-border)";
    t.accent = "var(--color-primary)";
    t.bg_highlight = "var(--color-accent)";
    t.bg_hover = "var(--color-accent)";
    t.marker_error = "#DC2626";
    t.marker_warning = "#CA8A04";
    t.marker_info = "#2563EB";
    t.marker_hint = "var(--color-muted-foreground)";
    t.code_fg = "var(--color-primary)";
    t.link = "var(--color-primary)";
    t.syntax = kode_leptos::SyntaxTheme::GithubLight;
    // Typography — DESIGN.md fonts
    t.content_font_family = Some("'DM Sans', sans-serif");
    t.heading_font_family = Some("'Instrument Serif', serif");
    t.code_font_family = Some("'Geist Mono', monospace");
    t.font_family = Some("'Geist Mono', monospace");
    // Content layout
    t.content_max_width = Some("100%");
    t.container_padding = Some("0");
    // Toolbar styling — also uses CSS vars for dark mode
    t.toolbar_bg = Some("var(--color-card)");
    t.toolbar_border_color = Some("var(--color-border)");
    t.toolbar_button_border_radius = Some("6px");
    t.toolbar_button_hover_bg = Some("var(--color-accent)");
    t.toolbar_button_selected_bg = Some("var(--color-primary)");
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

    // Parse `:identifier` param as `TEAM-123` format.
    // Split on the first `-` — team keys are alphanumeric (no hyphens).
    let identifier = Memo::new(move |_| {
        let raw = params.get().get("identifier").unwrap_or_default();
        let parts: Vec<&str> = raw.splitn(2, '-').collect();
        if parts.len() == 2 {
            let tk = parts[0].to_string();
            let num = parts[1].parse::<i32>().unwrap_or(0);
            (tk, num)
        } else {
            (String::new(), 0)
        }
    });
    let team_key = Memo::new(move |_| identifier.get().0);
    let number = Memo::new(move |_| identifier.get().1);

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    let server_issue = Resource::new(
        move || (team_key.get(), number.get()),
        move |(tk, num)| async move { get_issue(tk, num).await },
    );

    let issue_data = Signal::derive(move || {
        let tk = team_key.get();
        let num = number.get();
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if let Some(issue) = items.iter().find(|i| i.team_key == tk && i.number == num) {
                return Some(Ok(Some(issue.clone())));
            }
            if store.initialized().get() {
                return Some(Ok(None));
            }
        }
        server_issue.get()
    });

    // Only tracks load-state transitions (Loading → Loaded, etc.),
    // not SyncStore data changes. Prevents IssueDetailContent from being
    // recreated on every WebSocket update.
    #[derive(Clone, PartialEq)]
    enum PageState {
        Loading,
        Loaded(String, i32),
        NotFound,
        Error,
    }

    let page_state = Memo::new(move |_| {
        match issue_data.get() {
            Some(Ok(Some(ref issue))) => PageState::Loaded(issue.team_key.clone(), issue.number),
            Some(Ok(None)) => PageState::NotFound,
            Some(Err(_)) => PageState::Error,
            None => PageState::Loading,
        }
    });

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
                    {move || format!("{}-{}", team_key.get(), number.get())}
                </span>
            </div>

            // ── Content ────────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                {move || {
                    match page_state.get() {
                        PageState::Loaded(_, _) => {
                            let issue = match issue_data.get_untracked() {
                                Some(Ok(Some(i))) => i,
                                _ => return view! { <IssueDetailSkeleton/> }.into_any(),
                            };
                            view! {
                                <IssueDetailContent
                                    initial_issue=issue
                                />
                            }.into_any()
                        }
                        PageState::NotFound => {
                            view! { <IssueNotFound identifier=format!("{}-{}", team_key.get(), number.get())/> }.into_any()
                        }
                        PageState::Error => {
                            view! {
                                <div class="max-w-[860px] mx-auto w-full text-center py-16">
                                    <p class="text-muted-foreground">"Failed to load issue. Please try again."</p>
                                </div>
                            }.into_any()
                        }
                        PageState::Loading => {
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
/// Owns its own comments resource so comment refetches don't trigger
/// parent re-renders (which would destroy the description editor).
#[component]
fn IssueDetailContent(
    initial_issue: IssueWithDetails,
) -> impl IntoView {
    let number = initial_issue.number;
    let issue_team_key_for_lookup = initial_issue.team_key.clone();
    let initial_team_key = initial_issue.team_key.clone();
    let initial_description = initial_issue.description.clone().unwrap_or_default();
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let initial = RwSignal::new(initial_issue);

    let issue = Signal::derive(move || {
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if let Some(found) = items.iter().find(|i| i.team_key == issue_team_key_for_lookup && i.number == number) {
                return found.clone();
            }
        }
        initial.get()
    });

    // ── Comments: fetched locally, refetched via version bump ────────
    let comments_tk = initial_team_key.clone();
    let (comment_version, set_comment_version) = signal(0u32);
    let comments_resource = Resource::new(
        move || (comments_tk.clone(), number, comment_version.get()),
        move |(tk, num, _)| async move { list_comments(tk, num).await },
    );
    let refetch_comments = Callback::new(move |()| set_comment_version.update(|v| *v += 1));

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
    let (show_link_sub_issue, set_show_link_sub_issue) = signal(false);

    let on_sub_issue_created = {
        Callback::new(move |()| {
            set_show_new_sub_issue.set(false);
        })
    };

    // Clone fields needed in link-sub-issue closures
    let issue_id_for_link = issue.get_untracked().issue_id.clone();
    let issue_id_for_exclude = issue_id_for_link.clone();

    // No-op callback for components that need on_change but don't need parent notification
    let noop = Callback::new(|()| {});

    // ── Fine-grained memos: only re-render when the specific field changes ──
    let title = Memo::new(move |_| issue.get().title.clone());
    let parent_issue_id = Memo::new(move |_| issue.get().parent_issue_id.clone());
    let timestamps = Memo::new(move |_| {
        let i = issue.get();
        (i.created_at.clone(), i.updated_at.clone())
    });
    // Sidebar-relevant fields only — excludes title/description so those
    // edits don't cause the sidebar to flicker.
    let sidebar_key = Memo::new(move |_| {
        let i = issue.get();
        (i.status_id.clone(), i.status_category.clone(), i.priority,
         i.assignee_id.clone(), i.assignee_name.clone(),
         i.due_date.clone(), i.parent_issue_id.clone(),
         i.labels.clone(),
         i.project_id.clone(), i.project_name.clone(), i.milestone_id.clone())
    });

    view! {
        <div class="max-w-[1140px] mx-auto w-full flex flex-col md:flex-row gap-8">
            // ── Left column: main content ─────────────────────────────
            <div class="flex-1 min-w-0">
                // ── Parent breadcrumb ─────────────────────────────────
                {move || {
                    let pid = parent_issue_id.get()?;
                    let store = sync_store?;
                    let parent = store.issues().get().into_iter().find(|p| p.issue_id == pid)?;
                    Some(view! {
                        <a
                            href=format!("/issues/{}-{}", parent.team_key, parent.number)
                            class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1 mb-1"
                        >
                            <Icon icon=phosphor_leptos::ARROW_BEND_UP_LEFT size="12px"/>
                            {format!("{}-{} {}", parent.team_key, parent.number, parent.title)}
                        </a>
                    })
                }}

                // ── Title ──────────────────────────────────────────────
                {
                    let tk = initial_team_key.clone();
                    move || {
                        let t = title.get();
                        view! { <EditableTitle team_key=tk.clone() number=number title=t on_save=noop/> }
                    }
                }

                // ── Description ────────────────────────────────────────
                <DescriptionEditor
                    team_key=initial_team_key.clone()
                    number=number
                    description=initial_description.clone()
                />

                // ── Sub-issues section ────────────────────────────────
                <SubIssuesSection
                    sub_issues=sub_issues
                    on_add=Callback::new(move |()| set_show_new_sub_issue.set(true))
                    on_link=Callback::new(move |()| set_show_link_sub_issue.set(true))
                />

                // ── Relations section ─────────────────────────────────
                <RelationsSection
                    team_key=initial_team_key.clone()
                    number=number
                />

                // ── Divider ────────────────────────────────────────────
                <div class="border-t border-border my-6"></div>

                // ── Comments ───────────────────────────────────────────
                {move || {
                    let comments = comments_resource.get()
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    view! {
                        <CommentsSection
                            team_key=initial.get_untracked().team_key.clone()
                            number=number
                            comments=comments
                            on_comment_added=refetch_comments
                        />
                    }
                }}

                // ── Footer: timestamps ────────────────────────────────
                {move || {
                    let (created, updated) = timestamps.get();
                    view! {
                        <div class="mt-6 pb-4">
                            <div class="flex items-center gap-4 text-xs text-muted-foreground">
                                <span>{format!("Created {}", relative_time(&created))}</span>
                                <span>{format!("Updated {}", relative_time(&updated))}</span>
                            </div>
                        </div>
                    }
                }}
            </div>

            // ── Right column: metadata sidebar ────────────────────────
            <div class="w-full md:w-[280px] shrink-0">
                {move || {
                    sidebar_key.get();
                    let i = issue.get_untracked();
                    view! { <MetadataSidebar issue=i on_change=noop/> }
                }}
            </div>
        </div>

        // ── New Sub-Issue modal ───────────────────────────────────────
        <NewIssueModal
            show=Signal::derive(move || show_new_sub_issue.get())
            on_close=Callback::new(move |()| set_show_new_sub_issue.set(false))
            on_created=on_sub_issue_created
            team_id=Signal::derive(move || Some(issue.get().team_id.clone()))
            parent_issue_id=Signal::derive(move || Some(issue.get().issue_id.clone()))
            parent_title=Signal::derive(move || {
                let i = issue.get();
                Some(format!("{}-{} {}", i.team_key, i.number, i.title))
            })
        />

        // ── Link existing sub-issue modal ────────────────────────────────
        <IssuePickerModal
            show=Signal::derive(move || show_link_sub_issue.get())
            on_close=Callback::new(move |()| set_show_link_sub_issue.set(false))
            on_select=Callback::new({
                let parent_id = issue_id_for_link.clone();
                move |selected: IssueWithDetails| {
                    let child_team_key = selected.team_key.clone();
                    let child_number = selected.number;
                    let parent_id = parent_id.clone();
                    set_show_link_sub_issue.set(false);
                    leptos::task::spawn_local(async move {
                        let _ = update_issue(
                            child_team_key,
                            child_number,
                            None, None, None, None, None, None, None, None,
                            Some(parent_id),
                            None,
                            None,
                        ).await;
                    });
                }
            })
            exclude_ids=Signal::derive({
                let issue_id = issue_id_for_exclude.clone();
                move || {
                    let mut ids = vec![issue_id.clone()];
                    ids.extend(sub_issues.get().iter().map(|i| i.issue_id.clone()));
                    ids
                }
            })
            title="Link existing issue"
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Editable Title
// ─────────────────────────────────────────────────────────────────────────────

/// Inline-editable title — click to edit, Enter or blur to save.
#[component]
fn EditableTitle(
    team_key: String,
    number: i32,
    title: String,
    on_save: Callback<()>,
) -> impl IntoView {
    let (editing, set_editing) = signal(false);
    let (current_title, set_current_title) = signal(title.clone());
    let (saving, set_saving) = signal(false);
    let original_title = title.clone();
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let stored_team_key = StoredValue::new(team_key);

    let save_title = move || {
        let new_title = current_title.get_untracked();
        if new_title.trim().is_empty() || saving.get_untracked() {
            return;
        }
        set_editing.set(false);
        set_saving.set(true);

        let tk = stored_team_key.get_value();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
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
                None, // clear_sort_order
                None, // clear_parent
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
                        class="font-display text-foreground cursor-pointer hover:text-foreground/80 transition-colors"
                        style="font-size: 36px; font-weight: 400; line-height: 1.1; letter-spacing: -0.01em;"
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
                        class="font-display text-foreground bg-transparent border-b-2 border-primary outline-none w-full py-1"
                        style="font-size: 36px; font-weight: 400; line-height: 1.1; letter-spacing: -0.01em;"
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
// Metadata Sidebar
// ─────────────────────────────────────────────────────────────────────────────

/// Metadata sidebar showing status, priority, assignee, labels, due date, watch,
/// parent issue, and team — stacked vertically in the right column.
#[component]
fn MetadataSidebar(
    issue: IssueWithDetails,
    on_change: Callback<()>,
) -> impl IntoView {
    let number = issue.number;
    let issue_team_key = issue.team_key.clone();
    let current_status_id = issue.status_id.clone();
    let current_status_category = issue.status_category.clone();
    let priority = issue.priority;

    // ── Parent issue state ─────────────────────────────────────────────
    let (show_parent_picker, set_show_parent_picker) = signal(false);
    let issue_id_for_parent_exclude = issue.issue_id.clone();
    let parent_issue_id = issue.parent_issue_id.clone();
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Fetch statuses dynamically for the status dropdown.
    let statuses_resource = LocalResource::new(move || list_statuses(None));

    // ── Status change handler ───────────────────────────────────────────
    let stored_tk = StoredValue::new(issue_team_key.clone());
    let on_status_change = move |new_status_id: String| {
        let tk = stored_tk.get_value();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
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
                None, // clear_sort_order
                None, // clear_parent
            )
            .await;
            on_change.run(());
        });
    };

    // ── Priority change handler ─────────────────────────────────────────
    let on_priority_change = move |prio: i32| {
        let tk = stored_tk.get_value();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
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
                None, // clear_sort_order
                None, // clear_parent
            )
            .await;
            on_change.run(());
        });
    };

    // ── Project change handler ─────────────────────────────────────────
    let on_project_change = move |project_id: String| {
        let tk = stored_tk.get_value();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk, number,
                None, None, None, None, None, None,
                Some(project_id),
                Some(String::new()),
                None, None, None,
            ).await;
            on_change.run(());
        });
    };

    // ── Milestone change handler ───────────────────────────────────────
    let on_milestone_change = move |milestone_id: String| {
        let tk = stored_tk.get_value();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk, number,
                None, None, None, None, None, None,
                None,
                Some(milestone_id),
                None, None, None,
            ).await;
            on_change.run(());
        });
    };

    let status_variant = IssueStatusVariant::parse(&current_status_category);
    let (status_open, set_status_open) = signal(false);
    let (priority_open, set_priority_open) = signal(false);
    let status_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let priority_trigger_ref = NodeRef::<leptos::html::Div>::new();
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

    let team_key = issue.team_key.clone();

    let current_project_id = issue.project_id.clone();
    let current_project_name = issue.project_name.clone();
    let current_milestone_id = issue.milestone_id.clone();
    let (project_open, set_project_open) = signal(false);
    let project_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (milestone_open, set_milestone_open) = signal(false);
    let milestone_trigger_ref = NodeRef::<leptos::html::Div>::new();

    let milestones = RwSignal::new(Vec::<trakkt_types::models::ProjectMilestone>::new());
    let milestone_pid = current_project_id.clone();
    if let Some(ref pid) = milestone_pid {
        let pid = pid.clone();
        let milestones_resource = LocalResource::new(move || {
            let pid = pid.clone();
            async move { list_milestones(pid).await }
        });
        Effect::new(move || {
            if let Some(Ok(loaded)) = milestones_resource.get() {
                milestones.set(loaded);
            }
        });
    }

    let current_project_id_for_ms = current_project_id.clone();

    view! {
        <div class="space-y-5">
            // ── Status ─────────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Status"</div>
                <div node_ref=status_trigger_ref>
                    <DropdownTrigger
                        label="Status"
                        value=Signal::derive(current_status_name)
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
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Priority"</div>
                <div node_ref=priority_trigger_ref>
                    <DropdownTrigger
                        label="Priority"
                        value=Signal::derive(move || {
                            Some(match priority {
                                1 => "Urgent",
                                2 => "High",
                                3 => "Medium",
                                4 => "Low",
                                _ => "None",
                            }.to_string())
                        })
                        icon=Arc::new(move || {
                            view! { <PriorityIndicator priority=priority size=14/> }.into_any()
                        }) as ChildrenFn
                        on_click=Callback::new(move |()| set_priority_open.update(|o| *o = !*o))
                    />
                </div>
                <DropdownMenu
                    trigger_ref=priority_trigger_ref
                    open=Signal::derive(move || priority_open.get())
                    on_close=Callback::new(move |()| set_priority_open.set(false))
                >
                    {move || {
                        [(1, "Urgent"), (2, "High"), (3, "Medium"), (4, "Low"), (0, "None")]
                            .into_iter()
                            .map(|(prio, label)| {
                                view! {
                                    <DropdownItem
                                        label=label.to_string()
                                        selected=Signal::derive(move || priority == prio)
                                        on_select=Callback::new(move |()| {
                                            on_priority_change(prio);
                                            set_priority_open.set(false);
                                        })
                                        icon=Arc::new(move || view! { <PriorityIndicator priority=prio size=14/> }.into_any()) as ChildrenFn
                                    />
                                }
                            })
                            .collect_view()
                    }}
                </DropdownMenu>
            </div>

            // ── Assignee (display only — picker is future work) ────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Assignee"</div>
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
                team_key=issue_team_key.clone()
                number=number
                team_id=issue.team_id.clone()
                current_labels=issue.labels.clone()
                on_change=on_change
            />

            // ── Project ───────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Project"</div>
                <div class="flex items-center gap-1">
                    <div node_ref=project_trigger_ref class="flex-1 min-w-0">
                        <DropdownTrigger
                            label="Set project..."
                            value=Signal::derive({
                                let name = current_project_name.clone();
                                move || name.clone()
                            })
                            icon=Arc::new(move || {
                                view! { <Icon icon=phosphor_leptos::FOLDER_SIMPLE size="14px"/> }.into_any()
                            }) as ChildrenFn
                            on_click=Callback::new(move |()| set_project_open.update(|o| *o = !*o))
                        />
                    </div>
                    {
                        let has_project = current_project_id.is_some();
                        if has_project {
                            Some(view! {
                                <button
                                    class="text-muted-foreground hover:text-foreground transition-colors shrink-0"
                                    title="Remove project"
                                    on:click=move |_| on_project_change(String::new())
                                >
                                    <Icon icon=phosphor_leptos::X size="12px"/>
                                </button>
                            })
                        } else {
                            None
                        }
                    }
                </div>
                <DropdownMenu
                    trigger_ref=project_trigger_ref
                    open=Signal::derive(move || project_open.get())
                    on_close=Callback::new(move |()| set_project_open.set(false))
                    search_placeholder="Filter projects..."
                >
                    {
                        let current_pid = current_project_id.clone();
                        move || {
                            let projects = sync_store.map(|store| store.projects().get()).unwrap_or_default();
                            let none_item = {
                                view! {
                                    <DropdownItem
                                        label="None".to_string()
                                        selected=Signal::derive({
                                            let pid = current_pid.clone();
                                            move || pid.is_none()
                                        })
                                        on_select=Callback::new(move |()| {
                                            on_project_change(String::new());
                                            set_project_open.set(false);
                                        })
                                    />
                                }
                            };
                            let project_items = projects.into_iter().map({
                                let current_pid = current_pid.clone();
                                move |project| {
                                    let project_id = project.project_id.clone();
                                    let project_id_check = project.project_id.clone();
                                    let label = project.name.clone();
                                    let color = project.color.clone();
                                    let icon: ChildrenFn = if let Some(c) = color {
                                        Arc::new(move || {
                                            let c = c.clone();
                                            view! {
                                                <span
                                                    class="inline-block w-2.5 h-2.5 rounded-full shrink-0"
                                                    style=format!("background-color: {c}")
                                                />
                                            }.into_any()
                                        }) as ChildrenFn
                                    } else {
                                        Arc::new(move || {
                                            view! { <Icon icon=phosphor_leptos::FOLDER_SIMPLE size="14px"/> }.into_any()
                                        }) as ChildrenFn
                                    };
                                    view! {
                                        <DropdownItem
                                            label=label
                                            selected=Signal::derive({
                                                let pid = current_pid.clone();
                                                let check = project_id_check.clone();
                                                move || pid.as_deref() == Some(check.as_str())
                                            })
                                            on_select=Callback::new({
                                                let id = project_id.clone();
                                                move |()| {
                                                    on_project_change(id.clone());
                                                    set_project_open.set(false);
                                                }
                                            })
                                            icon=icon
                                        />
                                    }
                                }
                            }).collect_view();
                            view! { {none_item} {project_items} }
                        }
                    }
                </DropdownMenu>
            </div>

            // ── Milestone (only when project is set) ──────────────────
            {
                let has_project = current_project_id_for_ms.is_some();
                let current_mid = current_milestone_id.clone();
                if has_project {
                    Some(view! {
                        <div>
                            <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Milestone"</div>
                            <div class="flex items-center gap-1">
                                <div node_ref=milestone_trigger_ref class="flex-1 min-w-0">
                                    <DropdownTrigger
                                        label="Set milestone..."
                                        value=Signal::derive({
                                            let mid = current_mid.clone();
                                            move || {
                                                let mid = mid.as_deref()?;
                                                milestones.get().iter()
                                                    .find(|m| m.milestone_id == mid)
                                                    .map(|m| m.name.clone())
                                            }
                                        })
                                        icon=Arc::new(move || {
                                            view! { <Icon icon=phosphor_leptos::FLAG size="14px"/> }.into_any()
                                        }) as ChildrenFn
                                        on_click=Callback::new(move |()| set_milestone_open.update(|o| *o = !*o))
                                    />
                                </div>
                                {
                                    let has_milestone = current_mid.is_some();
                                    if has_milestone {
                                        Some(view! {
                                            <button
                                                class="text-muted-foreground hover:text-foreground transition-colors shrink-0"
                                                title="Remove milestone"
                                                on:click=move |_| on_milestone_change(String::new())
                                            >
                                                <Icon icon=phosphor_leptos::X size="12px"/>
                                            </button>
                                        })
                                    } else {
                                        None
                                    }
                                }
                            </div>
                            <DropdownMenu
                                trigger_ref=milestone_trigger_ref
                                open=Signal::derive(move || milestone_open.get())
                                on_close=Callback::new(move |()| set_milestone_open.set(false))
                                search_placeholder="Filter milestones..."
                            >
                                {
                                    let current_mid = current_mid.clone();
                                    move || {
                                        let ms_list = milestones.get();
                                        let none_item = {
                                            view! {
                                                <DropdownItem
                                                    label="None".to_string()
                                                    selected=Signal::derive({
                                                        let mid = current_mid.clone();
                                                        move || mid.is_none()
                                                    })
                                                    on_select=Callback::new(move |()| {
                                                        on_milestone_change(String::new());
                                                        set_milestone_open.set(false);
                                                    })
                                                />
                                            }
                                        };
                                        let milestone_items = ms_list.into_iter().map({
                                            let current_mid = current_mid.clone();
                                            move |ms| {
                                                let ms_id = ms.milestone_id.clone();
                                                let ms_id_check = ms.milestone_id.clone();
                                                let label = match ms.target_date {
                                                    Some(ref d) => format!("{} ({})", ms.name, d),
                                                    None => ms.name.clone(),
                                                };
                                                view! {
                                                    <DropdownItem
                                                        label=label
                                                        selected=Signal::derive({
                                                            let mid = current_mid.clone();
                                                            let check = ms_id_check.clone();
                                                            move || mid.as_deref() == Some(check.as_str())
                                                        })
                                                        on_select=Callback::new({
                                                            let id = ms_id.clone();
                                                            move |()| {
                                                                on_milestone_change(id.clone());
                                                                set_milestone_open.set(false);
                                                            }
                                                        })
                                                    />
                                                }
                                            }
                                        }).collect_view();
                                        view! { {none_item} {milestone_items} }
                                    }
                                }
                            </DropdownMenu>
                        </div>
                    })
                } else {
                    None
                }
            }

            // ── Due date (display only — date picker is future work) ───
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Due"</div>
                {if let Some(ref date) = issue.due_date {
                    view! {
                        <span class="text-sm text-foreground">{date.clone()}</span>
                    }.into_any()
                } else {
                    view! {
                        <span class="text-sm text-muted-foreground">"No due date"</span>
                    }.into_any()
                }}
            </div>

            // ── Watch toggle ──────────────────────────────────────────────
            <WatchToggle team_key=issue_team_key.clone() number=number/>

            // ── Parent issue ──────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Parent"</div>
                {
                    let parent_id = parent_issue_id.clone();
                    if let Some(ref pid) = parent_id {
                        let parent = sync_store.and_then(|store| {
                            store.issues().get().into_iter().find(|i| i.issue_id == *pid)
                        });
                        if let Some(p) = parent {
                            let parent_key = format!("{}-{}", p.team_key, p.number);
                            let parent_href = format!("/issues/{}-{}", p.team_key, p.number);
                            let parent_title = p.title.clone();
                            let tk_for_remove = issue_team_key.clone();
                            view! {
                                <div class="flex items-center gap-1.5 min-w-0">
                                    <a href=parent_href class="text-sm text-foreground hover:underline truncate flex items-center gap-1">
                                        <span class="font-mono text-xs text-muted-foreground">{parent_key}</span>
                                        {parent_title}
                                    </a>
                                    <button
                                        class="text-muted-foreground hover:text-foreground transition-colors shrink-0"
                                        title="Remove parent"
                                        on:click=move |_| {
                                            let tk = tk_for_remove.clone();
                                            leptos::task::spawn_local(async move {
                                                let _ = update_issue(
                                                    tk,
                                                    number,
                                                    None, None, None, None, None, None, None, None,
                                                    None,
                                                    None,
                                                    Some(true),
                                                ).await;
                                            });
                                        }
                                    >
                                        <Icon icon=phosphor_leptos::X size="12px"/>
                                    </button>
                                </div>
                            }.into_any()
                        } else {
                            // Parent exists but not in SyncStore (e.g. different team not loaded yet)
                            view! { <span class="text-sm text-muted-foreground italic">"Unknown"</span> }.into_any()
                        }
                    } else {
                        view! {
                            <button
                                class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                                on:click=move |_| set_show_parent_picker.set(true)
                            >
                                "Set parent..."
                            </button>
                        }.into_any()
                    }
                }
            </div>

            // ── Team ──────────────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Team"</div>
                <span class="text-sm text-foreground">{team_key.clone()}</span>
            </div>
        </div>

        // ── Parent issue picker modal ────────────────────────────────────
        <IssuePickerModal
            show=Signal::derive(move || show_parent_picker.get())
            on_close=Callback::new(move |()| set_show_parent_picker.set(false))
            on_select=Callback::new({
                let tk = issue_team_key.clone();
                move |selected: IssueWithDetails| {
                    let parent_id = selected.issue_id.clone();
                    let tk = tk.clone();
                    set_show_parent_picker.set(false);
                    leptos::task::spawn_local(async move {
                        let _ = update_issue(
                            tk,
                            number,
                            None, None, None, None, None, None, None, None,
                            Some(parent_id),
                            None,
                            None,
                        ).await;
                    });
                }
            })
            exclude_ids=Signal::derive({
                let issue_id = issue_id_for_parent_exclude;
                move || vec![issue_id.clone()]
            })
            title="Set parent issue"
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Watch Toggle
// ─────────────────────────────────────────────────────────────────────────────

/// Eye icon button that toggles watch/unwatch state for an issue.
#[component]
fn WatchToggle(team_key: String, number: i32) -> impl IntoView {
    let tk = team_key.clone();
    let (version, set_version) = signal(0u32);
    let watching_resource = Resource::new(
        move || (tk.clone(), number, version.get()),
        move |(tk, num, _)| async move { is_watching(tk, num).await },
    );

    let (loading, set_loading) = signal(false);

    let toggle = move |_| {
        if loading.get_untracked() {
            return;
        }
        let currently_watching = watching_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or(false);

        set_loading.set(true);
        let tk = team_key.clone();
        leptos::task::spawn_local(async move {
            let result = if currently_watching {
                unwatch_issue(tk, number).await
            } else {
                watch_issue(tk, number).await
            };
            if let Err(e) = result {
                tracing::warn!("Failed to toggle watch: {e}");
            }
            set_loading.set(false);
            set_version.update(|v| *v += 1);
        });
    };

    view! {
        <div>
            <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Watch"</div>
            <button
                class="flex items-center gap-1.5 px-2 py-1 rounded text-sm text-muted-foreground hover:text-foreground hover:bg-surface-alt transition-colors"
                on:click=toggle
                disabled=move || loading.get()
                title=move || {
                    let w = watching_resource.get().and_then(|r| r.ok()).unwrap_or(false);
                    if w { "Stop watching" } else { "Watch this issue" }
                }
            >
                {move || {
                    let w = watching_resource.get().and_then(|r| r.ok()).unwrap_or(false);
                    if w {
                        view! { <span class="text-foreground"><Icon icon=phosphor_leptos::EYE size="16px"/></span> }.into_any()
                    } else {
                        view! { <Icon icon=phosphor_leptos::EYE_SLASH size="16px"/> }.into_any()
                    }
                }}
                <span class="text-xs">
                    {move || {
                        let w = watching_resource.get().and_then(|r| r.ok()).unwrap_or(false);
                        if w { "Watching" } else { "Watch" }
                    }}
                </span>
            </button>
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
    team_key: String,
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
    let stored_tk = StoredValue::new(team_key);

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

        let tk = stored_tk.get_value();
        let label_ids_str = ids.join(",");
        leptos::task::spawn_local(async move {
            let _ = set_issue_labels(tk, number, label_ids_str).await;
            on_change.run(());
        });
    };

    view! {
        <div class="relative">
            <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Labels"</div>
            <div class="flex flex-wrap items-center gap-1">
                {move || {
                    let labels = current_display.get();
                    if labels.is_empty() {
                        view! {
                            <span class="text-sm text-muted-foreground">"None"</span>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex flex-wrap items-center gap-1">
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
    team_key: String,
    number: i32,
    description: String,
) -> impl IntoView {
    use kode_leptos::TreeWysiwygEditor;

    // Initial content — passed to the editor once, not updated reactively
    // (updating the content signal would cause re-render and focus loss).
    let initial_content = Signal::stored(description);
    let latest_text = RwSignal::new(String::new());
    let edit_version = RwSignal::new(0u32);

    let tk = team_key.clone();
    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        latest_text.set(text);
        edit_version.update(|v| *v += 1);
        let snapshot = edit_version.get_untracked();

        let tk = tk.clone();
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
                tk,
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
                None, // clear_sort_order
                None, // clear_parent
            )
            .await;
        });
    });

    let theme_state = use_context::<crate::components::theme::ThemeState>();
    let theme_signal = Signal::derive(move || {
        let mut theme = trakkt_kode_theme();
        theme.content_padding = Some("0");
        theme.bg = "var(--color-background)";
        if let Some(ts) = theme_state
            && ts.effective.get() == "dark"
        {
            theme.syntax = kode_leptos::SyntaxTheme::OneDark;
        }
        theme
    });

    view! {
        <div class="mt-6" style="min-height: 120px;">
            <TreeWysiwygEditor
                content=initial_content
                on_change=on_change
                show_fixed_toolbar=false
                show_floating_toolbar=true
                theme=theme_signal
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue Picker Modal
// ─────────────────────────────────────────────────────────────────────────────

/// Reusable modal for searching and selecting an existing issue.
///
/// Used by SubIssuesSection ("Link existing") and MetadataSidebar ("Set parent").
/// Fetches issues via `list_issues(search=...)` and excludes specified IDs
/// to prevent self-references and cycles.
#[component]
fn IssuePickerModal(
    /// Whether the modal is visible.
    show: Signal<bool>,
    /// Called when the modal should close.
    on_close: Callback<()>,
    /// Called when an issue is selected.
    on_select: Callback<IssueWithDetails>,
    /// Issue IDs to exclude from results (e.g. self + existing children).
    exclude_ids: Signal<Vec<String>>,
    /// Title for the modal header.
    title: &'static str,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());

    // Reset search when modal opens
    Effect::new(move || {
        if show.get() {
            set_search.set(String::new());
        }
    });

    // Fetch issues matching the search query
    let search_results = Resource::new(
        move || search.get(),
        move |query| async move {
            let q = if query.trim().is_empty() { None } else { Some(query) };
            list_issues(None, None, None, None, None, q, Some(20), None).await
        },
    );

    view! {
        <Modal
            show=show
            on_close=on_close
            title=title
            size=ModalSize::Md
        >
            // Search input
            <div class="mb-3">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |val: String| set_search.set(val))
                    placeholder="Search issues..."
                />
            </div>

            // Results list
            <div class="max-h-[300px] overflow-y-auto -mx-1">
                <Suspense fallback=|| view! {
                    <div class="py-4 text-center text-sm text-muted-foreground">"Searching..."</div>
                }>
                    {move || search_results.get().map(|result| {
                        match result {
                            Ok(issues) => {
                                let excluded = exclude_ids.get();
                                let filtered: Vec<_> = issues
                                    .into_iter()
                                    .filter(|i| !excluded.contains(&i.issue_id))
                                    .collect();

                                if filtered.is_empty() {
                                    view! {
                                        <div class="py-4 text-center text-sm text-muted-foreground">
                                            "No matching issues found."
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="space-y-0.5">
                                            {filtered.into_iter().map(|issue| {
                                                let issue_for_click = issue.clone();
                                                let status_variant = IssueStatusVariant::parse(&issue.status_category);
                                                let issue_key = format!("{}-{}", issue.team_key, issue.number);
                                                let issue_title = issue.title.clone();
                                                view! {
                                                    <button
                                                        class="w-full text-left flex items-center gap-2 px-3 py-2 rounded-md hover:bg-secondary/50 transition-colors"
                                                        on:click={
                                                            let issue_for_click = issue_for_click.clone();
                                                            move |_| {
                                                                on_select.run(issue_for_click.clone());
                                                                on_close.run(());
                                                            }
                                                        }
                                                    >
                                                        <IssueStatusBadge status=status_variant size=14/>
                                                        <span class="text-xs text-muted-foreground font-mono shrink-0">{issue_key}</span>
                                                        <span class="text-sm text-foreground truncate">{issue_title}</span>
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }
                            Err(_) => view! {
                                <div class="py-4 text-center text-sm text-destructive-foreground">
                                    "Failed to search issues."
                                </div>
                            }.into_any(),
                        }
                    })}
                </Suspense>
            </div>
        </Modal>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-issues Section
// ─────────────────────────────────────────────────────────────────────────────

/// Sub-issues section showing child issues with progress bar.
///
/// Displays: header with count + progress text, progress bar, compact issue list,
/// and "Link" / "Add" buttons.
#[component]
fn SubIssuesSection(
    sub_issues: Memo<Vec<IssueWithDetails>>,
    on_add: Callback<()>,
    on_link: Callback<()>,
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
                        <div class="flex items-center gap-2">
                            <button
                                class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                                on:click=move |_| on_link.run(())
                                title="Link existing issue"
                            >
                                <Icon icon=phosphor_leptos::LINK size="12px"/>
                                "Link"
                            </button>
                            <button
                                class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                                on:click=move |_| on_add.run(())
                                title="Add sub-issue"
                            >
                                <Icon icon=phosphor_leptos::PLUS size="12px"/>
                                "Add"
                            </button>
                        </div>
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
                            let child_href = format!("/issues/{}-{}", child.team_key, child.number);
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
// Relations Section
// ─────────────────────────────────────────────────────────────────────────────

/// Relations section showing blocking/blocked-by relationships with other issues.
///
/// Displays each relation with a direction indicator, linked issue identifier,
/// title, and a remove button. Includes an inline form for adding new relations.
#[component]
fn RelationsSection(
    team_key: String,
    number: i32,
) -> impl IntoView {
    let issue_identifier = format!("{team_key}-{number}");
    let tk = team_key.clone();
    let (version, set_version) = signal(0u32);
    let relations_resource = Resource::new(
        move || (tk.clone(), number, version.get()),
        move |(tk, num, _)| async move { list_issue_relations(tk, num).await },
    );

    // ── "Add relation" form state ─────────────────────────────────────
    let (show_add_form, set_show_add_form) = signal(false);
    let (add_direction, set_add_direction) = signal("blocks".to_string());
    let (add_identifier, set_add_identifier) = signal(String::new());
    let (adding, set_adding) = signal(false);
    let (add_error, set_add_error) = signal(Option::<String>::None);

    let stored_issue_identifier = StoredValue::new(issue_identifier);
    let handle_add = Callback::new(move |()| {
        let target_id_raw = add_identifier.get_untracked().trim().to_uppercase();
        if target_id_raw.is_empty() || adding.get_untracked() {
            return;
        }
        set_adding.set(true);
        set_add_error.set(None);

        let direction = add_direction.get_untracked();
        let current = stored_issue_identifier.get_value();

        // direction == "blocks"  => current blocks target => source=current, target=target, type="blocks"
        // direction == "blocked_by" => current is blocked by target => source=target, target=current, type="blocks"
        let (source, target) = if direction == "blocks" {
            (current, target_id_raw)
        } else {
            (target_id_raw, current)
        };

        leptos::task::spawn_local(async move {
            match add_relation(source, target, "blocks".to_string()).await {
                Ok(_) => {
                    set_add_identifier.set(String::new());
                    set_show_add_form.set(false);
                    set_adding.set(false);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_add_error.set(Some(format!("{e}")));
                    set_adding.set(false);
                }
            }
        });
    });

    view! {
        <div class="mt-6">
            <Suspense fallback=|| ()>
                {move || {
                    let relations = relations_resource.get()
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    let total = relations.len();

                    view! {
                        // ── Header ─────────────────────────────────────────
                        <div class="flex items-center justify-between mb-2">
                            <h2 class="text-xs text-muted-foreground font-medium">
                                {if total > 0 {
                                    format!("Relations ({})", total)
                                } else {
                                    "Relations".to_string()
                                }}
                            </h2>
                            <button
                                class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                                on:click=move |_| {
                                    set_show_add_form.update(|v| *v = !*v);
                                    set_add_error.set(None);
                                }
                                title="Add relation"
                            >
                                <Icon icon=phosphor_leptos::PLUS size="12px"/>
                                "Add"
                            </button>
                        </div>

                        // ── Relation rows ─────────────────────────────────
                        {(total > 0).then(|| {
                            let rows = relations.into_iter().map(|rel| {
                                let rel_id = rel.relation_id.clone();
                                let rel_key = format!("{}-{}", rel.team_key, rel.number);
                                let rel_href = format!("/issues/{}-{}", rel.team_key, rel.number);
                                let rel_title = rel.title.clone();
                                let direction = rel.direction.clone();
                                let status_variant = IssueStatusVariant::parse(&rel.status_category);

                                view! {
                                    <div class="group flex items-center gap-2 px-3 py-1.5 hover:bg-secondary/50 rounded-md transition-colors">
                                        // Direction label
                                        {if direction == "blocked_by" {
                                            view! {
                                                <span class="text-xs font-medium shrink-0 w-[72px] text-destructive">
                                                    "Blocked by"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="text-xs font-medium shrink-0 w-[72px] text-success-foreground">
                                                    "Blocks"
                                                </span>
                                            }.into_any()
                                        }}
                                        // Status icon
                                        <IssueStatusBadge status=status_variant size=12/>
                                        // Issue identifier
                                        <span class="text-xs text-muted-foreground font-mono shrink-0">{rel_key}</span>
                                        // Title link
                                        <a href=rel_href class="text-sm text-foreground hover:underline truncate flex-1">
                                            {rel_title}
                                        </a>
                                        // Remove button (appears on hover)
                                        <button
                                            class="opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground transition-all shrink-0"
                                            title="Remove relation"
                                            on:click=move |_| {
                                                let rid = rel_id.clone();
                                                leptos::task::spawn_local(async move {
                                                    if let Err(e) = remove_relation(rid).await {
                                                        tracing::warn!("Failed to remove relation: {e}");
                                                    }
                                                    set_version.update(|v| *v += 1);
                                                });
                                            }
                                        >
                                            <Icon icon=phosphor_leptos::X size="12px"/>
                                        </button>
                                    </div>
                                }
                            }).collect_view();
                            view! {
                                <div class="space-y-0.5">
                                    {rows}
                                </div>
                            }
                        })}

                        // ── Add relation inline form ──────────────────────
                        <Show when=move || show_add_form.get()>
                            <div class="mt-2 p-3 border border-border rounded-md space-y-2">
                                <div class="flex items-center gap-2">
                                    // Direction select
                                    <select
                                        class="text-xs bg-background border border-border rounded px-2 py-1.5 text-foreground"
                                        on:change=move |ev| set_add_direction.set(event_target_value(&ev))
                                        prop:value=move || add_direction.get()
                                    >
                                        <option value="blocks">"Blocks"</option>
                                        <option value="blocked_by">"Blocked by"</option>
                                    </select>
                                    // Identifier input
                                    <input
                                        type="text"
                                        class="flex-1 text-sm bg-background border border-border rounded px-2 py-1 text-foreground placeholder:text-muted-foreground font-mono"
                                        placeholder="e.g. TRA-12"
                                        prop:value=move || add_identifier.get()
                                        on:input=move |ev| set_add_identifier.set(event_target_value(&ev))
                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                            if ev.key() == "Enter" {
                                                handle_add.run(());
                                            } else if ev.key() == "Escape" {
                                                set_show_add_form.set(false);
                                            }
                                        }
                                    />
                                    // Submit button
                                    <Button
                                        size=ButtonSize::Sm
                                        disabled=Signal::derive(move || adding.get() || add_identifier.get().trim().is_empty())
                                        on:click=move |_| handle_add.run(())
                                    >
                                        {move || if adding.get() { "Adding..." } else { "Add" }}
                                    </Button>
                                </div>
                                // Error message
                                {move || add_error.get().map(|err| view! {
                                    <p class="text-xs text-destructive">{err}</p>
                                })}
                            </div>
                        </Show>
                    }
                }}
            </Suspense>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Comments Section
// ─────────────────────────────────────────────────────────────────────────────

/// Comments section with threaded display and new comment form.
#[component]
fn CommentsSection(
    team_key: String,
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
            <NewCommentForm team_key=team_key.clone() number=number on_created=on_comment_added/>
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
                <div class="mt-1 text-sm text-foreground" style="pointer-events: none;">
                    <kode_leptos::TreeWysiwygEditor
                        content=Signal::stored(comment.body.clone())
                        show_fixed_toolbar=false
                        theme={
                            let theme_state = use_context::<crate::components::theme::ThemeState>();
                            Signal::derive(move || {
                                let mut theme = trakkt_kode_theme();
                                theme.content_padding = Some("0");
                                theme.container_padding = Some("0");
                                theme.bg = "transparent";
                                if let Some(ts) = theme_state
                                    && ts.effective.get() == "dark"
                                {
                                    theme.syntax = kode_leptos::SyntaxTheme::OneDark;
                                }
                                theme
                            })
                        }
                    />
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
fn NewCommentForm(team_key: String, number: i32, on_created: Callback<()>) -> impl IntoView {
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
        let tk = team_key.clone();
        leptos::task::spawn_local(async move {
            match create_comment(tk, number, body, None).await {
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

    let theme_state = use_context::<crate::components::theme::ThemeState>();
    let theme_signal = Signal::derive(move || {
        let mut theme = trakkt_kode_theme();
        theme.content_padding = Some("0.75rem 1rem");
        if let Some(ts) = theme_state
            && ts.effective.get() == "dark"
        {
            theme.syntax = kode_leptos::SyntaxTheme::OneDark;
        }
        theme
    });

    view! {
        <div class="mt-6">
            <div class="border border-border rounded-md overflow-hidden" style="min-height: 120px;">
                <TreeWysiwygEditor
                    content=content.read_only()
                    on_change=on_change
                    show_fixed_toolbar=false
                    show_floating_toolbar=true
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

/// Skeleton loading state matching the two-column issue detail layout.
#[component]
fn IssueDetailSkeleton() -> impl IntoView {
    view! {
        <div class="max-w-[1140px] mx-auto w-full flex flex-col md:flex-row gap-8">
            // ── Left column ───────────────────────────────────────────
            <div class="flex-1 min-w-0">
                // Title
                <Skeleton class="h-8 w-2/3 mb-4"/>
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
            // ── Right column (metadata sidebar) ───────────────────────
            <div class="w-full md:w-[280px] shrink-0 space-y-5">
                {(0..8).map(|_| {
                    view! {
                        <div>
                            <Skeleton class="h-3 w-16 mb-1.5"/>
                            <Skeleton class="h-8 w-full"/>
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

/// Displayed when the issue identifier does not resolve to an issue.
#[component]
fn IssueNotFound(identifier: String) -> impl IntoView {
    view! {
        <div class="max-w-[860px] mx-auto w-full text-center py-16">
            <h2 class="text-xl font-semibold text-foreground mb-2">
                {format!("Issue {identifier} not found")}
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
