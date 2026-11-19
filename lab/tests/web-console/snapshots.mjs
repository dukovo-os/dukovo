// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/snapshots drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Snapshots
// and backups page (design 5) at /snapshots, /snapshots/backups, one backup
// place at /snapshots/backups/{name} and a snapshot's folder at
// /snapshots/browse. The page shows what `snap list --json` and `backup list
// --json` say (`GET /api/1/snapshots` and `GET /api/1/backups`): the banner
// for a backup that failed, with Back up now and Alert settings (a link to
// /alerts), What is protected, the snapshots by day and the
// backup places. Take a snapshot now (`snap create`, `POST
// /api/1/snapshots/take`), Browse (`snap browse`, `GET /api/1/snapshots/browse`
// and, for an administrator outside their home, `GET
// /api/1/snapshots/browse/all`) and Restore N chosen (`snap restore --beside`,
// `POST /api/1/snapshots/restore` and `POST /api/1/snapshots/restore/all`) work on
// the snapshots; Add a backup place (`backup add`, `POST /api/1/backups/add`,
// whose answer holds the recovery key), Back up now (`backup run`, `POST
// /api/1/backups/{name}/run`, a task), Check now (`backup verify --restore`,
// `POST /api/1/backups/{name}/verify`, a task), Show it again (`backup key`,
// `POST /api/1/backups/{name}/key`, a POST because its answer is a secret),
// Restore files… (`backup restore --place`, `POST /api/1/backups/{name}/restore`,
// a task), the SSH key a server is given (`backup ssh-key`, `GET
// /api/1/backups/ssh-key`), what a place holds (`backup show`, `GET
// /api/1/backups/{name}`) and Remove… (`backup remove`, `POST
// /api/1/backups/{name}/remove`) work on the backup places.
//
// The machine has a backup place to an SSH address that does not answer, which
// snapshots.toml makes and a Back up now to it fails, and a USB stick with a
// Btrfs volume on it, which this test adds as a backup place from the page. The
// engines run one after another on one machine, so each has a dead place, a
// volume, two files of the reader's and a snapshot name of its own (`namesOf`),
// and the steps after the browser remove those of every engine, also of one that
// was skipped. The browser is in America/New_York: a snapshot's label, "Nov 13
// 10:02 PM", is the time on that clock, and the files that Restore N chosen puts
// beside the current ones have it in their names, which snapshots.toml finds on
// the machine after. An administrator restores a file of another account; a
// reader restores a file of their own and sees no button that changes anything.
// The recovery key is shown once, saved as a file, and the file, the key shown
// again after the console was locked and unlocked, and the key that
// `backups.key` answers are the same. A phone at 390 by 844 shows the list and a
// snapshot's folder. Each look is the console's own: dark, light, high contrast,
// forced colours and less motion, each checked by axe (console.mjs's lookAll).
// The accounts are the two that snapshots.toml made, so no script needs the
// lab's password.

import { readFile } from "node:fs/promises";

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustHaveCalled,
  openConsole,
  signIn,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-snap";
const READER = "wc-snap-reader";

// The browser's clock and the account's: the labels of restored files are the
// snapshot's time in it.
const ZONE = "America/New_York";

// The snapshot that snapshots.toml made of /users with the reader's files in
// it, and the folder they were in. The snapshot is the same for each engine,
// which only reads it.
const BEFORE = "lab-before";
const HOME = `/users/${READER}`;

/**
 * The names `engine` makes and removes. The engines run one after another on
 * one machine and leave what they made, so each has names of its own: the
 * place that does not answer, which snapshots.toml made; the place on the
 * stick and its volume, which snapshots.toml mounted (a folder that holds
 * backups of another key is refused, so two places cannot share one); the
 * snapshot that Take a snapshot now makes; and the two files of the reader's
 * in BEFORE (the copy that Restore N chosen puts beside a file is refused
 * where one of that name is).
 */
function namesOf(engine) {
  return {
    dead: `lab-nas-${engine}`,
    stick: `lab-stick-${engine}`,
    volume: `/volumes/LABBACKUP-${engine}`,
    take: `lab-page-take-${engine}`,
    forAdmin: `notes-${engine}.txt`,
    forReader: `plans-${engine}.txt`,
  };
}

/** The path that Restore N chosen gives the copy of `file` beside it, for the snapshot made at `label`. */
const besideOf = (file, label) => `${HOME}/${file.replace(/\.txt$/, "")} (${label}).txt`;

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with the
// name of the place for {name}.
const ROUTES = {
  list: "GET /api/1/snapshots",
  browse: "GET /api/1/snapshots/browse",
  browseAll: "GET /api/1/snapshots/browse/all",
  take: "POST /api/1/snapshots/take",
  restore: "POST /api/1/snapshots/restore",
  restoreAll: "POST /api/1/snapshots/restore/all",
  backups: "GET /api/1/backups",
  show: "GET /api/1/backups/{name}",
  run: "POST /api/1/backups/{name}/run",
  verify: "POST /api/1/backups/{name}/verify",
  add: "POST /api/1/backups/add",
  sshKey: "GET /api/1/backups/ssh-key",
  remove: "POST /api/1/backups/{name}/remove",
  key: "POST /api/1/backups/{name}/key",
  restoreFrom: "POST /api/1/backups/{name}/restore",
};

/** The path of a route as the table names it, for the place `name`. */
const pathOf = (route, name = "") => route.slice(route.indexOf(" ") + 1).replace("{name}", encodeURIComponent(name));

/** The route as `track` writes it, for the place `name`. */
const routeFor = (route, name = "") => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, name)}`;

// What the browsers called, over all of them, for the check at the end.
const called = new Set();
const remember = (seen) => {
  for (const route of seen) called.add(route);
};

// --- The words of the page ---

const READER_WORDS = "Administrators see what a backup place holds.";
const KEPT_WORDS = "I have kept it";

/**
 * The label that Restore N chosen puts in a restored file's name for a
 * snapshot made at `ms`, as the console writes it: the day (with the year in
 * another year) and the clock of `zone`, "Nov 13 10:02 PM", in plain ASCII.
 */
function labelOf(ms, zone) {
  const plain = (text) => text.replace(/[  ]/g, " ");
  const year = (at) => new Intl.DateTimeFormat("en-US", { timeZone: zone, year: "numeric" }).format(at);
  const options = year(ms) === year(Date.now()) ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" };
  const day = new Intl.DateTimeFormat("en-US", { timeZone: zone, ...options }).format(ms);
  const clock = new Intl.DateTimeFormat("en-US", { timeZone: zone, hour: "numeric", minute: "2-digit" }).format(ms);
  return plain(`${day} ${clock}`);
}

// --- Reading the machine ---

/** What `GET /api/1/snapshots` answers, for one place when `place` is given. */
async function readSnapshots(page, place = null) {
  const route = place === null ? pathOf(ROUTES.list) : `${pathOf(ROUTES.list)}?place=${encodeURIComponent(place)}`;
  const answer = await fetchAsPage(page, "GET", route);
  ensure(answer.status === 200 && answer.json !== null, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** What `GET /api/1/backups` answers: `backup list --json`, `{places, staged}`. */
async function readBackups(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.backups));
  ensure(answer.status === 200 && Array.isArray(answer.json?.places), `${ROUTES.backups} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The backup place `name` of `backups`, or undefined. */
const placeOf = (backups, name) => backups.places.find((place) => place.name === name);

/** Reads the backup places until `test(place)` holds for `name`, for up to `seconds`, and gives the place. */
async function placeBecomes(page, name, test, what, seconds = 360) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const place = placeOf(await readBackups(page).catch(() => ({ places: [] })), name);
    if (place !== undefined && test(place)) return place;
    ensure(Date.now() < until, `${name} did not ${what} within ${seconds} seconds`);
    await sleep(2000);
  }
}

// --- Signing in ---

/** Opens the console in the browser's zone and signs in as `name`, with the account's zone the same. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", { timezoneId: ZONE, ...options });
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await leaveWelcome(t, page, ZONE);
  return { page, seen };
}

// --- The page ---

const protectedCard = (page) => page.locator("#protected");
const listCard = (page) => page.locator("#snapshots");
const placesCard = (page) => page.locator("#backups");
const banners = (page) => page.locator(".snap-banners .banner");
const buttonOf = (page, name) => page.getByRole("button", { name, exact: true });

/** Opens `path` under /snapshots, and waits for the three cards to be read. */
async function openSnapshots(t, page, path = "/snapshots") {
  await page.goto(new URL(path, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Snapshots and backups", level: 1, exact: true }));
  await page.waitForFunction(
    () => ["#protected", "#snapshots", "#backups"].every((id) => document.querySelector(id) !== null && !document.querySelector(id).hasAttribute("aria-busy")),
    null,
    { timeout: 30_000 },
  );
}

/** Opens the folder `folder` of the snapshot `snapshot` of `place`, and waits for its entries. */
async function openFolder(t, page, place, snapshot, folder) {
  const query = new URLSearchParams({ place, snapshot, folder });
  await page.goto(new URL(`/snapshots/browse?${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: folder, level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelector(".snap-table") !== null && !document.querySelector(".snap-table").hasAttribute("aria-busy"), null, { timeout: 30_000 });
}

/** What What is protected, the snapshots and the backup places say, held to what the machine's own reads say. */
async function cardsAsAnswered(t, page) {
  const list = await readSnapshots(page);
  const places = (list.places ?? []).map((place) => place.place);
  ensure(places.includes("/users") && places.includes("/shared"), `snap list names ${JSON.stringify(places)}, and not /users and /shared`);
  const rows = protectedCard(page).locator("tbody tr");
  ensure((await rows.count()) >= 2, "What is protected has fewer than two rows");
  await t.see(protectedCard(page).getByText("People's files", { exact: true }));
  await t.see(protectedCard(page).getByText("Shared folders and websites", { exact: true }));
  await t.see(protectedCard(page).getByText(/(Daily|Weekly|Newest) · /).first());
  // Each day's row has a Browse for each place.
  const browse = listCard(page).locator('a[href^="/snapshots/browse?"]');
  ensure((await browse.count()) >= 2, "the snapshots list has no Browse");
  await t.see(browse.first());
  return list;
}

// --- The backup that fails ---

/** A backup to an address that does not answer is a bad banner, with the last good one and what still protects, and a need on the Overview. */
async function failedBanner(t, page, names) {
  await openSnapshots(t, page);
  const banner = banners(page).filter({ hasText: `The last backup to ${names.dead} failed` });
  await t.see(banner);
  ensure((await banner.getAttribute("role")) === "alert", "the banner of a failed backup is not an alert");
  await t.see(banner.getByText(/Nothing has been backed up there yet\./));
  await t.see(banner.getByText(/Snapshots on this server are (current|not current either)/));
  await t.see(banner.getByRole("button", { name: "Back up now", exact: true }));
  // Alert settings is a link to the Alerts page, where the email and the webhooks are set.
  const settings = banner.getByRole("link", { name: "Alert settings", exact: true });
  await t.see(settings);
  ensure((await settings.getAttribute("href")) === "/alerts", `Alert settings of the banner goes to ${await settings.getAttribute("href")}, not to /alerts`);
  await t.see(placesCard(page).getByRole("row", { name: new RegExp(`^${names.dead}`) }).getByText("failed", { exact: true }));
  await t.see(protectedCard(page).getByText(/, failed/).first());

  await page.goto(new URL("/", page.url()).href);
  await listedInHealth(t, page, `backup-failed-${names.dead}`);
}

/**
 * The Overview's Health card lists the need `id`, in its list or, when the
 * places of the other engines fill the four rows it shows, in See all.
 */
async function listedInHealth(t, page, id) {
  const card = page.locator("#ov-health");
  const row = `[data-need="${id}"]`;
  await t.see(card.locator("ul.need"));
  if ((await card.locator(`ul.need ${row}`).count()) > 0) return;
  await card.getByRole("button", { name: /^See all \d+$/ }).click();
  const dialog = page.getByRole("dialog", { name: /^\d+ things to look at$/ });
  await t.see(dialog);
  await t.see(dialog.locator(row));
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
}

/** Back up now in the banner runs a task that fails again, and the button says how it goes until it ends. */
async function backUpFails(t, page, names) {
  await unlockNow(page);
  await openSnapshots(t, page);
  const before = placeOf(await readBackups(page), names.dead)?.state?.tried_ms ?? 0;
  const banner = banners(page).filter({ hasText: `The last backup to ${names.dead} failed` });
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.run, names.dead));
  await banner.getByRole("button", { name: "Back up now", exact: true }).click();
  const reply = await answered;
  ensure(reply.status() === 202 || reply.status() === 200, `${ROUTES.run} for ${names.dead} was answered ${reply.status()}`);
  // The task ends in a failure: the machine says so, and the banner is there again with its button.
  await placeBecomes(page, names.dead, (place) => place.running !== true && place.state?.tried_ms > before && place.state?.failed, "end in a failure", 300);
  await openSnapshots(t, page);
  await t.see(banners(page).filter({ hasText: `The last backup to ${names.dead} failed` }).getByRole("button", { name: "Back up now", exact: true }));
}

// --- Snapshots ---

/** Take a snapshot now makes a named snapshot of each place the machine keeps, and says so. */
async function takeOne(t, page, names) {
  await unlockNow(page);
  await openSnapshots(t, page);
  await page.getByRole("button", { name: "Take a snapshot now", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Take a snapshot now", exact: true });
  await t.see(dialog);
  // A name that is a time is refused where it is typed, and nothing is sent.
  await dialog.getByLabel("Name (optional)").fill("2026-11-14-0302");
  await dialog.getByRole("button", { name: "Take a snapshot", exact: true }).click();
  await t.see(dialog.getByText("A name like that is a time, which snap gives itself"));
  await dialog.getByLabel("Name (optional)").fill(names.take);
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.take));
  await dialog.getByRole("button", { name: "Take a snapshot", exact: true }).click();
  ensure((await answered).status() === 200, `${ROUTES.take} was not answered 200`);
  await t.see(dialog.getByText(/^Made a snapshot of /));
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
  const users = await readSnapshots(page, "/users");
  ensure((users.snapshots ?? []).some((one) => one.name === names.take && one.named === true), "the named snapshot of /users is not in `snap list /users`");
}

/** Browse opens a snapshot's folder with how each entry compares with now; a folder is entered by its link and left by the crumbs. */
async function browseOwn(t, page) {
  await openSnapshots(t, page);
  await listCard(page).locator('a[href^="/snapshots/browse?"]').first().click();
  await t.see(page.locator(".snap-browse .tbl"));
  await t.see(page.locator(".snap-trail"));
  ensure(page.url().includes("/snapshots/browse?"), `a Browse leads to ${page.url()}`);
  // The folders have a link, down to a person's home and back by the crumbs.
  const folders = page.locator(".snap-browse a.snap-folder");
  if ((await folders.count()) > 0) {
    await folders.first().click();
    await t.see(page.locator(".snap-trail a").first());
    await page.locator(".snap-trail a").first().click();
  }
  const bad = await fetchAsPage(page, "GET", `${pathOf(ROUTES.browse)}?place=%2Fusers&snapshot=${BEFORE}&folder=%2Fshared`);
  ensure(bad.status >= 400 && bad.status < 500, `a folder outside its place was answered ${bad.status}, not refused: ${bad.text}`);
}

/**
 * An administrator whose console is unlocked sees every file of a folder
 * outside their home, and Restore N chosen puts a copy of one of another
 * account's files beside it with the label in the browser's zone.
 */
async function restoreOfAnother(t, page, names) {
  await unlockNow(page);
  const made = (await readSnapshots(page, "/users")).snapshots.find((one) => one.name === BEFORE)?.made;
  ensure(typeof made === "string" && Number.isFinite(Date.parse(made)), `snap list /users has no snapshot ${BEFORE}`);
  const label = labelOf(Date.parse(made), ZONE);
  const utc = labelOf(Date.parse(made), "UTC");
  ensure(label !== utc, `the label in ${ZONE} is the label in UTC, ${label}`);

  await openFolder(t, page, "/users", BEFORE, HOME);
  await t.see(page.getByRole("row", { name: new RegExp(names.forAdmin) }));
  await t.see(page.getByRole("row", { name: new RegExp(names.forReader) }));
  // The files were deleted after the snapshot.
  await t.see(page.getByRole("row", { name: new RegExp(names.forAdmin) }).getByText("deleted since", { exact: true }));
  const restore = page.getByRole("button", { name: /^Restore \d+ chosen$/ });
  ensure(await restore.isDisabled(), "Restore 0 chosen is not disabled");
  await page.getByRole("checkbox", { name: `Pick ${names.forAdmin}`, exact: true }).check();
  await t.see(buttonOf(page, "Restore 1 chosen"));
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.restoreAll));
  await buttonOf(page, "Restore 1 chosen").click();
  ensure((await answered).status() === 200, `${ROUTES.restoreAll} was not answered 200`);
  const result = page.locator(".snap-result");
  await t.see(result.getByText("Restored 1 path beside the current one:"));
  await t.see(result.getByText(besideOf(names.forAdmin, label), { exact: true }));
  await lookAll(t, page, "snapshots-browse");
}

// --- The backup place on the stick ---

/** Chooses the disk, names the place, and adds it; the key is shown once and saved as a file. Gives the key. */
async function addOnStick(t, page, names) {
  await unlockNow(page);
  await openSnapshots(t, page);
  await page.getByRole("button", { name: "Add a backup place…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add a backup place", exact: true });
  await t.see(dialog);

  // An SSH server is asked for the machine's key, which it is given to accept.
  const ssh = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.sshKey));
  await dialog.getByText("An SSH server", { exact: true }).click();
  ensure((await ssh).status() === 200, `${ROUTES.sshKey} was not answered 200`);
  await t.see(dialog.getByText(/ssh-(ed25519|rsa) /).first());
  await dialog.getByLabel("Server and folder").fill("backup@nas");
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
  await t.see(dialog.getByText(/user@host:\/folder/).first());

  // A plugged-in disk, by hand, never checked automatically. The stick has a volume for each engine, and this one's is chosen.
  await dialog.getByText("A plugged-in disk", { exact: true }).click();
  await t.see(dialog.getByLabel("Disk", { exact: true }));
  await dialog.getByLabel("Disk", { exact: true }).selectOption(names.volume);
  await dialog.getByLabel("Name", { exact: true }).fill(names.stick);
  await dialog.getByLabel("Back up", { exact: true }).selectOption("hand");
  await dialog.getByLabel("Check by restoring", { exact: true }).selectOption("never");
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
  await t.see(page.getByRole("dialog", { name: `Recovery key for ${names.stick}`, exact: true }));
  const keyDialog = page.getByRole("dialog", { name: `Recovery key for ${names.stick}`, exact: true });
  const shown = /AGE-SECRET-KEY-1[0-9A-Z]+/.exec(await keyDialog.innerText())?.[0];
  ensure(shown !== undefined, "the dialog of the recovery key shows no AGE-SECRET-KEY-1 line");

  // Save as file: the file is the key and nothing else.
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), keyDialog.getByRole("button", { name: "Save as file", exact: true }).click()]);
  ensure(download.suggestedFilename() === `${names.stick}-recovery.key`, `the saved file is called ${download.suggestedFilename()}`);
  const file = await download.path();
  ensure(file !== null, "the browser kept no file for the recovery key");
  ensure((await readFile(file, "utf8")).trim() === shown, "the saved file is not the key that was shown");
  await lookAll(t, page, "snapshots-key");
  await keyDialog.getByRole("button", { name: KEPT_WORDS, exact: true }).click();
  await t.gone(keyDialog);

  // The key is gone from the page once the dialog is closed.
  ensure(!(await page.content()).includes(shown), "the recovery key is still in the page after the dialog closed");
  // Back up now? is asked once the key was kept.
  const ask = page.getByRole("dialog", { name: `Back up to ${names.stick} now?`, exact: true });
  await t.see(ask);
  const run = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.run, names.stick));
  await ask.getByRole("button", { name: "Back up now", exact: true }).click();
  const started = await run;
  ensure(started.status() === 202 || started.status() === 200, `${ROUTES.run} was answered ${started.status()}`);
  return shown;
}

/** The first backup to the stick ends, and the page says the place is OK, backed up today. */
async function firstBackup(t, page, names) {
  const place = await placeBecomes(page, names.stick, (one) => one.running !== true && one.state?.done_ms > 0 && !one.state?.failed, "finish its first backup", 600);
  ensure(place.every === "hand", `${names.stick} is backed up ${place.every}, not by hand`);
  await openSnapshots(t, page, `/snapshots/backups/${names.stick}`);
  const row = placesCard(page).getByRole("row", { name: new RegExp(`^${names.stick}`) });
  await t.see(row.getByText("OK", { exact: true }));
  await t.see(protectedCard(page).getByText(new RegExp(`${names.stick}`)).first());
}

/** The place's own page: what it holds and keeps, Check now to done, Show it again after the console was locked, and Restore files…. */
async function placePage(t, page, names, shown) {
  await unlockNow(page);
  await openSnapshots(t, page, `/snapshots/backups/${names.stick}`);
  await t.see(page.getByRole("heading", { name: names.stick, level: 3, exact: true }));
  // backup add backs up the four places, the apps among them.
  await t.see(placesCard(page).getByText("/users, /shared, settings and apps", { exact: false }));
  await t.see(placesCard(page).getByText(/^People's files, \d+ backups?$/));
  await t.see(placesCard(page).getByText(/^Shown once on /));
  await lookAll(t, page, "snapshots-place");

  // Check now restores the newest backups and compares them: a task.
  const verify = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.verify, names.stick));
  await placesCard(page).getByRole("button", { name: "Check now", exact: true }).click();
  const started = await verify;
  ensure(started.status() === 202 || started.status() === 200, `${ROUTES.verify} was answered ${started.status()}`);
  await placeBecomes(page, names.stick, (one) => one.running !== true && one.state?.verified_ms > 0 && !one.state?.verify_failed, "pass its check by a restore", 600);
  await openSnapshots(t, page, `/snapshots/backups/${names.stick}`);
  await t.see(placesCard(page).getByText(/restored the newest backups and compared them\. Passed\./));

  // Show it again asks to unlock, and shows the key once: the same one.
  const locked = await fetchAsPage(page, "POST", "/api/1/lock");
  ensure(locked.status === 200, `the console was not locked: ${locked.status} ${locked.text}`);
  await sleep(1500);
  const key = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.key, names.stick));
  await buttonOf(page, "Show it again").click();
  await t.see(unlockDialog(page));
  await answerUnlock(t, page);
  ensure((await key).status() === 200, `${ROUTES.key} was not answered 200`);
  const keyDialog = page.getByRole("dialog", { name: `Recovery key for ${names.stick}`, exact: true });
  await t.see(keyDialog);
  ensure((await keyDialog.innerText()).includes(shown), "Show it again shows another key than the one that was shown when the place was added");
  await keyDialog.getByRole("button", { name: KEPT_WORDS, exact: true }).click();
  await t.gone(keyDialog);
  // The machine's own read of the key, which the route runs, is the same.
  const again = await fetchAsPage(page, "POST", pathOf(ROUTES.key, names.stick), {});
  ensure(again.status === 200 && again.text.includes(shown), `${ROUTES.key} does not answer the key that was shown: ${again.status}`);
}

/** The names of the restored-TIME snapshots of /users, which Restore files… makes. */
async function restoredOf(page) {
  const users = await readSnapshots(page, "/users");
  return (users.snapshots ?? []).map((one) => one.name).filter((name) => name.startsWith("restored-"));
}

/** Restore files… receives the newest backup into the snapshots as restored-TIME; the page then shows it. */
async function restoreFromBackup(t, page, names) {
  await unlockNow(page);
  await openSnapshots(t, page, `/snapshots/backups/${names.stick}`);
  await t.see(placesCard(page).getByText(/^People's files, \d+ backups?$/));
  // The engines before this one left their restored-TIME snapshots, so the one that this restore makes is the one that was not there.
  const had = await restoredOf(page);
  await buttonOf(page, "Restore files…").click();
  const dialog = page.getByRole("dialog", { name: `Restore files from ${names.stick}`, exact: true });
  await t.see(dialog);
  await dialog.getByLabel("Place", { exact: true }).selectOption({ index: 0 });
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.restoreFrom, names.stick));
  await dialog.getByRole("button", { name: "Restore", exact: true }).click();
  const reply = await answered;
  ensure(reply.status() === 202 || reply.status() === 200, `${ROUTES.restoreFrom} was answered ${reply.status()}`);
  await t.gone(dialog);
  // When the restore is done this page shows the snapshot it made.
  await page.waitForURL((url) => url.pathname === "/snapshots/browse" && url.searchParams.get("snapshot")?.startsWith("restored-"), { timeout: 600_000 });
  await t.see(page.locator(".snap-browse .tbl"));
  ensure((await restoredOf(page)).some((name) => !had.includes(name)), "`snap list /users` has no new restored-TIME snapshot");
}

/** Remove… asks, and forgets the place; the backups stay where they are. */
async function removeDead(t, page, names) {
  await unlockNow(page);
  await openSnapshots(t, page, `/snapshots/backups/${names.dead}`);
  await t.see(page.getByRole("heading", { name: names.dead, level: 3, exact: true }));
  await buttonOf(page, "Remove…").click();
  const ask = page.getByRole("dialog", { name: `Remove ${names.dead}?`, exact: true });
  await t.see(ask);
  await t.see(ask.getByText(/Its backups stay /));
  // Cancel leaves it.
  await ask.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(ask);
  ensure(placeOf(await readBackups(page), names.dead) !== undefined, `${names.dead} was removed by Cancel`);
  await buttonOf(page, "Remove…").click();
  const removed = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.remove, names.dead));
  await page.getByRole("dialog", { name: `Remove ${names.dead}?`, exact: true }).getByRole("button", { name: "Remove", exact: true }).click();
  ensure((await removed).status() === 200, `${ROUTES.remove} was not answered 200`);
  await t.see(page.getByRole("heading", { name: "Snapshots and backups", level: 1, exact: true }));
  ensure(placeOf(await readBackups(page), names.dead) === undefined, `${names.dead} is still listed after Remove`);
  await t.gone(banners(page).filter({ hasText: names.dead }));
}

/** A place the machine does not have says so and leads back. */
async function noSuchPlace(t, page) {
  await openSnapshots(t, page, "/snapshots/backups/no-such-place");
  await t.see(page.getByText("There is no backup place called no-such-place."));
  await t.see(page.getByRole("link", { name: "Back to the backup places", exact: true }));
}

async function administrator(t, names) {
  const { page, seen } = await signedIn(t, ADMIN);
  await unlockNow(page);

  await failedBanner(t, page, names);
  await openSnapshots(t, page);
  await cardsAsAnswered(t, page);
  await lookAll(t, page, "snapshots-list");
  await backUpFails(t, page, names);

  await takeOne(t, page, names);
  await browseOwn(t, page);
  await restoreOfAnother(t, page, names);

  const shown = await addOnStick(t, page, names);
  await firstBackup(t, page, names);
  await placePage(t, page, names, shown);
  await restoreFromBackup(t, page, names);

  await removeDead(t, page, names);
  await noSuchPlace(t, page);
  await openSnapshots(t, page, "/snapshots/backups");
  await t.see(placesCard(page).getByRole("row", { name: new RegExp(`^${names.stick}`) }));

  remember(seen);
  mustHaveCalled(seen, [
    routeFor(ROUTES.list),
    routeFor(ROUTES.backups),
    routeFor(ROUTES.browseAll),
    routeFor(ROUTES.take),
    routeFor(ROUTES.restoreAll),
    routeFor(ROUTES.sshKey),
    routeFor(ROUTES.add),
    routeFor(ROUTES.run, names.stick),
    routeFor(ROUTES.run, names.dead),
    routeFor(ROUTES.show, names.stick),
    routeFor(ROUTES.verify, names.stick),
    routeFor(ROUTES.key, names.stick),
    routeFor(ROUTES.restoreFrom, names.stick),
    routeFor(ROUTES.remove, names.dead),
  ]);
  await signOut(t, page);
  await page.context().close();
}

// --- The reader and the phone ---

/**
 * A reader sees the cards, browses their own folder and restores one of their
 * own files beside the current one, and sees no button that changes anything
 * and none that reads what a backup place holds.
 */
async function reader(t, names) {
  const { page, seen } = await signedIn(t, READER);
  await openSnapshots(t, page);
  await cardsAsAnswered(t, page);
  for (const name of ["Take a snapshot now", "Add a backup place…", "Back up now", "Check now"]) {
    ensure((await buttonOf(page, name).count()) === 0, `a reader's page has ${name}`);
  }
  await openSnapshots(t, page, `/snapshots/backups/${names.stick}`);
  await t.see(placesCard(page).getByText(READER_WORDS));
  for (const name of ["Show it again", "Restore files…", "Remove…", "Back up now", "Check now"]) {
    ensure((await buttonOf(page, name).count()) === 0, `a reader's place page has ${name}`);
  }
  await lookAll(t, page, "snapshots-reader");

  // Their own folder: what the snapshot had, restored beside what is there as themselves.
  const made = (await readSnapshots(page, "/users")).snapshots.find((one) => one.name === BEFORE)?.made;
  ensure(typeof made === "string", `the reader's snap list /users has no ${BEFORE}`);
  const label = labelOf(Date.parse(made), ZONE);
  await openFolder(t, page, "/users", BEFORE, HOME);
  await t.see(page.getByRole("row", { name: new RegExp(names.forReader) }));
  await page.getByRole("checkbox", { name: `Pick ${names.forReader}`, exact: true }).check();
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.restore));
  await buttonOf(page, "Restore 1 chosen").click();
  ensure((await answered).status() === 200, `${ROUTES.restore} was not answered 200`);
  await t.see(page.locator(".snap-result").getByText(besideOf(names.forReader, label), { exact: true }));

  // The reader's console cannot see every file: a folder of another account is refused as theirs.
  const others = await fetchAsPage(page, "GET", `${pathOf(ROUTES.browseAll)}?place=%2Fusers&snapshot=${BEFORE}&folder=%2Fusers%2F${ADMIN}`);
  ensure(others.status === 403, `browse/all as a reader was answered ${others.status}, not 403: ${others.text}`);

  remember(seen);
  mustHaveCalled(seen, [routeFor(ROUTES.list), routeFor(ROUTES.backups), routeFor(ROUTES.browse), routeFor(ROUTES.restore)]);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list, and a snapshot's folder with its Restore, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE, timezoneId: ZONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page, ZONE);
  await unlockNow(page);

  await openSnapshots(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .snap button, main .snap a.btn");
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "snapshots-phone-list");

  await openFolder(t, page, "/users", BEFORE, HOME);
  ensure((await sidewaysScroll(page)) === 0, `the phone's folder scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallFolder = await smallControls(page, "main .snap-browse button, main .snap-browse a.btn, main .snap-pick");
  ensure(smallFolder.length === 0, `these controls of the phone's folder are under 44 px tall: ${smallFolder.join(", ")}`);
  await t.see(buttonOf(page, "Restore 0 chosen"));
  await lookAll(t, page, "snapshots-phone-browse");
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const names = namesOf(t.engine);
  await administrator(t, names);
  await reader(t, names);
  await phone(t);
  // Everything the table names was called by some browser.
  const wanted = Object.values(ROUTES).map((route) => routeFor(route, names.stick));
  const missing = wanted.filter((route) => !called.has(route) && !called.has(route.replace(encodeURIComponent(names.stick), encodeURIComponent(names.dead))));
  ensure(missing.length === 0, `no browser called ${missing.join(", ")}`);
}
