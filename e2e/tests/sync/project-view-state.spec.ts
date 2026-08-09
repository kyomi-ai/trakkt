import { test, expect, type Page } from '@playwright/test';
import * as crypto from 'crypto';
import {
  attachSyncProbe,
  createIssueViaUi,
  expectNoPanics,
  launchTwoClients,
  waitForSyncHandshake,
  type SyncProbe,
  type TwoClients,
} from './realtime-harness';

// TRA-10032 — the project detail page's Board and List tabs, in the same terms
// TRA-10030 settled its six sections in.
//
// `ProjectDetailPage`'s content closure reads six resources, so any of them
// settling reconstructs `ProjectDetailContent` and everything under it. The two
// view tabs are one level deeper than the sections TRA-10030 fixed: they are
// built by `ProjectDetailContent`'s `{move || active_view…}` child, whose own
// closure reads nothing but `active_view`. That extra level is exactly what
// made them look unaffected, and it does not help — a dynamic child's `rebuild`
// in `tachys` is an unconditional fresh `build()` followed by `unmount()` of
// the old one.
//
// `crates/trakkt-ui/src/pages/projects/project_detail.rs` has browser tests
// (`view_tab_rebuild_tests`) that prove the mechanism on the real components in
// five seconds. What they cannot prove is that the *production* nesting is the
// one they model: they mount a shim in `ProjectDetailContent`'s place. These
// two tests close that gap by driving the real page with a real sync frame.
//
// Each waits for window A's change to be *visible in B* before asserting, so
// there is no timing window to be lucky in — the same discipline as
// `milestone-realtime.spec.ts`, and for the same reason: a test that races the
// frame it is measuring reports flake instead of the defect.

let clients: TwoClients;
let pageA: Page;
let pageB: Page;
let probeB: SyncProbe;

/** `/projects/{id}` of the project both windows work in, created in setup. */
let projectHref: string;
/** Its name, so the issue sidebar picks THIS project and not a previous run's. */
let projectName: string;
const rand = () => crypto.randomBytes(4).toString('hex');

/**
 * U+25BE, the chevron `DropdownTrigger` renders after its label-or-value text.
 *
 * It lives in a `<span>` *inside* the `<button>` and is not `aria-hidden`, so it
 * is part of the button's accessible name — the group-by trigger is named
 * "Group ▾" and "Status ▾", never "Group" or "Status" on their own. Naming it
 * here rather than inlining the glyph gives the next person who changes it one
 * place to look: an exact-name match is what makes the grouping assertions
 * below discriminate, so the chevron has to be spelled out rather than matched
 * around, and a silent change to it would otherwise resurface as a test that
 * fails at its setup barrier with no hint why.
 * See `crates/trakkt-ui/src/components/dropdown.rs`.
 */
const TRIGGER_CHEVRON = '▾';

/** The `DropdownTrigger` button currently reading `text`, and nothing else. */
const triggerNamed = (page: Page, text: string) =>
  page.getByRole('button', { name: `${text} ${TRIGGER_CHEVRON}`, exact: true });

// Signup, two cold WASM boots and an issue created through the real UI all sit
// inside single tests here.
test.setTimeout(180_000);
test.describe.configure({ mode: 'serial' });

test.afterEach(async () => {
  expectNoPanics(clients);
});

/**
 * Create an issue from `page` and put it in the shared project.
 *
 * The project has to be set from the issue detail sidebar rather than at
 * creation time, the same route `milestone-realtime.spec.ts` takes: `/issues`
 * redirects to `/my-issues`, which has no create button, and the create modal
 * does not offer a project.
 *
 * The point of creating an issue rather than, say, a milestone is that an issue
 * lands in the `SyncStore`, which is what `ProjectDetailPage`'s `project_issues`
 * is derived from — so it re-runs the content closure *and* is visible on both
 * view tabs, which is what makes the barrier below possible.
 */
async function createIssueInProject(page: Page, title: string) {
  // The creation half is `createIssueViaUi` — it was inline here until TRA-9997
  // extracted it. Only the project assignment below is this suite's own.
  const href = await createIssueViaUi(page, title);

  await page.goto(href);
  await page.waitForLoadState('networkidle');
  await page.getByRole('button', { name: /Set project/ }).click();
  // Exact name: a rerun against the same database leaves earlier runs' projects
  // in this dropdown.
  await page.getByRole('option', { name: projectName, exact: true }).click();
  await expect(page.getByText(projectName, { exact: true }).first()).toBeVisible({
    timeout: 20_000,
  });
}

test.beforeAll(async () => {
  clients = await launchTwoClients();
  pageA = clients.pageA;
  pageB = clients.pageB;

  // Only B's socket is watched: every assertion here is about what B does with
  // a frame, and `waitForSyncHandshake` needs a probe attached before B's first
  // navigation because `page.on('websocket')` only reports sockets opened after
  // it is registered.
  probeB = attachSyncProbe(pageB);

  await pageA.goto('/projects');
  await pageA.waitForLoadState('networkidle');
  await pageA.getByRole('button', { name: 'New Project' }).first().click();
  projectName = `View state ${rand()}`;
  await pageA.getByPlaceholder('e.g. Q3 Launch').fill(projectName);
  await pageA.getByRole('button', { name: 'Create Project' }).click();
  await pageA.waitForURL(/\/projects\/[^/]+$/, { timeout: 30_000 });
  projectHref = new URL(pageA.url()).pathname;

  clients.panics.length = 0;
});

test.afterAll(async () => {
  await clients?.browser?.close();
});

test("a colleague's new issue does not clear the board filter you typed", async () => {
  const token = rand();
  const title = `Board ${token}`;

  await pageB.goto(`${projectHref}?view=board`);
  const filter = pageB.getByPlaceholder('Filter cards...');
  await expect(filter).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');

  await filter.fill(token);
  await expect(filter, 'the filter has to hold the token before anything is measured').toHaveValue(
    token,
  );

  await createIssueInProject(pageA, title);

  // The barrier. This card is on B's board under either outcome — if the filter
  // survived it matches `token`, and if the filter was cleared every card shows
  // — so waiting for it does not presuppose the answer. Once it is here, B has
  // processed the frame and the rebuild has already happened or never will.
  await expect(pageB.getByText(title, { exact: true })).toBeVisible({ timeout: 60_000 });

  await expect(
    filter,
    "another window's issue rebuilt the board and took the filter with it. The filter has to \
be owned by `ProjectDetailPage` — see `BoardViewState` and `ProjectEditState`",
  ).toHaveValue(token);
});

test("a colleague's new issue does not undo the grouping you chose on the list", async () => {
  const token = rand();
  const title = `List ${token}`;

  await pageB.goto(`${projectHref}?view=list`);
  const groupTrigger = triggerNamed(pageB, 'Group');
  await expect(groupTrigger).toBeVisible({ timeout: 30_000 });
  await waitForSyncHandshake(probeB, 'B');

  await groupTrigger.click();
  await pageB.getByRole('option', { name: 'Status', exact: true }).click();

  // `DropdownTrigger` shows the chosen value in place of its label, so the
  // button is named "Status ▾" exactly while the grouping is in force and
  // "Group ▾" exactly while it is not — `ListViewState::DEFAULT_GROUP_BY` is
  // `GroupBy::None`, whose label never reaches the trigger. That is the whole
  // assertion, in both directions: losing the grouping renames this button, so
  // an exact match on the grouped name cannot pass a rebuilt list.
  const groupedTrigger = triggerNamed(pageB, 'Status');
  await expect(groupedTrigger, 'the grouping has to be in force before anything is measured')
    .toBeVisible();

  await createIssueInProject(pageA, title);

  // Same barrier as above: the new row appears whether or not the grouping
  // survived.
  await expect(pageB.getByText(title, { exact: true })).toBeVisible({ timeout: 60_000 });

  await expect(
    groupedTrigger,
    "another window's issue rebuilt the list and put the grouping back to \"No grouping\". It \
has to be owned by `ProjectDetailPage` — see `ListViewState` and `ProjectEditState`",
  ).toBeVisible();
});
