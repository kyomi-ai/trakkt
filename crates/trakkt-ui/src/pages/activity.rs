// SPDX-License-Identifier: AGPL-3.0-or-later

//! Activity feed — chronological workspace-wide activity stream.
//!
//! Shows all issue activities across all teams, grouped by time period
//! (Today, Yesterday, This Week, Older). Supports filtering by team,
//! activity type, action source, and actor, with "Load more" pagination.

use std::collections::HashSet;

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::cache::store::SyncStore;
use crate::components::{Button, ButtonVariant, Select, SelectVariant, Spinner};
use crate::server_fns::activities::list_workspace_activities;
use crate::server_fns::team::list_workspace_members;
use crate::server_fns::teams::list_teams;
use crate::utils::github::github_author_login_from_metadata;
use crate::utils::relative_time::relative_time;
use crate::utils::time_group::{classify_time_group, TimeGroup};
use trakkt_types::models::WorkspaceActivity;

// ─── Activity helpers ────────────────────────────────────────────────────────

/// Map action_type to a phosphor icon view.
///
/// Matches the icons used in `issue_detail.rs` for visual consistency,
/// with the addition of `comment_added` (which issue detail filters out).
fn activity_icon(action_type: &str) -> AnyView {
    match action_type {
        "created" => view! { <Icon icon=phosphor_leptos::PLUS_CIRCLE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "status_changed" => view! { <Icon icon=phosphor_leptos::CIRCLE_DASHED size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "comment_added" => view! { <Icon icon=phosphor_leptos::CHAT_TEXT size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "assignee_changed" => view! { <Icon icon=phosphor_leptos::USER size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "priority_changed" => view! { <Icon icon=phosphor_leptos::CELL_SIGNAL_FULL size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "label_added" | "label_removed" => view! { <Icon icon=phosphor_leptos::TAG size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "title_changed" | "description_changed" => view! { <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "relation_added" => view! { <Icon icon=phosphor_leptos::LINK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "relation_removed" => view! { <Icon icon=phosphor_leptos::LINK_BREAK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "project_changed" => view! { <Icon icon=phosphor_leptos::BRIEFCASE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "milestone_changed" => view! { <Icon icon=phosphor_leptos::FLAG size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "due_date_changed" => view! { <Icon icon=phosphor_leptos::CALENDAR_BLANK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "parent_changed" => view! { <Icon icon=phosphor_leptos::TREE_STRUCTURE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "moved_to_team" => view! { <Icon icon=phosphor_leptos::ARROWS_LEFT_RIGHT size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "estimate_changed" => view! { <Icon icon=phosphor_leptos::GAUGE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "commit_pushed" => view! { <Icon icon=phosphor_leptos::GIT_COMMIT size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "pr_opened" | "pr_closed" => view! { <Icon icon=phosphor_leptos::GIT_PULL_REQUEST size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "pr_merged" => view! { <Icon icon=phosphor_leptos::GIT_MERGE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "branch_created" => view! { <Icon icon=phosphor_leptos::GIT_BRANCH size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        _ => view! { <Icon icon=phosphor_leptos::LIGHTNING size="16px" attr:class="text-muted-foreground"/> }.into_any(),
    }
}

/// Human-readable description for an activity entry.
fn activity_description(a: &WorkspaceActivity) -> String {
    let actor = a.actor_name.as_deref().unwrap_or("Someone");
    match a.action_type.as_str() {
        "created" => format!("{actor} created this issue"),
        "status_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (Some(old), Some(new)) => format!("{actor} changed status from {old} to {new}"),
                (_, Some(new)) => format!("{actor} set status to {new}"),
                _ => format!("{actor} changed status"),
            }
        }
        "comment_added" => format!("{actor} commented"),
        "assignee_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (_, Some(new)) => format!("{actor} assigned to {new}"),
                (Some(old), None) => format!("{actor} unassigned {old}"),
                _ => format!("{actor} changed assignee"),
            }
        }
        "priority_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (Some(old), Some(new)) => format!("{actor} changed priority from {old} to {new}"),
                _ => format!("{actor} changed priority"),
            }
        }
        "label_added" => {
            match a.new_value.as_deref() {
                Some(label) => format!("{actor} added label {label}"),
                None => format!("{actor} added a label"),
            }
        }
        "label_removed" => {
            match a.old_value.as_deref() {
                Some(label) => format!("{actor} removed label {label}"),
                None => format!("{actor} removed a label"),
            }
        }
        "title_changed" => format!("{actor} changed the title"),
        "description_changed" => format!("{actor} updated the description"),
        "estimate_changed" => format!("{actor} changed the estimate"),
        "due_date_changed" => format!("{actor} changed the due date"),
        "project_changed" => format!("{actor} moved to a different project"),
        "milestone_changed" => format!("{actor} changed the milestone"),
        "parent_changed" => format!("{actor} changed the parent issue"),
        "commit_pushed" => github_activity_description(a, "pushed a commit"),
        "pr_opened" => github_activity_description(a, "opened a pull request"),
        "pr_merged" => github_activity_description(a, "merged a pull request"),
        "pr_closed" => github_activity_description(a, "closed a pull request"),
        "branch_created" => github_activity_description(a, "created a branch"),
        _ => format!("{actor} updated the issue"),
    }
}

/// Description for a GitHub-sourced activity in the workspace feed.
///
/// Prefers the resolved Trakkt user name, falling back to the `author_login`
/// from metadata (`@login`), then `"Someone"`.
fn github_activity_description(a: &WorkspaceActivity, verb: &str) -> String {
    let actor = match a.actor_name.as_deref() {
        Some(name) => name.to_string(),
        None => github_author_login_from_metadata(a.metadata.as_deref())
            .unwrap_or_else(|| "Someone".to_string()),
    };
    format!("{actor} {verb}")
}

const PAGE_SIZE: i64 = 50;

// ─── Fetching, filtering and merging ─────────────────────────────────────────

/// What a completed fetch does to the rows the page is already holding.
///
/// The three modes exist because the page reads the same server function for
/// three different reasons and each one owes the loaded list something
/// different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchMode {
    /// Page one replaces everything held: the first load, and every filter
    /// change. Nothing on screen belongs to the new query.
    Replace,
    /// The next page is appended after the rows held — the "Load more" button.
    Append,
    /// Page one is re-read and merged into the rows held, so the pages the user
    /// paged back through survive. What a live activity frame does.
    MergePageOne,
}

/// The four filter selections the page's dropdowns hold.
///
/// An empty string is the dropdown's "All …" option, which the server is asked
/// as "no filter on this column" — see [`ActivityQuery::build`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ActivityFilters {
    team_key: String,
    action_type: String,
    actor_id: String,
    action_source: String,
}

/// The arguments one `list_workspace_activities` call is made with.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ActivityQuery {
    team_key: Option<String>,
    action_type: Option<String>,
    actor_id: Option<String>,
    action_source: Option<String>,
    offset: i64,
}

impl ActivityQuery {
    /// Build the request a fetch in `mode` makes against the filters currently
    /// selected.
    ///
    /// # What a live frame does about the filters
    ///
    /// Every mode carries the same four filters, including
    /// [`FetchMode::MergePageOne`], and that is the answer to what happens when
    /// a live frame arrives for an activity the user's filters exclude: the
    /// refetch it triggers is a filtered query, so a non-matching activity is
    /// simply not in the response and never reaches the feed. Nothing about the
    /// frame itself is inspected — the client is not told which activity moved,
    /// only that some activity did (`crates/trakkt-ui/src/cache/apply.rs`
    /// discards the payload and bumps a counter), and it could not evaluate the
    /// filters against it if it were: the frame carries no `team_key` and the
    /// counter carries nothing at all.
    ///
    /// That is also why the filters are not re-implemented here as a predicate
    /// over rows. The server owns the filter semantics
    /// (`activity_service::list_workspace_activities`), and a second copy on the
    /// client would be a copy that can disagree with it.
    fn build(mode: FetchMode, filters: &ActivityFilters, loaded_len: usize) -> Self {
        // The dropdowns' "All …" option is the empty string; the server takes
        // `None` for "do not filter on this column".
        let selected = |value: &str| {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        };

        Self {
            team_key: selected(&filters.team_key),
            action_type: selected(&filters.action_type),
            actor_id: selected(&filters.actor_id),
            action_source: selected(&filters.action_source),
            offset: match mode {
                FetchMode::Append => loaded_len as i64,
                // Both of these read the newest page. `Replace` because it is
                // starting over, `MergePageOne` because the newest page is
                // where a live change lands: an insert is the newest row, and
                // `coalesce_or_insert_activity`
                // (`crates/trakkt-auth/src/activity_service.rs`) moves the row
                // it coalesces onto to `NOW()`, which moves it to the top too.
                FetchMode::Replace | FetchMode::MergePageOne => 0,
            },
        }
    }
}

/// The state a completed fetch leaves the page in.
struct FetchOutcome {
    /// The rows to show.
    activities: Vec<WorkspaceActivity>,
    /// What this fetch has to say about older rows remaining on the server, or
    /// `None` when it has nothing to say and the previous answer stands.
    has_more: Option<bool>,
}

/// Fold a page of freshly fetched rows into the rows already held.
fn apply_fetched_page(
    mode: FetchMode,
    loaded: Vec<WorkspaceActivity>,
    fetched: Vec<WorkspaceActivity>,
) -> FetchOutcome {
    // A full page means the server had at least as many rows as we asked for,
    // so there is very likely another page behind it. This is the pre-existing
    // rule and it is sound for the two modes that read a page boundary the user
    // is actually sitting on.
    let full_page = fetched.len() as i64 == PAGE_SIZE;

    match mode {
        FetchMode::Replace => FetchOutcome {
            activities: fetched,
            has_more: Some(full_page),
        },
        FetchMode::Append => {
            let mut activities = loaded;
            activities.extend(fetched);
            FetchOutcome {
                activities,
                has_more: Some(full_page),
            }
        }
        FetchMode::MergePageOne => FetchOutcome {
            activities: merge_page_one(loaded, fetched),
            // Deliberately no answer. "Load more" asks whether anything older
            // than the oldest row held is still on the server, which is a
            // property of the tail — and a merge reads page one, which says
            // nothing about the tail. It is also a question the merge cannot
            // change the answer to: it removes no row, so the oldest row held
            // is the same row it was, and every row it adds is newer than that.
            //
            // Answering it from this fetch is the specific way this goes wrong:
            // a user who has paged to the end of a 120-row feed has `has_more`
            // false, and a full page one would flip it back to true and offer a
            // "Load more" that fetches nothing.
            has_more: None,
        },
    }
}

/// Merge a freshly read page one into the rows already loaded.
///
/// The page keeps every row it has loaded and takes page one's copy of any row
/// it already holds. Three things fall out of that, in the order they matter:
///
/// - **The loaded pages survive.** This is the whole point: a live frame used to
///   re-read page one and `set` it over the top, so anyone who had paged back
///   through history lost every page below the newest on the next thing anybody
///   in the workspace did.
/// - **Dedup is by `activity_id` across the whole loaded list, not just page
///   one.** `coalesce_or_insert_activity` in
///   `crates/trakkt-auth/src/activity_service.rs` refreshes an existing row's
///   `created_at` inside a 60s window instead of inserting, which moves that row
///   to the top of the feed — so a row the client is holding on page three can
///   reappear in page one. Deduping only within page one would leave the stale
///   copy sitting further down the list, visibly duplicated.
/// - **Page one's copy wins.** It is the newer read of the same row: the
///   coalesced `created_at`, and the current issue title, which the row carries
///   denormalised.
///
/// Rows are ordered by the `created_at` string, descending — the same column and
/// direction as the server's `ORDER BY a.created_at DESC`, so the merged list
/// comes out in the order the server would have returned the union in. On SQLite
/// that is literally the same comparison, the column being TEXT. On Postgres the
/// column is a `TIMESTAMPTZ` and what arrives here is its `CAST(… AS TEXT)`
/// rendering, which is fixed-width and most-significant-first
/// (`2026-08-09 11:50:26.863802+00`), so it orders the same way — including at
/// the one boundary that is not obvious, a whole second against the same second
/// carrying a fraction, where `+` sorts ahead of `.`. There is a test for that
/// case below.
///
/// The sort is stable, so rows sharing a timestamp keep page one's ordering
/// ahead of the tail's rather than shuffling between merges. Sorting rather than
/// concatenating keeps the ordering a property of this function instead of one
/// inherited from the two arguments happening to be disjoint and pre-sorted.
///
/// What this does not do is drop a held row that page one no longer contains,
/// which would be the way to notice a deletion. No server path emits a `Delete`
/// for an activity — `entity_types::ACTIVITY` is written only by
/// `insert_activity` and `coalesce_or_insert_activity`, as an `Insert` and an
/// `Update` — so there is no deletion to notice, and inferring one from absence
/// would instead delete the far-side row of any `created_at` tie that the
/// server's `LIMIT` happened to cut between.
fn merge_page_one(
    loaded: Vec<WorkspaceActivity>,
    page_one: Vec<WorkspaceActivity>,
) -> Vec<WorkspaceActivity> {
    let refetched: HashSet<&str> = page_one
        .iter()
        .map(|activity| activity.activity_id.as_str())
        .collect();

    let retained: Vec<WorkspaceActivity> = loaded
        .into_iter()
        .filter(|activity| !refetched.contains(activity.activity_id.as_str()))
        .collect();

    let mut merged = page_one;
    merged.extend(retained);
    merged.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    merged
}

// ─── Grouped activities ──────────────────────────────────────────────────────

struct GroupedActivities {
    today: Vec<WorkspaceActivity>,
    yesterday: Vec<WorkspaceActivity>,
    this_week: Vec<WorkspaceActivity>,
    older: Vec<WorkspaceActivity>,
}

fn group_activities(activities: &[WorkspaceActivity]) -> GroupedActivities {
    let mut today = Vec::new();
    let mut yesterday = Vec::new();
    let mut this_week = Vec::new();
    let mut older = Vec::new();

    for a in activities {
        match classify_time_group(&a.created_at) {
            TimeGroup::Today => today.push(a.clone()),
            TimeGroup::Yesterday => yesterday.push(a.clone()),
            TimeGroup::ThisWeek => this_week.push(a.clone()),
            TimeGroup::Older => older.push(a.clone()),
        }
    }

    GroupedActivities {
        today,
        yesterday,
        this_week,
        older,
    }
}

// ─── Live activity frames ────────────────────────────────────────────────────

/// Re-run `refetch` whenever a live ACTIVITY sync frame bumps the store's
/// activity counter.
///
/// This page reads its rows through the `list_workspace_activities` server
/// function, never from the sync store, so the counter is the only reactive
/// dependency that can tell it another user, another tab or an agent recorded
/// something. Without it the feed shows what it read on mount until it is
/// navigated away from and back.
///
/// # The counter is resolved here, never inside the effect
///
/// [`SyncStore::activities_version`] builds a fresh `Signal` wrapper on every
/// call, and each wrapper is an owner-registered arena item. Calling it from a
/// closure that re-runs allocates another one per run, under whichever owner
/// happens to be current at the time — and reading one after its owner is
/// disposed panics. That is the shape TRA-9977 was reverted for, so the getter
/// is called once, before the effect exists. The rule is recorded on this
/// function rather than only at its call site, so it survives an edit made
/// here in isolation.
///
/// # Why `store` is optional
///
/// `use_context::<SyncStore>()` returns an `Option`. `Layout`
/// (`crates/trakkt-ui/src/components/layout.rs:38`) provides the store and
/// wraps every authenticated route, `/activity` among them, but that is a
/// property of the route table rather than of the type, and every other reader
/// in this crate — `NotificationsPage`, `IssueDetailContent`,
/// `AttachmentsSection` — treats it as optional. With no store there is no
/// counter to subscribe to and nothing to wire; the page still loads and
/// paginates through its own server calls.
fn refetch_on_live_activity(store: Option<SyncStore>, refetch: impl Fn() + Send + Sync + 'static) {
    let Some(version) = store.map(|store| store.activities_version()) else {
        return;
    };

    Effect::new(move |previous: Option<u32>| {
        let current = version.get();
        // Skip the first fire: the initial-load effect has already asked for
        // page one, and the same shape is what the filter effect below uses.
        if previous.is_some_and(|previous| previous != current) {
            refetch();
        }
        current
    });
}

// ─── Page component ──────────────────────────────────────────────────────────

#[component]
pub fn ActivityPage() -> impl IntoView {
    // Filter state
    let (team_filter, set_team_filter) = signal(String::new());
    let (type_filter, set_type_filter) = signal(String::new());
    let (source_filter, set_source_filter) = signal(String::new());
    let (actor_filter, set_actor_filter) = signal(String::new());

    // Pagination state
    let loaded_activities: RwSignal<Vec<WorkspaceActivity>> = RwSignal::new(Vec::new());
    let has_more = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let initial_loaded = RwSignal::new(false);

    // Load teams for the team filter dropdown
    let teams_resource = Resource::new(|| (), |_| async move { list_teams().await });

    let team_options = Signal::derive(move || {
        let mut opts = vec![("".to_string(), "All teams".to_string())];
        if let Some(Ok(ref teams)) = teams_resource.get() {
            for team in teams {
                opts.push((team.key.clone(), team.name.clone()));
            }
        }
        opts
    });

    let type_options = Signal::derive(|| {
        vec![
            ("".to_string(), "All types".to_string()),
            ("created".to_string(), "Created".to_string()),
            ("status_changed".to_string(), "Status changes".to_string()),
            ("assignee_changed".to_string(), "Assignments".to_string()),
            ("priority_changed".to_string(), "Priority".to_string()),
            ("comment_added".to_string(), "Comments".to_string()),
            ("label_added".to_string(), "Labels added".to_string()),
            ("label_removed".to_string(), "Labels removed".to_string()),
            ("title_changed".to_string(), "Title changes".to_string()),
            ("description_changed".to_string(), "Description changes".to_string()),
            ("estimate_changed".to_string(), "Estimate changes".to_string()),
            ("due_date_changed".to_string(), "Due date changes".to_string()),
            ("project_changed".to_string(), "Project changes".to_string()),
            ("milestone_changed".to_string(), "Milestone changes".to_string()),
            ("parent_changed".to_string(), "Parent changes".to_string()),
            ("relation_added".to_string(), "Relations added".to_string()),
            ("relation_removed".to_string(), "Relations removed".to_string()),
            ("moved_to_team".to_string(), "Moved to team".to_string()),
            ("commit_pushed".to_string(), "Commits".to_string()),
            ("pr_opened".to_string(), "PRs opened".to_string()),
            ("pr_merged".to_string(), "PRs merged".to_string()),
            ("pr_closed".to_string(), "PRs closed".to_string()),
            ("branch_created".to_string(), "Branches created".to_string()),
        ]
    });

    let source_options = Signal::derive(|| {
        vec![
            ("".to_string(), "All sources".to_string()),
            ("user".to_string(), "User".to_string()),
            ("agent".to_string(), "Agent".to_string()),
            ("api".to_string(), "API".to_string()),
            ("github".to_string(), "GitHub".to_string()),
        ]
    });

    // Load workspace members for the actor filter dropdown
    let members_resource = Resource::new(|| (), |_| async move { list_workspace_members().await });

    let actor_options = Signal::derive(move || {
        let mut opts = vec![("".to_string(), "All members".to_string())];
        if let Some(Ok(ref members)) = members_resource.get() {
            for m in members {
                let display_name = m.name.clone().unwrap_or_else(|| m.email.clone());
                opts.push((m.user_id.clone(), display_name));
            }
        }
        opts
    });

    // Fetch function — loads a page of activities and updates state
    let fetch_activities = move |mode: FetchMode| {
        if loading.get_untracked() {
            return;
        }
        loading.set(true);

        let filters = ActivityFilters {
            team_key: team_filter.get_untracked(),
            action_type: type_filter.get_untracked(),
            actor_id: actor_filter.get_untracked(),
            action_source: source_filter.get_untracked(),
        };
        let query = ActivityQuery::build(mode, &filters, loaded_activities.with_untracked(|held| held.len()));

        leptos::task::spawn_local(async move {
            match list_workspace_activities(query.team_key, query.action_type, query.actor_id, query.action_source, None, None, Some(PAGE_SIZE), Some(query.offset)).await {
                Ok(fetched) => {
                    // The held rows are taken out of the signal and the result
                    // put back, rather than cloned out and set over the top: a
                    // merge rebuilds the list from both sides anyway, and that
                    // list is every page the user has loaded.
                    let mut has_more_answer = None;
                    loaded_activities.update(|held| {
                        let outcome = apply_fetched_page(mode, std::mem::take(held), fetched);
                        *held = outcome.activities;
                        has_more_answer = outcome.has_more;
                    });
                    // A fetch with nothing to say about the tail leaves the
                    // standing answer alone — see `apply_fetched_page`.
                    if let Some(answer) = has_more_answer {
                        has_more.set(answer);
                    }
                    initial_loaded.set(true);
                }
                Err(e) => {
                    tracing::warn!("Failed to load activities: {e}");
                    initial_loaded.set(true);
                }
            }
            loading.set(false);
        });
    };

    // Initial load
    let fetch_initial = fetch_activities;
    Effect::new(move |_| {
        fetch_initial(FetchMode::Replace);
    });

    // Re-fetch on filter change (track all four signals)
    let fetch_on_filter = fetch_activities;
    Effect::new(move |prev: Option<ActivityFilters>| {
        let current = ActivityFilters {
            team_key: team_filter.get(),
            action_type: type_filter.get(),
            actor_id: actor_filter.get(),
            action_source: source_filter.get(),
        };

        // Skip the first fire — the initial load Effect handles that
        if prev.is_some_and(|prev_val| prev_val != current) {
            fetch_on_filter(FetchMode::Replace);
        }

        current
    });

    // Re-fetch on a live activity frame from another client.
    //
    // Page one only, merged into what is already loaded rather than replacing
    // it — a filter change starts over because the rows on screen belong to a
    // query the user has left, but a live frame means one more row exists under
    // the query they are still reading, and everything they paged back through
    // with "Load more" is still theirs. Page one is where a live change lands,
    // whether it inserted a row or coalesced onto one; `merge_page_one` is where
    // the rest of that reasoning lives.
    let fetch_on_live_activity = fetch_activities;
    refetch_on_live_activity(use_context::<SyncStore>(), move || {
        fetch_on_live_activity(FetchMode::MergePageOne);
    });

    let fetch_more = fetch_activities;

    view! {
        <div class="h-full flex flex-col">
            // Page header
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Activity"</h1>
            </div>

            // Filter bar
            <div class="flex items-center gap-2 px-5 py-3 border-b border-border">
                <Select
                    value=Signal::derive(move || team_filter.get())
                    options=team_options
                    on_change=Callback::new(move |v: String| set_team_filter.set(v))
                    variant=SelectVariant::Compact
                />
                <Select
                    value=Signal::derive(move || type_filter.get())
                    options=type_options
                    on_change=Callback::new(move |v: String| set_type_filter.set(v))
                    variant=SelectVariant::Compact
                />
                <Select
                    value=Signal::derive(move || source_filter.get())
                    options=source_options
                    on_change=Callback::new(move |v: String| set_source_filter.set(v))
                    variant=SelectVariant::Compact
                />
                <Select
                    value=Signal::derive(move || actor_filter.get())
                    options=actor_options
                    on_change=Callback::new(move |v: String| set_actor_filter.set(v))
                    variant=SelectVariant::Compact
                />
            </div>

            // Content area
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if !initial_loaded.get() {
                        // Initial loading state
                        return view! {
                            <div class="flex items-center justify-center py-12">
                                <Spinner/>
                            </div>
                        }.into_any();
                    }

                    let activities = loaded_activities.get();

                    if activities.is_empty() {
                        // Empty state
                        return view! {
                            <div class="flex flex-col items-center justify-center py-16 text-muted-foreground">
                                <Icon icon=phosphor_leptos::CLOCK_COUNTER_CLOCKWISE weight=IconWeight::Light size="48px" attr:class="mb-4 text-muted-foreground/50"/>
                                <p class="text-lg font-medium">"No activity yet"</p>
                            </div>
                        }.into_any();
                    }

                    let grouped = group_activities(&activities);
                    let groups: Vec<(TimeGroup, Vec<WorkspaceActivity>)> = vec![
                        (TimeGroup::Today, grouped.today),
                        (TimeGroup::Yesterday, grouped.yesterday),
                        (TimeGroup::ThisWeek, grouped.this_week),
                        (TimeGroup::Older, grouped.older),
                    ];

                    let show_load_more = has_more.get();
                    let is_loading = loading.get();

                    view! {
                        <div class="divide-y divide-border">
                            {groups.into_iter().filter(|(_, items)| !items.is_empty()).map(|(group, items)| {
                                view! {
                                    <div>
                                        <div class="px-6 py-2">
                                            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">
                                                {group.label()}
                                            </span>
                                        </div>
                                        {items.into_iter().map(|activity| {
                                            view! { <ActivityRow activity=activity/> }
                                        }).collect_view()}
                                    </div>
                                }
                            }).collect_view()}
                        </div>

                        // Load more button
                        {show_load_more.then(|| {
                            let fetch = fetch_more;
                            view! {
                                <div class="flex justify-center py-4">
                                    <Button
                                        variant=ButtonVariant::GhostMuted
                                        on:click=move |_| fetch(FetchMode::Append)
                                    >
                                        {if is_loading { "Loading..." } else { "Load more" }}
                                    </Button>
                                </div>
                            }
                        })}
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─── Activity row ────────────────────────────────────────────────────────────

#[component]
fn ActivityRow(activity: WorkspaceActivity) -> impl IntoView {
    let icon = activity_icon(&activity.action_type);
    let description = activity_description(&activity);
    let via_suffix = crate::components::attribution::render_via_suffix(
        activity.action_source,
        activity.action_source_label.clone(),
    );
    let identifier = format!("{}-{}", activity.team_key, activity.issue_number);
    let href = format!("/issues/{}-{}", activity.team_key, activity.issue_number);
    let issue_title = activity.issue_title.clone();
    let timestamp = relative_time(&activity.created_at);

    view! {
        <a href=href class="flex items-start gap-3 px-6 py-2.5 hover:bg-accent transition-colors">
            <div class="flex-shrink-0 pt-0.5">
                {icon}
            </div>
            <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-2">
                    <span class="text-sm text-foreground">{description}{via_suffix}</span>
                    <span class="text-xs text-muted-foreground font-mono">{identifier}</span>
                </div>
                <p class="text-sm text-muted-foreground truncate mt-0.5">{issue_title}</p>
            </div>
            <span class="flex-shrink-0 text-xs text-muted-foreground pt-0.5">
                {timestamp}
            </span>
        </a>
    }
}

// ─── Merge tests ─────────────────────────────────────────────────────────────

/// What a live activity frame does to the loaded feed, tested where it is
/// decided: [`apply_fetched_page`] and [`ActivityQuery::build`] are the whole of
/// it, and both are ordinary functions of their arguments. Everything around
/// them — reading the four filter signals, awaiting the server function, writing
/// the two signals back — is in `ActivityPage` and needs a browser and a server.
///
/// Host-side rather than `wasm_bindgen_test`, so these run under
/// `cargo test --workspace` and do not add a browser launch to the wasm sweep.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod merge_tests {
    use super::*;
    use trakkt_types::enums::ActionSource;

    /// An activity carrying the two things these tests turn on — which row it is
    /// and where it sits in time — with plausible filler for the rest.
    ///
    /// Timestamps are written in the form Postgres renders `TIMESTAMPTZ` in,
    /// because that is what production sends and what the ordering claim on
    /// [`merge_page_one`] is about.
    fn row(activity_id: &str, created_at: &str) -> WorkspaceActivity {
        WorkspaceActivity {
            activity_id: activity_id.to_string(),
            issue_id: format!("issue-for-{activity_id}"),
            workspace_id: "ws-1".to_string(),
            actor_id: Some("user-1".to_string()),
            actor_name: Some("Ada".to_string()),
            action_type: "status_changed".to_string(),
            field: Some("status".to_string()),
            old_value: Some("Backlog".to_string()),
            new_value: Some("In Progress".to_string()),
            metadata: None,
            action_source: ActionSource::User,
            action_source_label: None,
            created_at: created_at.to_string(),
            team_key: "TRA".to_string(),
            issue_number: 42,
            issue_title: "Something happened".to_string(),
        }
    }

    fn ids(activities: &[WorkspaceActivity]) -> Vec<&str> {
        activities
            .iter()
            .map(|activity| activity.activity_id.as_str())
            .collect()
    }

    /// The rows a user who has pressed "Load more" twice is holding, newest
    /// first.
    fn six_loaded_rows() -> Vec<WorkspaceActivity> {
        vec![
            row("a6", "2026-08-09 11:06:00+00"),
            row("a5", "2026-08-09 11:05:00+00"),
            row("a4", "2026-08-09 11:04:00+00"),
            row("a3", "2026-08-09 11:03:00+00"),
            row("a2", "2026-08-09 11:02:00+00"),
            row("a1", "2026-08-09 11:01:00+00"),
        ]
    }

    #[test]
    fn a_live_frame_keeps_the_pages_the_user_paged_back_through() {
        // Somebody else records an activity while this user is reading history.
        // The page re-reads page one — which now leads with the new row — and
        // merges it.
        let page_one = vec![
            row("a7", "2026-08-09 11:07:00+00"),
            row("a6", "2026-08-09 11:06:00+00"),
            row("a5", "2026-08-09 11:05:00+00"),
        ];

        let outcome = apply_fetched_page(FetchMode::MergePageOne, six_loaded_rows(), page_one);

        assert_eq!(
            ids(&outcome.activities),
            ["a7", "a6", "a5", "a4", "a3", "a2", "a1"],
            "a live frame must add the new row and leave every page the user \
             loaded with \"Load more\" where it was — replacing the list with \
             page one is the regression this test exists for, and it is silent: \
             the feed simply snaps back to the newest page and the reader loses \
             their place"
        );
    }

    #[test]
    fn a_coalesced_row_moves_in_place_instead_of_appearing_twice() {
        // `coalesce_or_insert_activity` refreshes an existing row's `created_at`
        // instead of inserting when the same actor edits the same field again
        // within 60s. The row it refreshes can be one the client is holding well
        // below page one — here `a2`, three pages down — and it comes back at
        // the top of page one under the same `activity_id`.
        let mut refreshed = row("a2", "2026-08-09 11:07:00+00");
        refreshed.new_value = Some("the second edit".to_string());

        let page_one = vec![
            refreshed,
            row("a6", "2026-08-09 11:06:00+00"),
            row("a5", "2026-08-09 11:05:00+00"),
        ];

        let outcome = apply_fetched_page(FetchMode::MergePageOne, six_loaded_rows(), page_one);

        assert_eq!(
            ids(&outcome.activities),
            ["a2", "a6", "a5", "a4", "a3", "a1"],
            "the coalesced row is one row that moved, not a new one: it must \
             appear exactly once, at the position its refreshed `created_at` \
             puts it. Deduplicating only against page one leaves the stale copy \
             sitting in the tail and the user sees the same entry twice"
        );

        let held = outcome
            .activities
            .first()
            .expect("the merged feed to still hold the coalesced row");
        assert_eq!(
            held.created_at, "2026-08-09 11:07:00+00",
            "page one's copy of a row the client already held is the newer read \
             of it, and on this path it is the entire point of the frame — the \
             coalescing branch's whole effect is moving `created_at` forward"
        );
        assert_eq!(
            held.new_value.as_deref(),
            Some("the second edit"),
            "and the rest of page one's copy comes with it, rather than the \
             fresh timestamp being grafted onto the stale row"
        );
    }

    #[test]
    fn an_activity_excluded_by_the_active_filters_never_reaches_the_feed() {
        // The user is filtered to one team and has paged back through it.
        // Someone in another team then changes something, which bumps the same
        // workspace-wide counter and triggers the same refetch — but the
        // refetch is filtered (see the test below), so the response is page one
        // *of this filter*, and the other team's activity is simply not in it.
        let loaded = vec![
            row("t3", "2026-08-09 11:03:00+00"),
            row("t2", "2026-08-09 11:02:00+00"),
            row("t1", "2026-08-09 11:01:00+00"),
        ];
        let page_one = vec![
            row("t3", "2026-08-09 11:03:00+00"),
            row("t2", "2026-08-09 11:02:00+00"),
        ];

        let outcome = apply_fetched_page(FetchMode::MergePageOne, loaded, page_one);

        assert_eq!(
            ids(&outcome.activities),
            ["t3", "t2", "t1"],
            "the feed shows what the filtered query returned and what it had \
             already loaded under the same filters, and nothing else — a live \
             frame can never introduce a row the server did not send, which is \
             what keeps it from bypassing a filter the user set"
        );
    }

    #[test]
    fn a_live_refetch_asks_the_server_with_the_filters_the_user_has_set() {
        let filters = ActivityFilters {
            team_key: "TRA".to_string(),
            action_type: "status_changed".to_string(),
            actor_id: "user-1".to_string(),
            action_source: "agent".to_string(),
        };

        let query = ActivityQuery::build(FetchMode::MergePageOne, &filters, 120);

        assert_eq!(
            query,
            ActivityQuery {
                team_key: Some("TRA".to_string()),
                action_type: Some("status_changed".to_string()),
                actor_id: Some("user-1".to_string()),
                action_source: Some("agent".to_string()),
                offset: 0,
            },
            "the refetch a live frame triggers carries the filters the user is \
             looking through, which is what makes the server the only thing \
             deciding whether a live activity belongs in this feed. Dropping \
             them here would fetch the unfiltered newest page and merge it in, \
             putting rows the user filtered out on screen"
        );
    }

    #[test]
    fn an_unset_dropdown_is_no_filter_at_all() {
        let query = ActivityQuery::build(FetchMode::Replace, &ActivityFilters::default(), 0);

        assert_eq!(
            query,
            ActivityQuery {
                team_key: None,
                action_type: None,
                actor_id: None,
                action_source: None,
                offset: 0,
            },
            "each dropdown's \"All …\" option is the empty string, and the \
             server takes `None` for an inactive filter — sending the empty \
             string would match no team, no action type and no actor"
        );
    }

    #[test]
    fn load_more_asks_for_the_rows_after_the_ones_already_held() {
        let filters = ActivityFilters::default();

        assert_eq!(
            ActivityQuery::build(FetchMode::Append, &filters, 120).offset,
            120,
            "\"Load more\" continues from the end of what is loaded"
        );
        assert_eq!(
            ActivityQuery::build(FetchMode::Replace, &filters, 120).offset,
            0,
            "a filter change starts over from the newest page"
        );
    }

    #[test]
    fn merging_page_one_leaves_has_more_where_the_tail_left_it() {
        let full_page: Vec<WorkspaceActivity> = (0..PAGE_SIZE)
            .map(|i| row(&format!("p{i}"), &format!("2026-08-09 11:00:{i:02}+00")))
            .collect();

        let outcome = apply_fetched_page(FetchMode::MergePageOne, six_loaded_rows(), full_page);

        assert!(
            outcome.has_more.is_none(),
            "whether older rows remain unfetched is a property of the tail the \
             user has paged to, and page one says nothing about it. A user who \
             has reached the end of the feed has no \"Load more\" button, and \
             answering this from a full page one would hand them one that \
             fetches nothing"
        );
    }

    #[test]
    fn paging_answers_has_more_from_the_size_of_the_page_it_got() {
        let full_page: Vec<WorkspaceActivity> = (0..PAGE_SIZE)
            .map(|i| row(&format!("p{i}"), &format!("2026-08-09 11:00:{i:02}+00")))
            .collect();

        assert_eq!(
            apply_fetched_page(FetchMode::Replace, Vec::new(), full_page.clone()).has_more,
            Some(true),
            "a page as long as the limit means the server had at least that many"
        );
        assert_eq!(
            apply_fetched_page(FetchMode::Replace, Vec::new(), vec![row("a1", "2026-08-09 11:01:00+00")])
                .has_more,
            Some(false),
            "a short page is the end of the feed"
        );
        assert_eq!(
            apply_fetched_page(FetchMode::Append, six_loaded_rows(), full_page).has_more,
            Some(true),
            "and \"Load more\" answers it the same way, from the page it just \
             asked for"
        );
    }

    #[test]
    fn load_more_appends_the_page_it_asked_for() {
        let older = vec![
            row("a0", "2026-08-09 11:00:00+00"),
            row("z9", "2026-08-09 10:59:00+00"),
        ];

        let outcome = apply_fetched_page(FetchMode::Append, six_loaded_rows(), older);

        assert_eq!(
            ids(&outcome.activities),
            ["a6", "a5", "a4", "a3", "a2", "a1", "a0", "z9"],
            "the next page continues the list"
        );
    }

    #[test]
    fn a_filter_change_replaces_what_is_on_screen() {
        let outcome = apply_fetched_page(
            FetchMode::Replace,
            six_loaded_rows(),
            vec![row("b1", "2026-08-09 09:00:00+00")],
        );

        assert_eq!(
            ids(&outcome.activities),
            ["b1"],
            "the rows on screen belong to the query the user has just left, so \
             a filter change keeps none of them — this is the one mode that is \
             supposed to discard the loaded pages"
        );
    }

    #[test]
    fn the_merged_list_is_ordered_by_time_not_by_which_fetch_supplied_a_row() {
        // Constructed, not observed: the server pages contiguously, so page one
        // and the tail below it do not normally interleave. The ordering of the
        // feed should not depend on that staying true — of the server keeping
        // this exact paging, and of no filter narrowing page one to a window
        // inside the loaded range.
        let loaded = vec![
            row("a5", "2026-08-09 11:05:00+00"),
            row("a4", "2026-08-09 11:04:00+00"),
            row("a1", "2026-08-09 11:01:00+00"),
        ];
        let page_one = vec![
            row("b7", "2026-08-09 11:07:00+00"),
            row("b3", "2026-08-09 11:03:00+00"),
        ];

        let outcome = apply_fetched_page(FetchMode::MergePageOne, loaded, page_one);

        assert_eq!(
            ids(&outcome.activities),
            ["b7", "a5", "a4", "b3", "a1"],
            "each row sits where its `created_at` puts it, not where the fetch \
             that carried it does — concatenating page one onto the tail is \
             right only for as long as the two never overlap"
        );
    }

    #[test]
    fn a_whole_second_sorts_behind_the_same_second_carrying_a_fraction() {
        // The ordering claim on `merge_page_one` is that comparing Postgres's
        // `TIMESTAMPTZ::TEXT` rendering as a string orders it chronologically.
        // Every field of that rendering is fixed-width except the fractional
        // seconds, which Postgres omits entirely at a whole second — so this is
        // the one place the claim could fail, and it holds because `+` (0x2B)
        // precedes `.` (0x2E).
        let outcome = apply_fetched_page(
            FetchMode::MergePageOne,
            vec![row("whole", "2026-08-09 11:00:26+00")],
            vec![row("fraction", "2026-08-09 11:00:26.5+00")],
        );

        assert_eq!(
            ids(&outcome.activities),
            ["fraction", "whole"],
            "11:00:26.5 is later than 11:00:26 and the feed is newest-first"
        );
    }
}

// ─── Browser tests ───────────────────────────────────────────────────────────

/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::store::SyncStore;
    use crate::wasm_test_support::{boot_leptos_executor, mount_container, stub_server_fns};

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// What [`refetch_on_live_activity`] is wired to in `ActivityPage`, reduced
    /// to a counter: a closure that re-reads page one and merges it into the
    /// pages already loaded.
    ///
    /// `Rc<Cell<u32>>` rather than a signal so the assertions read a plain
    /// value and cannot themselves be the thing that is reactive.
    fn counting_refetch() -> (Rc<Cell<u32>>, impl Fn() + Send + Sync + 'static) {
        let runs = Rc::new(Cell::new(0u32));
        let counted = send_wrapper::SendWrapper::new(Rc::clone(&runs));
        (runs, move || counted.set(counted.get() + 1))
    }

    #[wasm_bindgen_test]
    async fn a_live_activity_frame_reloads_the_workspace_feed() {
        boot_leptos_executor();
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();
        let (runs, refetch) = counting_refetch();
        refetch_on_live_activity(Some(store), refetch);

        TimeoutFuture::new(20).await;
        assert_eq!(
            runs.get(),
            0,
            "the first fire is the effect registering its dependency — the page's own \
             initial-load effect has already asked for page one, so refetching here would \
             be a second request for the same rows"
        );

        store.bump_activities_version();
        TimeoutFuture::new(20).await;

        assert_eq!(
            runs.get(),
            1,
            "the workspace activity feed reads its rows through \
             `list_workspace_activities`, not from the sync store, so this counter is the \
             only thing that can tell it another user or an agent recorded something — \
             without it the page shows what it read on mount until it is navigated away \
             from and back"
        );

        store.bump_activities_version();
        TimeoutFuture::new(20).await;
        assert_eq!(
            runs.get(),
            2,
            "and it keeps following the counter, rather than reacting once and going quiet"
        );
    }

    #[wasm_bindgen_test]
    async fn only_activity_frames_reload_the_workspace_feed() {
        // The feed shows activities and nothing else, so a comment or a relation
        // arriving must not send it back to the server. Without this the test
        // above would pass just as well against a wiring that refetched on any
        // frame at all.
        boot_leptos_executor();
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();
        let (runs, refetch) = counting_refetch();
        refetch_on_live_activity(Some(store), refetch);

        TimeoutFuture::new(20).await;

        store.bump_comments_version();
        store.bump_relations_version();
        TimeoutFuture::new(20).await;

        assert_eq!(
            runs.get(),
            0,
            "only the activity counter drives this page's refetch"
        );
    }

    #[wasm_bindgen_test]
    async fn the_feed_still_works_without_a_sync_store() {
        // `use_context::<SyncStore>()` is an `Option`, and this is what the
        // `None` arm has to do: nothing, quietly. The page's own initial load and
        // its filter effects are untouched by it.
        boot_leptos_executor();
        let owner = Owner::new();
        owner.set();

        let (runs, refetch) = counting_refetch();
        refetch_on_live_activity(None, refetch);

        TimeoutFuture::new(20).await;
        assert_eq!(
            runs.get(),
            0,
            "with no store there is no counter to subscribe to, so there is nothing to \
             refetch on"
        );
    }

    // ── The page's own wiring ───────────────────────────────────────────────
    //
    // Everything above asks `refetch_on_live_activity` whether it works. This
    // asks `ActivityPage` whether it uses it, which is a different question and
    // was the unasked one: deleting the call from the page left the whole suite
    // green, and only clippy's `dead_code` objected — a guard that evaporates
    // the moment the function acquires any second caller.
    //
    // So this mounts the real `ActivityPage`, with no shim standing in for it,
    // and watches the wire it actually talks over. See `stub_server_fns` for how
    // the server functions are answered.

    /// How many times the mounted page asked the server for activities.
    ///
    /// The page's own request is the observation on purpose. Every cheaper probe
    /// — a counting closure, a rebuilt source signal, a shim component — is
    /// something the *test* wired up, and would keep passing with the page's own
    /// wiring cut. Counting requests leaves nothing for the test to get right on
    /// the page's behalf.
    ///
    /// What this pins is that a live frame reaches the server at all. It does
    /// not pin which [`FetchMode`] the page asks in: `Replace` and
    /// `MergePageOne` differ only in what they do to the rows already held, and
    /// both send the same request — same filters, `offset` 0 — so no observation
    /// of the wire can tell them apart. That distinction is covered where it is
    /// decided, by `apply_fetched_page` in `merge_tests` above.
    const ACTIVITIES_FN: &str = "list_workspace_activities";

    #[wasm_bindgen_test]
    async fn the_mounted_page_refetches_when_a_live_activity_frame_arrives() {
        boot_leptos_executor();

        let server = stub_server_fns(&[
            // The feed itself, plus the two dropdowns' `Resource`s. All three
            // answer with an empty list: what the page renders is not what is
            // under test, and an empty feed still issues every request.
            (ACTIVITIES_FN, "[]"),
            ("list_teams", "[]"),
            ("list_workspace_members", "[]"),
        ]);

        let container = mount_container();
        let store = SyncStore::new();

        // `Layout` provides the store to every authenticated route; here the
        // mount owner does, so `use_context::<SyncStore>()` inside the page
        // resolves the same way it does in production.
        let handle = leptos::mount::mount_to(container.clone(), move || {
            provide_context(store);
            view! { <ActivityPage/> }
        });

        // Long enough for the initial-load effect to have issued its request and
        // for the stub to have answered it — the page refuses to start a second
        // fetch while one is in flight (`loading`), so a bump landing before this
        // settles would be dropped and the assertion below would blame the
        // wiring for a race in the test.
        TimeoutFuture::new(300).await;
        assert_eq!(
            server.calls_to(ACTIVITIES_FN),
            1,
            "the page did not load its feed on mount, so nothing below this line \
             measures the live-refetch wiring — fix this first"
        );

        // Somebody else records an activity: the sync engine bumps this counter
        // (`cache/apply.rs` discards the frame's payload and bumps), which is the
        // only signal this page gets that anything happened.
        store.bump_activities_version();
        TimeoutFuture::new(300).await;

        assert_eq!(
            server.calls_to(ACTIVITIES_FN),
            2,
            "`ActivityPage` did not refetch when the activity counter moved, so \
             the feed will sit at whatever it read on mount until the user \
             navigates away and back — the TRA-9987 bug, restored.\n\
             The page reads its rows through `list_workspace_activities` rather \
             than from the sync store, so this counter is the only thing that can \
             tell it another user, another tab or an agent recorded something. \
             `refetch_on_live_activity` is what subscribes to it; check that \
             `ActivityPage` still calls it, and still passes it \
             `use_context::<SyncStore>()` rather than `None`."
        );

        assert!(
            server.unmatched().is_empty(),
            "the page requested {:?}, which the stub table has no answer for. A \
             server function that was renamed, or a new one added to this page, \
             makes the counts above measure something other than what they name",
            server.unmatched()
        );

        drop(handle);
        container.remove();
    }
}
