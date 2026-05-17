// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared issue row component used by both `issue_list` and `my_issues`.
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer
//! Order: Priority | Status | Issue ID (with team key) | Title | Labels | Date | Assignee

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::location::State;
use leptos_router::NavigateOptions;

use phosphor_leptos::Icon;

use crate::components::{
    Avatar, IssueStatusBadge, IssueStatusVariant, LabelBadge, PriorityIndicator,
};
use crate::types::IssueNavState;
use crate::utils::date::format_short_date;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// Issue Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue row in a list.
///
/// DESIGN.md Issue Row Pattern:
/// ```text
/// [priority] [status] ENG-42  Fix login redirect loop  [bug] [auth]  May 8  @j
/// ```
///
/// Row height: 36px (h-9), padding: px-3 py-[6px], gap: gap-2.5
///
/// Supports keyboard navigation highlighting: when `selected_index` matches
/// `index`, the row renders with a distinct selected background.
#[component]
pub fn IssueRow(
    issue: IssueWithDetails,
    /// This row's index in the list.
    index: usize,
    /// The currently keyboard-selected index (None = no selection).
    #[prop(into)]
    selected_index: Signal<Option<usize>>,
    /// Whether this issue is archived (completed/cancelled older than threshold).
    /// When true, the row renders with reduced opacity.
    #[prop(optional, default = false)]
    archived: bool,
) -> impl IntoView {
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let issue_href = format!("/issues/{issue_key}");
    let issue_href_click = issue_href.clone();
    let status = IssueStatusVariant::parse(&issue.status_category);
    let row_ref = NodeRef::<leptos::html::A>::new();
    let location = use_location();

    // ── Estimate badge: look up team settings from SyncStore ─────────
    let estimate_label = {
        let sync_store = use_context::<crate::cache::store::SyncStore>();
        let team_id = issue.team_id.clone();
        let estimate = issue.estimate;
        estimate.and_then(|val| {
            // Resolve team settings to get the scale for formatting
            let scale = sync_store
                .and_then(|store| {
                    store.teams().get_untracked().into_iter()
                        .find(|t| t.team_id == team_id)
                        .and_then(|t| t.settings)
                        .and_then(|s| s.estimate_scale)
                })?;
            Some(scale.format_label(val))
        })
    };

    let is_selected = Memo::new(move |_| selected_index.get() == Some(index));

    // Scroll the selected row into view when keyboard-navigated.
    Effect::new(move || {
        if is_selected.get() && let Some(el) = row_ref.get() {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });

    let row_class = move || {
        let base = if is_selected.get() {
            "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border bg-primary/5 ring-1 ring-primary/20 focus-visible:outline-none transition-colors cursor-pointer no-underline text-inherit"
        } else {
            "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
        };
        if archived {
            format!("{base} opacity-50")
        } else {
            base.to_string()
        }
    };

    view! {
        <a
            node_ref=row_ref
            href=issue_href
            class=row_class
            role="listitem"
            tabindex="0"
            on:click={
                let issue_href = issue_href_click.clone();
                move |ev: web_sys::MouseEvent| {
                    // Let modifier clicks (ctrl, meta, shift, middle-click) use default browser behavior
                    if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.alt_key() || ev.button() != 0 {
                        return;
                    }
                    ev.prevent_default();
                    let path = location.pathname.get_untracked();
                    let search = location.search.get_untracked();
                    let nav_state = IssueNavState::from_current_path(&path, &search);
                    let json = nav_state.to_json();
                    let nav = use_navigate();
                    nav(&issue_href, NavigateOptions {
                        state: State::from(wasm_bindgen::JsValue::from_str(&json)),
                        ..Default::default()
                    });
                }
            }
        >
            // Priority icon (first — most important for triage scanning)
            <PriorityIndicator priority=issue.priority/>

            // Status icon
            <IssueStatusBadge status=status/>

            // Issue ID with team key (Geist Mono)
            <span class="font-mono text-xs text-muted-foreground shrink-0">
                {issue_key}
            </span>

            // Title + parent reference
            <span class="flex-1 min-w-0 flex items-center gap-1">
                <span class="text-sm font-medium text-foreground truncate">
                    {issue.title.clone()}
                </span>
                {issue.parent_identifier.as_ref().map(|parent_id| {
                    view! {
                        <span class="text-xs text-muted-foreground shrink-0">
                            {format!("\u{2190} {parent_id}")}
                        </span>
                    }
                })}
            </span>

            // Labels
            <div class="hidden sm:flex items-center gap-1 shrink-0">
                {issue.labels.iter().map(|label| {
                    view! {
                        <LabelBadge
                            name=label.name.clone()
                            color=label.color.clone()
                        />
                    }
                }).collect_view()}
            </div>

            // Estimate badge (only when team has estimates enabled and issue has an estimate)
            {estimate_label.map(|label| {
                view! {
                    <span class="hidden sm:inline-flex items-center gap-0.5 text-xs text-muted-foreground shrink-0" title="Estimate">
                        <Icon icon=phosphor_leptos::GAUGE size="12px" weight=phosphor_leptos::IconWeight::Bold/>
                        <span class="font-mono">{label}</span>
                    </span>
                }
            })}

            // Date (Geist Mono)
            <span class="font-mono text-xs text-muted-foreground shrink-0 hidden sm:inline">
                {format_short_date(&issue.created_at)}
            </span>

            // Assignee avatar (18px)
            {if issue.assignee_name.is_some() {
                view! {
                    <Avatar name=issue.assignee_name.clone().unwrap_or_default()/>
                }.into_any()
            } else {
                // Empty placeholder to keep alignment
                view! {
                    <span class="w-[18px] h-[18px] shrink-0"></span>
                }.into_any()
            }}
        </a>
    }
}
