// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/storage drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Storage page
// (design 4) at /storage and one disk's page at /storage/{disk}, with a disk's
// name for {disk}. It shows what `storage show --json` says, which
// `GET /api/1/storage` runs: a card of use for each volume, the disks drawn to
// scale with a health pill each, and, beside the list, the panel of the disk
// that is open, with Check now (`diskhealth`,
// `GET /api/1/storage/disks/{disk}/health`), Scrub the mirror (`scrub start`,
// `POST /api/1/storage/scrub`, a task) and Grow into it (`storage grow`,
// `POST /api/1/storage/disks/{disk}/grow`). Under them is the log's own space
// (`logtrim --show`, `GET /api/1/storage/log`), whose limits are changed
// (`POST /api/1/storage/log/limits`) and which is trimmed now
// (`POST /api/1/storage/log/trim`). The last hour of reads and writes is
// `GET /api/1/storage/usage/{range}`.
//
// The disks are what the machine has: the lab machine's own disk, and a memory
// card in the card reader and a USB stick that storage.toml puts in and makes
// into a mirror of two disks in RAID1, each with room after its partition. The
// page's disks are held to what the route answers, and the route's to the
// machine's own `storage show` and `lsblk` in the steps before. The mirror's
// card says "Mirror healthy" with "each a full copy"; its disk's panel scrubs
// it, with the button saying how far the scrub is, until the last scrub is told
// as "in the last scrub"; the mirror's disk with the most room grows into the
// room after its partition and then has none left, which leaves the card and
// the stick to the first two engines, and the third is shown no Grow into it; the log's limits are changed, shown in
// the card's own sentence and put back, and the log is trimmed. A virtual disk,
// a card and a stick give no health, and the page says so ("Not checked") and
// offers no Check now: when a disk of this machine does give it, Check now is
// pressed and its table is read.
//
// A reader sees the cards, the disks and a disk's panel, the words that say
// that administrators see the log's size, and no button that changes or reads
// the hardware. A disk the machine does not have says so and leads back to
// the list. A phone at 390 by 844 shows the list and a disk's panel, with no
// sideways scroll and every control 44 px tall. Each look is the console's
// own: dark, light, high contrast, forced colours and less motion, each
// checked by axe (console.mjs's lookAll). The accounts are the two that
// storage.toml made, so no script needs the lab's password.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustHaveCalled,
  openConsole,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, smallControls, unlockNow } from "./trial.mjs";

const ADMIN = "wc-storage";
const READER = "wc-storage-reader";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with the
// disk's name for {disk} and `1h` for {range}.
const ROUTES = {
  show: "GET /api/1/storage", // storage.show
  usage: "GET /api/1/storage/usage/{range}",
  health: "GET /api/1/storage/disks/{disk}/health",
  scrub: "POST /api/1/storage/scrub",
  grow: "POST /api/1/storage/disks/{disk}/grow",
  log: "GET /api/1/storage/log",
  limits: "POST /api/1/storage/log/limits", // storage.log-set
  trim: "POST /api/1/storage/log/trim", // storage.log-trim
};

/** The path of a route as the table names it, for `disk`: `GET /api/1/storage/disks/{disk}/health` is `/api/1/storage/disks/sdb/health`. */
const pathOf = (route, disk = "") => route.slice(route.indexOf(" ") + 1).replace("{disk}", encodeURIComponent(disk)).replace("{range}", "1h");

/** The route as `track` writes it, for `disk`. */
const routeFor = (route, disk = "") => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, disk)}`;

// The words the page says where this test looks for them.
const LOG_LOCKED = "Unlock to see the log's size.";
const LOG_READER = "Administrators see the log's size.";
const NO_HEALTH = "Not checked";

// The label storage.toml gives the volume it makes.
const MIRROR_LABEL = "LABMIRROR";

// --- Reading the machine ---

/** What `GET /api/1/storage` answers: `storage show --json`, `{disks, volumes, filesystems}`. */
async function readStorage(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.show));
  ensure(answer.status === 200 && Array.isArray(answer.json?.disks), `${ROUTES.show} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The volume of `answer` labelled `label`, which the steps before made. */
function volumeLabelled(answer, label) {
  const volume = answer.volumes.find((one) => one.label === label);
  ensure(volume !== undefined, `${ROUTES.show} lists no volume labelled ${label}: ${JSON.stringify(answer.volumes.map((one) => one.label))}`);
  return volume;
}

/** The disks of `answer` that hold a device of `volume`. */
const disksOfVolume = (answer, volume) =>
  answer.disks.filter((disk) => (disk.partitions ?? []).some((partition) => volume.devices.includes(partition.name)));

/** Reads the disks until `test(answer)` holds, for up to `seconds`, and gives them. */
async function storageBecomes(page, test, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const answer = await readStorage(page).catch(() => null);
    if (answer !== null && test(answer)) return answer;
    ensure(Date.now() < until, `the machine did not ${what} within ${seconds} seconds`);
    await sleep(1000);
  }
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The page ---

const diskRows = (page) => page.locator("#disks li.disk[data-disk]");
const diskRow = (page, name) => page.locator(`#disks li.disk[data-disk="${name}"]`);
const panel = (page) => page.locator("#disk");
const logCard = (page) => page.locator("#log");
const scrubButton = (page) => panel(page).locator("[data-scrub]");
const cardOf = (page, title) => page.locator(".sto-card[data-card]").filter({ has: page.getByRole("heading", { level: 2, name: new RegExp(`^${title}`) }) });

/**
 * Opens the list, or one disk's page, and waits for the disks to be read: the rows are there and the cards are not busy.
 * On a phone an opened disk is a view of its own, with a way back to the list in place of the list.
 */
async function openStorage(t, page, disk = null, { phone = false } = {}) {
  await page.goto(new URL(disk === null ? "/storage" : `/storage/${encodeURIComponent(disk)}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: disk ?? "Storage", level: 1, exact: true }));
  const list = page.getByRole("heading", { level: 2, name: "Disks, drawn to scale", exact: true });
  if (phone && disk !== null) {
    await t.see(page.getByRole("link", { name: "Back to the disks", exact: true }));
    await t.gone(list);
  } else {
    await t.see(list);
  }
  await page.waitForFunction(() => document.querySelectorAll("#disks li.disk[data-disk]").length > 0 && !document.querySelector(".sto-cards[aria-busy]"), null, { timeout: 30_000 });
  if (disk !== null) await t.see(panel(page));
}

/** The page's disks are the machine's: the same names in the same order, each a link to its own page, with its health word. */
async function disksAsAnswered(t, page) {
  const answer = await readStorage(page);
  const shown = await diskRows(page).evaluateAll((rows) => rows.map((row) => row.dataset.disk));
  ensure(JSON.stringify(shown) === JSON.stringify(answer.disks.map((disk) => disk.name)), `the page shows the disks ${JSON.stringify(shown)} and ${ROUTES.show} gives ${JSON.stringify(answer.disks.map((disk) => disk.name))}`);
  for (const disk of answer.disks) {
    const row = diskRow(page, disk.name);
    ensure((await row.getByRole("link", { name: disk.name, exact: true }).getAttribute("href")) === `/storage/${encodeURIComponent(disk.name)}`, `${disk.name} is not a link to its own page`);
    await t.see(row.locator(".st"));
    if (disk.health === null || disk.health === undefined) await t.see(row.locator(".st", { hasText: NO_HEALTH }));
  }
  return answer;
}

/** The mirror's card says so in words, and the disks are named in it. */
async function mirrorShown(t, page, answer) {
  const volume = volumeLabelled(answer, MIRROR_LABEL);
  const names = disksOfVolume(answer, volume).map((disk) => disk.name);
  ensure(names.length === 2, `the mirror is on ${names.length} disks, ${JSON.stringify(names)}`);
  const card = cardOf(page, MIRROR_LABEL);
  await t.see(card.locator(".pill", { hasText: "Mirror healthy" }));
  await t.see(card.getByText(new RegExp(`on (${names[0]} and ${names[1]}|${names[1]} and ${names[0]}), each a full copy`)));
  return { volume, names };
}

/** Nothing is wrong with the machine, so there is no banner. */
async function noBanner(page) {
  ensure((await page.locator(".sto-banner .banner").count()) === 0, `the page has a banner: ${await page.locator(".sto-banner").innerText()}`);
}

// --- The administrator ---

/** The log's card: locked it says to unlock, and unlocking shows its sentences. */
async function logUnlocks(t, page) {
  await t.see(logCard(page).getByText(LOG_LOCKED));
  await logCard(page).getByRole("button", { name: "Unlock", exact: true }).click();
  await answerUnlock(t, page);
  await t.see(logCard(page).getByText(/^The system's log takes /));
  await t.see(logCard(page).getByText(/^It keeps /));
}

/** Change… sets the days that records are kept, the card's sentence says so, and the machine has it; then the days go back. */
async function logLimits(t, page) {
  const before = (await fetchAsPage(page, "GET", pathOf(ROUTES.log))).json;
  ensure(Number.isInteger(before?.limits?.keep_days), `${ROUTES.log} gives no limits: ${JSON.stringify(before)}`);
  const days = before.limits.keep_days === 45 ? 46 : 45;
  for (const [to, words] of [[days, `It keeps ${days} days of records`], [before.limits.keep_days, `It keeps ${before.limits.keep_days} day`]]) {
    await logCard(page).getByRole("button", { name: "Change…", exact: true }).click();
    await t.see(page.locator(".sto-form"));
    await page.getByLabel(/^Keep records for/).fill(String(to));
    const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.limits));
    await logCard(page).getByRole("button", { name: "Save", exact: true }).click();
    ensure((await answered).status() === 200, `${ROUTES.limits} was not answered 200`);
    await t.see(logCard(page).getByText(new RegExp(`^${words}`)));
    const now = (await fetchAsPage(page, "GET", pathOf(ROUTES.log))).json;
    ensure(now.limits.keep_days === to, `the machine keeps ${now.limits.keep_days} days, not ${to}`);
  }
  // The form that is left alone sends nothing.
  await logCard(page).getByRole("button", { name: "Change…", exact: true }).click();
  await logCard(page).getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(page.locator(".sto-form"));
}

/** Trim now trims the log and the card says what it takes then. */
async function logTrims(t, page) {
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.trim));
  await logCard(page).getByRole("button", { name: "Trim now", exact: true }).click();
  ensure((await answered).status() === 200, `${ROUTES.trim} was not answered 200`);
  await t.see(logCard(page).getByText(/^The system's log takes /));
}

/** Check now where a disk gives its health; where none does, the page says so and offers no button. */
async function checkNow(t, page, answer) {
  const healthy = answer.disks.find((disk) => disk.health);
  if (healthy === undefined) {
    const plain = answer.disks[0].name;
    await openStorage(t, page, plain);
    ensure((await panel(page).getByRole("button", { name: "Check now", exact: true }).count()) === 0, `${plain} gives no health and has a Check now`);
    // The route still answers for it, with the machine's own words.
    const asked = await fetchAsPage(page, "GET", pathOf(ROUTES.health, plain));
    ensure(asked.status === 200 || typeof asked.json?.error?.id === "string", `${ROUTES.health} for ${plain} was answered ${asked.status}: ${asked.text}`);
    console.log(`no disk of this machine gives its health, so Check now was not pressed (${plain}: ${asked.status})`);
    return null;
  }
  await openStorage(t, page, healthy.name);
  const answered = page.waitForResponse((reply) => reply.request().method() === "GET" && new URL(reply.url()).pathname === pathOf(ROUTES.health, healthy.name));
  await panel(page).getByRole("button", { name: "Check now", exact: true }).click();
  ensure((await answered).status() === 200, `${ROUTES.health} was not answered 200`);
  await t.see(panel(page).getByText(`${healthy.name} checked:`));
  return healthy.name;
}

/** Scrub the mirror runs as a task: the button says how far it is, and when it is done the panel tells the scrub's last result. */
async function scrubMirror(t, page, mirror) {
  await openStorage(t, page, mirror.names[0]);
  await t.see(panel(page).getByText(/^Half of the .* mirror/));
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === pathOf(ROUTES.scrub));
  await scrubButton(page).click();
  const reply = await answered;
  ensure(reply.status() === 200 || reply.status() === 202, `${ROUTES.scrub} was answered ${reply.status()}`);
  const until = Date.now() + 8 * 60 * 1000;
  for (;;) {
    const words = (await scrubButton(page).innerText()).split("\n")[0].trim();
    if (words === "Scrub the mirror") break;
    ensure(/^Scrubbing…/.test(words), `the scrub's button says ${JSON.stringify(words)}`);
    ensure(Date.now() < until, "the scrub was not done within 8 minutes");
    await sleep(1000);
  }
  const done = await storageBecomes(page, (answer) => volumeLabelled(answer, MIRROR_LABEL).scrub?.result === "ok", "keep a scrub that found no error");
  ensure(volumeLabelled(done, MIRROR_LABEL).scrub.errors === 0, "the scrub found errors on a new card and a new stick");
  await t.see(panel(page).getByText(/in the last scrub/));
}

/** Whether `disk` has room after its last partition: more than 1 percent of it. */
const hasRoom = (disk) => disk.free_after / disk.size > 0.01;

/**
 * The mirror's disk with the most room after its partition grows into it,
 * and then has none. The engines run one after another on the one mirror, and
 * each grows a disk for good: the card and the stick give two engines a disk
 * to grow, and an engine after them finds no room on either, has no Grow into
 * it offered, and gives null.
 */
async function growDisk(t, page, answer) {
  const volume = volumeLabelled(answer, MIRROR_LABEL);
  const [disk] = disksOfVolume(answer, volume).sort((one, other) => other.free_after - one.free_after);
  if (!hasRoom(disk)) {
    await openStorage(t, page, disk.name);
    await t.see(scrubButton(page));
    ensure((await panel(page).getByRole("button", { name: "Grow into it", exact: true }).count()) === 0, `${disk.name} has no room after its partition and is offered Grow into it`);
    return null;
  }
  await openStorage(t, page, disk.name);
  await t.see(panel(page).getByText(/after its last partition/));
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === pathOf(ROUTES.grow, disk.name));
  await panel(page).getByRole("button", { name: "Grow into it", exact: true }).click();
  ensure((await answered).status() === 200, `${ROUTES.grow} was not answered 200`);
  await storageBecomes(page, (now) => {
    const found = now.disks.find((one) => one.name === disk.name);
    return found !== undefined && found.free_after / found.size <= 0.01;
  }, "have no room left after the grown partition");
  await t.gone(panel(page).getByRole("button", { name: "Grow into it", exact: true }));
  return disk.name;
}

/** The steps to replace a mirror's disk are words and commands to copy, and run nothing. */
async function stepsShown(t, page) {
  await page.getByRole("button", { name: "Use a new disk…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Use a new disk", exact: true });
  await t.see(dialog);
  await t.see(dialog.getByText("elevate mkfs-btrfs", { exact: false }));
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
}

async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await openStorage(t, page);
  const answer = await disksAsAnswered(t, page);
  const mirror = await mirrorShown(t, page, answer);
  await noBanner(page);
  await t.see(page.getByRole("heading", { level: 2, name: "Reads and writes, last hour", exact: true }));
  await logUnlocks(t, page);
  await lookAll(t, page, "storage-list");

  await stepsShown(t, page);
  await checkNow(t, page, answer);
  await scrubMirror(t, page, mirror);
  await lookAll(t, page, "storage-disk");
  const grew = await growDisk(t, page, answer);

  await openStorage(t, page);
  await logLimits(t, page);
  await logTrims(t, page);
  await page.goto(new URL("/storage#log", page.url()).href);
  await t.see(logCard(page).getByText(/^The system's log takes /));

  await page.goto(new URL("/storage/no-such-disk", page.url()).href);
  await t.see(page.getByRole("heading", { level: 2, name: "There is no disk called no-such-disk", exact: true }));
  await t.see(page.getByRole("link", { name: "Back to the disks", exact: true }));

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.show),
    routeFor(ROUTES.usage),
    routeFor(ROUTES.scrub),
    ...(grew === null ? [] : [routeFor(ROUTES.grow, grew)]),
    routeFor(ROUTES.log),
    routeFor(ROUTES.limits),
    routeFor(ROUTES.trim),
  ]);
  await signOut(t, page);
  await page.context().close();
}

// --- The reader and the phone ---

/** A reader sees the cards, the disks and a disk's panel, and nothing that changes or reads the hardware. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openStorage(t, page);
  const answer = await disksAsAnswered(t, page);
  await mirrorShown(t, page, answer);
  await t.see(logCard(page).getByText(LOG_READER));
  for (const name of ["Trim now", "Change…", "Unlock"]) {
    ensure((await logCard(page).getByRole("button", { name, exact: true }).count()) === 0, `a reader's log card has ${name}`);
  }
  await openStorage(t, page, answer.disks[0].name);
  for (const name of [/^Check now/, /^Scrub/, /^Check every block/, /^Replace this disk/, /^Grow into it/]) {
    ensure((await panel(page).getByRole("button", { name }).count()) === 0, `a reader's disk panel has a button ${name}`);
  }
  await lookAll(t, page, "storage-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list and a disk's panel, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  await openStorage(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .sto button, main .sto a.btn");
  ensure(small.length === 0, `these controls of the phone's list are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "storage-phone-list");

  const disks = await readStorage(page);
  await openStorage(t, page, disks.disks[0].name, { phone: true });
  ensure((await sidewaysScroll(page)) === 0, `the phone's disk scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallDisk = await smallControls(page, "main .sto button, main .sto a.btn");
  ensure(smallDisk.length === 0, `these controls of the phone's disk are under 44 px tall: ${smallDisk.join(", ")}`);
  await lookAll(t, page, "storage-phone-disk");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
