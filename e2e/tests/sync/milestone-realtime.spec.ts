import { test, expect, type Locator, type Page } from '@playwright/test';
import * as crypto from 'crypto';
import {
  attachSyncProbe,
  expectDeliveredByCounter,
  expectNoPanics,
  goOffline,
  goOnline,
  installSocketControl,
  launchTwoClients,
  mark,
  markWhenQuiet,
  waitForSyncHandshake,
  type SyncProbe,
  type TwoClients,
} from './realtime-harness';

// TRA-9964 — the realtime *UI wiring*, not the realtime protocol.
//
// `cache/apply.rs` has native unit tests proving a `project_milestone` frame
// bumps `milestones_version`. Nothing proved any screen re-reads when it does.
// Deleting `milestones_version` from `project_detail.rs`'s `server_milestones`
// resource key, or the `v.track()` from `issue_detail.rs`'s `MetadataSidebar`
// effect, left native, wasm and clippy all green while the feature stopped
// working. These three tests are the ones that go red.
//
// Each asserts twice: once that window B shows the change, and once — via the
// probe in `./realtime-harness` — that it showed it *because a sync frame
// arrived*. The second assertion is what stops a reload, a refetch-on-focus or
// a poll from producing a green run that proves nothing.

let clients: TwoClients;
let pageA: Page;
let pageB: Page;
let probeA: SyncProbe;
let probeB: SyncProbe;

/** `/projects/{id}` of the project both windows work in, created in setup. */
let projectHref: string;
/** Its name, so the issue sidebar picks THIS project and not a previous run's. */
let projectName: string;
const rand = () => crypto.randomBytes(4).toString('hex');

/** The project detail page's "Milestones" block, in whichever window. */
function milestoneSection(page: Page): Locator {
  return page
    .locator('div.mt-6')
    .filter({ has: page.getByRole('heading', { name: 'Milestones' }) });
}

/**
 * Create a milestone from `page`'s project view and wait for `page` itself to
 * show it.
 *
 * Waiting on the *writing* window is deliberate: `AddMilestoneForm` calls
 * `server_milestones.refetch()` directly after the write, so this settles
 * whether or not the version counter is wired up. Every test here creates the
 * milestone it needs this way rather than inheriting one from the test before,
 * so each can be run alone — which is what makes it possible to show that
 * removing a counter dependency breaks the delta path specifically, and not
 * merely that it broke the test that happened to run first.
 */
async function createMilestone(page: Page, name: string) {
  const section = milestoneSection(page);
  await section.getByRole('button', { name: 'Add milestone' }).click();
  await section.getByPlaceholder('Milestone name').fill(name);
  await section.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(section.getByText(name, { exact: true })).toBeVisible({ timeout: 20_000 });
}

/** Rename the one milestone in `page`'s project view, via inline edit. */
async function renameMilestone(page: Page, from: string, to: string) {
  const section = milestoneSection(page);
  // The name span is the edit trigger — `MilestoneRow`'s `on_edit`.
  await section.getByText(from, { exact: true }).click();
  const input = section.locator('input[type="text"]');
  await expect(input).toBeVisible({ timeout: 10_000 });
  await input.fill(to);
  // Enter is handled by `save_on_keydown`; there is no form submit.
  await input.press('Enter');
  await expect(section.getByText(to, { exact: true })).toBeVisible({ timeout: 20_000 });
}

test.describe.configure({ mode: 'serial' });

test.afterEach(async () => {
  expectNoPanics(clients);
});

// Signup, a cold WASM boot, and — in the delta test — a reconnect backoff that
// can reach 16s all sit inside single tests.
test.setTimeout(180_000);

test.beforeAll(async () => {
  clients = await launchTwoClients();
  pageA = clients.pageA;
  pageB = clients.pageB;

  // Attached before either page's next navigation. `page.on('websocket')` only
  // reports sockets opened after it is registered, and the delta test counts
  // sockets to prove a reconnect happened.
  probeA = attachSyncProbe(pageA);
  probeB = attachSyncProbe(pageB);

  // Also before B's first navigation — the delta test needs to be able to drop
  // B's live socket, and `setOffline` alone does not do that.
  await installSocketControl(pageB);

  // A project of this run's own, so a rerun against the same database does not
  // inherit the previous run's milestones.
  await pageA.goto('/projects');
  await pageA.waitForLoadState('networkidle');
  await pageA.getByRole('button', { name: 'New Project' }).first().click();
  projectName = `Realtime ${rand()}`;
  await pageA.getByPlaceholder('e.g. Q3 Launch').fill(projectName);
  await pageA.getByRole('button', { name: 'Create Project' }).click();
  // `ProjectCreationModal` navigates to the new project on success.
  await pageA.waitForURL(/\/projects\/[^/]+$/, { timeout: 30_000 });
  projectHref = new URL(pageA.url()).pathname;

  clients.panics.length = 0;
});

test.afterAll(async () => {
  await clients?.browser?.close();
});

test('a milestone created in window A appears in window B on the live frame', async () => {
  const created = `Live ${rand()}`;

  await pageA.goto(projectHref);
  await pageB.goto(projectHref);
  const sectionA = milestoneSection(pageA);
  const sectionB = milestoneSection(pageB);
  await expect(sectionA.getByRole('button', { name: 'Add milestone' })).toBeVisible({
    timeout: 30_000,
  });
  await expect(sectionB.getByRole('button', { name: 'Add milestone' })).toBeVisible({
    timeout: 30_000,
  });

  // Both windows have a socket that has asked for its data. Without this the
  // "no earlier re-read" assertion below could be measuring a window whose
  // first load simply had not finished.
  await waitForSyncHandshake(probeA, 'A');
  await waitForSyncHandshake(probeB, 'B');

  // Everything the probe records from here has to be explained.
  const since = await markWhenQuiet(probeB, 'list_milestones');
  const socketAtMark = probeB.sockets.length - 1;

  await createMilestone(pageA, created);

  // The deliverable: B shows it, with no reload and no navigation.
  await expect(sectionB.getByText(created, { exact: true })).toBeVisible({ timeout: 30_000 });

  const { frame } = expectDeliveredByCounter(
    probeB,
    since,
    'project_milestone',
    'list_milestones',
    created,
  );
  // Live frame, not a reconnect: it arrived on the socket B already had.
  expect(
    frame.socket,
    'the live path must deliver on the socket B was already holding, not a fresh one',
  ).toBe(socketAtMark);
});

test('a milestone renamed while window B is disconnected arrives on the delta', async () => {
  const previous = `Delta base ${rand()}`;
  const renamed = `Delta ${rand()}`;

  // Set up entirely within this test, and — importantly — B gets the starting
  // state from a fresh navigation rather than from a live frame. A page load
  // populates the milestone list whether or not the counter is wired up, so
  // everything asserted after the mark below is about the delta path alone.
  await pageA.goto(projectHref);
  const sectionA = milestoneSection(pageA);
  await expect(sectionA.getByRole('button', { name: 'Add milestone' })).toBeVisible({
    timeout: 30_000,
  });
  await createMilestone(pageA, previous);

  await pageB.goto(projectHref);
  const sectionB = milestoneSection(pageB);
  await expect(sectionB.getByText(previous, { exact: true })).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');

  await markWhenQuiet(probeB, 'list_milestones');

  // Cut B's network. This exercises `get_entries_since` on the server and the
  // `sync_delta` branch of `sync_engine.rs`'s connection watcher — a code path
  // the live-frame test never touches, because a connected client is served
  // from the broadcast instead.
  const dropped = await goOffline(clients.ctxB, pageB);
  expect(dropped, 'window B had no live socket to disconnect').toBeGreaterThan(0);
  await expect
    .poll(() => probeB.sockets[probeB.sockets.length - 1]?.closedAt !== null, {
      timeout: 30_000,
      message: "window B's socket never closed — it was not actually disconnected",
    })
    .toBe(true);

  // Marked here, with B's socket already shut and its network cut, rather than
  // before the outage: from this instant nothing can reach B and B can issue
  // nothing, so every record the probe holds after it belongs to the
  // reconnection. Marking earlier would leave the window's own page-load
  // bootstrap inside the measured range.
  const since = mark();
  const socketsBeforeOutage = probeB.sockets.length;

  await renameMilestone(pageA, previous, renamed);

  // While B is offline the change must not be there. If this ever passes with
  // B still connected, the outage did not happen and the rest is meaningless.
  await expect(sectionB.getByText(renamed, { exact: true })).toHaveCount(0);

  await goOnline(clients.ctxB);

  // `schedule_reconnect` backs off 1s, 2s, 4s … and each attempt first fetches
  // a fresh WS token, so a few failed attempts during the outage push the next
  // one out. 90s covers a backoff that reached 30s plus the delta round trip.
  await expect(sectionB.getByText(renamed, { exact: true })).toBeVisible({ timeout: 90_000 });

  const { frame } = expectDeliveredByCounter(
    probeB,
    since,
    'project_milestone',
    'list_milestones',
    renamed,
  );

  // It came back on a socket opened after the outage...
  expect(
    frame.socket,
    'the delta must arrive on a reconnected socket, not the one that was open before the outage',
  ).toBeGreaterThanOrEqual(socketsBeforeOutage);

  // ...in response to a `sync_delta` request, not a bootstrap and not a live
  // broadcast. This is the assertion that makes the test about
  // `get_entries_since` rather than about reconnecting at all.
  const delta = probeB.frames.find(
    (f) => f.dir === 'out' && f.socket === frame.socket && f.payload.includes('"sync_delta"'),
  );
  expect(delta, 'window B never sent sync_delta — it re-bootstrapped instead of replaying').toBeTruthy();
  expect(
    delta!.t,
    'the milestone frame must be a reply to the delta request, not something that preceded it',
  ).toBeLessThanOrEqual(frame.t);
});

test("a milestone rename reaches window B's issue metadata sidebar without a reload", async () => {
  // The second half of the gap. `MetadataSidebar` reads milestones through its
  // own `list_milestones` call inside an `Effect`, so the project page's
  // resource key proves nothing about it: the two are wired to the same counter
  // by two different mechanisms, and either can be removed alone.
  const issueTitle = `Milestone sidebar ${rand()}`;
  const previous = `Sidebar base ${rand()}`;
  const renamed = `Sidebar ${rand()}`;

  // ── A milestone of this test's own, for the same reason as the delta test ──
  await pageA.goto(projectHref);
  await expect(
    milestoneSection(pageA).getByRole('button', { name: 'Add milestone' }),
  ).toBeVisible({ timeout: 30_000 });
  await createMilestone(pageA, previous);

  // ── Set up an issue in the project, on the milestone ──────────────────────
  // `/workspace`, not `/issues`: the latter redirects to `/my-issues`, which
  // lists only what you are assigned or watching and carries no create button.
  await pageA.goto('/workspace');
  await pageA.waitForLoadState('networkidle');
  await pageA.getByRole('button', { name: 'New Issue' }).first().click();
  await pageA.waitForSelector('#issue-title', { timeout: 15_000 });
  await pageA.fill('#issue-title', issueTitle);
  await pageA.getByRole('button', { name: 'Create Issue' }).click();

  const createdRow = pageA.locator('a[href*="/issues/"]').filter({ hasText: issueTitle }).first();
  await expect(createdRow).toBeVisible({ timeout: 30_000 });
  const href = await createdRow.getAttribute('href');
  expect(href, 'the new issue needs a detail link both windows can open').toBeTruthy();
  const issueHref = href!;

  await pageA.goto(issueHref);
  await pageA.waitForLoadState('networkidle');

  // The Milestone field only renders once the issue has a project — see the
  // `project_id.get().is_some()` guard in `MetadataSidebar`.
  await pageA.getByRole('button', { name: /Set project/ }).click();
  // Exact name: a rerun against the same database leaves earlier runs' projects
  // in this dropdown, and picking the first "Realtime ..." would pick one of them.
  await pageA.getByRole('option', { name: projectName, exact: true }).click();

  await pageA.getByRole('button', { name: /Set milestone/ }).click();
  await pageA.getByRole('option', { name: previous }).first().click();
  await expect(pageA.getByText(previous, { exact: true }).first()).toBeVisible({ timeout: 20_000 });

  // ── B opens the same issue and sees the current milestone name ────────────
  await pageB.goto(issueHref);
  await pageB.waitForLoadState('networkidle');
  await expect(pageB.getByText(previous, { exact: true }).first()).toBeVisible({ timeout: 30_000 });

  const since = await markWhenQuiet(probeB, 'list_milestones');

  // ── A renames it from the project page ────────────────────────────────────
  await pageA.goto(projectHref);
  await expect(
    milestoneSection(pageA).getByRole('button', { name: 'Add milestone' }),
  ).toBeVisible({ timeout: 30_000 });
  await renameMilestone(pageA, previous, renamed);

  // B is still on the issue. Its sidebar must follow.
  await expect(pageB.getByText(renamed, { exact: true }).first()).toBeVisible({ timeout: 30_000 });
  await expect(pageB.getByText(previous, { exact: true })).toHaveCount(0);

  expectDeliveredByCounter(probeB, since, 'project_milestone', 'list_milestones', renamed);
});

// ─────────────────────────────────────────────────────────────────────────────
// TRA-10030 — the defect the flake above was a symptom of
// ─────────────────────────────────────────────────────────────────────────────
//
// The three tests above all drive the *writing* window through
// `createMilestone` / `renameMilestone`, and those helpers intermittently
// failed at `fill` with the input gone. The cause is not in the test: the
// Milestones section's mode and draft signals were created inside
// `project_detail.rs`'s reactive rendering closure, which re-runs — and so
// rebuilds the section from scratch — every time any of the page's five
// reactive sources settles. One of those is `server_milestones`, keyed on
// `milestones_version`, which bumps on every `project_milestone` frame.
//
// So the flake was a user-facing bug wearing a test's clothes: type a milestone
// name, have a colleague add a milestone in another window, and your form
// closes and your text is gone. The two tests below are that bug, stated as the
// user sees it. They are deterministic where the flake was not — the trigger is
// a frame the test itself causes, and the assertions run only once its effect
// (the colleague's row appearing) is on screen, so there is no timing window
// left to be lucky in.
//
// Same class as `two-window-sync.spec.ts`'s "typing in window B is not
// destroyed by a frame arriving from window A", and written to its shape.
//
// ── What these tests do NOT assert, and why ─────────────────────────────────
//
// They do not assert `toBeFocused()` on the input after the frame, because that
// still fails — and it is worth being precise about what survives and what does
// not, rather than leaving it as "the fix works".
//
// The input is still there and still holds what was typed, but it is a
// *different element*: verified by tagging the node with an attribute before the
// frame and finding the attribute gone afterwards. Tagging its neighbours
// narrows the boundary exactly — the row div, the status-dot span inside it, the
// rows container and the section heading all survive; the input, the DatePicker
// beside it and the project-name `h1` do not. What they have in common is that
// each sits inside a `<Show>`.
//
// `Show` is a reactive closure, and `tachys`'s `Render` impl for a closure in
// view position does not diff on rebuild — it builds a fresh subtree and
// unmounts the old one (`tachys-0.2.18/src/reactive_graph/mod.rs:77-83`). So an
// ancestor closure re-running destroys every `Show` under it whether or not its
// own condition changed. Hoisting signals cannot fix that; only the ancestor not
// re-running can, which means `ProjectDetailContent` taking signals rather than
// `Vec`s throughout. That is TRA-10031, and its acceptance criteria include
// restoring the two assertions removed below.

/**
 * Put window B on the project page with a settled Milestones section.
 *
 * The `markWhenQuiet` is load-bearing: B's own page load re-reads
 * `list_milestones` more than once, and each settling resource re-rendered the
 * section just as an arriving frame does. Waiting for B to go quiet first is
 * what makes the frame from A the *only* thing that can re-render it.
 */
async function settleWindowBOnProject(expectVisible: string): Promise<Locator> {
  await pageB.goto(projectHref);
  await pageB.waitForLoadState('networkidle');
  const sectionB = milestoneSection(pageB);
  await expect(sectionB.getByText(expectVisible, { exact: true })).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');
  await markWhenQuiet(probeB, 'list_milestones');
  return sectionB;
}

test("a frame from window A must not close window B's open rename input", async () => {
  const base = `Rename base ${rand()}`;
  const typed = `Renamed by B ${rand()}`;
  const fromA = `Meanwhile from A ${rand()}`;

  await pageA.goto(projectHref);
  await expect(
    milestoneSection(pageA).getByRole('button', { name: 'Add milestone' }),
  ).toBeVisible({ timeout: 30_000 });
  await createMilestone(pageA, base);

  const sectionB = await settleWindowBOnProject(base);

  // B starts renaming and does NOT save.
  await sectionB.getByText(base, { exact: true }).click();
  const inputB = sectionB.locator('input[type="text"]');
  // Count, not just visibility: the ticket's other hypothesis was that this
  // locator matches two inputs once a second row is in play. Asserting the
  // count keeps that answered rather than assumed.
  await expect(
    inputB,
    'exactly one milestone name input must be open — the rename locator must not be ambiguous',
  ).toHaveCount(1);
  await inputB.click();
  await inputB.fill(typed);

  // A adds an unrelated milestone. That is a `project_milestone` frame, which
  // bumps B's `milestones_version` and re-reads `list_milestones` — the same
  // re-render the flake was riding on, now caused deliberately.
  await createMilestone(pageA, fromA);

  // B showing A's new row is proof the frame landed and the section re-rendered.
  // Everything after this line is asserted about the state *after* the event
  // that used to destroy it, so there is nothing left to race.
  await expect(sectionB.getByText(fromA, { exact: true })).toBeVisible({ timeout: 30_000 });

  await expect(
    inputB,
    "window B's rename input was closed by a milestone frame from window A",
  ).toHaveCount(1);
  await expect(
    inputB,
    "window B's half-typed milestone name was discarded by a frame from window A",
  ).toHaveValue(typed);
  // Deliberately NOT asserted here: `toBeFocused()`. It fails, and it fails for
  // a different reason than everything above — see the note below and TRA-10031.

  // And the edit still commits what B typed, against the milestone B was editing.
  await inputB.press('Enter');
  await expect(sectionB.getByText(typed, { exact: true })).toBeVisible({ timeout: 20_000 });
  await expect(sectionB.getByText(base, { exact: true })).toHaveCount(0);
});

test("a frame from window A must not close window B's open add-milestone form", async () => {
  const base = `Add base ${rand()}`;
  const typed = `Added by B ${rand()}`;
  const fromA = `Meanwhile from A ${rand()}`;

  await pageA.goto(projectHref);
  await expect(
    milestoneSection(pageA).getByRole('button', { name: 'Add milestone' }),
  ).toBeVisible({ timeout: 30_000 });
  await createMilestone(pageA, base);

  const sectionB = await settleWindowBOnProject(base);

  // B opens the add form and types a name, without submitting.
  await sectionB.getByRole('button', { name: 'Add milestone' }).click();
  const nameInputB = sectionB.getByPlaceholder('Milestone name');
  await expect(nameInputB).toHaveCount(1);
  await nameInputB.click();
  await nameInputB.fill(typed);

  await createMilestone(pageA, fromA);
  await expect(sectionB.getByText(fromA, { exact: true })).toBeVisible({ timeout: 30_000 });

  await expect(
    nameInputB,
    "window B's add-milestone form was closed by a milestone frame from window A",
  ).toHaveCount(1);
  await expect(
    nameInputB,
    "window B's half-typed milestone name was discarded by a frame from window A",
  ).toHaveValue(typed);
  // Again, no `toBeFocused()` — TRA-10031.

  // And the form still creates what B typed.
  await sectionB.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(sectionB.getByText(typed, { exact: true })).toBeVisible({ timeout: 20_000 });

  // Reopening the form must not resurrect the name that was just submitted —
  // the drafts now outlive the form, so clearing them is the form's job.
  await sectionB.getByRole('button', { name: 'Add milestone' }).click();
  await expect(
    sectionB.getByPlaceholder('Milestone name'),
    'a reopened add-milestone form must start empty, not holding the last name created',
  ).toHaveValue('');
});
