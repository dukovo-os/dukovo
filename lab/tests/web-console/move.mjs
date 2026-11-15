// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/move drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Move to new
// hardware page (design 6) at /move. It reads what `backup find --json` says
// (`move.find`, `GET /api/1/move/sources`) and what `backup restore --machine
// --plan --json` says (`move.plan`, `POST /api/1/move/plan`, a POST because it
// takes the recovery key), stages the restore (`move.stage`, `POST
// /api/1/move/stage`, a task) and cancels what was staged (`move.cancel`,
// `POST /api/1/move/cancel`). The machine has backups of itself on a USB stick,
// and the place's key as a file on the stick named after the machine, which
// move.toml makes; a second place gives a key that is valid and is not the
// backups'. The key is typed: it goes in the body of `move.plan` and `move.stage`
// and in no other request, and the field is empty once the plan is shown, so
// Stage it asks for it again. With the key file chosen by itself the path goes
// instead, and the key is never typed. A wrong key is refused under its field
// (BAK-103). A restore staged first by another tab, while the page shows Stage
// it, makes the page's own stage a task that fails (BAK-104), and the page shows
// that one as Staged, with Cancel. A restore that is staged is shown as Staged
// when the page opens, with Stage it off. A console that is
// locked is asked to unlock before the disks are read; a reader is told the page
// is for administrators; a phone at 390 by 844 has one column and controls a
// thumb reaches. Each look is the console's own: dark, light, high contrast,
// forced colours and less motion, each checked by axe (console.mjs's lookAll).
// Nothing is staged when a run ends, so that the next engine finds the machine
// as this one did. The restart that the last step names is not pressed: the
// boot test admin-move restarts a second machine and finds the first one's name,
// accounts and network. The accounts are the two that move.toml made.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustHaveCalled,
  openConsole,
  press,
  signIn,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, sendsNothing, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-move";
const READER = "wc-move-reader";

// The disk that move.toml made, and the two backup places it made: the one
// that holds this machine's backups, and the one whose key is the wrong one.
const DISK = "LABMOVE";
const PLACE = "lab-move";
const WRONG = "lab-wrong";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end.
const ROUTES = {
  sources: "GET /api/1/move/sources",
  plan: "POST /api/1/move/plan",
  stage: "POST /api/1/move/stage",
  cancel: "POST /api/1/move/cancel",
};
const SOURCES = "/api/1/move/sources";
const PLAN = "/api/1/move/plan";
const STAGE = "/api/1/move/stage";
const CANCEL = "/api/1/move/cancel";
const SHOW = "/api/1/show";
const BACKUPS = "/api/1/backups";
const KEY_OF = (place) => `/api/1/backups/${encodeURIComponent(place)}/key`;

// The words of the page, whole.
const HEADING = "Move to new hardware";
const NOTHING_CHOSEN = "Choose the backup to move from.";
const NOT_A_PATH = "Write the whole path, starting with /.";
const LOCKED_WORDS = "Unlock the console to look for backups on the plugged-in disks.";
const NOT_KEPT = "The key is not kept.";
const STAGED_WORDS = "Nothing changes until this machine restarts, and you can cancel until then.";
const RESTART = "elevate reboot";
const ADMINS_ONLY = "This page is for administrators.";

// What the browsers called, over all of them, for the check at the end.
const called = new Set();
const remember = (seen) => {
  for (const route of seen) called.add(route);
};

// --- Reading the machine ---

/** What `GET /api/1/move/sources` answers: `backup find --json`, `{sources, keys}`. */
async function readSources(page) {
  const answer = await fetchAsPage(page, "GET", SOURCES);
  ensure(answer.status === 200 && Array.isArray(answer.json?.sources) && Array.isArray(answer.json?.keys), `GET ${SOURCES} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The backups that `backup find` found on the stick, and the key file that is named after the machine. */
async function stick(page) {
  const found = await readSources(page);
  const source = found.sources.find((one) => one.volume === DISK);
  ensure(source !== undefined, `backup find lists no backups on ${DISK}: ${JSON.stringify(found.sources)}`);
  const key = found.keys.find((one) => one.path === `/volumes/${DISK}/${source.machine}-recovery.key`);
  ensure(key !== undefined, `backup find lists no key file ${source.machine}-recovery.key on ${DISK}: ${JSON.stringify(found.keys)}`);
  return { source: source.source, machine: source.machine, key: key.path };
}

/** What `GET /api/1/backups` answers: the places, and `staged` when a restore is staged. */
async function readBackups(page) {
  const answer = await fetchAsPage(page, "GET", BACKUPS);
  ensure(answer.status === 200 && Array.isArray(answer.json?.places), `GET ${BACKUPS} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The recovery key of the place `name`, as `backups.key` answers it. */
async function keyOf(page, name) {
  const answer = await fetchAsPage(page, "POST", KEY_OF(name), {});
  const key = /AGE-SECRET-KEY-1[0-9A-Z]+/.exec(answer.text)?.[0];
  ensure(answer.status === 200 && key !== undefined, `${KEY_OF(name)} was answered ${answer.status} with no key`);
  return key;
}

/** Takes away what is staged, if anything is, and says that nothing is. */
async function nothingStaged(page) {
  await fetchAsPage(page, "POST", CANCEL, {});
  ensure((await readBackups(page)).staged === null, "a restore is still staged after Cancel");
}

/** The task `id` once it has ended, as `GET /api/1/tasks/{id}` gives it, waiting up to `seconds`. */
async function taskEnd(page, id, seconds) {
  let task = null;
  await waitUntil(async () => {
    const answer = await fetchAsPage(page, "GET", `/api/1/tasks/${id}`);
    ensure(answer.status === 200, `GET /api/1/tasks/${id} was answered ${answer.status}: ${answer.text}`);
    task = answer.json;
    return task.state !== "waiting" && task.state !== "running";
  }, `task ${id}'s end`, seconds);
  return task;
}

/** Polls until `test()` holds, for up to `seconds`. */
async function waitUntil(test, what, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  while (!(await test())) {
    ensure(Date.now() < until, `${what} did not happen within ${seconds} seconds`);
    await sleep(500);
  }
}

// --- Watching what the page sends ---

/** The bodies of the requests that `page` makes, `{route, body}`, for what a key must not be found in. */
function watchBodies(page) {
  const sent = [];
  page.on("request", (request) => {
    const body = request.postData();
    if (body !== null) sent.push({ route: `${request.method()} ${new URL(request.url()).pathname}`, body });
  });
  return sent;
}

/** The requests of `sent` to `path`. */
const toPath = (sent, path) => sent.filter((one) => one.route === `POST ${path}`);

/** Throws unless `key` is in the body of no request but those to `allowed`, and not in the page. */
async function keyOnlyIn(page, sent, key, allowed) {
  const leaked = sent.filter((one) => one.body.includes(key) && !allowed.includes(one.route.replace(/^POST /, "")));
  ensure(leaked.length === 0, `the recovery key was sent to ${leaked.map((one) => one.route).join(", ")}`);
  ensure(!(await page.content()).includes(key), "the recovery key is in the page");
}

// --- Signing in ---

/** Opens the console and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await leaveWelcome(t, page);
  return { page, seen };
}

// --- The page ---

/** The panel of the step `id`: `find`, `unlock`, `check` or `restart`, and the one that is open. */
const panel = (page, id) => page.locator(`[data-step="${id}"]`);
const openPanel = (page, id) => page.locator(`[data-step="${id}"]:not([hidden])`);
const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
const keyField = (scope) => scope.getByLabel("Recovery key", { exact: true });

/** Opens /move with `query`, and waits for the page to have read the places and the disks. */
async function openMove(t, page, query = "") {
  await page.goto(new URL(`/move${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: HEADING, level: 1, exact: true }));
  await t.see(page.locator(".mv-list"));
  await page.waitForFunction(() => !document.querySelector('[data-step="find"]')?.textContent.includes("Looking for backups"), null, { timeout: 30_000 });
}

/** Whether the step `id` is the open one. */
const isOpen = async (page, id) => (await openPanel(page, id).count()) === 1 && (await page.locator('[data-step]:not([hidden])').count()) === 1;

/** Chooses the stick in step 1 and goes on. */
async function chooseStick(t, page, machine) {
  const find = openPanel(page, "find");
  const radio = find.getByRole("radio", { name: new RegExp(`(USB disk|Disk) ${DISK}`) });
  await t.see(radio);
  ensure((await find.innerText()).includes(machine), `step 1 does not name ${machine} for ${DISK}`);
  await radio.check();
  await buttonOf(find, "Continue").click();
  await t.see(openPanel(page, "unlock"));
}

/** The first step's own words: nothing chosen, a folder that is not a path, and Look again. */
async function findStep(t, page) {
  await openMove(t, page);
  const find = openPanel(page, "find");
  await t.see(find.getByRole("heading", { name: "Find the backup", exact: true }));
  // Continue says it is off until a backup is chosen; pressed as a person
  // may (Playwright would wait for it to come on), it says why.
  await buttonOf(find, "Continue").click({ force: true });
  await t.see(find.getByText(NOTHING_CHOSEN));

  await find.getByRole("radio", { name: /^A folder…/ }).check();
  await find.getByLabel("Folder", { exact: true }).fill("backups/there");
  await buttonOf(find, "Continue").click();
  await t.see(find.getByText(NOT_A_PATH));
  ensure(await isOpen(page, "find"), "a folder that is not a path went on");

  const again = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === SOURCES);
  await buttonOf(find, "Look again").click();
  ensure((await again).status() === 200, `Look again was not answered 200`);
  await lookAll(t, page, "move-find");
}

/**
 * Step 2 with the key typed: a key that is not the backups' is refused under
 * its field, and the right key opens them; the key is in the body of `move.plan`
 * alone, as a key and with no file, and is gone from the field after.
 */
async function typedKey(t, page, sent, { machine }, keys) {
  await openMove(t, page);
  await chooseStick(t, page, machine);
  const unlock = openPanel(page, "unlock");
  const file = unlock.getByRole("radio", { name: /^Recovery key from the .* stick/ });
  await t.see(file);
  ensure(await file.isChecked(), `the key file named after ${machine} is not chosen by itself`);
  await t.see(unlock.getByText(NOT_KEPT, { exact: false }));
  await lookAll(t, page, "move-key");

  // A key that is valid and is not the backups': refused, and the field is empty again.
  await keyField(unlock).fill(keys.wrong);
  ensure(!(await file.isChecked()), "a typed key did not take the choice of the file away");
  let answered = await press(page, buttonOf(unlock, "Unlock it"), PLAN);
  ensure(answered.status() >= 400 && answered.status() < 500, `a key that is not the backups' was answered ${answered.status()}`);
  await t.see(unlock.getByText(/does not open these backups|BAK-103/).first());
  ensure((await keyField(unlock).inputValue()) === "", "a key that was refused is still in its field");
  ensure(await isOpen(page, "unlock"), "a wrong key went on to the next step");

  // The right key.
  await keyField(unlock).fill(keys.right);
  answered = await press(page, buttonOf(unlock, "Unlock it"), PLAN);
  ensure(answered.status() === 200, `${ROUTES.plan} was answered ${answered.status()} for the right key`);
  const plan = await answered.json();
  await t.see(openPanel(page, "check"));
  // The step is hidden now, its field with it.
  const unlocked = page.locator('[data-step="unlock"]');
  ensure((await keyField(unlocked).inputValue()) === "", "the key is still in its field after Unlock it");
  const bodies = toPath(sent, PLAN).map((one) => one.body);
  ensure(bodies.at(-1).includes(keys.right) && !bodies.at(-1).includes("key_file"), "the body of the plan has not the typed key alone");
  ensure(plan.machine === machine && Array.isArray(plan.accounts), `the plan is not of ${machine}: ${JSON.stringify(plan)}`);
  // The room is the root disk's, where the restore is staged, not that of the kernel's empty /.
  ensure(Number.isInteger(plan.free) && plan.free > 64 * 2 ** 20, `the plan gives ${plan.free} bytes free, not the root's`);
  await keyOnlyIn(page, sent, keys.right, [PLAN]);
  return plan;
}

/** Step 3 held to the plan the machine gave: what comes across, what differs, what happens after the restart, and the commands. */
async function checkStep(t, page, sent, plan) {
  const check = openPanel(page, "check");
  await t.see(check.getByRole("heading", { name: "What comes across", exact: true }));
  await t.see(check.getByText("Name, accounts and passwords", { exact: true }));
  for (const name of plan.accounts) ensure((await check.innerText()).includes(name), `step 3 does not name the account ${name}`);
  await t.see(check.getByText("Older snapshots", { exact: true }));
  await t.see(check.getByText("Not copied", { exact: true }));
  await t.see(check.getByRole("heading", { name: "Differences in the hardware", exact: true }));
  ensure(plan.record_format === 2 && plan.hardware !== null, `the backups are not of record format 2: ${plan.record_format}`);
  await t.see(check.getByText("Network ports", { exact: true }));
  await t.see(check.getByText("Disks", { exact: true }));
  await t.see(check.getByText("Processor and graphics", { exact: true }));
  await t.see(check.getByRole("heading", { name: "After the restart", exact: true }));
  await t.see(check.getByText(`Turn the old ${plan.machine} off before this machine restarts.`));
  await t.see(check.getByText(new RegExp(`After the restart, open https://${plan.machine}\\.local`)));

  // The command of Stage it, and the restart's, come once the choices stop changing.
  await t.see(check.getByRole("group", { name: "The command that restarts this machine" }).getByText(RESTART, { exact: true }));
  await waitUntil(() => toPath(sent, SHOW).length > 0, `a Show command for Stage it`);
  ensure(toPath(sent, SHOW).every((one) => !one.body.includes("AGE-SECRET-KEY-")), "a recovery key was sent to the Show command panel");
  await lookAll(t, page, "move-check");
}

/** The choice of each old card's address: with a typed key the plan stays, and with a key file it is read again with the choice. */
async function cards(t, page, sent, plan, how) {
  if (!Array.isArray(plan.cards) || plan.cards.length === 0) return;
  const check = openPanel(page, "check");
  const before = toPath(sent, PLAN).length;
  const automatic = check.getByRole("radio", { name: "Let it get an address automatically" }).first();
  await t.see(automatic);
  const again = how === "file" ? page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === PLAN) : null;
  await automatic.check();
  if (again !== null) {
    ensure((await again).status() === 200, "the plan was not read again with the choice of a card");
    const body = toPath(sent, PLAN).at(-1).body;
    ensure(body.includes("automatic") && body.includes("cards"), `the plan was read again without the card's choice: ${body}`);
  } else {
    await sleep(1500);
    ensure(toPath(sent, PLAN).length === before, "the plan was read again, and the typed key is not there to read it with");
  }
  await t.see(check.getByText(/1 change for the new network port/));
}

/** Step 4 with a typed key: Stage it asks for the key again, runs as a task, and ends as Staged with Cancel. */
async function stageTyped(t, page, sent, plan, keys) {
  await unlockNow(page);
  await buttonOf(openPanel(page, "check"), "Continue").click();
  const restart = openPanel(page, "restart");
  await t.see(restart.getByRole("heading", { name: "Restart to finish", exact: true }));
  await t.see(keyField(restart));
  ensure(await sendsNothing(page, buttonOf(restart, "Stage it"), STAGE), "Stage it sent a request with no key typed");
  await t.see(restart.getByText(/Type the recovery key/));

  await keyField(restart).fill(keys.right);
  const answered = await press(page, buttonOf(restart, "Stage it"), STAGE);
  ensure(answered.status() === 202 || answered.status() === 200, `${ROUTES.stage} was answered ${answered.status()}`);
  const body = toPath(sent, STAGE).at(-1).body;
  ensure(body.includes(keys.right) && !body.includes("key_file") && body.includes(plan.at), `the body of Stage it is not the typed key and the time that was checked: ${body.replace(keys.right, "KEY")}`);
  await restart.getByText(/^Staged/).first().waitFor({ timeout: 300_000 });
  await t.see(restart.getByText(STAGED_WORDS));
  await t.see(restart.getByRole("group", { name: "The command that restarts this machine" }).getByText(RESTART, { exact: true }));
  ensure((await restart.getByRole("button", { name: "Stage it", exact: true }).count()) === 0, "Stage it is there while a restore is staged");
  ensure((await restart.getByRole("button", { name: "Restart", exact: true }).count()) === 0, "the page has a Restart button, which the Hardware page brings");
  ensure((await readBackups(page)).staged !== null, "the machine says nothing is staged after Stage it");
  await keyOnlyIn(page, sent, keys.right, [PLAN, STAGE]);
  await lookAll(t, page, "move-staged");

  // Cancel takes it away, and Stage it is there again.
  const cancelled = await press(page, buttonOf(restart, "Cancel"), CANCEL);
  ensure(cancelled.status() === 200, `${ROUTES.cancel} was answered ${cancelled.status()}`);
  await t.see(buttonOf(openPanel(page, "restart"), "Stage it"));
  ensure((await readBackups(page)).staged === null, "a restore is still staged after Cancel");
}

/** The key file by itself: chosen, sent as a path and no key, never asked for again; a restore staged first by another tab is shown when Stage it fails; and one that is staged is Staged on opening. */
async function fileKey(t, page, sent, { source, machine, key }) {
  await unlockNow(page);
  await openMove(t, page, `?source=${encodeURIComponent(source)}`);
  const unlock = openPanel(page, "unlock");
  const file = unlock.getByRole("radio", { name: /^Recovery key from the .* stick/ });
  ensure(await file.isChecked(), `the key file named after ${machine} is not chosen by itself`);
  const answered = await press(page, buttonOf(unlock, "Unlock it"), PLAN);
  ensure(answered.status() === 200, `${ROUTES.plan} was answered ${answered.status()} with the key file`);
  const plan = await answered.json();
  const body = toPath(sent, PLAN).at(-1).body;
  ensure(body.includes(key) && !body.includes("AGE-SECRET-KEY-"), `the body of the plan is not the key file's path alone: ${body}`);
  await t.see(openPanel(page, "check"));
  await cards(t, page, sent, plan, "file");

  await buttonOf(openPanel(page, "check"), "Continue").click();
  const restart = openPanel(page, "restart");
  ensure((await restart.locator("input").count()) === 0, "a key is asked for again when a key file was chosen");
  // Another tab stages the restore while this one shows Stage it.
  const first = await fetchAsPage(page, "POST", STAGE, { source, key_file: key });
  ensure(first.status === 202 && Number.isInteger(first.json?.task), `a stage from another tab was answered ${first.status}: ${first.text}`);
  const firstEnd = await taskEnd(page, first.json.task, 300);
  ensure(firstEnd.state === "done", `a stage from another tab ended ${firstEnd.state}: ${firstEnd.note}`);

  // This one's stage is a task that fails, as one is staged already (BAK-104), and the page shows that one, with Cancel.
  const staged = await press(page, buttonOf(restart, "Stage it"), STAGE);
  ensure(staged.status() === 202, `${ROUTES.stage} was answered ${staged.status()} with the key file`);
  ensure(toPath(sent, STAGE).at(-1).body.includes(key), "the body of Stage it has not the key file's path");
  const refused = await taskEnd(page, (await staged.json()).task, 300);
  ensure(refused.state === "failed" && String(refused.note).includes("BAK-104"), `a second stage ended ${refused.state}, not with BAK-104: ${refused.note}`);
  await restart.getByText(/^Staged/).first().waitFor({ timeout: 30_000 });
  await t.see(buttonOf(restart, "Cancel"));
  ensure(!(await restart.textContent()).includes("Nothing is staged"), "the page says nothing is staged while the first restore is");
  ensure((await readBackups(page)).staged !== null, "the restore staged first is gone after a second was refused");

  // Opened again, the page says Staged, with Cancel and no Stage it.
  await openMove(t, page);
  const again = openPanel(page, "restart");
  await t.see(again.getByText(/^Staged/).first());
  await t.see(buttonOf(again, "Cancel"));
  ensure((await again.getByRole("button", { name: "Stage it", exact: true }).count()) === 0, "Stage it is on while a restore is staged");
  await press(page, buttonOf(again, "Cancel"), CANCEL);
  ensure((await readBackups(page)).staged === null, "a restore is still staged after Cancel");
}

/** The backup place by its name: the page opens at the key, and the key file made for the place is chosen. */
async function fromPlace(t, page) {
  await openMove(t, page, `?from=${PLACE}`);
  const unlock = openPanel(page, "unlock");
  await t.see(unlock.getByRole("heading", { name: "Unlock it", exact: true }));
  ensure(await unlock.getByRole("radio", { name: /^Recovery key from the .* stick/ }).isChecked(), `the key made for ${PLACE} is not chosen by itself`);
  const answered = await press(page, buttonOf(unlock, "Unlock it"), PLAN);
  ensure(answered.status() === 200, `${ROUTES.plan} was answered ${answered.status()} for ${PLACE}`);
  await t.see(openPanel(page, "check"));
}

/** A console that is locked is asked to unlock before the disks are read. */
async function locked(t, page) {
  const lock = await fetchAsPage(page, "POST", "/api/1/lock");
  ensure(lock.status === 200, `the console was not locked: ${lock.status} ${lock.text}`);
  await sleep(1500);
  await page.goto(new URL("/move", page.url()).href);
  await t.see(page.getByRole("heading", { name: HEADING, level: 1, exact: true }));
  const find = openPanel(page, "find");
  await t.see(find.getByText(LOCKED_WORDS));
  await buttonOf(find, "Unlock").click();
  await t.see(unlockDialog(page));
  await answerUnlock(t, page);
  await t.see(find.getByRole("radio", { name: new RegExp(`(USB disk|Disk) ${DISK}`) }));
}

async function administrator(t) {
  const { page, seen } = await signedIn(t, ADMIN);
  const sent = watchBodies(page);
  await unlockNow(page);
  await nothingStaged(page);
  const where = await stick(page);
  const keys = { right: await keyOf(page, PLACE), wrong: await keyOf(page, WRONG) };
  ensure(keys.right !== keys.wrong, "the two places have the same key");

  await findStep(t, page);
  const plan = await typedKey(t, page, sent, where, keys);
  await checkStep(t, page, sent, plan);
  await cards(t, page, sent, plan, "typed");
  await stageTyped(t, page, sent, plan, keys);

  await fileKey(t, page, sent, where);
  await fromPlace(t, page);
  await nothingStaged(page);
  await locked(t, page);
  await nothingStaged(page);
  ensure(!sent.some((one) => one.route === `POST ${SHOW}` && one.body.includes(keys.wrong)), "a recovery key was sent to the Show command panel");

  remember(seen);
  mustHaveCalled(seen, [ROUTES.sources, ROUTES.plan, ROUTES.stage, ROUTES.cancel]);
  await signOut(t, page);
  await page.context().close();
}

// --- The reader and the phone ---

/** A reader is told that the page is for administrators, and has nothing to press. */
async function reader(t) {
  const { page, seen } = await signedIn(t, READER);
  await page.goto(new URL("/move", page.url()).href);
  await t.see(page.getByText(ADMINS_ONLY));
  for (const name of ["Stage it", "Unlock it", "Cancel"]) ensure((await buttonOf(page, name).count()) === 0, `a reader's page has ${name}`);
  await lookAll(t, page, "move-reader");
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: one column, no sideways scroll, controls of 44 px, and the command in a box that scrolls inside itself. */
async function phone(t) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);
  const where = await stick(page);
  const controls = "main .mv button, main .mv .radio-card, main .mv a.btn";
  const flat = async (what) => {
    ensure((await sidewaysScroll(page)) === 0, `the phone's ${what} scrolls ${await sidewaysScroll(page)} px sideways`);
    const small = await smallControls(page, controls);
    ensure(small.length === 0, `these controls of the phone's ${what} are under 44 px tall: ${small.join(", ")}`);
  };

  await openMove(t, page);
  await flat("first step");
  await lookAll(t, page, "move-phone-find");
  await openMove(t, page, `?source=${encodeURIComponent(where.source)}`);
  await flat("key step");
  await buttonOf(openPanel(page, "unlock"), "Unlock it").click();
  await t.see(openPanel(page, "check"));
  await flat("check");
  await lookAll(t, page, "move-phone-check");
  await buttonOf(openPanel(page, "check"), "Continue").click();
  await t.see(openPanel(page, "restart"));
  await buttonOf(openPanel(page, "restart"), "Stage it").click();
  await openPanel(page, "restart").getByText(/^Staged/).first().waitFor({ timeout: 300_000 });
  // The note that the stage runs goes when it ends, so that it covers nothing on a phone.
  await waitUntil(async () => (await page.getByText(/: running as task \d+$/).count()) === 0, "the note that the stage runs to go", 15);
  // The mouse rests where Stage it was, under the notes, and would hold the time of the one that the stage
  // ended; a phone has no mouse, so it is moved off, and that note goes after its 8 seconds.
  await page.mouse.move(0, 0);
  await flat("staged step");
  await lookAll(t, page, "move-phone-staged");
  await buttonOf(openPanel(page, "restart"), "Cancel").click();
  await t.see(buttonOf(openPanel(page, "restart"), "Stage it"));
  await nothingStaged(page);
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
  // Everything the table names was called by some browser.
  const missing = Object.values(ROUTES).filter((route) => !called.has(route));
  ensure(missing.length === 0, `no browser called ${missing.join(", ")}`);
}
