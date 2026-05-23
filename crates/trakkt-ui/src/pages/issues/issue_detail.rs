// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue detail page — full view of a single issue with editing.
//!
//! Layout follows Linear's two-column pattern:
//! - Header: back button + issue number (font-mono)
//! - Left column: title (editable), description (kode WYSIWYG), relations (unified), comments, timestamps
//! - Right column (280px sidebar): status, priority, assignee, labels, due date, watch, team
//! - Responsive: sidebar stacks below main content on mobile
//!
//! Key interactions:
//! - Title: click to edit, Enter/blur to save
//! - Status/Priority: DropdownMenu pickers with immediate save
//! - Description: kode WYSIWYG editor with debounced auto-save
//! - Relations: unified section showing parent, children, blocks, blocked-by
//! - Comments: threaded display with new comment form

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate, use_params_map};
use phosphor_leptos::Icon;

use crate::components::{
    Avatar, AvatarSize, Button, ButtonSize, ButtonVariant,
    DatePicker, DropdownItem, DropdownMenu, DropdownTrigger,
    IssueStatusBadge, IssueStatusVariant,
    LabelBadge, Modal, ModalSize, PriorityIndicator, SearchInput, Select, SelectVariant, Skeleton,
    ToggleButton,
};
use crate::pages::issues::issue_list::NewIssueModal;
use crate::server_fns::activities::list_issue_activities;
use crate::server_fns::attachments::{list_issue_attachments, detach_attachment_from_issue};
use crate::server_fns::github::{list_github_links_for_issue, GitHubLinkDisplay};
use crate::server_fns::comments::create_comment;
use crate::server_fns::issues::{get_issue, list_issues, set_issue_labels, update_issue};
use crate::server_fns::labels::list_labels;
use crate::server_fns::projects::list_milestones;
use crate::server_fns::relations::{add_relation, list_issue_relations, remove_relation};
use crate::server_fns::statuses::list_statuses;
use crate::server_fns::team::list_workspace_members;
use crate::server_fns::watchers::{is_watching, watch_issue, unwatch_issue};
use crate::types::{IssueNavState, WorkspaceMember};
use crate::utils::relative_time::{format_datetime, relative_time};
use trakkt_types::models::{Comment, IssueActivity, IssueWithDetails};

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
            // Issue not in SyncStore — may be archived (excluded from sync).
            // Fall through to server_issue rather than returning None immediately.
            if store.initialized().get() {
                return server_issue.get();
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

    // ── Read navigation state from browser History API ───────────────
    let nav_state = {
        let location = use_location();
        let state = location.state.get_untracked();
        let js = state.to_js_value();
        js.as_string()
            .and_then(|s| match serde_json::from_str::<IssueNavState>(&s) {
                Ok(state) => Some(state),
                Err(e) => {
                    tracing::warn!("Failed to parse IssueNavState: {e}");
                    None
                }
            })
    };
    let back_path = nav_state
        .as_ref()
        .map(|s| s.back_path.clone())
        .unwrap_or_else(|| "/my-issues".to_string());
    let back_label = nav_state
        .as_ref()
        .map(|s| s.back_label.clone())
        .unwrap_or_else(|| "My Issues".to_string());

    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Header ─────────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::IconSm
                    aria_label=format!("Back to {back_label}")
                    on:click={
                        let back_path = back_path.clone();
                        move |_| {
                            let nav = use_navigate();
                            nav(&back_path, Default::default());
                        }
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
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let initial = RwSignal::new(initial_issue);

    let issue = Memo::new(move |_| {
        if let Some(store) = sync_store {
            let items = store.issues().get();
            if let Some(found) = items.iter().find(|i| i.team_key == issue_team_key_for_lookup && i.number == number) {
                return found.clone();
            }
        }
        initial.get()
    });

    // ── Comments: derived from SyncStore (real-time via WebSocket) ────
    let issue_id_for_comments = initial.get_untracked().issue_id.clone();
    let comments = Memo::new(move |_| {
        sync_store.map(|store| {
            let mut filtered: Vec<Comment> = store.comments().get()
                .into_iter()
                .filter(|c| c.issue_id == issue_id_for_comments)
                .collect();
            filtered.sort_by_key(|a| a.created_at);
            filtered
        }).unwrap_or_default()
    });

    // No-op callback for components that need on_change but don't need parent notification
    let noop = Callback::new(|()| {});

    // ── Fine-grained memos: only re-render when the specific field changes ──
    let title = Memo::new(move |_| issue.get().title.clone());
    let description = Memo::new(move |_| issue.get().description.clone().unwrap_or_default());
    let parent_identifier = Memo::new(move |_| issue.get().parent_identifier.clone());
    let timestamps = Memo::new(move |_| {
        let i = issue.get();
        (i.created_at.clone(), i.updated_at.clone())
    });

    // Shared lightbox state for all editors (description, comments)
    let lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>> = RwSignal::new(None);

    view! {
        <div class="max-w-[1140px] mx-auto w-full flex flex-col md:flex-row gap-8">
            // ── Left column: main content ─────────────────────────────
            <div class="flex-1 min-w-0">
                // ── Parent breadcrumb ─────────────────────────────────
                <Show when=move || parent_identifier.get().is_some()>
                    <a
                        href=move || format!("/issues/{}", parent_identifier.get().unwrap_or_default())
                        class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1 mb-1"
                    >
                        <Icon icon=phosphor_leptos::ARROW_BEND_UP_LEFT size="12px"/>
                        {move || {
                            let pid = parent_identifier.get().unwrap_or_default();
                            let ptitle = issue.get().parent_title.clone();
                            if let Some(t) = ptitle {
                                format!("{pid} {t}")
                            } else {
                                pid
                            }
                        }}
                    </a>
                </Show>

                // ── Title ──────────────────────────────────────────────
                <EditableTitle team_key=initial_team_key.clone() number=number title=title on_save=noop/>

                // ── Description ────────────────────────────────────────
                <DescriptionEditor
                    team_key=initial_team_key.clone()
                    number=number
                    description=Signal::from(description)
                    lightbox_state=lightbox_state
                    issue_id=initial.get_untracked().issue_id.clone()
                />

                // ── Relations section (unified: parent, children, blocks, blocked-by) ──
                <RelationsSection
                    team_key=initial_team_key.clone()
                    number=number
                    issue_id=initial.get_untracked().issue_id.clone()
                    team_id=initial.get_untracked().team_id.clone()
                />

                // ── Attachments section (file uploads linked to this issue) ──
                <AttachmentsSection
                    team_key=initial_team_key.clone()
                    number=number
                    issue_id=initial.get_untracked().issue_id.clone()
                    lightbox_state=lightbox_state
                />

                // ── GitHub activity (PRs, branches, commits linked to this issue) ──
                <GitHubActivitySection
                    team_key=initial_team_key.clone()
                    number=number
                />

                // ── Divider ────────────────────────────────────────────
                <div class="border-t border-border my-6"></div>

                // ── Activity timeline (activities + comments merged) ───
                <IssueTimeline
                    team_key=initial.get_untracked().team_key.clone()
                    number=number
                    comments=Signal::from(comments)
                    lightbox_state=lightbox_state
                    issue_id=initial.get_untracked().issue_id.clone()
                />

                // ── Footer: timestamps ────────────────────────────────
                <div class="mt-6 pb-4">
                    <div class="flex items-center gap-4 text-xs text-muted-foreground">
                        <span>{move || { let (created, _) = timestamps.get(); format!("Created {}", relative_time(&created)) }}</span>
                        <span>{move || { let (_, updated) = timestamps.get(); format!("Updated {}", relative_time(&updated)) }}</span>
                    </div>
                </div>
            </div>

            // ── Right column: metadata sidebar ────────────────────────
            <div class="w-full md:w-[280px] shrink-0">
                <MetadataSidebar issue=Signal::from(issue) on_change=noop/>
            </div>
        </div>

        // ── Lightbox overlay (shared across all editors on this page) ──
        <crate::components::lightbox::Lightbox state=lightbox_state/>
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
    title: Memo<String>,
    on_save: Callback<()>,
) -> impl IntoView {
    let (editing, set_editing) = signal(false);
    let (current_title, set_current_title) = signal(title.get_untracked());
    let (saving, set_saving) = signal(false);
    let original_title = title.get_untracked();
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
                None, None, None, None, None, None, None, None, None,
                None,
            )
            .await;
            // Guard: component may have been destroyed while the future was in flight.
            if set_saving.try_set(false).is_some() {
                on_save.try_run(());
            }
        });
    };

    // Focus the input when entering edit mode
    Effect::new(move || {
        if editing.get() && let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    view! {
        <Show
            when=move || editing.get()
            fallback=move || {
                view! {
                    <h1
                        class="font-display text-foreground cursor-pointer hover:text-foreground/80 transition-colors"
                        style="font-size: 36px; font-weight: 400; line-height: 1.1; letter-spacing: -0.01em;"
                        on:click=move |_| {
                            set_editing.set(true);
                        }
                        title="Click to edit title"
                    >
                        {move || title.get()}
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
/// and team — stacked vertically in the right column.
#[component]
fn MetadataSidebar(
    issue: Signal<IssueWithDetails>,
    on_change: Callback<()>,
) -> impl IntoView {
    // ── Per-field Memos: only re-render the UI slice that actually changed ──
    let number = Memo::new(move |_| issue.get().number);
    let team_key = Memo::new(move |_| issue.get().team_key.clone());
    let team_id = Memo::new(move |_| issue.get().team_id.clone());
    let status_id = Memo::new(move |_| issue.get().status_id.clone());
    let status_category = Memo::new(move |_| issue.get().status_category.clone());
    let status_name = Memo::new(move |_| issue.get().status_name.clone());
    let priority = Memo::new(move |_| issue.get().priority);
    let assignee_id = Memo::new(move |_| issue.get().assignee_id.clone());
    let assignee_name = Memo::new(move |_| issue.get().assignee_name.clone());
    let project_id = Memo::new(move |_| issue.get().project_id.clone());
    let project_name = Memo::new(move |_| issue.get().project_name.clone());
    let milestone_id = Memo::new(move |_| issue.get().milestone_id.clone());
    let due_date = Memo::new(move |_| issue.get().due_date.clone());
    let estimate = Memo::new(move |_| issue.get().estimate);
    let labels = Memo::new(move |_| issue.get().labels.clone());

    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Fetch statuses dynamically for the status dropdown.
    let statuses_resource = LocalResource::new(move || list_statuses(None));

    // Fetch workspace members for the assignee dropdown.
    let members_resource = LocalResource::new(list_workspace_members);

    // team_key and number are stable identifiers for the lifetime of this page.
    let stored_tk = StoredValue::new(team_key.get_untracked());
    let stored_number = number.get_untracked();

    // ── Status change handler ───────────────────────────────────────────
    let on_status_change = move |new_status_id: String| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
                n,
                None, None,
                Some(new_status_id),
                None, None, None, None, None, None, None,
                None,
            )
            .await;
            on_change.try_run(());
        });
    };

    // ── Priority change handler ─────────────────────────────────────────
    let on_priority_change = move |prio: i32| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
                n,
                None, None, None,
                Some(prio),
                None, None, None, None, None, None,
                None,
            )
            .await;
            on_change.try_run(());
        });
    };

    // ── Assignee reactive state (local signals for optimistic updates) ──
    let (current_assignee_id, set_current_assignee_id) = signal(assignee_id.get_untracked());
    let (current_assignee_name, set_current_assignee_name) = signal(assignee_name.get_untracked());

    // Sync from server-pushed changes via the Memos
    Effect::new(move || {
        if set_current_assignee_id.try_set(assignee_id.get()).is_some() {
            tracing::warn!("MetadataSidebar: assignee_id signal disposed before sync");
        }
    });
    Effect::new(move || {
        if set_current_assignee_name.try_set(assignee_name.get()).is_some() {
            tracing::warn!("MetadataSidebar: assignee_name signal disposed before sync");
        }
    });

    // ── Assignee change handler ────────────────────────────────────────
    let on_assignee_change = move |user_id: String, display_name: Option<String>| {
        let is_clear = user_id.is_empty();
        set_current_assignee_id.set(if is_clear { None } else { Some(user_id.clone()) });
        set_current_assignee_name.set(display_name);
        let tk = stored_tk.get_value();
        let n = stored_number;
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
                n,
                None, None, None, None,
                if is_clear { None } else { Some(user_id) },
                None, None, None, None, None,
                if is_clear { Some("assignee".to_string()) } else { None },
            )
            .await;
            on_change.try_run(());
        });
    };

    // ── Project change handler ─────────────────────────────────────────
    let on_project_change = move |pid: String| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        let is_clear = pid.is_empty();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk, n,
                None, None, None, None, None, None,
                if is_clear { None } else { Some(pid) },
                None, None, None,
                Some(if is_clear { "project,milestone".to_string() } else { "milestone".to_string() }),
            ).await;
            on_change.try_run(());
        });
    };

    // ── Milestone change handler ───────────────────────────────────────
    let on_milestone_change = move |mid: String| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        let is_clear = mid.is_empty();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk, n,
                None, None, None, None, None, None,
                None,
                if is_clear { None } else { Some(mid) },
                None, None,
                if is_clear { Some("milestone".to_string()) } else { None },
            ).await;
            on_change.try_run(());
        });
    };

    // ── Due date change handler ───────────────────────────────────────
    let on_due_date_change = move |date: Option<String>| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        let is_clear = date.is_none();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
                n,
                None, None, None, None, None,
                date,
                None, None, None, None,
                if is_clear { Some("due_date".to_string()) } else { None },
            )
            .await;
            on_change.try_run(());
        });
    };

    // ── Estimate: look up team settings from SyncStore ────────────────
    let team_settings: Signal<Option<trakkt_types::models::TeamSettings>> = Signal::derive(move || {
        let tid = team_id.get();
        sync_store
            .and_then(|store| {
                store.teams().get().into_iter()
                    .find(|t| t.team_id == tid)
                    .and_then(|t| t.settings.clone())
            })
    });
    let estimates_enabled = Memo::new(move |_| team_settings.get().and_then(|s| s.estimate_scale).is_some());

    // ── Estimate change handler ────────────────────────────────────────
    let on_estimate_change = move |value: Option<i32>| {
        let tk = stored_tk.get_value();
        let n = stored_number;
        let is_clear = value.is_none();
        leptos::task::spawn_local(async move {
            let _ = update_issue(
                tk,
                n,
                None, None, None, None, None, None, None, None, None,
                if is_clear { None } else { value },
                if is_clear { Some("estimate".to_string()) } else { None },
            )
            .await;
            on_change.try_run(());
        });
    };

    let (estimate_open, set_estimate_open) = signal(false);
    let estimate_trigger_ref = NodeRef::<leptos::html::Div>::new();

    let status_variant = Memo::new(move |_| IssueStatusVariant::parse(&status_category.get(), &status_name.get()));
    let (status_open, set_status_open) = signal(false);
    let (priority_open, set_priority_open) = signal(false);
    let status_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let priority_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let statuses = RwSignal::new(Vec::<trakkt_types::models::Status>::new());

    Effect::new(move || {
        if let Some(Ok(loaded)) = statuses_resource.get()
            && statuses.try_set(loaded).is_some()
        {
            tracing::warn!("MetadataSidebar: statuses signal disposed before load");
        }
    });

    let members = RwSignal::new(Vec::<WorkspaceMember>::new());

    Effect::new(move || {
        if let Some(Ok(loaded)) = members_resource.get()
            && members.try_set(loaded).is_some()
        {
            tracing::warn!("MetadataSidebar: members signal disposed before load");
        }
    });

    let (assignee_open, set_assignee_open) = signal(false);
    let assignee_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (assignee_search, set_assignee_search) = signal(String::new());

    let current_status_name = move || {
        let id = status_id.get();
        statuses.get().iter()
            .find(|s| s.status_id == id)
            .map(|s| s.name.clone())
    };

    let (project_open, set_project_open) = signal(false);
    let project_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (project_search, set_project_search) = signal(String::new());
    let (milestone_open, set_milestone_open) = signal(false);
    let milestone_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (milestone_search, set_milestone_search) = signal(String::new());

    // Milestones: reactive resource that refetches when project_id changes
    let milestones = RwSignal::new(Vec::<trakkt_types::models::ProjectMilestone>::new());
    Effect::new(move || {
        let pid = project_id.get();
        if let Some(pid) = pid {
            leptos::task::spawn_local(async move {
                match list_milestones(pid).await {
                    Ok(loaded) => {
                        if milestones.try_set(loaded).is_some() {
                            tracing::warn!("MetadataSidebar: milestones signal disposed before load");
                        }
                    }
                    Err(e) => { tracing::warn!("Failed to load milestones: {e}"); }
                }
            });
        } else {
            if milestones.try_set(Vec::new()).is_none() {
                tracing::warn!("MetadataSidebar: milestones signal disposed before clear");
            }
        }
    });

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
                            view! { <IssueStatusBadge status=status_variant.get() size=12/> }.into_any()
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
                    {move || statuses.get().into_iter().map({
                        move |status| {
                            let sid = status.status_id.clone();
                            let sid_check = status.status_id.clone();
                            let label = status.name.clone();
                            let variant = IssueStatusVariant::parse(&status.category, &status.name);
                            view! {
                                <DropdownItem
                                    label=label
                                    selected=Signal::derive(move || status_id.get() == sid_check)
                                    on_select=Callback::new({
                                        let id = sid.clone();
                                        move |()| {
                                            on_status_change(id.clone());
                                            set_status_open.set(false);
                                        }
                                    })
                                    icon=Arc::new(move || view! { <IssueStatusBadge status=variant size=14/> }.into_any()) as ChildrenFn
                                />
                            }
                        }
                    }).collect_view()}
                </DropdownMenu>
            </div>

            // ── Priority ───────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Priority"</div>
                <div node_ref=priority_trigger_ref>
                    <DropdownTrigger
                        label="Priority"
                        value=Signal::derive(move || {
                            Some(match priority.get() {
                                1 => "Urgent",
                                2 => "High",
                                3 => "Medium",
                                4 => "Low",
                                _ => "None",
                            }.to_string())
                        })
                        icon=Arc::new(move || {
                            view! { <PriorityIndicator priority=priority.get() size=14/> }.into_any()
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
                                        selected=Signal::derive(move || priority.get() == prio)
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

            // ── Assignee ───────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Assignee"</div>
                <div node_ref=assignee_trigger_ref>
                    <DropdownTrigger
                        label="Assign..."
                        value=Signal::derive(move || current_assignee_name.get())
                        icon=Arc::new(move || {
                            if let Some(n) = current_assignee_name.get() {
                                view! { <Avatar name=n size=AvatarSize::Sm/> }.into_any()
                            } else {
                                view! { <Icon icon=phosphor_leptos::USER size="14px"/> }.into_any()
                            }
                        }) as ChildrenFn
                        on_click=Callback::new(move |()| set_assignee_open.update(|o| *o = !*o))
                    />
                </div>
                <DropdownMenu
                    trigger_ref=assignee_trigger_ref
                    open=Signal::derive(move || assignee_open.get())
                    on_close=Callback::new(move |()| { set_assignee_open.set(false); set_assignee_search.set(String::new()); })
                    search_placeholder="Filter members..."
                    on_search=Callback::new(move |text: String| set_assignee_search.set(text))
                >
                    {move || {
                        let search = assignee_search.get().to_lowercase();
                        let filtered: Vec<_> = members.get()
                            .into_iter()
                            .filter(|m| {
                                if search.is_empty() {
                                    return true;
                                }
                                let name_match = m.name.as_deref()
                                    .map(|n| n.to_lowercase().contains(&search))
                                    .unwrap_or(false);
                                let email_match = m.email.to_lowercase().contains(&search);
                                name_match || email_match
                            })
                            .collect();
                        let none_item = view! {
                            <DropdownItem
                                label="Unassigned".to_string()
                                selected=Signal::derive(move || current_assignee_id.get().is_none())
                                on_select=Callback::new(move |()| {
                                    on_assignee_change(String::new(), None);
                                    set_assignee_open.set(false);
                                })
                            />
                        };
                        let member_items = filtered.into_iter().map(|member| {
                            let user_id = member.user_id.clone();
                            let user_id_check = member.user_id.clone();
                            let display_name = member.name.clone()
                                .unwrap_or_else(|| member.email.clone());
                            let avatar_name = display_name.clone();
                            let label = display_name.clone();
                            view! {
                                <DropdownItem
                                    label=label
                                    selected=Signal::derive(move || {
                                        current_assignee_id.get().as_deref() == Some(user_id_check.as_str())
                                    })
                                    on_select=Callback::new({
                                        let id = user_id.clone();
                                        let name = display_name.clone();
                                        move |()| {
                                            on_assignee_change(id.clone(), Some(name.clone()));
                                            set_assignee_open.set(false);
                                        }
                                    })
                                    icon=Arc::new({
                                        let name = avatar_name.clone();
                                        move || view! { <Avatar name=name.clone() size=AvatarSize::Sm/> }.into_any()
                                    }) as ChildrenFn
                                />
                            }
                        }).collect_view();
                        view! { {none_item} {member_items} }
                    }}
                </DropdownMenu>
            </div>

            // ── Labels ─────────────────────────────────────────────────
            <LabelPicker
                team_key=stored_tk.get_value()
                number=stored_number
                team_id=team_id.get_untracked()
                current_labels=labels
                on_change=on_change
            />

            // ── Project ───────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Project"</div>
                <div class="flex items-center gap-1">
                    <div node_ref=project_trigger_ref class="flex-1 min-w-0">
                        <DropdownTrigger
                            label="Set project..."
                            value=Signal::derive(move || project_name.get())
                            icon=Arc::new(move || {
                                view! { <Icon icon=phosphor_leptos::FOLDER_SIMPLE size="14px"/> }.into_any()
                            }) as ChildrenFn
                            on_click=Callback::new(move |()| set_project_open.update(|o| *o = !*o))
                        />
                    </div>
                    {move || {
                        if project_id.get().is_some() {
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
                    }}
                </div>
                <DropdownMenu
                    trigger_ref=project_trigger_ref
                    open=Signal::derive(move || project_open.get())
                    on_close=Callback::new(move |()| { set_project_open.set(false); set_project_search.set(String::new()); })
                    search_placeholder="Filter projects..."
                    on_search=Callback::new(move |text: String| set_project_search.set(text))
                >
                    {move || {
                        let search = project_search.get().to_lowercase();
                        let projects: Vec<_> = sync_store.map(|store| store.projects().get()).unwrap_or_default()
                            .into_iter()
                            .filter(|p| search.is_empty() || p.name.to_lowercase().contains(&search))
                            .collect();
                        let none_item = {
                            view! {
                                <DropdownItem
                                    label="None".to_string()
                                    selected=Signal::derive(move || project_id.get().is_none())
                                    on_select=Callback::new(move |()| {
                                        on_project_change(String::new());
                                        set_project_open.set(false);
                                    })
                                />
                            }
                        };
                        let project_items = projects.into_iter().map({
                            move |project| {
                                let pid = project.project_id.clone();
                                let pid_check = project.project_id.clone();
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
                                            let check = pid_check.clone();
                                            move || project_id.get().as_deref() == Some(check.as_str())
                                        })
                                        on_select=Callback::new({
                                            let id = pid.clone();
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
                    }}
                </DropdownMenu>
            </div>

            // ── Milestone (only when project is set) ──────────────────
            {move || {
                if project_id.get().is_some() {
                    Some(view! {
                        <div>
                            <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Milestone"</div>
                            <div class="flex items-center gap-1">
                                <div node_ref=milestone_trigger_ref class="flex-1 min-w-0">
                                    <DropdownTrigger
                                        label="Set milestone..."
                                        value=Signal::derive(move || {
                                            let mid = milestone_id.get()?;
                                            milestones.get().iter()
                                                .find(|m| m.milestone_id == mid)
                                                .map(|m| m.name.clone())
                                        })
                                        icon=Arc::new(move || {
                                            view! { <Icon icon=phosphor_leptos::FLAG size="14px"/> }.into_any()
                                        }) as ChildrenFn
                                        on_click=Callback::new(move |()| set_milestone_open.update(|o| *o = !*o))
                                    />
                                </div>
                                {move || {
                                    if milestone_id.get().is_some() {
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
                                }}
                            </div>
                            <DropdownMenu
                                trigger_ref=milestone_trigger_ref
                                open=Signal::derive(move || milestone_open.get())
                                on_close=Callback::new(move |()| { set_milestone_open.set(false); set_milestone_search.set(String::new()); })
                                search_placeholder="Filter milestones..."
                                on_search=Callback::new(move |text: String| set_milestone_search.set(text))
                            >
                                {move || {
                                    let search = milestone_search.get().to_lowercase();
                                    let ms_list: Vec<_> = milestones.get()
                                        .into_iter()
                                        .filter(|m| search.is_empty() || m.name.to_lowercase().contains(&search))
                                        .collect();
                                    let none_item = {
                                        view! {
                                            <DropdownItem
                                                label="None".to_string()
                                                selected=Signal::derive(move || milestone_id.get().is_none())
                                                on_select=Callback::new(move |()| {
                                                    on_milestone_change(String::new());
                                                    set_milestone_open.set(false);
                                                })
                                            />
                                        }
                                    };
                                    let milestone_items = ms_list.into_iter().map({
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
                                                        let check = ms_id_check.clone();
                                                        move || milestone_id.get().as_deref() == Some(check.as_str())
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
                                }}
                            </DropdownMenu>
                        </div>
                    })
                } else {
                    None
                }
            }}

            // ── Due date ────────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Due"</div>
                <DatePicker
                    value=Signal::derive(move || due_date.get())
                    on_change=Callback::new(on_due_date_change)
                    placeholder="Set due date..."
                />
            </div>

            // ── Estimate (only when team has estimates enabled) ──────────
            {move || {
                if !estimates_enabled.get() {
                    return None;
                }
                let settings = team_settings.get()?;
                let scale = settings.estimate_scale.as_ref()?;
                let options = scale.options(settings.estimate_extended, settings.estimate_allow_zero);
                let stored_options = StoredValue::new(options);
                let scale_for_label = scale.clone();
                Some(view! {
                    <div>
                        <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Estimate"</div>
                        <div node_ref=estimate_trigger_ref>
                            <DropdownTrigger
                                label="Estimate"
                                value=Signal::derive(move || {
                                    estimate.get().map(|v| scale_for_label.format_label(v))
                                })
                                icon=Arc::new(move || {
                                    view! { <Icon icon=phosphor_leptos::GAUGE size="14px"/> }.into_any()
                                }) as ChildrenFn
                                on_click=Callback::new(move |()| set_estimate_open.update(|o| *o = !*o))
                            />
                        </div>
                        <DropdownMenu
                            trigger_ref=estimate_trigger_ref
                            open=Signal::derive(move || estimate_open.get())
                            on_close=Callback::new(move |()| set_estimate_open.set(false))
                        >
                            {move || {
                                // "No estimate" option (clears the value)
                                let none_item = view! {
                                    <DropdownItem
                                        label="No estimate".to_string()
                                        selected=Signal::derive(move || estimate.get().is_none())
                                        on_select=Callback::new(move |()| {
                                            on_estimate_change(None);
                                            set_estimate_open.set(false);
                                        })
                                    />
                                };
                                let option_items = stored_options.get_value().iter().map(|opt| {
                                    let value = opt.value;
                                    let label = opt.label.clone();
                                    view! {
                                        <DropdownItem
                                            label=label
                                            selected=Signal::derive(move || estimate.get() == Some(value))
                                            on_select=Callback::new(move |()| {
                                                on_estimate_change(Some(value));
                                                set_estimate_open.set(false);
                                            })
                                        />
                                    }
                                }).collect_view();
                                view! { {none_item} {option_items} }
                            }}
                        </DropdownMenu>
                    </div>
                })
            }}

            // ── Watch toggle ──────────────────────────────────────────────
            <WatchToggle team_key=stored_tk.get_value() number=stored_number/>

            // ── Team ──────────────────────────────────────────────────────
            <div>
                <div class="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-1.5">"Team"</div>
                <span class="text-sm text-foreground">{move || team_key.get()}</span>
            </div>
        </div>
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
            // Guard: component may have been destroyed while the future was in flight.
            let _ = set_loading.try_set(false);
            let _ = set_version.try_update(|v| *v += 1);
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
    current_labels: Memo<Vec<trakkt_types::models::Label>>,
    on_change: Callback<()>,
) -> impl IntoView {
    let (show_picker, set_show_picker) = signal(false);
    let initial = current_labels.get_untracked();
    let current_ids = RwSignal::new(
        initial.iter().map(|l| l.label_id.clone()).collect::<Vec<_>>()
    );
    let current_display = RwSignal::new(initial);
    let stored_tk = StoredValue::new(team_key);

    Effect::new(move || {
        let external = current_labels.get();
        if current_ids.try_set(external.iter().map(|l| l.label_id.clone()).collect::<Vec<_>>()).is_none() {
            tracing::warn!("LabelPicker: current_ids signal disposed before sync effect ran");
        }
        if current_display.try_set(external).is_none() {
            tracing::warn!("LabelPicker: current_display signal disposed before sync effect ran");
        }
    });

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
            on_change.try_run(());
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
///
/// Uses a gated content signal to prevent WebSocket echoes from
/// overwriting local edits while a save is in flight.
#[component]
fn DescriptionEditor(
    team_key: String,
    number: i32,
    description: Signal<String>,
    lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>>,
    /// Issue ID for auto-linking inline uploads to this issue.
    issue_id: String,
) -> impl IntoView {
    use kode_leptos::TreeWysiwygEditor;

    let latest_text = RwSignal::new(String::new());
    let edit_version = RwSignal::new(0u32);

    // Gated content signal: suppresses WebSocket echoes while editing.
    // Only forwards external description changes when no local save is pending.
    let editing = RwSignal::new(false);
    let gated_content = RwSignal::new(description.get_untracked());

    Effect::new(move || {
        let external = description.get();
        if !editing.get_untracked()
            && gated_content.try_set(external).is_some()
        {
            tracing::warn!("DescriptionEditor: gated_content signal disposed before sync");
        }
    });

    // Derived signal that auto-links issue identifiers in the description markdown.
    // The editor renders these as clickable links; on_change guards against
    // saving auto-link-only changes back to the server.
    let auto_linked_content = Signal::derive(move || {
        crate::utils::auto_link::auto_link_issue_identifiers(&gated_content.get())
    });

    let tk = team_key.clone();
    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        // Detect if this change was caused solely by auto-linking.
        // If the incoming text matches what auto_link would produce from the
        // current raw content, but differs from the raw content, it's an
        // auto-link echo — update gated_content so subsequent edits have the
        // linked version as baseline, but don't trigger a server save.
        let current = gated_content.get_untracked();
        let auto_linked = crate::utils::auto_link::auto_link_issue_identifiers(&current);
        if text == auto_linked && text != current {
            gated_content.set(text);
            return;
        }

        editing.set(true);
        latest_text.set(text);
        edit_version.update(|v| *v += 1);
        let snapshot = edit_version.get_untracked();

        let tk = tk.clone();
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            // Guard: component may have been destroyed during the debounce wait.
            let Some(current_version) = edit_version.try_get_untracked() else {
                return;
            };
            if current_version != snapshot {
                return;
            }
            let Some(current_text) = latest_text.try_get_untracked() else {
                return;
            };
            let desc = if current_text.trim().is_empty() {
                Some(String::new())
            } else {
                Some(current_text.clone())
            };
            let _ = update_issue(
                tk,
                number,
                None,
                desc,
                None, None, None, None, None, None, None, None,
                None,
            )
            .await;
            // Save acknowledged — allow external updates again and sync the
            // gated signal to what we just saved so kode's content sync Effect
            // sees no diff against its DocState.
            // Guard: component may have been destroyed while the save was in flight.
            let _ = gated_content.try_set(current_text);
            let _ = editing.try_set(false);
        });
    });

    // Attachment callbacks — pass issue_id so uploads are auto-linked.
    let error_toast = crate::components::toast::capture_error_toast();
    let upload_complete: RwSignal<Option<kode_leptos::UploadComplete>> = RwSignal::new(None);
    let on_upload = crate::components::attachment_hooks::make_upload_callback(upload_complete, Some(issue_id.clone()), error_toast.clone());
    let on_delete = crate::components::attachment_hooks::make_delete_callback(error_toast.clone());
    let on_click = crate::components::attachment_hooks::make_click_callback(lightbox_state);

    // ── "Attach file" slash command extension ───────────────────────────
    let attach_file_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let (attach_triggered, set_attach_triggered) = signal(false);
    let on_change_for_attach = on_change.clone();

    let attach_ext = Arc::new(
        crate::components::attachment_extension::AttachFileExtension::new(
            Arc::new(move || set_attach_triggered.set(true)),
        ),
    );

    // When the extension fires, click the hidden file input.
    Effect::new(move || {
        if attach_triggered.get() {
            set_attach_triggered.set(false);
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(input_el) = attach_file_input_ref.get() {
                    let input: &web_sys::HtmlInputElement = input_el.as_ref();
                    input.click();
                }
            }
        }
    });

    // When a file is selected via the extension's file picker, read, validate,
    // upload, and inject the resulting markdown into the editor content.
    let stored_issue_id = StoredValue::new(issue_id);
    let error_toast_for_attach = error_toast.clone();
    let on_attach_file_selected = move |_ev: leptos::ev::Event| {
        let _stored_issue_id = stored_issue_id;
        let _on_change_for_attach = on_change_for_attach.clone();
        let _error_toast = error_toast_for_attach.clone();

        #[cfg(target_arch = "wasm32")]
        {
            use crate::components::attachment_hooks::{
                upload_file, ALLOWED_CONTENT_TYPES, ALLOWED_EXTENSIONS, MAX_FILE_SIZE,
            };

            let input_el = match attach_file_input_ref.get() {
                Some(el) => el,
                None => return,
            };
            let input: &web_sys::HtmlInputElement = input_el.as_ref();
            let files = match input.files() {
                Some(f) => f,
                None => return,
            };
            let file = match files.get(0) {
                Some(f) => f,
                None => return,
            };

            let name = file.name();
            let content_type = file.type_();
            let size = file.size() as u64;

            if size > MAX_FILE_SIZE {
                tracing::warn!("Attach file: too large ({size} bytes)");
                _error_toast("File exceeds 10 MB size limit".to_string());
                input.set_value("");
                return;
            }

            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if !ALLOWED_EXTENSIONS.contains(&ext.as_str())
                && !ALLOWED_CONTENT_TYPES.contains(&content_type.as_str())
            {
                tracing::warn!("Attach file: type not allowed (ext={ext}, content_type={content_type})");
                _error_toast("File type not allowed".to_string());
                input.set_value("");
                return;
            }

            let issue_id = _stored_issue_id.get_value();
            let error_toast_spawn = _error_toast.clone();

            leptos::task::spawn_local(async move {
                let data = {
                    use wasm_bindgen_futures::JsFuture;
                    let promise = file.array_buffer();
                    match JsFuture::from(promise).await {
                        Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                        Err(_) => {
                            tracing::warn!("Attach file: failed to read file bytes");
                            error_toast_spawn("Failed to read file".to_string());
                            return;
                        }
                    }
                };

                match upload_file(&data, &name, &content_type, Some(&issue_id)).await {
                    Ok(resp) => {
                        // Build inline markdown for the uploaded attachment.
                        let markdown_snippet = if content_type.starts_with("image/") {
                            format!("\n![{}]({})\n", name, resp.url)
                        } else {
                            format!("\n[{}]({})\n", name, resp.url)
                        };
                        // Append the snippet to the current editor content.
                        // This triggers the content sync Effect in TreeWysiwygEditor,
                        // which re-parses and renders the new image/file node.
                        let Some(current) = gated_content.try_get_untracked() else {
                            return;
                        };
                        let updated = format!("{}{}", current.trim_end(), markdown_snippet);
                        let _ = gated_content.try_set(updated.clone());
                        _on_change_for_attach(updated);
                    }
                    Err(e) => {
                        tracing::warn!("Attach file: upload failed: {e}");
                        error_toast_spawn(format!("Upload failed: {e}"));
                    }
                }
                if let Some(input_el) = attach_file_input_ref.get() {
                    let input: &web_sys::HtmlInputElement = input_el.as_ref();
                    input.set_value("");
                }
            });
        }
    };

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
            // Hidden file input for the "Attach file" slash command extension
            <input
                type="file"
                node_ref=attach_file_input_ref
                class="hidden"
                accept=".png,.jpg,.jpeg,.gif,.webp,.svg,.pdf,.csv,.txt,.json,.log"
                on:change=on_attach_file_selected
            />
            <TreeWysiwygEditor
                content=auto_linked_content
                on_change=on_change
                show_fixed_toolbar=false
                show_floating_toolbar=true
                theme=theme_signal
                on_upload=on_upload
                on_delete_attachment=on_delete
                on_click_attachment=on_click
                upload_complete=upload_complete
                extensions=vec![attach_ext as Arc<dyn kode_leptos::Extension>]
            />
        </div>
    }
}


// ─────────────────────────────────────────────────────────────────────────────
// Relations Section (unified: parent, children, blocks, blocked-by)
// ─────────────────────────────────────────────────────────────────────────────

/// Unified relations section showing ALL relation types in one list.
///
/// Consolidates the old Parent sidebar section, SubIssuesSection, and
/// RelationsSection into a single component. Displays parent, duplicate-of,
/// blocked-by, blocks, has-duplicate, and child relations grouped in that order.
///
/// Provides:
/// - "+ Add relation" button that opens a modal with a relation-type selector and issue picker
/// - "+ New sub-issue" button that opens the NewIssueModal with parent_issue_id set
/// - Per-row remove button (hover-reveal)
#[component]
fn RelationsSection(
    team_key: String,
    number: i32,
    /// The current issue's ID (used as parent_issue_id when creating sub-issues).
    issue_id: String,
    /// The current issue's team_id (used to scope the NewIssueModal team).
    team_id: String,
) -> impl IntoView {
    let issue_identifier = format!("{team_key}-{number}");
    let tk = team_key.clone();
    let (version, set_version) = signal(0u32);
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let ws_version = Signal::derive(move || {
        sync_store.map(|s| s.relations_version().get()).unwrap_or(0)
    });
    let relations_resource = Resource::new(
        move || (tk.clone(), number, version.get(), ws_version.get()),
        move |(tk, num, _, _)| async move { list_issue_relations(tk, num).await },
    );

    // ── "Add relation" modal state ───────────────────────────────────
    let (show_add_modal, set_show_add_modal) = signal(false);
    let (add_relation_type, set_add_relation_type) = signal("child_of".to_string());

    // ── "New sub-issue" modal state ──────────────────────────────────
    let (show_new_sub_issue, set_show_new_sub_issue) = signal(false);

    let stored_issue_identifier = StoredValue::new(issue_identifier);
    let stored_issue_id = StoredValue::new(issue_id);
    let stored_team_id = StoredValue::new(team_id);
    let stored_team_key = StoredValue::new(team_key.clone());

    // Handler for when a relation is added via the picker modal
    let on_relation_selected = Callback::new(move |selected: IssueWithDetails| {
        let rel_type = add_relation_type.get_untracked();
        let current = stored_issue_identifier.get_value();
        let selected_ident = format!("{}-{}", selected.team_key, selected.number);

        // Determine source, target, and relation_type for the API call
        let (source, target, api_type) = match rel_type.as_str() {
            "parent" => (selected_ident, current, "parent".to_string()),
            "child_of" => (current, selected_ident, "parent".to_string()),
            "blocks" => (current, selected_ident, "blocks".to_string()),
            "blocked_by" => (selected_ident, current, "blocks".to_string()),
            "duplicate" => (current, selected_ident, "duplicate".to_string()),
            "relates_to" => (current, selected_ident, "relates_to".to_string()),
            _ => return,
        };

        set_show_add_modal.set(false);
        leptos::task::spawn_local(async move {
            match add_relation(source, target, api_type).await {
                Ok(_) => {
                    let _ = set_version.try_update(|v| *v += 1);
                }
                Err(e) => {
                    tracing::warn!("Failed to add relation: {e}");
                }
            }
        });
    });

    // Handler for when a new sub-issue is created
    let on_sub_issue_created = Callback::new(move |()| {
        set_show_new_sub_issue.set(false);
        set_version.update(|v| *v += 1);
    });

    // Modal title depends on relation type
    let modal_title = Memo::new(move |_| {
        match add_relation_type.get().as_str() {
            "parent" => "Set parent issue".to_string(),
            "child_of" => "Add sub-issue".to_string(),
            "blocks" => "Add issue this blocks".to_string(),
            "blocked_by" => "Add issue that blocks this".to_string(),
            "duplicate" => "Add duplicate relation".to_string(),
            "relates_to" => "Add related issue".to_string(),
            _ => "Add relation".to_string(),
        }
    });

    // Sort relations into display order: Parent, Duplicate of, Blocked by,
    // Blocks, Has duplicate, Sub-issue.
    // Note: `direction` describes the CURRENT issue's role, but labels describe
    // the OTHER issue. "parent" means current IS the parent → other is "Sub-issue".
    fn direction_order(direction: &str) -> u8 {
        match direction {
            "child_of" => 0,
            "duplicate" => 1,
            "blocked_by" => 2,
            "blocks" => 3,
            "relates_to" => 4,
            "has_duplicate" => 5,
            "parent" => 6,
            _ => 7,
        }
    }

    fn direction_label(direction: &str) -> &'static str {
        match direction {
            "child_of" => "Parent",
            "duplicate" => "Duplicate of",
            "blocked_by" => "Blocked by",
            "blocks" => "Blocks",
            "has_duplicate" => "Has duplicate",
            "relates_to" => "Related to",
            "parent" => "Sub-issue",
            _ => "Related",
        }
    }

    view! {
        <div class="mt-6">
            <Suspense fallback=|| ()>
                {move || {
                    let mut relations = relations_resource.get()
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    relations.sort_by_key(|r| direction_order(&r.direction));
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
                            <div class="flex items-center gap-2">
                                <button
                                    class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                                    on:click=move |_| set_show_add_modal.set(true)
                                    title="Add relation"
                                >
                                    <Icon icon=phosphor_leptos::PLUS size="12px"/>
                                    "Add relation"
                                </button>
                                <button
                                    class="text-xs text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                                    on:click=move |_| set_show_new_sub_issue.set(true)
                                    title="Create new sub-issue"
                                >
                                    <Icon icon=phosphor_leptos::PLUS size="12px"/>
                                    "New sub-issue"
                                </button>
                            </div>
                        </div>

                        // ── Relation rows ─────────────────────────────────
                        {if total > 0 {
                            let rows = relations.into_iter().map(|rel| {
                                let rel_id = rel.relation_id.clone();
                                let rel_key = format!("{}-{}", rel.team_key, rel.number);
                                let rel_href = format!("/issues/{}-{}", rel.team_key, rel.number);
                                let rel_title = rel.title.clone();
                                let direction = rel.direction.clone();
                                let label = direction_label(&direction);
                                let status_variant = IssueStatusVariant::parse(&rel.status_category, &rel.status_name);

                                view! {
                                    <div class="group flex items-center gap-2 px-3 py-1.5 hover:bg-secondary/50 rounded-md transition-colors">
                                        // Type label
                                        <span class="text-xs text-muted-foreground font-medium shrink-0 w-[90px]">
                                            {label}
                                        </span>
                                        // Status icon
                                        <IssueStatusBadge status=status_variant size=12/>
                                        // Issue identifier
                                        <a href=rel_href.clone() class="text-xs text-muted-foreground font-mono shrink-0 hover:underline">
                                            {rel_key}
                                        </a>
                                        // Title link
                                        <a href=rel_href class="text-sm text-foreground hover:underline truncate flex-1">
                                            {rel_title}
                                        </a>
                                        // Remove button (appears on hover)
                                        <button
                                            class="opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground transition-all shrink-0 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                                            title="Remove relation"
                                            on:click=move |_| {
                                                let rid = rel_id.clone();
                                                leptos::task::spawn_local(async move {
                                                    match remove_relation(rid).await {
                                                        Ok(_) => { let _ = set_version.try_update(|v| *v += 1); },
                                                        Err(e) => tracing::warn!("Failed to remove relation: {e}"),
                                                    }
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
                            }.into_any()
                        } else {
                            view! {
                                <div class="text-sm text-muted-foreground py-2">
                                    "No relations"
                                </div>
                            }.into_any()
                        }}
                    }
                }}
            </Suspense>
        </div>

        // ── Add relation picker modal ────────────────────────────────────
        <Modal
            show=Signal::derive(move || show_add_modal.get())
            on_close=Callback::new(move |()| set_show_add_modal.set(false))
            title=modal_title
            size=ModalSize::Md
        >
            // Relation type selector
            <div class="mb-3">
                <Select
                    value=add_relation_type
                    options=Signal::derive(|| vec![
                        ("child_of".to_string(), "Sub-issue".to_string()),
                        ("parent".to_string(), "Parent".to_string()),
                        ("blocks".to_string(), "Blocks".to_string()),
                        ("blocked_by".to_string(), "Blocked by".to_string()),
                        ("duplicate".to_string(), "Duplicate of".to_string()),
                        ("relates_to".to_string(), "Related to".to_string()),
                    ])
                    on_change=Callback::new(move |val| set_add_relation_type.set(val))
                    variant=SelectVariant::Form
                />
            </div>
            // Inline issue picker (search + results)
            <AddRelationPicker
                on_select=on_relation_selected
                exclude_ids=Signal::derive({
                    let issue_id = stored_issue_id.get_value();
                    move || vec![issue_id.clone()]
                })
            />
        </Modal>

        // ── New Sub-Issue modal ──────────────────────────────────────────
        <NewIssueModal
            show=Signal::derive(move || show_new_sub_issue.get())
            on_close=Callback::new(move |()| set_show_new_sub_issue.set(false))
            on_created=on_sub_issue_created
            team_id=Signal::derive(move || Some(stored_team_id.get_value()))
            parent_issue_id=Signal::derive(move || Some(stored_issue_id.get_value()))
            parent_title=Signal::derive(move || {
                let tk = stored_team_key.get_value();
                Some(format!("{tk}-{number}"))
            })
        />
    }
}

/// Inline issue search + results list used inside the Add Relation modal.
///
/// Separated into its own component so the search Resource lives inside the
/// modal lifecycle and resets properly on each open.
#[component]
fn AddRelationPicker(
    on_select: Callback<IssueWithDetails>,
    exclude_ids: Signal<Vec<String>>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());

    let search_results = Resource::new(
        move || search.get(),
        move |query| async move {
            let q = if query.trim().is_empty() { None } else { Some(query) };
            list_issues(None, None, None, None, None, q, Some(20), None).await
        },
    );

    view! {
        <div class="mb-3">
            <SearchInput
                value=Signal::derive(move || search.get())
                on_input=Callback::new(move |val: String| set_search.set(val))
                placeholder="Search issues..."
            />
        </div>
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
                                            let status_variant = IssueStatusVariant::parse(&issue.status_category, &issue.status_name);
                                            let issue_key = format!("{}-{}", issue.team_key, issue.number);
                                            let issue_title = issue.title.clone();
                                            view! {
                                                <button
                                                    class="w-full text-left flex items-center gap-2 px-3 py-2 rounded-md hover:bg-secondary/50 transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                    on:click={
                                                        let issue_for_click = issue_for_click.clone();
                                                        move |_| {
                                                            on_select.run(issue_for_click.clone());
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
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Comments Section
// ─────────────────────────────────────────────────────────────────────────────
// Issue Timeline (activities + comments merged chronologically)
// ─────────────────────────────────────────────────────────────────────────────

/// A single entry in the timeline — either an activity or a comment.
#[derive(Clone)]
enum TimelineEntry {
    Activity(IssueActivity),
    Comment(Comment),
}

impl TimelineEntry {
    /// Sort key — normalized ISO timestamp for correct chronological ordering.
    /// Activity timestamps from Postgres use space separator; comments use T.
    fn sort_key(&self) -> String {
        match self {
            TimelineEntry::Activity(a) => a.created_at.replace(' ', "T"),
            TimelineEntry::Comment(c) => c.created_at.to_rfc3339(),
        }
    }
}

/// Filter mode for the timeline.
#[derive(Clone, Copy, PartialEq)]
enum TimelineFilter {
    All,
    CommentsOnly,
}

/// Activity timeline that merges activities and comments chronologically.
///
/// Activities are fetched reactively via `list_issue_activities`, using the
/// `activities_version` signal from SyncStore as a refetch trigger. Comments
/// come from the SyncStore (already reactive).
#[component]
fn IssueTimeline(
    team_key: String,
    number: i32,
    comments: Signal<Vec<Comment>>,
    lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>>,
    /// Issue ID for auto-linking inline uploads in the comment form.
    issue_id: String,
) -> impl IntoView {
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let (filter, set_filter) = signal(TimelineFilter::All);

    // Activities version from SyncStore — bumps on WebSocket activity events
    let activities_version = Signal::derive(move || {
        sync_store.map(|s| s.activities_version().get()).unwrap_or(0)
    });

    // Fetch activities reactively, re-fetching when version bumps
    let tk = team_key.clone();
    let activities_resource = Resource::new(
        move || (tk.clone(), number, activities_version.get()),
        move |(tk, num, _version)| async move {
            list_issue_activities(tk, num).await
        },
    );

    let tk_for_form = team_key.clone();
    let issue_id_for_comment_form = issue_id;

    view! {
        <div>
            // ── Header with filter toggle ─────────────────────────────
            <div class="flex items-center justify-between mb-4">
                <h2 class="text-sm font-medium text-foreground">
                    "Activity"
                </h2>
                <div class="flex items-center gap-1 bg-secondary/50 rounded-md p-0.5">
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if filter.get() == TimelineFilter::All {
                                ButtonVariant::PillActive
                            } else {
                                ButtonVariant::Pill
                            }
                        })
                        size=ButtonSize::Pill
                        on:click=move |_| set_filter.set(TimelineFilter::All)
                    >
                        "All"
                    </ToggleButton>
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if filter.get() == TimelineFilter::CommentsOnly {
                                ButtonVariant::PillActive
                            } else {
                                ButtonVariant::Pill
                            }
                        })
                        size=ButtonSize::Pill
                        on:click=move |_| set_filter.set(TimelineFilter::CommentsOnly)
                    >
                        "Comments only"
                    </ToggleButton>
                </div>
            </div>

            // ── Timeline entries ───────────────────────────────────────
            <div class="space-y-3">
                {move || {
                    let current_filter = filter.get();
                    let current_comments = comments.get();

                    // Build timeline entries
                    let mut entries: Vec<TimelineEntry> = Vec::new();

                    // Add top-level comments (exclude replies — they render nested)
                    let top_level_comments: Vec<Comment> = current_comments
                        .iter()
                        .filter(|c| c.parent_id.is_none())
                        .cloned()
                        .collect();
                    let replies: Vec<Comment> = current_comments
                        .iter()
                        .filter(|c| c.parent_id.is_some())
                        .cloned()
                        .collect();

                    for comment in &top_level_comments {
                        entries.push(TimelineEntry::Comment(comment.clone()));
                    }

                    // Add activities (skip comment_added — the comment itself appears)
                    if current_filter == TimelineFilter::All
                        && let Some(Ok(activities)) = activities_resource.get()
                    {
                        entries.extend(
                            activities.into_iter()
                                .filter(|a| a.action_type != "comment_added")
                                .map(TimelineEntry::Activity)
                        );
                    }

                    // Sort by timestamp
                    entries.sort_by_key(|e| e.sort_key());

                    // Render
                    entries.into_iter().map(|entry| {
                        match entry {
                            TimelineEntry::Comment(comment) => {
                                let comment_id = comment.comment_id.clone();
                                let comment_replies: Vec<Comment> = replies
                                    .iter()
                                    .filter(|r| r.parent_id.as_deref() == Some(&comment_id))
                                    .cloned()
                                    .collect();
                                view! {
                                    <div>
                                        <CommentItem comment=comment lightbox_state=lightbox_state/>
                                        // Threaded replies
                                        {if !comment_replies.is_empty() {
                                            Some(view! {
                                                <div class="ml-10 space-y-3 mt-3">
                                                    {comment_replies.into_iter().map(|reply| {
                                                        view! { <CommentItem comment=reply lightbox_state=lightbox_state/> }
                                                    }).collect_view()}
                                                </div>
                                            })
                                        } else {
                                            None
                                        }}
                                    </div>
                                }.into_any()
                            }
                            TimelineEntry::Activity(activity) => {
                                let name = activity.actor_name.clone()
                                    .unwrap_or_else(|| "Someone".to_string());
                                view! {
                                    <ActivityEntry
                                        activity=activity
                                        actor_name=name
                                    />
                                }.into_any()
                            }
                        }
                    }).collect_view()
                }}
            </div>

            // ── New comment form ───────────────────────────────────────
            <NewCommentForm team_key=tk_for_form number=number lightbox_state=lightbox_state issue_id=issue_id_for_comment_form/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Activity Entry (compact single-line with icon)
// ─────────────────────────────────────────────────────────────────────────────

/// Render a single activity entry as a compact line:
/// `[icon] Actor name  action description  · relative time`
#[component]
fn ActivityEntry(
    activity: IssueActivity,
    actor_name: String,
) -> impl IntoView {
    let description = format_activity_description(&activity);
    let timestamp = relative_time(&activity.created_at);
    let icon = activity_icon(&activity.action_type);
    let via_suffix = crate::components::attribution::render_via_suffix(
        activity.action_source,
        activity.action_source_label.clone(),
    );

    view! {
        <div class="flex items-center gap-2 py-1 text-xs text-muted-foreground">
            <span class="shrink-0 w-5 h-5 flex items-center justify-center">
                {icon}
            </span>
            <span class="font-medium text-foreground/80">{actor_name}{via_suffix}</span>
            <span>{description}</span>
            <span class="shrink-0">{format!("\u{b7} {timestamp}")}</span>
        </div>
    }
}

/// Map activity action_type to a phosphor icon view.
fn activity_icon(action_type: &str) -> leptos::prelude::AnyView {
    match action_type {
        "created" => view! { <Icon icon=phosphor_leptos::PLUS_CIRCLE size="14px"/> }.into_any(),
        "status_changed" => view! { <Icon icon=phosphor_leptos::CIRCLE_DASHED size="14px"/> }.into_any(),
        "priority_changed" => view! { <Icon icon=phosphor_leptos::CELL_SIGNAL_FULL size="14px"/> }.into_any(),
        "assignee_changed" => view! { <Icon icon=phosphor_leptos::USER size="14px"/> }.into_any(),
        "title_changed" | "description_changed" => view! { <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px"/> }.into_any(),
        "label_added" | "label_removed" => view! { <Icon icon=phosphor_leptos::TAG size="14px"/> }.into_any(),
        "relation_added" => view! { <Icon icon=phosphor_leptos::LINK size="14px"/> }.into_any(),
        "relation_removed" => view! { <Icon icon=phosphor_leptos::LINK_BREAK size="14px"/> }.into_any(),
        "project_changed" => view! { <Icon icon=phosphor_leptos::BRIEFCASE size="14px"/> }.into_any(),
        "milestone_changed" => view! { <Icon icon=phosphor_leptos::FLAG size="14px"/> }.into_any(),
        "due_date_changed" => view! { <Icon icon=phosphor_leptos::CALENDAR_BLANK size="14px"/> }.into_any(),
        "parent_changed" => view! { <Icon icon=phosphor_leptos::TREE_STRUCTURE size="14px"/> }.into_any(),
        "moved_to_team" => view! { <Icon icon=phosphor_leptos::ARROWS_LEFT_RIGHT size="14px"/> }.into_any(),
        "estimate_changed" => view! { <Icon icon=phosphor_leptos::GAUGE size="14px"/> }.into_any(),
        _ => view! { <Icon icon=phosphor_leptos::CLOCK_COUNTER_CLOCKWISE size="14px"/> }.into_any(),
    }
}

/// Format a human-readable description from an activity's action_type and values.
///
/// Returns an `AnyView` so that issue identifiers in the text can be rendered
/// as clickable links (via [`crate::utils::auto_link::auto_link_view`]).
/// Relation activities parse metadata JSON to extract identifiers directly.
fn format_activity_description(activity: &IssueActivity) -> leptos::prelude::AnyView {
    use crate::utils::auto_link::auto_link_view;

    match activity.action_type.as_str() {
        "relation_added" => {
            if let Some(ref meta_str) = activity.metadata {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_str) {
                    let identifier = meta.get("related_identifier").and_then(|v| v.as_str());
                    let rel_type = meta.get("relation_type").and_then(|v| v.as_str());
                    let direction = meta.get("direction").and_then(|v| v.as_str());
                    if let (Some(identifier), Some(rel_type), Some(direction)) =
                        (identifier, rel_type, direction)
                    {
                        let label = match (rel_type, direction) {
                            ("blocks", "outward") => "blocks",
                            ("blocks", "inward") => "blocked by",
                            ("parent", "outward") => "sub-issue of",
                            ("parent", "inward") => "parent of",
                            ("duplicate", "outward") => "duplicate of",
                            ("duplicate", "inward") => "has duplicate",
                            _ => "related to",
                        };
                        let href = format!("/issues/{identifier}");
                        let id_owned = identifier.to_string();
                        return view! {
                            <span>
                                {format!("added {label} relation to ")}
                                <a href=href class="text-accent-foreground hover:underline font-medium transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm">{id_owned}</a>
                            </span>
                        }
                        .into_any();
                    }
                } else {
                    tracing::warn!("Failed to parse relation_added metadata: {meta_str}");
                }
            }
            view! { <span>"added a relation"</span> }.into_any()
        }
        "relation_removed" => {
            if let Some(ref meta_str) = activity.metadata {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_str) {
                    let identifier = meta.get("related_identifier").and_then(|v| v.as_str());
                    let rel_type = meta.get("relation_type").and_then(|v| v.as_str());
                    let direction = meta.get("direction").and_then(|v| v.as_str());
                    if let (Some(identifier), Some(rel_type), Some(direction)) =
                        (identifier, rel_type, direction)
                    {
                        let label = match (rel_type, direction) {
                            ("blocks", "outward") => "blocks",
                            ("blocks", "inward") => "blocked by",
                            ("parent", "outward") => "sub-issue of",
                            ("parent", "inward") => "parent of",
                            ("duplicate", "outward") => "duplicate of",
                            ("duplicate", "inward") => "has duplicate",
                            _ => "related to",
                        };
                        let href = format!("/issues/{identifier}");
                        let id_owned = identifier.to_string();
                        return view! {
                            <span>
                                {format!("removed {label} relation to ")}
                                <a href=href class="text-accent-foreground hover:underline font-medium transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm">{id_owned}</a>
                            </span>
                        }
                        .into_any();
                    }
                } else {
                    tracing::warn!("Failed to parse relation_removed metadata: {meta_str}");
                }
            }
            view! { <span>"removed a relation"</span> }.into_any()
        }
        _ => {
            let text = format_activity_text(activity);
            auto_link_view(&text)
        }
    }
}

/// Build the plain-text description for non-relation activity types.
fn format_activity_text(activity: &IssueActivity) -> String {
    match activity.action_type.as_str() {
        "created" => "created this issue".to_string(),
        "status_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("changed status from {old} to {new}"),
            (None, Some(new)) => format!("set status to {new}"),
            _ => "changed status".to_string(),
        },
        "priority_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("changed priority from {old} to {new}"),
            (None, Some(new)) => format!("set priority to {new}"),
            _ => "changed priority".to_string(),
        },
        "assignee_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("reassigned from {old} to {new}"),
            (None, Some(new)) => format!("assigned to {new}"),
            (Some(old), None) => format!("unassigned {old}"),
            _ => "changed assignee".to_string(),
        },
        "title_changed" => "changed the title".to_string(),
        "description_changed" => "updated the description".to_string(),
        "label_added" => match &activity.new_value {
            Some(label) => format!("added label {label}"),
            None => "added a label".to_string(),
        },
        "label_removed" => match &activity.old_value {
            Some(label) => format!("removed label {label}"),
            None => "removed a label".to_string(),
        },
        "project_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("moved from project {old} to {new}"),
            (None, Some(new)) => format!("added to project {new}"),
            (Some(old), None) => format!("removed from project {old}"),
            _ => "changed project".to_string(),
        },
        "milestone_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("changed milestone from {old} to {new}"),
            (None, Some(new)) => format!("set milestone to {new}"),
            (Some(old), None) => format!("removed milestone {old}"),
            _ => "changed milestone".to_string(),
        },
        "due_date_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(_old), Some(new)) => format!("changed due date to {new}"),
            (None, Some(new)) => format!("set due date to {new}"),
            (Some(_old), None) => "removed due date".to_string(),
            _ => "changed due date".to_string(),
        },
        "parent_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(_), Some(new)) => format!("changed parent to {new}"),
            (None, Some(new)) => format!("set parent to {new}"),
            (Some(_), None) => "removed parent".to_string(),
            _ => "changed parent".to_string(),
        },
        "moved_to_team" => match &activity.new_value {
            Some(team) => format!("moved to team {team}"),
            None => "moved to another team".to_string(),
        },
        "estimate_changed" => match (&activity.old_value, &activity.new_value) {
            (Some(old), Some(new)) => format!("changed estimate from {old} to {new}"),
            (None, Some(new)) => format!("set estimate to {new}"),
            (Some(_), None) => "removed estimate".to_string(),
            _ => "changed estimate".to_string(),
        },
        other => format!("performed action: {other}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Comment Item
// ─────────────────────────────────────────────────────────────────────────────

/// A single comment with avatar, author name, timestamp, and body.
#[component]
fn CommentItem(
    comment: Comment,
    lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>>,
) -> impl IntoView {
    let author = comment
        .author_name
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let timestamp = format_datetime(&comment.created_at);
    let via_suffix = crate::components::attribution::render_via_suffix(
        comment.action_source,
        comment.action_source_label.clone(),
    );

    let on_click = crate::components::attachment_hooks::make_click_callback(lightbox_state);

    view! {
        <div class="flex gap-3">
            <Avatar name=author.clone() size=AvatarSize::Md/>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm font-medium text-foreground">{author}{via_suffix}</span>
                    <span class="text-xs text-muted-foreground">{timestamp}</span>
                </div>
                <div class="mt-1 text-sm text-foreground">
                    <kode_leptos::TreeWysiwygEditor
                        content=Signal::stored(crate::utils::auto_link::auto_link_issue_identifiers(&comment.body))
                        show_fixed_toolbar=false
                        readonly=true
                        on_click_attachment=on_click
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
fn NewCommentForm(
    team_key: String,
    number: i32,
    lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>>,
    /// Issue ID for auto-linking inline uploads to this issue.
    issue_id: String,
) -> impl IntoView {
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
                    // Guard: component may have been destroyed while the future was in flight.
                    let _ = content.try_set(String::new());
                    let _ = set_submitting.try_set(false);
                    // Comment will appear reactively via the SyncStore pipeline
                    // (server broadcasts sync action -> WS -> sync engine -> store update).
                }
                Err(e) => {
                    tracing::warn!("Failed to create comment: {e}");
                    let _ = set_submitting.try_set(false);
                }
            }
        });
    };

    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        content.set(text);
    });

    // Attachment callbacks — pass issue_id so uploads are auto-linked.
    let error_toast = crate::components::toast::capture_error_toast();
    let upload_complete: RwSignal<Option<kode_leptos::UploadComplete>> = RwSignal::new(None);
    let on_upload = crate::components::attachment_hooks::make_upload_callback(upload_complete, Some(issue_id.clone()), error_toast.clone());
    let on_delete = crate::components::attachment_hooks::make_delete_callback(error_toast.clone());
    let on_click = crate::components::attachment_hooks::make_click_callback(lightbox_state);

    // ── "Attach file" slash command extension ───────────────────────────
    let attach_file_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let (attach_triggered, set_attach_triggered) = signal(false);
    let on_change_for_attach = on_change.clone();

    let attach_ext = Arc::new(
        crate::components::attachment_extension::AttachFileExtension::new(
            Arc::new(move || set_attach_triggered.set(true)),
        ),
    );

    // When the extension fires, click the hidden file input.
    Effect::new(move || {
        if attach_triggered.get() {
            set_attach_triggered.set(false);
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(input_el) = attach_file_input_ref.get() {
                    let input: &web_sys::HtmlInputElement = input_el.as_ref();
                    input.click();
                }
            }
        }
    });

    // When a file is selected via the extension's file picker, read, validate,
    // upload, and inject the resulting markdown into the editor content.
    let stored_issue_id = StoredValue::new(issue_id);
    let error_toast_for_attach = error_toast.clone();
    let on_attach_file_selected = move |_ev: leptos::ev::Event| {
        let _stored_issue_id = stored_issue_id;
        let _on_change_for_attach = on_change_for_attach.clone();
        let _error_toast = error_toast_for_attach.clone();

        #[cfg(target_arch = "wasm32")]
        {
            use crate::components::attachment_hooks::{
                upload_file, ALLOWED_CONTENT_TYPES, ALLOWED_EXTENSIONS, MAX_FILE_SIZE,
            };

            let input_el = match attach_file_input_ref.get() {
                Some(el) => el,
                None => return,
            };
            let input: &web_sys::HtmlInputElement = input_el.as_ref();
            let files = match input.files() {
                Some(f) => f,
                None => return,
            };
            let file = match files.get(0) {
                Some(f) => f,
                None => return,
            };

            let name = file.name();
            let content_type = file.type_();
            let size = file.size() as u64;

            if size > MAX_FILE_SIZE {
                tracing::warn!("Attach file: too large ({size} bytes)");
                _error_toast("File exceeds 10 MB size limit".to_string());
                input.set_value("");
                return;
            }

            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if !ALLOWED_EXTENSIONS.contains(&ext.as_str())
                && !ALLOWED_CONTENT_TYPES.contains(&content_type.as_str())
            {
                tracing::warn!("Attach file: type not allowed (ext={ext}, content_type={content_type})");
                _error_toast("File type not allowed".to_string());
                input.set_value("");
                return;
            }

            let issue_id = _stored_issue_id.get_value();
            let error_toast_spawn = _error_toast.clone();

            leptos::task::spawn_local(async move {
                let data = {
                    use wasm_bindgen_futures::JsFuture;
                    let promise = file.array_buffer();
                    match JsFuture::from(promise).await {
                        Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                        Err(_) => {
                            tracing::warn!("Attach file: failed to read file bytes");
                            error_toast_spawn("Failed to read file".to_string());
                            return;
                        }
                    }
                };

                match upload_file(&data, &name, &content_type, Some(&issue_id)).await {
                    Ok(resp) => {
                        // Build inline markdown for the uploaded attachment.
                        let markdown_snippet = if content_type.starts_with("image/") {
                            format!("\n![{}]({})\n", name, resp.url)
                        } else {
                            format!("\n[{}]({})\n", name, resp.url)
                        };
                        // Append the snippet to the current comment content.
                        let Some(current) = content.try_get_untracked() else {
                            return;
                        };
                        let updated = format!("{}{}", current.trim_end(), markdown_snippet);
                        let _ = content.try_set(updated.clone());
                        _on_change_for_attach(updated);
                    }
                    Err(e) => {
                        tracing::warn!("Attach file: upload failed: {e}");
                        error_toast_spawn(format!("Upload failed: {e}"));
                    }
                }
                // Reset the file input so the same file can be re-selected.
                if let Some(input_el) = attach_file_input_ref.get() {
                    let input: &web_sys::HtmlInputElement = input_el.as_ref();
                    input.set_value("");
                }
            });
        }
    };

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
            // Hidden file input for the "Attach file" slash command extension
            <input
                type="file"
                node_ref=attach_file_input_ref
                class="hidden"
                accept=".png,.jpg,.jpeg,.gif,.webp,.svg,.pdf,.csv,.txt,.json,.log"
                on:change=on_attach_file_selected
            />
            <div class="border border-border rounded-md overflow-hidden bg-card">
                <TreeWysiwygEditor
                    content=content.read_only()
                    on_change=on_change
                    show_fixed_toolbar=false
                    show_floating_toolbar=true
                    theme=theme_signal
                    on_upload=on_upload
                    on_delete_attachment=on_delete
                    on_click_attachment=on_click
                    upload_complete=upload_complete
                    extensions=vec![attach_ext as Arc<dyn kode_leptos::Extension>]
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
// Attachments Section
// ─────────────────────────────────────────────────────────────────────────────

/// Format a byte count as a human-readable string (e.g. "1.2 KB", "3.5 MB").
fn format_bytes(bytes: i64) -> String {
    let bytes = bytes as f64;
    if bytes < 1024.0 {
        format!("{} B", bytes as i64)
    } else if bytes < 1_048_576.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{:.1} MB", bytes / 1_048_576.0)
    }
}

/// Return the appropriate Phosphor icon for a given content type.
fn attachment_icon(content_type: &str) -> phosphor_leptos::IconData {
    if content_type.starts_with("image/") {
        phosphor_leptos::IMAGE
    } else if content_type == "application/pdf" {
        phosphor_leptos::FILE_PDF
    } else if content_type.starts_with("text/")
        || content_type == "application/json"
    {
        phosphor_leptos::FILE_TEXT
    } else {
        phosphor_leptos::FILE
    }
}

/// Display linked attachments for an issue with upload and detach support.
///
/// Always renders (even when empty) — shows the header with an "+ Attach"
/// button and lists attachment rows with thumbnails, filenames, sizes, and
/// hover-reveal detach buttons.
#[component]
fn AttachmentsSection(
    team_key: String,
    number: i32,
    /// The current issue's ID (used for upload auto-linking).
    issue_id: String,
    lightbox_state: RwSignal<Option<crate::components::attachment_hooks::LightboxState>>,
) -> impl IntoView {
    let tk = team_key.clone();
    let tk_for_detach = team_key.clone();
    let (version, set_version) = signal(0u32);

    let attachments_resource = Resource::new(
        move || (tk.clone(), number, version.get()),
        move |(tk, num, _)| async move { list_issue_attachments(tk, num).await },
    );

    // ── Upload via hidden file input ────────────────────────────────────
    let file_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let (uploading, set_uploading) = signal(false);
    let error_toast = crate::components::toast::capture_error_toast();
    let stored_issue_id = StoredValue::new(issue_id);

    // Upload handler: reads the selected file from the input, uploads via
    // fetch to `/api/v1/attachments?issue_id=...`, then bumps version.
    let error_toast_for_upload = error_toast.clone();
    let on_file_selected = move |_ev: leptos::ev::Event| {
        let _set_uploading = set_uploading;
        let _stored_issue_id = stored_issue_id;
        let _error_toast = error_toast_for_upload.clone();

        #[cfg(target_arch = "wasm32")]
        {
            use crate::components::attachment_hooks::{
                upload_file, ALLOWED_CONTENT_TYPES, ALLOWED_EXTENSIONS, MAX_FILE_SIZE,
            };

            let input_el = match file_input_ref.get() {
                Some(el) => el,
                None => return,
            };
            let input: &web_sys::HtmlInputElement = input_el.as_ref();
            let files = match input.files() {
                Some(f) => f,
                None => return,
            };
            let file = match files.get(0) {
                Some(f) => f,
                None => return,
            };

            let name = file.name();
            let content_type = file.type_();
            let size = file.size() as u64;

            if size > MAX_FILE_SIZE {
                tracing::warn!("File too large: {size} bytes");
                _error_toast("File exceeds 10 MB size limit".to_string());
                input.set_value("");
                return;
            }

            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if !ALLOWED_EXTENSIONS.contains(&ext.as_str())
                && !ALLOWED_CONTENT_TYPES.contains(&content_type.as_str())
            {
                tracing::warn!("File type not allowed: ext={ext}, content_type={content_type}");
                _error_toast("File type not allowed".to_string());
                input.set_value("");
                return;
            }

            let issue_id = _stored_issue_id.get_value();
            _set_uploading.set(true);
            let error_toast_spawn = _error_toast.clone();

            leptos::task::spawn_local(async move {
                let data = {
                    use wasm_bindgen_futures::JsFuture;
                    let promise = file.array_buffer();
                    match JsFuture::from(promise).await {
                        Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                        Err(_) => {
                            tracing::warn!("Failed to read file");
                            error_toast_spawn("Failed to read file".to_string());
                            _set_uploading.set(false);
                            return;
                        }
                    }
                };
                match upload_file(&data, &name, &content_type, Some(&issue_id)).await {
                    Ok(_) => {
                        let _ = set_version.try_update(|v| *v += 1);
                    }
                    Err(e) => {
                        tracing::warn!("Attachment upload failed: {e}");
                        error_toast_spawn(format!("Upload failed: {e}"));
                    }
                }
                _set_uploading.set(false);
                if let Some(input_el) = file_input_ref.get() {
                    let input: &web_sys::HtmlInputElement = input_el.as_ref();
                    input.set_value("");
                }
            });
        }
    };

    view! {
        <div class="mt-6">
            // ── Hidden file input ──────────────────────────────────────
            <input
                type="file"
                node_ref=file_input_ref
                class="hidden"
                accept=".png,.jpg,.jpeg,.gif,.webp,.svg,.pdf,.csv,.txt,.json,.log"
                on:change=on_file_selected
            />

            <Suspense fallback=|| ()>
                {move || {
                    let attachments = match attachments_resource.get() {
                        Some(Ok(v)) => v,
                        Some(Err(e)) => { tracing::warn!("Failed to load attachments: {e}"); vec![] }
                        None => vec![],
                    };
                    let total = attachments.len();

                    view! {
                        // ── Header ─────────────────────────────────────────
                        <div class="flex items-center justify-between mb-2">
                            <h2 class="text-xs text-muted-foreground font-medium">
                                {if total > 0 {
                                    format!("Attachments ({})", total)
                                } else {
                                    "Attachments".to_string()
                                }}
                            </h2>
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || uploading.get())
                                on:click=move |_| {
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        if let Some(input_el) = file_input_ref.get() {
                                            let input: &web_sys::HtmlInputElement = input_el.as_ref();
                                            input.click();
                                        }
                                    }
                                }
                            >
                                <Icon icon=phosphor_leptos::PLUS size="12px"/>
                                {move || if uploading.get() { "Uploading..." } else { "Attach" }}
                            </Button>
                        </div>

                        // ── Attachment rows ────────────────────────────────
                        {if total > 0 {
                            let tk_for_rows = tk_for_detach.clone();
                            let error_toast_for_rows = error_toast.clone();
                            let rows = attachments.into_iter().map(move |att| {
                                let att_id_for_detach = att.attachment_id.clone();
                                let download_url = format!("/api/v1/attachments/{}/download", att.attachment_id);
                                let download_url_click = download_url.clone();
                                let is_image = att.content_type.starts_with("image/");
                                let icon_data = attachment_icon(&att.content_type);
                                let size_str = format_bytes(att.size_bytes);
                                let filename = att.filename.clone();
                                let tk_row = tk_for_rows.clone();
                                let error_toast_row = error_toast_for_rows.clone();

                                view! {
                                    <div
                                        class="group flex items-center gap-2 px-3 py-1.5 hover:bg-secondary/50 rounded-md transition-colors cursor-pointer"
                                        on:click={
                                            let url = download_url_click.clone();
                                            move |_| {
                                                if is_image {
                                                    lightbox_state.set(Some(crate::components::attachment_hooks::LightboxState {
                                                        src: url.clone(),
                                                    }));
                                                } else {
                                                    #[cfg(target_arch = "wasm32")]
                                                    {
                                                        if let Some(window) = web_sys::window()
                                                            && let Err(e) = window.open_with_url_and_target_and_features(&url, "_blank", "noopener,noreferrer")
                                                        {
                                                            tracing::warn!("Failed to open attachment in new tab: {e:?}");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    >
                                        // Thumbnail or icon
                                        {if is_image {
                                            let thumb_url = download_url.clone();
                                            view! {
                                                <img
                                                    src=thumb_url
                                                    alt=att.filename.clone()
                                                    class="w-8 h-8 rounded object-cover shrink-0"
                                                    loading="lazy"
                                                />
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="w-8 h-8 flex items-center justify-center text-muted-foreground shrink-0">
                                                    <Icon icon=icon_data size="16px"/>
                                                </span>
                                            }.into_any()
                                        }}
                                        // Filename
                                        <span class="text-sm text-foreground truncate flex-1 min-w-0">
                                            {filename}
                                        </span>
                                        // File size
                                        <span class="text-xs text-muted-foreground font-mono shrink-0">
                                            {size_str}
                                        </span>
                                        <Button
                                            variant=ButtonVariant::GhostMuted
                                            size=ButtonSize::IconSm
                                            class="opacity-0 group-hover:opacity-100 shrink-0"
                                            aria_label="Detach file"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                let aid = att_id_for_detach.clone();
                                                let tk = tk_row.clone();
                                                let error_toast_detach = error_toast_row.clone();
                                                leptos::task::spawn_local(async move {
                                                    match detach_attachment_from_issue(tk, number, aid).await {
                                                        Ok(_) => { let _ = set_version.try_update(|v| *v += 1); },
                                                        Err(e) => {
                                                            tracing::warn!("Failed to detach attachment: {e}");
                                                            error_toast_detach(format!("Failed to remove attachment: {e}"));
                                                        }
                                                    }
                                                });
                                            }
                                        >
                                            <Icon icon=phosphor_leptos::X size="12px"/>
                                        </Button>
                                    </div>
                                }
                            }).collect_view();
                            view! {
                                <div class="space-y-0.5">
                                    {rows}
                                </div>
                            }.into_any()
                        } else {
                            ().into_any()
                        }}
                    }
                }}
            </Suspense>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GitHub Activity Section
// ─────────────────────────────────────────────────────────────────────────────

/// Display linked GitHub activity (PRs, branches, commits) for an issue.
///
/// Only renders when there are GitHub links. Groups by type: PRs first,
/// then branches, then commits. Shows first 3 if > 5 total, with an
/// expander to show the rest.
#[component]
fn GitHubActivitySection(
    team_key: String,
    number: i32,
) -> impl IntoView {
    let tk = team_key.clone();
    let links_resource = Resource::new(
        move || (tk.clone(), number),
        move |(tk, num)| async move { list_github_links_for_issue(tk, num).await },
    );

    let (expanded, set_expanded) = signal(false);

    view! {
        <Suspense fallback=|| ()>
            {move || {
                let links = match links_resource.get() {
                    Some(Ok(l)) => l,
                    Some(Err(e)) => {
                        tracing::warn!("Failed to fetch GitHub links: {e}");
                        return None;
                    }
                    None => return None,
                };

                if links.is_empty() {
                    return None;
                }

                // Group by type: PRs first, then branches, then commits
                let mut prs: Vec<GitHubLinkDisplay> = Vec::new();
                let mut branches: Vec<GitHubLinkDisplay> = Vec::new();
                let mut commits: Vec<GitHubLinkDisplay> = Vec::new();

                for link in links {
                    match link.link_type.as_str() {
                        "pull_request" => prs.push(link),
                        "branch" => branches.push(link),
                        "commit" => commits.push(link),
                        other => {
                            tracing::warn!(link_type = %other, "unknown GitHub link type");
                        }
                    }
                }

                let mut ordered: Vec<GitHubLinkDisplay> = Vec::new();
                ordered.extend(prs);
                ordered.extend(branches);
                ordered.extend(commits);

                let total = ordered.len();
                let is_expanded = expanded.get();
                let show_expander = total > 5;
                let visible_count = if show_expander && !is_expanded { 3 } else { total };
                let hidden_count = total.saturating_sub(visible_count);

                let visible_links = ordered.into_iter().take(visible_count).collect::<Vec<_>>();

                let rows = visible_links.into_iter().map(|link| {
                    render_github_link(link)
                }).collect_view();

                Some(view! {
                    <div class="mt-6">
                        <h2 class="text-xs text-muted-foreground font-medium uppercase tracking-wider mb-2">
                            "Development"
                        </h2>
                        <div class="space-y-0.5">
                            {rows}
                        </div>
                        {if show_expander && !is_expanded {
                            Some(view! {
                                <Button
                                    variant=ButtonVariant::GhostMuted
                                    size=ButtonSize::Sm
                                    class="mt-1.5"
                                    on:click=move |_| set_expanded.set(true)
                                >
                                    {format!("Show {} more", hidden_count)}
                                </Button>
                            })
                        } else {
                            None
                        }}
                    </div>
                })
            }}
        </Suspense>
    }
}

/// Map PR state to a design-token CSS class.
fn pr_state_color(state: &str) -> &'static str {
    match state {
        "open" => "text-success-foreground",
        "merged" => "text-purple-600",
        "closed" => "text-error-foreground",
        _ => "text-muted-foreground",
    }
}

/// Render a single GitHub link row based on its type.
fn render_github_link(link: GitHubLinkDisplay) -> impl IntoView {
    match link.link_type.as_str() {
        "pull_request" => render_pr_link(link).into_any(),
        "branch" => render_branch_link(link).into_any(),
        "commit" => render_commit_link(link).into_any(),
        _ => view! { <div></div> }.into_any(),
    }
}

/// Render a pull request link row.
fn render_pr_link(link: GitHubLinkDisplay) -> impl IntoView {
    let icon_class = link.state.as_deref().map(pr_state_color).unwrap_or("text-muted-foreground");

    let state_label = link.state.clone();
    let display_text = if let Some(ref title) = link.title {
        format!("#{} {}", link.ref_identifier, title)
    } else {
        format!("#{}", link.ref_identifier)
    };
    let author_text = link.author_login.as_deref().map(|a| format!("@{a}"));
    let time_text = relative_time(&link.created_at);
    let close_intent = link.close_intent;
    let url = link.url.clone();
    let repo = link.repo_full_name.clone();

    view! {
        <div class="flex items-start gap-2 py-1.5">
            <span class=format!("mt-0.5 shrink-0 {icon_class}")>
                <Icon icon=phosphor_leptos::GIT_PULL_REQUEST size="14px"/>
            </span>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <a
                        href=url
                        target="_blank"
                        rel="noopener noreferrer"
                        class="text-sm text-foreground hover:text-accent transition-colors truncate"
                    >
                        {display_text}
                    </a>
                    {if let Some(ref state) = state_label {
                        let badge_class = pr_state_color(state);
                        Some(view! {
                            <span class=format!("text-xs shrink-0 {badge_class}")>
                                {state.clone()}
                            </span>
                        })
                    } else {
                        None
                    }}
                </div>
                <div class="flex items-center gap-1 text-xs text-muted-foreground">
                    <span>{repo}</span>
                    {author_text.map(|a| view! { <><span>{"\u{2022}"}</span><span>{a}</span></> })}
                    <span>{"\u{2022}"}</span>
                    <span>{time_text}</span>
                </div>
                {if close_intent {
                    Some(view! {
                        <span class="text-xs text-muted-foreground italic">"Closes this issue"</span>
                    })
                } else {
                    None
                }}
            </div>
        </div>
    }
}

/// Render a branch link row.
fn render_branch_link(link: GitHubLinkDisplay) -> impl IntoView {
    let url = link.url.clone();
    let ref_id = link.ref_identifier.clone();
    let repo = link.repo_full_name.clone();

    view! {
        <div class="flex items-start gap-2 py-1.5">
            <span class="mt-0.5 shrink-0 text-muted-foreground">
                <Icon icon=phosphor_leptos::GIT_BRANCH size="14px"/>
            </span>
            <div class="flex-1 min-w-0">
                <a
                    href=url
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-sm text-foreground hover:text-accent transition-colors truncate block"
                >
                    {ref_id}
                </a>
                <span class="text-xs text-muted-foreground">{repo}</span>
            </div>
        </div>
    }
}

/// Render a commit link row.
fn render_commit_link(link: GitHubLinkDisplay) -> impl IntoView {
    let url = link.url.clone();
    let short_sha = if link.ref_identifier.len() > 7 {
        link.ref_identifier[..7].to_string()
    } else {
        link.ref_identifier.clone()
    };
    let display_text = if let Some(ref title) = link.title {
        format!("{short_sha} {title}")
    } else {
        short_sha.clone()
    };
    let author_text = link.author_login.as_deref().map(|a| format!("@{a}"));
    let time_text = relative_time(&link.created_at);
    let repo = link.repo_full_name.clone();

    view! {
        <div class="flex items-start gap-2 py-1.5">
            <span class="mt-0.5 shrink-0 text-muted-foreground">
                <Icon icon=phosphor_leptos::GIT_COMMIT size="14px"/>
            </span>
            <div class="flex-1 min-w-0">
                <a
                    href=url
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-sm text-foreground hover:text-accent transition-colors truncate block"
                >
                    {display_text}
                </a>
                <div class="flex items-center gap-1 text-xs text-muted-foreground">
                    <span>{repo}</span>
                    {author_text.map(|a| view! { <><span>{"\u{2022}"}</span><span>{a}</span></> })}
                    <span>{"\u{2022}"}</span>
                    <span>{time_text}</span>
                </div>
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
