// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/reliability drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the
// Reliability pages (design 7): the page at /reliability, one start at
// /reliability/starts/{start} and the journal at /reliability/changes. An
// administrator signs in with the console locked and finds the cards that
// anyone reads held to what the API answers the same page at the same time
// (Health, Starts and A failed start), and the three that only an unlocked
// administrator reads (the errors of the log under Services, Crashes and
// Alerts) saying "Unlock to see errors, crashes and alerts." with an Unlock
// button, having asked for none of them. After the unlock they are read: the
// line that reliability.toml's root step wrote with `logger -p user.err` is
// counted under Services for today and listed when today is chosen, with the
// links to the logs and the changes of that day; Crashes and Alerts are what
// `GET /api/1/dumps` and `GET /api/1/alerts/history` say; the range control
// keeps its choice in the address. A start's page says what
// `GET /api/1/starts/{start}` (`startup --start N --compare`) says, or that no
// times are kept.
//
// The journal lists the changes this test makes through the console (the
// service lab-reliability restarted and then turned off, which leave rows of
// their own), is narrowed by who, how and day, and "Copy as a script" of the
// two gives the text that this test builds itself from the API's rows and
// the list of which commands run as an administrator. The script is saved in
// the lab PC's /tmp/lab-reliability-script.sh, which reliability.toml copies
// to the machine and runs as root (without `elevate`, which root needs no
// more than the script says) after putting the service's file back as it was
// before the two changes; `cmp` then finds the file as the console left it.
// A lab test has one machine, so the script runs on the machine that made
// the changes and not on a fresh one. Copy settings to Git makes a folder of
// the settings for each engine, which reliability.toml's root steps compare
// with /system/settings but for the paths that SECRETS.md lists.
//
// A reader sees the same page with the cards of the administrator saying
// "Administrators see errors, crashes and alerts." and asks for none of them.
// A phone at 390 by 844 shows the days as a list for each day, the page and
// the journal without sideways scrolling, and controls a thumb can press.
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll). The accounts
// are the two that reliability.toml made, so no script needs the lab's
// password. The script runs once for each engine, one after the other, and
// leaves lab-reliability turned off: each run begins by turning it on.

import { writeFile } from "node:fs/promises";
import {
  answerTo,
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  postAsPage,
  sessionOf,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { microsOf } from "./log-lines.mjs";

// The password that reliability.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";
const ADMIN = "wc-reliability";
const READER = "wc-reliability-reader";

const PHONE = { width: 390, height: 844 };

// The service of the test's own, whose file the console changes, and the line
// that a root step logged as its error.
const SERVICE = "lab-reliability";
const LINE = "lab-reliability: the NAS did not answer";

// Where the script of the journal is kept for reliability.toml to copy to the
// machine, and the folder that Copy settings to Git is told to write (one for
// each engine, since an export that changes nothing makes no commit).
export const SCRIPT_FILE = "/tmp/lab-reliability-script.sh";
const folderOf = (t) => `/users/${ADMIN}/lab-settings-${t.engine}`;

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end.
const ROUTES = {
  starts: "GET /api/1/starts",
  failed: "GET /api/1/starts/failed", // reliability.failed
  start: "GET /api/1/starts/{start}",
  dumps: "GET /api/1/dumps", // reliability.dumps
  alerts: "GET /api/1/alerts/history", // reliability.alerts
  logsAll: "GET /api/1/logs/all",
  changes: "GET /api/1/changes",
  actions: "GET /api/1/actions",
  checks: "GET /api/1/checks/last",
  export: "POST /api/1/console/export",
};

/** The path of a route as the table names it: `GET /api/1/starts` is `/api/1/starts`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

// The words the pages say where a card cannot read, and for a start that has no times.
const LOCKED_LINE = "Unlock to see errors, crashes and alerts.";
const READER_LINE = "Administrators see errors, crashes and alerts.";
const NO_TIMES = "No times are kept for this start.";
const UNLOCK_FOR = "see errors, crashes and alerts";

// The alert that reliability.toml raises and clears, so that the history the Alerts card reads has rows.
const ALERT_TITLE = "Lab alert for the Reliability page";

// The cards that only an administrator reads.
const ADMIN_CARDS = ["events", "crashes", "alerts"];

const unlockDialog = (page) => page.getByRole("dialog", { name: /^Unlock/ });

/** The routes of an administrator's reads, which a page must not ask for while the console is locked or the account is a reader's. */
const ADMIN_PATHS = [pathOf(ROUTES.dumps), pathOf(ROUTES.alerts), pathOf(ROUTES.logsAll)];

/** Counts the requests a page makes to an administrator's reads until `stop()`. Gives `{count, stop}`. */
function watchAdminReads(page) {
  let counted = 0;
  const note = (request) => {
    if (ADMIN_PATHS.includes(new URL(request.url()).pathname)) counted += 1;
  };
  page.on("request", note);
  return { count: () => counted, stop: () => page.off("request", note) };
}

/** What `route` answers as JSON, which must be `status`. */
async function readJson(page, route, status = 200) {
  const answer = await fetchAsPage(page, "GET", route);
  ensure(answer.status === status, `GET ${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Today in UTC, as the page names the day: the accounts have UTC for their zone. */
const today = () => new Date().toISOString().slice(0, 10);

/** Answers the dialog that asks to unlock with the account's password. */
async function unlockHere(t, page) {
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText(UNLOCK_FOR, { exact: true }));
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const answered = answerTo(page, "/api/1/unlock");
  await dialog.getByRole("button", { name: "Unlock", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `the unlock was answered ${answer.status()}`);
  await t.gone(dialog);
}

// --- The page ---

/** Waits until the `cards` cards of the page that are shown have read, which is when none says "Loading…". */
const cardsRead = (page, cards = 7) =>
  page.waitForFunction(
    (count) => {
      const bodies = [...document.querySelectorAll(".rel .card-body")];
      return bodies.length >= count && bodies.every((body) => body.textContent.trim() !== "Loading…" || body.closest("section").hidden);
    },
    cards,
    { timeout: 30_000 },
  );

/** Opens /reliability, optionally with a query, and waits for its cards. */
async function openReliability(t, page, query = "") {
  await page.goto(new URL(`/reliability${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Reliability", level: 1, exact: true }));
  await cardsRead(page);
}

/** The numbers of the starts drawn in the Starts card, in order. */
const startsShown = (page) =>
  page.locator("#ov-starts table tbody tr[data-start]").evaluateAll((rows) => rows.map((row) => Number(row.dataset.start)));

/** The Starts card lists the starts the machine keeps, newest first, ten at first, and Show older lists more. */
async function startsAsAnswered(t, page) {
  const starts = await readJson(page, pathOf(ROUTES.starts));
  ensure(Array.isArray(starts), `${ROUTES.starts} did not answer a list: ${JSON.stringify(starts)}`);
  const numbers = starts.map((one) => one.start);
  const card = page.locator("#ov-starts");
  if (numbers.length === 0) {
    await t.see(card.getByText("No start is kept yet.", { exact: true }));
    return starts;
  }
  const shown = await startsShown(page);
  ensure(JSON.stringify(shown) === JSON.stringify(numbers.slice(0, 10)), `the Starts card shows ${shown.join(", ")} and ${ROUTES.starts} gives ${numbers.join(", ")}`);
  const bad = await card.locator("table tbody tr .st.bad").count();
  const failed = starts.slice(0, 10).filter((one) => one.result === "failed" || one.result === "fell-back").length;
  ensure(bad === failed, `the Starts card marks ${bad} starts as bad and ${ROUTES.starts} has ${failed} that failed or fell back`);
  const hrefs = await card.locator("table tbody tr[data-start] th a").evaluateAll((links) => links.map((link) => link.getAttribute("href")));
  ensure(JSON.stringify(hrefs) === JSON.stringify(shown.map((number) => `/reliability/starts/${number}`)), `the links of the Starts card are ${hrefs.join(", ")}`);
  const older = card.getByRole("button", { name: /^Show older/ });
  if (numbers.length > 10) {
    await older.click();
    const more = await startsShown(page);
    ensure(more.length === Math.min(20, numbers.length), `Show older shows ${more.length} of ${numbers.length} starts`);
  } else {
    await t.gone(older);
  }
  return starts;
}

/** The card of a failed start is there when the machine has one, with its start and the offer to find its change, and not when it has none. */
async function failedAsAnswered(t, page) {
  const answer = await readJson(page, pathOf(ROUTES.failed));
  const card = page.locator("#ov-failed");
  if (answer.failed === null || answer.failed === undefined) {
    ensure(!(await card.isVisible()), "the card of a failed start is shown and the machine has none");
    return;
  }
  await t.see(card);
  const link = card.getByRole("link", { name: "Open the start", exact: true });
  ensure((await link.getAttribute("href")) === `/reliability/starts/${answer.failed.start}`, `Open the start leads to ${await link.getAttribute("href")}`);
  if (answer.failed.check) ensure((await card.innerText()).includes(answer.failed.check), `the card of the failed start does not name ${answer.failed.check}`);
  const changes = Array.isArray(answer.since_good) ? answer.since_good : [];
  ensure((await card.locator("ul.rel-changes li").count()) === changes.length, `the card lists ${await card.locator("ul.rel-changes li").count()} changes and ${ROUTES.failed} has ${changes.length}`);
  if (changes.length >= 2 && !answer.bisect?.running) await t.see(card.locator("pre.cmdtext", { hasText: "elevate changes bisect" }));
  await t.see(card.locator("pre.cmdtext", { hasText: "troubleshoot" }));
}

/** Health is the score of the last checks, or the words that say why there is none. */
async function healthAsAnswered(t, page) {
  const state = (answer) => {
    if (answer.status === 200 && Array.isArray(answer.json?.results)) return "ok";
    const id = answer.json?.error?.id;
    return id === "ADM-129" ? "missing" : id === "ADM-123" ? "none" : "error";
  };
  const words = { missing: "Checks are not on this server yet.", none: "Not checked yet.", error: "The checks could not be read." };
  const before = state(await fetchAsPage(page, "GET", pathOf(ROUTES.checks)));
  const card = page.locator("#ov-health");
  await t.see(card);
  const text = (await card.locator(".card-body").innerText()).trim();
  const after = state(await fetchAsPage(page, "GET", pathOf(ROUTES.checks)));
  const shown = /^\d+, (good|could be better|needs you)\b/.test(text) ? "ok" : Object.entries(words).find(([, line]) => text.includes(line))?.[0];
  ensure(shown === before || shown === after, `the Health card says "${text}", which is not the state of ${ROUTES.checks}: ${before}, then ${after}`);
  const link = card.getByRole("link", { name: "How the score works", exact: true });
  ensure((await link.getAttribute("href")) === "/", `How the score works leads to ${await link.getAttribute("href")}`);
}

/** The cell for `day` in the row of `kind` of the table of events as drawn at a desktop's width. */
const cellOf = (page, kind, day) =>
  page
    .locator("#ov-events table.rel-table tbody tr")
    .filter({ has: page.locator("th", { hasText: new RegExp(`^${kind}$`) }) })
    .locator(`.rel-cell[data-key="${day}"]`);

/** The page before the unlock: the cards that anyone reads, and the three of the administrator asking to unlock, having read nothing. */
async function lockedPage(t, page) {
  const watch = watchAdminReads(page);
  await openReliability(t, page);
  await startsAsAnswered(t, page);
  await failedAsAnswered(t, page);
  await healthAsAnswered(t, page);
  for (const name of ADMIN_CARDS) {
    const note = page.locator(`#ov-${name} .read-note`);
    await t.see(note.getByText(LOCKED_LINE, { exact: true }));
    await t.see(note.getByRole("button", { name: "Unlock", exact: true }));
  }
  ensure(watch.count() === 0, `the page asked for ${watch.count()} of the administrator's reads while the console was locked`);
  watch.stop();
  // reliability.dumps and reliability.alerts: the routes refuse a locked administrator, which the page does not ask for.
  for (const route of [ROUTES.dumps, ROUTES.alerts]) mustBe(await fetchAsPage(page, "GET", pathOf(route)), 403, "ADM-106", `a locked administrator's ${route}`);
  await lookAll(t, page, "reliability-locked");
}

/** The cards that an unlock lets be read: Crashes and Alerts as the API gives them. */
async function crashesAndAlerts(t, page) {
  const dumps = await readJson(page, pathOf(ROUTES.dumps));
  ensure(Array.isArray(dumps), `${ROUTES.dumps} did not answer a list: ${JSON.stringify(dumps)}`);
  const crashes = page.locator("#ov-crashes");
  if (dumps.length === 0) {
    await t.see(crashes.getByText("No crash dumps are kept.", { exact: true }));
  } else {
    const newest = [...dumps].sort((a, b) => (b.at_ms ?? 0) - (a.at_ms ?? 0)).slice(0, 5);
    for (const dump of newest) await t.see(crashes.locator("pre.cmdtext", { hasText: `elevate dumps show ${dump.name}` }));
  }

  const lines = await readJson(page, pathOf(ROUTES.alerts));
  ensure(Array.isArray(lines), `${ROUTES.alerts} did not answer a list: ${JSON.stringify(lines)}`);
  const kept = lines.filter((line) => Number.isFinite(line?.at) && line.alert).length;
  const alerts = page.locator("#ov-alerts");
  if (kept === 0) {
    await t.see(alerts.getByText("No alert has been raised or cleared.", { exact: true }));
  } else {
    const rows = await alerts.locator("tbody tr").count();
    ensure(rows === Math.min(20, kept), `the Alerts card shows ${rows} rows and the history has ${kept} lines`);
  }

  // The alert that the machine raised and cleared before the page opened is in the history, and the card says each change.
  const states = lines.filter((line) => line.alert?.title === ALERT_TITLE).map((line) => line.state);
  ensure(states.includes("raised") && states.includes("cleared"), `${ROUTES.alerts} has the changes ${JSON.stringify(states)} of "${ALERT_TITLE}", not a raise and a clear`);
  const mine = alerts.locator("tbody tr", { hasText: ALERT_TITLE });
  await t.see(mine.filter({ hasText: "Raised" }).first());
  await t.see(mine.filter({ hasText: "Cleared" }).first());
}

/** The records of today's errors in the whole log, as `GET /api/1/logs/all` gives them. */
async function errorsToday(page) {
  const since = Math.floor(Date.parse(`${today()}T00:00:00Z`) / 1000);
  const records = await readJson(page, `${pathOf(ROUTES.logsAll)}?lines=1000&priority=error&since=${since}`);
  ensure(Array.isArray(records), `${ROUTES.logsAll} did not answer a list: ${JSON.stringify(records)}`);
  return records.filter((record) => new Date(microsOf(record.time) / 1000).toISOString().slice(0, 10) === today());
}

/** The line that the root step logged is counted under Services for today, and is listed when today is chosen. */
async function errorsCounted(t, page) {
  const before = await errorsToday(page);
  ensure(before.some((record) => record.text.includes(LINE)), `${ROUTES.logsAll} does not have "${LINE}": the root step's logger did not write it`);
  const cell = cellOf(page, "Services", today());
  await t.see(cell);
  const label = (await cell.getAttribute("aria-label")) ?? "";
  const found = /^(\d+) errors?, /.exec(label);
  ensure(found !== null, `today's cell under Services says "${label}"`);
  const after = await errorsToday(page);
  // Errors may have come since the page read, and the page counts what it read: it has the marked line, and no more than the log has now.
  const count = Number(found[1]);
  const marked = before.filter((record) => record.text.includes(LINE)).length;
  ensure(count >= marked && count <= after.length, `today counts ${count} errors under Services, and the log has ${marked} of the marked line and ${after.length} errors`);

  await cell.click();
  const day = page.locator("#ov-day");
  await t.see(day);
  ensure((await cell.getAttribute("aria-pressed")) === "true", "the chosen day is not marked as pressed");
  await t.see(day.locator(".tl-body b", { hasText: new RegExp(`^${SERVICE}: \\d+ errors?$`) }));
  await t.see(day.getByText(`The last: ${LINE}`, { exact: false }).first());
  const logs = day.getByRole("link", { name: "Logs for this day", exact: true });
  ensure((await logs.getAttribute("href")).startsWith("/logs?"), `Logs for this day leads to ${await logs.getAttribute("href")}`);
  const changes = day.getByRole("link", { name: "Changes this day", exact: true });
  ensure((await changes.getAttribute("href")) === `/reliability/changes?day=${today()}`, `Changes this day leads to ${await changes.getAttribute("href")}`);
  // A second press on the cell puts the day away.
  await cell.click();
  await t.gone(day);
}

/** The range control: a year is 12 columns, 30 days are 30, and the address keeps the choice and is read back. */
async function ranges(t, page) {
  const columns = (count) =>
    page.waitForFunction((wanted) => document.querySelectorAll("#ov-events table.rel-table thead th").length === wanted + 1, count, { timeout: 30_000 });
  const pick = (id) => page.locator(`button[data-range="${id}"]`);
  await pick("1y").click();
  await columns(12);
  ensure((await pick("1y").getAttribute("aria-pressed")) === "true", "1 year is not pressed after it was chosen");
  ensure(new URL(page.url()).searchParams.get("range") === "1y", `the address is ${page.url()} after choosing a year`);
  await pick("30d").click();
  await columns(30);
  ensure(new URL(page.url()).searchParams.get("range") === "30d", `the address is ${page.url()} after choosing 30 days`);
  await openReliability(t, page, "?range=7d");
  ensure((await pick("7d").getAttribute("aria-pressed")) === "true", "the address's range is not the one pressed");
  await columns(7);
}

/** The unlock from the Crashes card reads what only an unlocked administrator reads, and the three cards say what they read. */
async function unlockedPage(t, page) {
  await page.locator("#ov-crashes .read-note").getByRole("button", { name: "Unlock", exact: true }).click();
  await unlockHere(t, page);
  for (const name of ADMIN_CARDS) await t.gone(page.locator(`#ov-${name} .read-note`));
  await cardsRead(page);
  await crashesAndAlerts(t, page);
  await errorsCounted(t, page);
  await ranges(t, page);
  await lookAll(t, page, "reliability");
}

// --- One start ---

/** A start's page says what `startup --start N --compare` says, or that no times are kept; a number the machine has no start of says so. */
async function startPage(t, page, starts) {
  ensure(starts.length > 0, "the machine keeps no start, and one start's page has nothing to open");
  const number = starts[0].start;
  await page.goto(new URL(`/reliability/starts/${number}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: `Start ${number}`, level: 1, exact: true }));
  await cardsRead(page, 3);
  await t.see(page.locator("#ov-start dl.rows"));
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.start).replace("{start}", String(number)));
  if (answer.status === 200) {
    const steps = page.locator("#ov-steps table tbody tr");
    await t.see(steps.first());
    ensure((await steps.count()) === answer.json.steps.length, `the page shows ${await steps.count()} steps and startup has ${answer.json.steps.length}`);
    await t.see(page.locator("#rel-against"));
    for (const one of (answer.json.slowest ?? []).slice(0, 2)) await t.see(page.locator("#ov-slow").getByText(one.name, { exact: true }).first());
  } else {
    ensure(answer.status === 422 && answer.json?.error?.id === "ADM-123", `${ROUTES.start} was answered ${answer.status}: ${answer.text}`);
    await t.see(page.locator("#ov-steps").getByText(NO_TIMES, { exact: true }));
  }
  await look(t, page, "reliability-start");

  await page.goto(new URL("/reliability/starts/999999", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Start 999999", level: 1, exact: true }));
  await t.see(page.getByText("The machine has no record of start 999999.", { exact: true }));
  await page.getByRole("link", { name: "Open Reliability", exact: true }).click();
  await t.see(page.getByRole("heading", { name: "Reliability", level: 1, exact: true }));
}

// --- The journal ---

/** A word of a shell line as admind's Show command writes it. */
const shellWord = (word) => (/^[A-Za-z0-9@%+=:,./_-]+$/.test(word) ? word : `'${word.replaceAll("'", "'\\''")}'`);

/** The script that the journal's `rows` make, built here from the rows and `runs`, which says who runs each action. */
function expectedScript(rows, runs) {
  const lines = [...rows]
    .sort((a, b) => a.at - b.at || a.id - b.id)
    .flatMap((row) => {
      const command = row.argv.map(shellWord).join(" ");
      const when = `${new Date(row.at * 1000).toISOString().slice(0, 19).replace("T", " ")} UTC`;
      return ["", `# ${when}: ${String(row.title ?? row.action).replace(/\s+/g, " ")}`, runs.get(row.action) === "as-administrator" ? `elevate ${command}` : command];
    });
  return ["#!/bin/sh", "# Changes made with the Dukovo console, oldest first.", "set -e", ...lines, ""].join("\n");
}

/** The rows of the journal as the API gives them, newest first. */
async function journalRows(page) {
  const answer = await readJson(page, `${pathOf(ROUTES.changes)}?since=0&limit=200`);
  ensure(Array.isArray(answer?.changes), `${ROUTES.changes} did not answer {changes}: ${JSON.stringify(answer)}`);
  return answer.changes;
}

/** Posts the change `verb` of the service and checks that it was answered 200. */
async function change(page, verb) {
  const done = await postAsPage(page, `/api/1/services/${SERVICE}/${verb}`, {});
  ensure(done.status() === 200, `${verb} of ${SERVICE} was answered ${done.status()}: ${await done.text()}`);
}

/** Turns lab-reliability on if the last engine left it off, and waits for it to run. */
async function serviceOn(page) {
  const until = Date.now() + 30_000;
  for (;;) {
    const status = await readJson(page, `/api/1/services/${SERVICE}`);
    if (status.service.enabled === true && status.service.state === "running") return;
    if (status.service.enabled !== true) await change(page, "enable");
    ensure(Date.now() < until, `${SERVICE} is ${status.service.state}, enabled ${status.service.enabled}, and does not run`);
    await sleep(1000);
  }
}

/** Restarts and then turns off lab-reliability through the console, and gives the two rows of the journal that it made. */
async function makeChanges(page) {
  await serviceOn(page);
  const before = (await journalRows(page)).reduce((most, row) => Math.max(most, row.id), 0);
  await change(page, "restart");
  await change(page, "disable");
  const until = Date.now() + 30_000;
  for (;;) {
    const rows = (await journalRows(page)).filter((row) => row.id > before && row.who === ADMIN && row.how === "console");
    const mine = ["service.restart", "service.disable"].map((action) => rows.find((row) => row.action === action));
    if (mine.every((row) => row !== undefined)) return mine;
    ensure(Date.now() < until, `the journal has no row of the restart and the disable after row ${before}: ${JSON.stringify(rows.map((row) => row.action))}`);
    await sleep(1000);
  }
}

/** The ids of the rows of the journal's table, in order. */
const rowsShown = (page) =>
  page.locator(".rel-journal table tbody tr[data-id]").evaluateAll((rows) => rows.map((row) => Number(row.dataset.id)));

/** The journal: its rows are the API's, who, how and day narrow them and are kept in the address, and Copy as a script gives the text this test builds. */
async function journal(t, page) {
  const mine = await makeChanges(page);
  const first = await journalRows(page);
  const actions = await readJson(page, pathOf(ROUTES.actions));
  const runs = new Map(actions.map((one) => [one.id, one.runs]));
  ensure(runs.get("service.restart") === "as-administrator", `GET /api/1/actions says service.restart runs ${runs.get("service.restart")}`);

  await page.goto(new URL("/reliability/changes", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Journal", level: 1, exact: true }));
  await t.see(page.locator(".rel-journal table tbody tr[data-id]").first());
  // A row may have come (the console finds hand edits every 30 seconds): the page has what the API gave before it read or after.
  const second = await journalRows(page);
  const known = new Map([...first, ...second].map((row) => [row.id, row]));
  const ids = (rows) => JSON.stringify(rows.map((row) => row.id));
  const shown = await rowsShown(page);
  ensure(JSON.stringify(shown) === ids(first.slice(0, 50)) || JSON.stringify(shown) === ids(second.slice(0, 50)), `the journal shows ${shown.join(", ")} and the API's newest are ${first.slice(0, 50).map((row) => row.id).join(", ")}, then ${second.slice(0, 50).map((row) => row.id).join(", ")}`);
  if (second.length > 50) {
    await page.getByRole("button", { name: /^Show older/ }).click();
    ensure((await rowsShown(page)).length === Math.min(100, second.length), "Show older does not show the next 50 rows");
  }

  // Who, how and day narrow the rows, and are in the address.
  const query = () => new URL(page.url()).searchParams;
  await page.locator("#rel-who").selectOption(ADMIN);
  const byAdmin = await rowsShown(page);
  const wanted = (rows) => ids(rows.filter((row) => row.who === ADMIN).slice(0, 50));
  ensure(JSON.stringify(byAdmin) === wanted(first) || JSON.stringify(byAdmin) === wanted(second), `Who ${ADMIN} shows ${byAdmin.join(", ")}, and the API has ${JSON.parse(wanted(second)).join(", ")}`);
  ensure(query().get("who") === ADMIN, `the address is ${page.url()} after the choice of who`);
  await page.locator("#rel-how").selectOption("console");
  ensure(query().get("how") === "console", `the address is ${page.url()} after the choice of how`);
  await page.locator("#rel-day").selectOption(today());
  ensure(query().get("day") === today(), `the address is ${page.url()} after the choice of day`);
  const narrowed = await rowsShown(page);
  for (const row of mine) ensure(narrowed.includes(row.id), `row ${row.id} (${row.action}) is not among the changes ${ADMIN} made in the console today`);
  ensure(narrowed.every((id) => known.get(id)?.who === ADMIN), "the journal shows a row that another person made under Who");

  // The address holds the filters: opened by it, the page has them.
  await page.goto(new URL(`/reliability/changes?who=${ADMIN}&how=console&day=${today()}`, page.url()).href);
  await t.see(page.locator(".rel-journal table tbody tr[data-id]").first());
  ensure((await page.locator("#rel-who").inputValue()) === ADMIN && (await page.locator("#rel-how").inputValue()) === "console" && (await page.locator("#rel-day").inputValue()) === today(), "the address's filters are not in the choices");
  ensure(JSON.stringify(await rowsShown(page)) === JSON.stringify(narrowed), "the page opened by the address shows other rows than the choices did");
  await page.locator("#rel-how").selectOption("hand");
  const byHand = await rowsShown(page);
  ensure(byHand.every((id) => known.get(id)?.how === "by hand"), "By hand shows a row that was not made by hand");
  await page.locator("#rel-how").selectOption("console");

  // Nothing chosen gives no script; the two chosen give the text this test builds.
  const copy = page.getByRole("button", { name: "Copy as a script", exact: true });
  await copy.click();
  await t.see(page.getByText("Choose the changes to copy first.", { exact: true }));
  for (const row of mine) await page.locator(`#rel-pick-${row.id}`).check();
  ensure((await page.getByText("2 chosen", { exact: true }).count()) === 1, "the count of chosen rows is not 2");
  await copy.click();
  const box = page.locator(".rel-script pre.cmdtext");
  await t.see(box);
  const script = await box.evaluate((element) => element.textContent);
  const expected = expectedScript(mine, runs);
  ensure(script === expected, `the script is\n${script}\nand the rows make\n${expected}`);
  ensure(script.includes(`elevate service ${SERVICE} restart`) && script.includes(`elevate service ${SERVICE} disable`), "the script has no elevated line of each change");
  await writeFile(SCRIPT_FILE, expected);

  // Choose all shown chooses every row that can be a line, and nothing else.
  for (const row of mine) await page.locator(`#rel-pick-${row.id}`).uncheck();
  await page.getByRole("button", { name: "Choose all shown", exact: true }).click();
  const ticked = await page.locator(".rel-journal input[type=checkbox]:checked").evaluateAll((boxes) => boxes.map((one) => Number(one.id.replace("rel-pick-", ""))));
  for (const row of mine) ensure(ticked.includes(row.id), `Choose all shown left row ${row.id} out`);
  ensure(ticked.every((id) => narrowed.includes(id)), "Choose all shown chose a row that is not shown");
  await look(t, page, "reliability-journal");

  // A row opens the dialog of what it changed, which the Overview's What changed card has too.
  await page.locator(`tr[data-id="${mine[1].id}"]`).getByRole("button", { name: /^See the change/ }).click();
  const dialog = page.locator("dialog[open]");
  await t.see(dialog);
  await t.see(dialog.getByText("enabled = false", { exact: false }).first());
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
}

/** Copy settings to Git: the form's folder, the answer of the command and the words that follow. */
async function copySettings(t, page) {
  await openReliability(t, page);
  const folder = folderOf(t);
  const open = page.getByRole("button", { name: "Copy settings to Git…", exact: true });
  await open.click();
  ensure((await open.getAttribute("aria-expanded")) === "true", "Copy settings to Git… is not marked as open");
  const field = page.getByLabel("Folder", { exact: true });
  await t.see(field);
  ensure((await field.inputValue()).startsWith(`/users/${ADMIN}/`), `the folder is "${await field.inputValue()}", not one of the account's own`);
  await field.fill(folder);
  const answered = answerTo(page, pathOf(ROUTES.export));
  await page.getByRole("button", { name: "Copy settings", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `${ROUTES.export} was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(page.locator(".rel-export-output").getByText(`Copied to ${folder}.`, { exact: true }));
  const said = await page.locator(".rel-export-output pre.cmdtext").innerText();
  ensure(/committed [0-9a-f]+: Settings of /.test(said) || /already/.test(said) || said.includes("unchanged"), `admin export said "${said}"`);
  await look(t, page, "reliability-copied");
  await page.locator(".rel-export-form").getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(page.locator(".rel-export-output"));
}

/** The Overview's What changed card leads to the journal. */
async function seeAll(t, page) {
  await page.goto(new URL("/", page.url()).href);
  const link = page.getByRole("link", { name: "See all changes", exact: true });
  await t.see(link);
  ensure((await link.getAttribute("href")) === "/reliability/changes", `See all leads to ${await link.getAttribute("href")}`);
  await link.click();
  await t.see(page.getByRole("heading", { name: "Journal", level: 1, exact: true }));
  ensure(new URL(page.url()).pathname === "/reliability/changes", `See all led to ${page.url()}`);
}

// --- The people ---

/** An administrator: locked, unlocked, the days, a start, the journal, the copy of the settings and the way from the Overview. */
async function administrator(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, ADMIN, PASSWORD);
  const session = await sessionOf(page);
  ensure(session.name === ADMIN && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);

  await lockedPage(t, page);
  await unlockedPage(t, page);
  const starts = await readJson(page, pathOf(ROUTES.starts));
  await startPage(t, page, starts);
  await journal(t, page);
  await seeAll(t, page);
  await copySettings(t, page);

  mustHaveCalled(seen, [
    ROUTES.starts,
    ROUTES.failed,
    ROUTES.start.replace("{start}", String(starts[0].start)),
    ROUTES.dumps,
    ROUTES.alerts,
    ROUTES.logsAll,
    ROUTES.changes,
    ROUTES.actions,
    ROUTES.checks,
    ROUTES.export,
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the page with the administrator's cards saying who sees them, and asks for none of them. */
async function reader(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, READER, PASSWORD);
  const watch = watchAdminReads(page);

  await openReliability(t, page);
  await startsAsAnswered(t, page);
  await failedAsAnswered(t, page);
  for (const name of ADMIN_CARDS) {
    const note = page.locator(`#ov-${name} .read-note`);
    await t.see(note.getByText(READER_LINE, { exact: true }));
    ensure((await note.getByRole("button").count()) === 0, `a reader's ${name} card has a button`);
  }
  await t.see(page.locator("#ov-health").getByText("The health score is for administrators.", { exact: true }));
  ensure(watch.count() === 0, `a reader's page asked for ${watch.count()} of the administrator's reads`);
  watch.stop();
  // reliability.dumps and reliability.alerts: the routes refuse a reader, as the cards tell it.
  for (const route of [ROUTES.dumps, ROUTES.alerts]) mustBe(await fetchAsPage(page, "GET", pathOf(route)), 403, "ADM-122", `a reader's ${route}`);
  await look(t, page, "reliability-reader");

  // The journal is every account's: a reader sees its rows and no script of them.
  await page.goto(new URL("/reliability/changes", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Journal", level: 1, exact: true }));
  mustHaveCalled(seen, [ROUTES.starts, ROUTES.failed, ROUTES.changes]);
  await signOut(t, page);
  await page.context().close();
}

/** The controls of the pages that are under 44 px tall, which a thumb needs. */
const smallControls = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("main .rel button, main .rel a.btn, main .rel .rel-pick-box")]
      .filter((element) => element.offsetParent !== null)
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.height < 43.5)
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );

/** At 390 wide: the days as a list for each day, the starts as cards and no sideways scroll, on the page and on the journal. */
async function phone(t) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);

  await openReliability(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  ensure(!(await page.locator("#ov-events .rel-wide").isVisible()), "the phone shows the wide table of the days");
  await t.see(page.locator("#ov-events .rel-narrow"));
  ensure((await page.locator("#ov-events .rel-narrow > li").count()) === 7, `the phone's list has ${await page.locator("#ov-events .rel-narrow > li").count()} days, not 7`);
  const small = await smallControls(page);
  ensure(small.length === 0, `these controls are under 44 px tall on a phone: ${small.join(", ")}`);
  await lookAll(t, page, "reliability-phone");

  await page.goto(new URL("/reliability/changes", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Journal", level: 1, exact: true }));
  await t.see(page.locator(".rel-journal table tbody tr[data-id]").first());
  ensure((await sidewaysScroll(page)) === 0, `the phone's journal scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallJournal = await smallControls(page);
  ensure(smallJournal.length === 0, `these controls of the journal are under 44 px tall on a phone: ${smallJournal.join(", ")}`);
  await lookAll(t, page, "reliability-phone-journal");

  mustHaveCalled(seen, [ROUTES.starts, ROUTES.changes]);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
