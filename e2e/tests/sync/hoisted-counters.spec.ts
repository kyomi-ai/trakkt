import { test, expect, type Locator, type Page } from '@playwright/test';
import * as crypto from 'crypto';
import {
  attachSyncProbe,
  createIssueViaUi,
  expectDeliveredByCounter,
  expectNoPanics,
  launchTwoClients,
  markWhenQuiet,
  waitForEntityId,
  waitForSyncHandshake,
  type SyncProbe,
  type TwoClients,
} from './realtime-harness';

// TRA-9997 — browser coverage for three SyncStore version counters, and the
// disposal check the `SyncStore` getter contract asks for, on the two pages
// PR #292 edited.
//
// ── What #292 did, and what it did not do ───────────────────────────────────
//
// TRA-9991 filed six `sync_store.map(|s| s.…_version())` sites as a latent
// panic, and TRA-9997 inherits that framing. It is wrong, and this file must not
// repeat it: the premise was investigated and adjudicated FALSE before #292
// merged. `docs/review-logs/2026-08-03.md` (12:40) re-derived the mechanism from
// `reactive_graph-0.2.14` source, restored the pre-hoist inline shape at
// `IssueTimeline` and ran the whole browser suite against it — 59/59 green — and
// #292's own commit message (b59d44c) says so outright: "That premise was
// wrong … So this is a robustness fix, not a bug fix."
//
// What #292 changed is where each getter is *called*: once at component setup
// instead of once per evaluation of the closure that reads it, which drops one
// abandoned arena item per evaluation. It did not change what is tracked, and
// all six sites still read the hoisted handle from inside the closure that has
// to depend on it — `v.track()` at `issue_detail.rs:872`, `.get()` at
// `issue_detail.rs:1994` and `:2392`, `project_detail.rs:330`, `:345`, `:355`.
//
// ── The contract that is real ───────────────────────────────────────────────
//
// `SyncStore`'s getter contract (`crates/trakkt-ui/src/cache/store.rs:98-140`)
// is about disposal. Every getter returns a newly built `Signal` registered with
// whichever owner is current at the moment of the call, and the shape that
// panics is a wrapper that *outlives its resolution point* — retained across a
// rebuild and then read by a separately-triggered subscriber, which is what
// forced the revert of #283. Resolving at setup is the form that cannot reach
// that shape; the inline form was safe, but one refactor away from it.
//
// ── So what are these tests for ─────────────────────────────────────────────
//
// Not verification of a production bug. Two things, both permanent:
//
//  1. Regression cover for the tracking reads listed above. Their loss is
//     invisible to everything else in the pipeline: delete a `.get()` from a
//     resource's source closure and the page still renders, still compiles,
//     still passes clippy, and silently stops updating. TRA-9964 demonstrated
//     exactly that by deleting `milestones_version` from `project_detail.rs`'s
//     resource key and watching native, wasm and clippy all stay green. Only a
//     second window watching the first one make a change goes red.
//  2. The disposal check itself, exercised by building and tearing these two
//     pages' subtrees down repeatedly — the last test — since a wrapper that
//     outlived its owner surfaces on teardown, not on first render.
//
// Three of the six counters already have (1):
//   milestones_version   — milestone-realtime.spec.ts, both pages
//   activities_version   — two-window-sync.spec.ts, issue timeline
// This file covers the remaining three, one test each:
//   relations_version        crates/trakkt-ui/src/pages/issues/issue_detail.rs:1987
//   project_updates_version  crates/trakkt-ui/src/pages/projects/project_detail.rs:340
//   project_members_version  crates/trakkt-ui/src/pages/projects/project_detail.rs:350
//
// Every test asserts twice, the same discipline as milestone-realtime.spec.ts:
// once that window B shows the change, and once — through the probe — that it
// showed it *because a sync frame arrived*. Without the second assertion a
// reload, a refetch-on-focus or a poll would produce an identical screen and a
// green run that proves nothing.

let clients: TwoClients;
let pageA: Page;
let pageB: Page;
let probeB: SyncProbe;

/** `/projects/{id}` of the project both windows work in, created in setup. */
let projectHref: string;
/** Just the `{id}`. It is what a project_member payload carries. */
let projectId: string;
/** Its name, so the issue sidebar picks THIS project and not a previous run's. */
let projectName: string;
/** `/issues/{KEY}-{n}` of the issue in that project, created in setup. */
let issueHref: string;

const rand = () => crypto.randomBytes(4).toString('hex');

/**
 * U+2019, the right single quote in the update composer's placeholder.
 *
 * `project_detail.rs:1574` writes it as `"What\u{2019}s the latest?"`, so the
 * ASCII apostrophe does not match and `getByPlaceholder` would find nothing.
 * Naming it here rather than pasting the glyph inline gives the next person who
 * changes that string one place to look — the same reason
 * `project-view-state.spec.ts` names its `▾`.
 */
const POST_UPDATE_PLACEHOLDER = 'What’s the latest?';

/**
 * One of the project detail page's `<h3>`-headed blocks — Milestones, Updates
 * or Members.
 *
 * Each is a `div.mt-6` whose first child is the heading (`project_detail.rs`
 * :1463, :1729, and `MilestoneSection`), and scoping to it is what keeps the
 * assertions discriminating: the member's display name, for instance, also
 * appears in the page's lead selector, and "Post" is a prefix of "Post update".
 */
function projectSection(page: Page, heading: string): Locator {
  return page
    .locator('div.mt-6')
    .filter({ has: page.getByRole('heading', { name: heading, exact: true }) });
}

/**
 * The issue detail page's Relations block.
 *
 * Keyed on its "Add relation" button rather than on its heading, because the
 * heading is `"Relations"` while the issue has none and `"Relations (1)"` once
 * it has one (`issue_detail.rs:2105-2109`) — a locator that changes name the
 * moment the thing under test happens is not usable as a scope.
 */
function relationsSection(page: Page): Locator {
  return page
    .locator('div.mt-6')
    .filter({ has: page.getByRole('button', { name: 'Add relation' }) });
}

/** Put an existing issue in the shared project, from its detail sidebar. */
async function setIssueProject(page: Page, href: string) {
  await page.goto(href);
  await page.waitForLoadState('networkidle');
  await page.getByRole('button', { name: /Set project/ }).click();
  // Exact name: a rerun against the same database leaves earlier runs' projects
  // in this dropdown, and picking the first "Hoisted ..." would pick one of them.
  await page.getByRole('option', { name: projectName, exact: true }).click();
  await expect(page.getByText(projectName, { exact: true }).first()).toBeVisible({
    timeout: 20_000,
  });
}

test.describe.configure({ mode: 'serial' });

test.afterEach(async () => {
  expectNoPanics(clients);
});

// Signup, two cold WASM boots, and an issue created through the real UI all sit
// inside single tests here — the same budget the other two probe suites use.
test.setTimeout(180_000);

test.beforeAll(async () => {
  clients = await launchTwoClients();
  pageA = clients.pageA;
  pageB = clients.pageB;

  // Only B's socket is watched: every delivery assertion here is about what B
  // does with a frame. It has to be attached before B's first navigation —
  // `page.on('websocket')` only reports sockets opened after it is registered,
  // and `waitForEntityId` reads the frames those sockets carry.
  probeB = attachSyncProbe(pageB);

  // A project of this run's own, so a rerun against the same database does not
  // inherit the previous run's updates or members. It matters more here than in
  // the milestone suite: the members test below depends on the project having
  // nobody on it yet, which is only true of a project this run created.
  await pageA.goto('/projects');
  await pageA.waitForLoadState('networkidle');
  await pageA.getByRole('button', { name: 'New Project' }).first().click();
  projectName = `Hoisted ${rand()}`;
  await pageA.getByPlaceholder('e.g. Q3 Launch').fill(projectName);
  await pageA.getByRole('button', { name: 'Create Project' }).click();
  // `ProjectCreationModal` navigates to the new project on success.
  await pageA.waitForURL(/\/projects\/[^/]+$/, { timeout: 30_000 });
  projectHref = new URL(pageA.url()).pathname;
  projectId = projectHref.split('/').pop() ?? '';
  expect(projectId, 'the project id is the discriminating string for member frames').not.toBe('');

  // One issue, in that project. The relations test hangs a relation off it, and
  // the navigation test uses it as the far end of the route cycle — it has to be
  // *in* the project for the project's overview to link to it.
  issueHref = await createIssueViaUi(pageA, `Hoisted hub ${rand()}`);
  await setIssueProject(pageA, issueHref);

  clients.panics.length = 0;
});

test.afterAll(async () => {
  await clients?.browser?.close();
});

test("a relation added in window A reaches window B's relations section without a reload", async () => {
  // Guards `relations_version` at `issue_detail.rs:1987` and the
  // `ws_version.map(|v| v.get())` read in the resource's source closure two
  // lines below it. `RelationsSection` also holds a local `version` signal that
  // its own adds and removes bump, so the section keeps working in the window
  // that made the change even with the store counter gone — which is exactly why
  // this has to be asserted in the *other* window.
  const targetTitle = `Relation target ${rand()}`;

  await pageB.goto(issueHref);
  const sectionB = relationsSection(pageB);
  await expect(sectionB.getByRole('button', { name: 'Add relation' })).toBeVisible({
    timeout: 30_000,
  });
  // The starting state, asserted rather than assumed: everything below is about
  // a row appearing, so it has to be established that there was no row.
  await expect(sectionB.getByText('No relations')).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');

  // A creates the issue the relation will point at. B is connected, so the
  // `issue` insert frame reaches it — and that frame is how this test learns the
  // target's UUID. It needs one: every field of an `IssueRelation` payload is a
  // server-minted UUID, so without it there is no string that ties a frame to
  // *this* relation rather than to any other. See `waitForEntityId`.
  await createIssueViaUi(pageA, targetTitle);
  const targetIssueId = await waitForEntityId(probeB, 'issue', targetTitle);

  const since = await markWhenQuiet(probeB, 'list_issue_relations');

  // ── A adds the relation from its own copy of the issue ────────────────────
  await pageA.goto(issueHref);
  const sectionA = relationsSection(pageA);
  await expect(sectionA.getByRole('button', { name: 'Add relation' })).toBeVisible({
    timeout: 30_000,
  });
  await sectionA.getByRole('button', { name: 'Add relation' }).click();

  // The relation type is left at `add_relation_type`'s initial "child_of"
  // (`issue_detail.rs:2002`), which sends `relation_type: "parent"` with this
  // issue as the source. Which type it is does not matter to the counter — any
  // `issue_relation` frame bumps it — so the default is used rather than a click
  // that would only add a way for the setup to fail.
  const search = pageA.getByPlaceholder('Search issues...');
  await expect(search).toBeVisible({ timeout: 15_000 });
  await search.fill(targetTitle);

  // The title is unique to this test, so the picker must be down to one result.
  // Asserting the count rather than taking `.first()` is what makes that a check
  // instead of a hope — a picker still showing every issue would otherwise pick
  // an arbitrary one and the failure would surface much later as a mystery.
  const result = pageA.getByRole('button').filter({ hasText: targetTitle });
  await expect(result, 'the issue picker must narrow to the one target issue').toHaveCount(1);
  await result.click();

  // A's own section updates through its local `version` counter, so this proves
  // the write succeeded and nothing more.
  await expect(sectionA.getByRole('link', { name: targetTitle, exact: true })).toBeVisible({
    timeout: 30_000,
  });

  // ── The deliverable: B, still on the same document, follows ───────────────
  await expect(sectionB.getByRole('link', { name: targetTitle, exact: true })).toBeVisible({
    timeout: 30_000,
  });
  await expect(sectionB.getByText('No relations')).toHaveCount(0);

  expectDeliveredByCounter(
    probeB,
    since,
    'issue_relation',
    'list_issue_relations',
    targetIssueId,
  );
});

test("a project update posted in window A reaches window B's Updates section without a reload", async () => {
  // Guards `project_updates_version` at `project_detail.rs:340` and its read in
  // `server_updates`' source closure at :345. `HealthUpdateSection` calls
  // `server_updates.refetch()` directly after its own post (:1456), so — as with
  // relations — the posting window is not evidence of anything and the assertion
  // has to be made in the other one.
  const body = `Update from A ${rand()}`;

  await pageB.goto(projectHref);
  const updatesB = projectSection(pageB, 'Updates');
  await expect(updatesB.getByRole('button', { name: 'Post update' })).toBeVisible({
    timeout: 30_000,
  });
  await waitForSyncHandshake(probeB, 'B');

  const since = await markWhenQuiet(probeB, 'list_project_updates');

  await pageA.goto(projectHref);
  const updatesA = projectSection(pageA, 'Updates');
  await expect(updatesA.getByRole('button', { name: 'Post update' })).toBeVisible({
    timeout: 30_000,
  });
  await updatesA.getByRole('button', { name: 'Post update' }).click();

  const composer = updatesA.getByPlaceholder(POST_UPDATE_PLACEHOLDER);
  await expect(composer, 'the composer must open before anything is typed').toBeVisible({
    timeout: 15_000,
  });
  await composer.fill(body);

  // The health pills are left at `UpdateDraft::DEFAULT_HEALTH` ("on_track",
  // :152). `exact: true` on the submit button is load-bearing: "Post update" —
  // the button the composer replaced — contains "Post", so a substring match
  // would still find that button if the composer had failed to open, and the
  // test would post nothing while reading as though it had.
  await updatesA.getByRole('button', { name: 'Post', exact: true }).click();
  await expect(updatesA.getByText(body, { exact: true })).toBeVisible({ timeout: 30_000 });

  // ── The deliverable ───────────────────────────────────────────────────────
  await expect(updatesB.getByText(body, { exact: true })).toBeVisible({ timeout: 30_000 });

  // The body is the discriminating string, and it is one this test chose rather
  // than the project id: the project id would also match a frame for some other
  // update on the same project, which is precisely the confusion
  // `payloadMustContain` exists to prevent. `ProjectUpdate.body` carries it
  // (`crates/trakkt-types/src/models.rs:187-194`).
  expectDeliveredByCounter(probeB, since, 'project_update', 'list_project_updates', body);
});

test("a member added in window A reaches window B's Members section without a reload", async () => {
  // Guards `project_members_version` at `project_detail.rs:350` and its read in
  // `server_members`' source closure at :355.
  //
  // ── Why this drives the ADD and not the remove ────────────────────────────
  // The harness signs up exactly one user (self_hosted closes registration after
  // the first), so there is one candidate in the workspace and the direction has
  // to be one that a single user can reach. It is: `create_project`
  // (`crates/trakkt-auth/src/project_service.rs:279-341`) writes the `projects`
  // row and its sync entry and nothing else — no membership — so a project this
  // run created starts with nobody on it and the sole user is addable.
  //
  // The remove direction is unusable here for a second, independent reason, and
  // it is worth recording so nobody adds it later expecting it to work:
  // `remove_project_member` broadcasts a delete with `None` as its payload
  // (:917-927, deliberately — there is no row left to send), and
  // `expectDeliveredByCounter` matches its discriminating string against the
  // frame's `data`. A remove could be asserted on screen but not tied to its
  // frame, which is the weaker half of the pair this suite refuses to settle for.
  await pageB.goto(projectHref);
  const membersB = projectSection(pageB, 'Members');
  await expect(
    membersB.getByText('No members added yet'),
    'this project must start with no members, or the add below has nobody to add',
  ).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');

  const since = await markWhenQuiet(probeB, 'list_project_members');

  await pageA.goto(projectHref);
  const membersA = projectSection(pageA, 'Members');
  await expect(membersA.getByRole('button', { name: 'Add member' })).toBeVisible({
    timeout: 30_000,
  });
  await membersA.getByRole('button', { name: 'Add member' }).click();

  // `Select`'s trigger is a `button[aria-haspopup="listbox"]` — the attribute is
  // at `components/dropdown.rs:520`, inside the `SelectVariant::Form` arm that
  // spans :512-533. Each member row carries one too — the
  // role selector — so this locator is only unambiguous while the list is empty,
  // which is the state asserted above. The count assertion keeps that dependency
  // honest instead of leaving it to `.first()`.
  const picker = membersA.locator('button[aria-haspopup="listbox"]');
  await expect(
    picker,
    'with no members yet, the add-member picker must be the only select in this section',
  ).toHaveCount(1);
  await picker.click();

  // `add_member_options` (:1688-1703) is the placeholder plus every workspace
  // member not already on the project. With this suite's single user that is
  // exactly two entries, and asserting it is how the test states its dependency
  // on the single-user harness out loud: if a second user is ever seeded, this
  // fails here with a readable reason rather than silently adding whoever
  // happened to sort first.
  const options = pageA.getByRole('option');
  await expect(
    options,
    'the picker must offer the placeholder and exactly one addable workspace member',
  ).toHaveCount(2);
  const memberOption = options.filter({ hasNotText: 'Select a member...' });
  await expect(memberOption).toHaveCount(1);

  // Read the label off the option rather than hardcoding it: `resolve_name`
  // (:1631-1642) and the option list both render `name` falling back to `email`,
  // and which one the signup produced depends on whether the form offered a name
  // field. Taking it from the picker means the row assertion below compares
  // against whatever the app actually chose.
  const memberName = ((await memberOption.textContent()) ?? '').trim();
  expect(memberName.length, 'the addable member must have a visible label').toBeGreaterThan(0);
  await memberOption.click();

  await expect(membersA.getByText(memberName, { exact: true })).toBeVisible({ timeout: 30_000 });

  // ── The deliverable ───────────────────────────────────────────────────────
  await expect(membersB.getByText(memberName, { exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(membersB.getByText('No members added yet')).toHaveCount(0);

  // `ProjectMember` carries `project_id`, `user_id`, `role` and `created_at`
  // (`crates/trakkt-types/src/models.rs:166-171`) — all server-side values. The
  // project id is the one this test knows, and it is discriminating because the
  // project belongs to this run: no other project's membership frame carries it.
  expectDeliveredByCounter(probeB, since, 'project_member', 'list_project_members', projectId);
});

test('cycling between the project and issue pages rebuilds both without a disposed-signal panic', async () => {
  // The disposal half, which the three tests above cannot reach: they each load
  // a page once and leave it there, and a wrapper that outlives its owner is
  // only readable *after* that owner is cleaned up. Per the getter contract
  // (`store.rs:98-140`), a `Signal` handed out by a `SyncStore` getter is
  // registered with whichever owner is current at the call, and reading one
  // whose owner has since been disposed panics with "you tried to access a
  // reactive value ... but it has already been disposed" — which `tachys` then
  // turns into "entered unreachable code" in its class-rendering cascade. Both
  // strings are watched by `watchForPanics`, and `expectNoPanics` in `afterEach`
  // is what fails this test; there is no separate assertion for it, by design.
  //
  // Nothing here claims the current code has that defect — the review that
  // cleared #292 established it does not. This is the check that keeps saying so
  // once these six sites are edited again, on the two pages that own them.
  //
  // The cycle runs between the two pages TRA-9991 edited, so each pass disposes
  // one counter-fed subtree and builds the other:
  //   project_detail.rs   milestones_version :325, project_updates_version :340,
  //                       project_members_version :350
  //   issue_detail.rs     milestones_version :863 (MetadataSidebar effect),
  //                       relations_version :1987, activities_version :2383
  await pageA.goto(projectHref);
  await expect(projectSection(pageA, 'Milestones').getByRole('button', { name: 'Add milestone' }))
    .toBeVisible({ timeout: 30_000 });

  // Set on the document, checked at the end. A full page load would clear it,
  // and that distinction is the whole test: `goto` and a reload rebuild the app
  // from scratch, which disposes nothing and would let this pass while proving
  // nothing about teardown. Only a router-driven route change within one
  // document unmounts a subtree while the reactive graph around it stays alive.
  await pageA.evaluate(() => {
    (window as any).__trakktNavCycle = 'same-document';
  });

  // Three passes, not one. The first build of a page happens under a fresh
  // owner; only from the second does a subtree get built under an owner that has
  // already torn one down, which is the state a stale wrapper is reachable from.
  for (let cycle = 1; cycle <= 3; cycle++) {
    // Into the issue via the project's own overview row. `ProjectIssueRow`'s
    // handler calls `ev.prevent_default()` and then `use_navigate`
    // (`project_detail.rs:2185-2194`), so the click is a route change rather
    // than a document load. Located by exact href — the sidebar and the row list
    // both contain issue links, and only this one addresses the hub issue.
    const issueRow = pageA.locator(`a[href="${issueHref}"]`);
    await expect(
      issueRow,
      `cycle ${cycle}: the hub issue must be listed on the project overview to navigate from`,
    ).toHaveCount(1);
    await issueRow.click();

    // Wait for elements the counters actually feed, not for `networkidle`: an
    // idle network says the requests finished, not that the subtrees they feed
    // were rebuilt. The Relations block is `relations_version`'s, the timeline
    // row is `activities_version`'s, and the Milestone field only renders once
    // the issue has a project (`MetadataSidebar`'s `project_id.get().is_some()`
    // guard), which is why setup put the hub issue in one.
    await expect(
      relationsSection(pageA).getByRole('button', { name: 'Add relation' }),
      `cycle ${cycle}: the issue's relations section never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      pageA.getByText('created this issue').first(),
      `cycle ${cycle}: the issue's timeline never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      pageA.getByRole('button', { name: /Set milestone/ }),
      `cycle ${cycle}: the issue's metadata sidebar never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });

    // And back. The step in was a `pushState` within this document, so going
    // back is a same-document history traversal — another route change, not a
    // load. The marker check below is what holds that claim to account.
    await pageA.goBack();

    // All three of the project page's counter-fed sections, every pass.
    await expect(
      projectSection(pageA, 'Milestones').getByRole('button', { name: 'Add milestone' }),
      `cycle ${cycle}: the project's Milestones section never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      projectSection(pageA, 'Updates').getByRole('button', { name: 'Post update' }),
      `cycle ${cycle}: the project's Updates section never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      projectSection(pageA, 'Members').getByRole('button', { name: 'Add member' }),
      `cycle ${cycle}: the project's Members section never rebuilt`,
    ).toBeVisible({ timeout: 30_000 });
  }

  expect(
    await pageA.evaluate(() => (window as any).__trakktNavCycle),
    'the document was reloaded somewhere in the cycle, so nothing was ever torn down and \
rebuilt — this test proved nothing about disposal',
  ).toBe('same-document');

  // The panic assertion itself is `expectNoPanics` in `afterEach`.
});
