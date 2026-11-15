// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/logs drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Logs page
// (design 5), at "/logs" with its query in the address. logs.toml's root
// steps wrote the lines that are looked at, each marked with the tag
// lab-logs, and started lab-ticker, which writes an error, "lab-follow tick
// N", every two seconds. The browser is in America/New_York, and so is the
// account's own zone.
//
// A locked administrator and a reader see the lines of their own programs
// only; the administrator unlocks from the page's banner and then sees the
// whole log. Every list the page shows is held to what the API gives for the
// same query, line by line: its time to the second in the zone shown, its
// importance, who wrote it and its text, so the page's own reading of times,
// zones and words is checked by a second one that is written here. The
// ranges end where the script began, so that a line the machine writes later
// can never change what a list should hold. The test goes through each
// filter, the three zones, a custom range across both changes of summer time
// (the shown command must hold the UTC times), folded lines, older lines
// (the whole set comes back with no line twice), a row's details and its
// "Show 5 minutes around this", Save as file (each saved line against the
// log's own record), Follow (lines that come, Pause, a second window that
// takes the follow, Lock, which ends it with its sentence, and a tab hidden
// for over a minute, whose lines are read when it is seen again) and the
// command the page shows, which runs in the administrator's own terminal
// and must say as many lines as the page shows. A reader opens the Terminal
// page, runs `logger`, and sees that line, and no other, in Logs. The phone
// is 390 by 844. Each look is dark, light, high contrast, forced colours and
// less motion, checked by axe.
//
// A 3,000-character line is not written: logger keeps 256 bytes of a line
// and no more, so the longest the test can write is 214 characters, and the
// row, the details and the saved line are held to the whole of it.

import { readFile } from "node:fs/promises";

import {
  ensure,
  fetchAsPage,
  look,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  press,
  sessionOf,
  sidewaysScroll,
  signOut,
  sleep,
} from "./console.mjs";
import {
  argsFor,
  commandWith,
  literal,
  microsOf,
  OWN,
  PAGE_LINES,
  PASSWORD,
  PHONE,
  readLog,
  rowButtons,
  rowsFor,
  sameRows,
  settled,
  showCommands,
  shownRows,
  signInAs,
  smallTargets,
  stampOf,
  wallOf,
  whoOf,
  ZONE,
} from "./log-lines.mjs";

const adminOf = (engine) => `wc-logs-${engine}`;
const readerOf = (engine) => `wc-logs-reader-${engine}`;

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them.
const ROUTES = {
  own: "GET /api/1/logs",
  all: "GET /api/1/logs/all",
  follow: "GET /api/1/logs/follow",
  about: "GET /api/1/console/about",
  services: "GET /api/1/services",
  show: "POST /api/1/show",
  unlock: "POST /api/1/unlock",
  lock: "POST /api/1/lock",
};

// The lines logs.toml wrote, as the log keeps them. The long one is as long
// as the kernel keeps a record whole (200 bytes, the program's name and ": "
// first), and wraps on a page of any width.
const LONG = `lab-logs long ${"0123456789".repeat(16)}`;
const ALPHA = { error: "lab-logs alpha error", warning: "lab-logs alpha warning", info: "lab-logs alpha info", debug: "lab-logs alpha debug" };
const LITERAL = "lab-logs a.b(c) [d] *e* f|g";
const BULK_LINES = 250;

// --- What the page shows ---

const heading = (page) => page.getByRole("heading", { name: "Logs", level: 1 });
const field = (page, label) => page.getByLabel(label, { exact: true });
const live = (page) => page.locator(".pill", { hasText: "Live" });
const banner = (page) => page.locator(".logs-who");
const followButton = (page) => page.getByRole("button", { name: /^(Follow new lines|Pause)$/ });

/** What one row says when it is opened: the pairs of its details, by name. */
const detailsOf = (page) =>
  page.evaluate(() => {
    const open = document.querySelector('.loglist .lrow[aria-expanded="true"]');
    const list = open?.parentElement.querySelector(".ldet dl");
    if (!list) return null;
    return Object.fromEntries([...list.querySelectorAll("dt")].map((name) => [name.textContent.trim(), name.nextElementSibling.textContent.trim()]));
  });

/** The address of the page now, as the query it holds. */
const addressOf = (page) => new URL(page.url()).searchParams;

/** The address of a custom range of `scope`, `{since, until}` in Unix seconds, and what `extra` adds to it. */
function addressFor({ since, until }, extra = {}) {
  const query = new URLSearchParams({ when: "custom", since: String(since * 1000), until: String(until * 1000), priority: "debug", ...extra });
  return `/logs?${query}`;
}

/** Goes to `path` and waits for the page's first read. */
async function goTo(t, page, path) {
  await page.goto(new URL(path, page.url()).href);
  await t.see(heading(page));
  await settled(page);
}

/** Makes `change` and waits for the read that it causes, which the console must answer. */
async function reading(page, change, route = "/api/1/logs/all") {
  const answered = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === route, { timeout: 30_000 });
  await change();
  const answer = await answered;
  ensure(answer.status() === 200, `${route} was answered ${answer.status()} after a change of the filters: ${await answer.text()}`);
  await settled(page);
}

/** Holds the list on the page to the API's read of `args`, and gives the records. */
async function sameAsApi(page, args, what, { zone = ZONE, fold = false, all = true } = {}) {
  const records = await readLog(page, args, all);
  sameRows(await shownRows(page), rowsFor(records, zone, fold), what);
  return records;
}

// --- The command the page shows ---

// --- Signing in and unlocking ---

/** Unlocks from the banner of the Logs page, and waits for the list to be read again as the whole log. */
async function unlockFromBanner(t, page) {
  await banner(page).getByRole("button", { name: "Unlock", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes", exact: true });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.gone(dialog);
  await t.gone(banner(page).locator(".banner"));
  await settled(page);
}

// --- The filters, the zones and the ranges ---

/** The levels that each importance shows. */
const SHOWN_AT = { error: ["error"], warning: ["error", "warning"], info: ["error", "warning", "info"], debug: ["error", "warning", "info", "debug"] };

/** Each importance, then the words, and the fold, each against the API. */
async function filters(t, page, scope) {
  await goTo(t, page, addressFor(scope, { q: "lab-logs" }));
  const base = (more = {}) => argsFor(scope, { grep: "lab-logs", ...more });
  const first = await sameAsApi(page, base(), "Everything with lab-logs");
  ensure(first.length === PAGE_LINES, `a page of lab-logs lines has ${first.length} lines, not ${PAGE_LINES}`);
  await t.see(page.getByRole("button", { name: "Show older lines", exact: true }));

  for (const [label, priority] of [["Errors only", "error"], ["Warnings and errors", "warning"], ["Information", "info"], ["Everything", "debug"]]) {
    await reading(page, () => field(page, "Importance").selectOption({ label }));
    if (priority === "error") {
      // The error lines of lab-logs are there before the list is compared with
      // the API's, so that the comparison is of them.
      for (const line of [ALPHA.error, LONG]) await t.see(page.locator(".loglist .lrow .tx").filter({ hasText: line }).first());
    }
    const records = await sameAsApi(page, base({ priority }), label);
    const stray = records.filter((record) => !SHOWN_AT[priority].includes(record.level)).map((record) => record.level);
    ensure(stray.length === 0, `${label} shows lines of the importance ${stray.join(", ")}`);
    if (priority === "error") {
      const texts = (await shownRows(page)).map((row) => row.text);
      ensure(texts.includes(ALPHA.error) && texts.includes(LONG), `Errors only shows ${JSON.stringify(texts)}, without the error of lab-logs and its long line`);
      ensure(!texts.includes(ALPHA.warning), "Errors only shows a warning");
    }
  }
  const total = (await readLog(page, base({ lines: 1000 }), true)).length;
  ensure(total >= BULK_LINES + 13, `the log holds ${total} lines of lab-logs, fewer than the ${BULK_LINES + 13} that the root steps wrote`);

  // Words are searched for as typed: a dot is a dot, a bracket is a bracket, and capitals match case.
  // The lines are the ones that lab-logs wrote, since a word may be in the machine's own lines too.
  const words = async (text, expected, what) => {
    await reading(page, () => field(page, "Words in the line").fill(text));
    const records = await sameAsApi(page, argsFor(scope, { grep: literal(text) }), what);
    const texts = records.filter((record) => record.name === "lab-logs").map((record) => record.text);
    ensure(JSON.stringify(texts) === JSON.stringify(expected), `${what}: the lines of lab-logs are ${JSON.stringify(texts)}, not ${JSON.stringify(expected)}`);
  };
  await words(LITERAL, [LITERAL], "Words with the characters of a pattern");
  await words("lab-logs a.b", [LITERAL], "Words with a dot");
  await words("lab-logs capital word", ["lab-logs Capital word", "lab-logs capital word"], "Words in lower case");
  await words("lab-logs Capital word", ["lab-logs Capital word"], "Words with a capital");

  // Folded lines are one row that says how often they came.
  const repeated = "lab-logs repeated line";
  await reading(page, () => field(page, "Words in the line").fill(repeated));
  const lines = await sameAsApi(page, argsFor(scope, { grep: literal(repeated) }), "the repeated lines");
  ensure(lines.filter((record) => record.name === "lab-logs").length === 4, "the log does not hold the four lines that repeat");
  await page.getByLabel("Fold repeated lines", { exact: true }).check();
  await sameAsApi(page, argsFor(scope, { grep: literal(repeated) }), "the repeated lines, folded", { fold: true });
  const row = (await shownRows(page)).find((one) => one.from === "lab-logs");
  ensure(row?.repeated === 4, `the four lines that repeat are ${JSON.stringify(row)} when folded, not one row of 4`);
  await look(t, page, "logs-folded");
  await page.getByLabel("Fold repeated lines", { exact: true }).uncheck();
  await reading(page, () => field(page, "Words in the line").fill(""));
}

/** From: the machine itself and a service that wrote lines, each against the API, over the week before the script began. */
async function sources(t, page, scope) {
  const week = { since: scope.until - 7 * 86_400, until: scope.until };
  await goTo(t, page, addressFor(week));
  await reading(page, () => field(page, "From").selectOption({ label: "The machine itself" }));
  const kernel = await sameAsApi(page, argsFor(week, { kernel: true }), "The machine itself");
  ensure(kernel.length > 0 && kernel.every((record) => record.from === "kernel"), "the machine itself shows lines that are not the kernel's, or none");

  // The service with most lines in the week, among those the From filter offers.
  const offered = await field(page, "From").locator("option").allTextContents();
  const everything = await readLog(page, argsFor(week, { lines: 1000 }), true);
  const counts = new Map();
  for (const record of everything) if (record.from === "service" && offered.includes(record.name)) counts.set(record.name, (counts.get(record.name) ?? 0) + 1);
  const [service] = [...counts].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map(([name]) => name);
  if (service === undefined) {
    console.log(`${t.engine}: no service the filter offers has a line in the week, so its filter by a service is skipped`);
    return;
  }
  await reading(page, () => field(page, "From").selectOption({ label: service }));
  const lines = await sameAsApi(page, argsFor(week, { service }), `From ${service}`);
  ensure(lines.length > 0, `From ${service} shows no line`);
  ensure(addressOf(page).get("service") === service, `the address does not name ${service} after it is chosen under From`);
}

/** Times in: your zone, the server's and UTC, each row and each opened row's three stamps. */
async function zones(t, page, scope) {
  await goTo(t, page, addressFor(scope, { q: "lab-logs alpha" }));
  const records = await sameAsApi(page, argsFor(scope, { grep: "lab-logs alpha" }), "four lines of alpha");
  ensure(records.length === 4, `the four lines of alpha are ${records.length} lines in the log`);
  const about = await fetchAsPage(page, "GET", "/api/1/console/about");
  ensure(about.status === 200, `${ROUTES.about} was answered ${about.status}: ${about.text}`);
  const server = typeof about.json?.zone === "string" && about.json.zone !== "" ? about.json.zone : null;
  const choices = { you: ZONE, utc: "UTC", ...(server ? { server } : {}) };
  ensure(((await field(page, "Times in").locator('option[value="server"]').count()) === 1) === (server !== null), "Times in offers the server's zone only when the server has one");

  for (const [choice, zone] of Object.entries(choices)) {
    await field(page, "Times in").selectOption(choice);
    sameRows(await shownRows(page), rowsFor(records, zone, false), `Times in ${choice}`);
    ensure(addressOf(page).get("zone") === (choice === "you" ? null : choice), `the address says zone ${addressOf(page).get("zone")} after ${choice} is chosen`);
    await t.see(page.getByText(`Times are in ${zone}.`));
  }

  // An opened row says the line's time in every zone, to the microsecond, and where it came from.
  await field(page, "Times in").selectOption("you");
  const record = records.find((one) => one.text === ALPHA.error);
  ensure(record !== undefined, `the log has no line "${ALPHA.error}"`);
  await rowButtons(page).filter({ hasText: ALPHA.error }).click();
  const details = await detailsOf(page);
  ensure(details !== null, "a row did not open its details");
  ensure(details[`Logged, Your zone (${ZONE})`] === stampOf(record.time, ZONE), `the details say ${details[`Logged, Your zone (${ZONE})`]} for your zone, not ${stampOf(record.time, ZONE)}`);
  ensure(details["Logged, UTC"] === stampOf(record.time, "UTC"), `the details say ${details["Logged, UTC"]} for UTC, not ${stampOf(record.time, "UTC")}`);
  if (server) ensure(details[`Logged, The server's zone (${server})`] === stampOf(record.time, server), `the details say ${details[`Logged, The server's zone (${server})`]} for the server's zone`);
  ensure(details.Importance === "error" && details.From === `program ${record.name}`, `the details say ${JSON.stringify(details)} of a line of a program, importance error`);
  const [process, user] = [record.process, record.user].map((number) => (number === null || number === undefined ? undefined : String(number)));
  ensure(details.Process === process && details.User === user, `the details say process ${details.Process} and user ${details.User}, not ${process} and ${user}`);
  ensure(details.Start === record.boot && details.Number === String(record.seq), `the details say start ${details.Start} number ${details.Number}, not ${record.boot} and ${record.seq}`);
  await lookAll(t, page, "logs-opened");
}

/** A custom range in New York's clock across both changes of summer time: the address and the command hold its UTC times. */
async function summerTime(t, page) {
  await goTo(t, page, `/logs?${new URLSearchParams({ when: "1h", priority: "debug" })}`);
  await showCommands(t, page);
  await t.gone(field(page, "Start date"));
  await reading(page, () => field(page, "When").selectOption({ label: "Custom range" }));
  await t.see(field(page, "Start date"));
  ensure(addressOf(page).get("when") === "custom" && /^[1-9]\d+$/.test(addressOf(page).get("since") ?? ""), `the address after Custom range is ${addressOf(page)}, which holds no start`);
  await lookAll(t, page, "logs-custom-range");

  const cases = [
    // The clocks go back at 02:00 on 2 November 2025: 00:30 is summer time and 03:30 is not.
    { what: "the end of summer time", since: ["2025-11-02", "00:30:00"], until: ["2025-11-02", "03:30:00"], from: "2025-11-02T04:30:00Z", to: "2025-11-02T08:30:00Z" },
    // The clocks go forward at 02:00 on 9 March 2025: 02:30 does not exist, and is 03:30.
    { what: "the start of summer time, with a time the clocks skip", since: ["2025-03-09", "02:30:00"], until: ["2025-03-09", "04:30:00"], from: "2025-03-09T07:30:00Z", to: "2025-03-09T08:30:00Z" },
    // 01:30 on 2 November 2025 is on the clocks twice: the first is summer time.
    { what: "a time the clocks show twice", since: ["2025-11-02", "01:30:00"], until: ["2025-11-02", "01:45:00"], from: "2025-11-02T05:30:00Z", to: "2025-11-02T05:45:00Z" },
  ];
  for (const one of cases) {
    const [since, until] = [Date.parse(one.from), Date.parse(one.to)];
    // A field that is filled while the range is not whole yet is said to be wrong, and is not wrong once the last is filled.
    await field(page, "Start date").fill(one.since[0]);
    await field(page, "Start time").fill(one.since[1]);
    await field(page, "End date").fill(one.until[0]);
    await field(page, "End time").fill(one.until[1]);
    await page.waitForFunction(([from, to]) => {
      const query = new URLSearchParams(location.search);
      return query.get("since") === from && query.get("until") === to;
    }, [String(since), String(until)], { timeout: 20_000 });
    await settled(page);
    const wanted = { since: since / 1000, until: until / 1000 };
    await commandWith(page, [one.from.replace("T", " ").replace("Z", " UTC"), one.to.replace("T", " ").replace("Z", " UTC")]);
    await sameAsApi(page, argsFor(wanted), `the range of ${one.what}`);
    ensure((await page.locator(".field .msg:not(:empty)").count()) === 0, `a field says it is wrong after the range of ${one.what}`);
  }
}

/** Show older reads the lines before the oldest with none twice; Save as file writes what is shown, line by line, against the log's records. */
async function olderAndSaved(t, page, scope) {
  await goTo(t, page, addressFor(scope, { q: "lab-logs" }));
  const rowsBefore = await rowButtons(page).count();
  ensure(rowsBefore === PAGE_LINES, `the first page holds ${rowsBefore} lines, not ${PAGE_LINES}`);
  await page.getByRole("button", { name: "Show older lines", exact: true }).click();
  await page.waitForFunction((before) => document.querySelectorAll(".loglist .lrow").length > before, rowsBefore, { timeout: 30_000 });
  await t.see(page.getByText("These are the oldest lines in this time."));
  const all = await sameAsApi(page, argsFor(scope, { grep: "lab-logs", lines: 1000 }), "the lines of lab-logs after Show older");
  ensure(all.length > PAGE_LINES, `the lines of lab-logs are ${all.length}, which one page holds`);
  await t.gone(page.getByRole("button", { name: "Show older lines", exact: true }));

  for (const choice of ["you", "utc"]) {
    const zone = choice === "you" ? ZONE : "UTC";
    await field(page, "Times in").selectOption(choice);
    const before = wallOf(Date.now(), zone);
    const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), page.getByRole("button", { name: "Save as file…", exact: true }).click()]);
    const after = wallOf(Date.now(), zone);
    const file = await download.path();
    ensure(file !== null, `the browser kept no file for ${download.suggestedFilename()}`);
    const text = await readFile(file, "utf8");
    const named = /^log-.+-(\d{8})-(\d{4})\.txt$/.exec(download.suggestedFilename());
    ensure(named !== null, `the saved file is called ${download.suggestedFilename()}, not log-HOST-YYYYMMDD-HHMM.txt`);
    const stamps = [before, after].map((wall) => `${wall.date.replaceAll("-", "")}${wall.clock.slice(0, 5).replace(":", "")}`);
    ensure(stamps.includes(`${named[1]}${named[2]}`), `the file's name has the time ${named[1]}-${named[2]}, which in ${zone} was ${stamps.join(" or ")}`);
    ensure(text.endsWith("\n"), "the saved text does not end with a line's end");
    const lines = text.slice(0, -1).split("\n");
    ensure(lines.length === all.length, `the saved file has ${lines.length} lines and the log ${all.length}`);
    all.forEach((record, at) => {
      const wanted = `${stampOf(record.time, zone)} ${record.level} ${whoOf(record)}: ${record.text}`;
      ensure(lines[at] === wanted, `line ${at + 1} of the saved file is "${lines[at]}", and the log's record says "${wanted}"`);
    });
    await t.see(page.getByText(`Saved ${all.length} lines as ${download.suggestedFilename()}.`));
  }
  await field(page, "Times in").selectOption("you");
}

/** The keys move between rows, with one row in the tab order. */
async function keys(t, page, scope) {
  await goTo(t, page, addressFor(scope, { q: "lab-logs alpha" }));
  const rows = rowButtons(page);
  ensure((await rows.count()) === 4, "the lines of alpha are not four rows");
  ensure((await page.locator('.loglist .lrow[tabindex="0"]').count()) === 1, "more than one row is in the tab order");
  await page.locator('.loglist .lrow[tabindex="0"]').focus();
  const focused = (at) => rows.nth(at).evaluate((row) => row === document.activeElement);
  await page.keyboard.press("ArrowUp");
  ensure(await focused(2), "ArrowUp did not move to the row above the newest");
  await page.keyboard.press("Home");
  ensure(await focused(0), "Home did not move to the first row");
  await page.keyboard.press("End");
  ensure(await focused(3), "End did not move to the last row");
}

/** A row's "Show 5 minutes around this" shows the lines either side of it, of a line the ticker wrote a little while ago. */
async function around(t, page) {
  await goTo(t, page, `/logs?${new URLSearchParams({ when: "15m", priority: "error", q: "lab-follow tick" })}`);
  // The ticker writes a line every two seconds from before the browser began.
  for (let tries = 0; (await rowButtons(page).count()) < 12 && tries < 30; tries += 1) {
    await sleep(5000);
    await reading(page, () => page.getByRole("button", { name: "Read again", exact: true }).click());
  }
  const ticks = await ticksOf(page);
  ensure(ticks.length >= 12, `the list holds ${ticks.length} lines of lab-ticker, which writes one every two seconds`);
  const words = `lab-follow tick ${ticks.at(-10)}`;
  const now = Math.floor(Date.now() / 1000);
  const record = (await readLog(page, argsFor({ since: now - 900, until: now + 5 }, { priority: "error", grep: words }), true)).find((one) => one.text === words);
  ensure(record !== undefined, `the log has no line "${words}"`);
  const ofRecord = rowButtons(page).filter({ hasText: new RegExp(`${words}$`) });
  await ofRecord.click();
  await page.getByRole("button", { name: "Show 5 minutes around this", exact: true }).click();
  const seconds = Math.floor(microsOf(record.time) / 1_000_000);
  await page.waitForFunction((since) => new URLSearchParams(location.search).get("since") === since, String((seconds - 300) * 1000), { timeout: 20_000 });
  await settled(page);
  const query = addressOf(page);
  ensure(query.get("when") === "custom" && query.get("until") === String((seconds + 301) * 1000) && query.get("q") === null, `the address after Show 5 minutes around this is ${query}, which is not five minutes either side with no words`);
  await t.see(ofRecord);

  // The lines shown are lines of that time. A line the ticker wrote after the list was read is not in it, so the list is held to the API's whole read of the time.
  const range = { since: seconds - 300, until: seconds + 301 };
  const wanted = new Set(rowsFor(await readLog(page, argsFor(range, { priority: "error", lines: 1000 }), true), ZONE, false).map((row) => JSON.stringify(row)));
  const stray = (await shownRows(page)).filter((row) => !wanted.has(JSON.stringify(row)));
  ensure(stray.length === 0, `the five minutes around a line show ${JSON.stringify(stray[0])}, which the API's read of the time does not hold`);
}

// --- Following ---

/** The numbers of the lines of lab-ticker that the list holds. */
const ticksOf = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll(".loglist .lrow .tx")]
      .map((text) => /lab-follow tick (\d+)$/.exec(text.textContent.trim()))
      .filter((found) => found !== null)
      .map((found) => Number(found[1])),
  );

/** Waits until the list holds `count` ticks above `above`, and gives all that are above it, in order. */
async function ticksAbove(page, above, count, seconds = 45) {
  await page.waitForFunction(
    ([floor, wanted]) =>
      [...document.querySelectorAll(".loglist .lrow .tx")]
        .map((text) => /lab-follow tick (\d+)$/.exec(text.textContent.trim()))
        .filter((found) => found !== null && Number(found[1]) > floor).length >= wanted,
    [above, count],
    { timeout: seconds * 1000 },
  );
  return (await ticksOf(page)).filter((tick) => tick > above).sort((a, b) => a - b);
}

/** Throws unless `ticks` follow one another with none missed or twice. */
function contiguous(ticks, what) {
  ensure(ticks.every((tick, at) => at === 0 || tick === ticks[at - 1] + 1), `${what}: the ticks are ${ticks.join(", ")}, which are not one after the other`);
}

/** The tab as the page sees it when it is hidden, and when it is seen again. */
const hideTab = (page) =>
  page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
const showTab = (page) =>
  page.evaluate(() => {
    delete document.visibilityState;
    document.dispatchEvent(new Event("visibilitychange"));
  });

/** Follow: lines come as the machine writes them, Pause stops them, a second window takes the follow, and Lock ends it with its sentence. */
async function following(t, page) {
  await goTo(t, page, `/logs?${new URLSearchParams({ when: "15m", priority: "error", q: "lab-follow tick" })}`);
  await showCommands(t, page);
  const floor = Math.max(0, ...(await ticksOf(page)));
  await followButton(page).click();
  await t.see(live(page));
  await t.see(page.getByRole("button", { name: "Pause", exact: true }));
  const shown = await commandWith(page, ["--follow"]);
  ensure(shown.includes("lab-follow"), `the command of a follow is "${shown}", which does not search for lab-follow`);
  const came = await ticksAbove(page, floor, 3);
  contiguous(came, "lines that came while following");
  await look(t, page, "logs-following");

  // A second window of the same account follows, and the first is told that it was replaced.
  const second = await page.context().newPage();
  await second.goto(page.url());
  await t.see(heading(second));
  await followButton(second).click();
  await t.see(live(second));
  await t.see(page.getByText("Following stopped: you began following in another window.", { exact: true }));
  await t.gone(live(page));
  await t.see(page.getByRole("button", { name: "Follow again", exact: true }));
  await look(t, page, "logs-stopped");
  await page.getByRole("button", { name: "Follow again", exact: true }).click();
  await t.see(live(page));
  await t.see(second.getByText("Following stopped: you began following in another window.", { exact: true }));
  await second.close();

  // Pause leaves the lines as they are.
  await ticksAbove(page, Math.max(...(await ticksOf(page))), 1);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  await t.gone(live(page));
  await t.see(page.getByRole("button", { name: "Follow new lines", exact: true }));
  const paused = await ticksOf(page);
  await sleep(6500);
  ensure(JSON.stringify(await ticksOf(page)) === JSON.stringify(paused), "lines came after Pause");

  // Lock ends a follow of the whole log, and the page says why; its banner asks to unlock again.
  await followButton(page).click();
  await t.see(live(page));
  await page.getByRole("banner").getByRole("button", { name: "Lock", exact: true }).click();
  await t.see(page.getByText("Following stopped: you locked the console.", { exact: true }));
  await t.gone(live(page));
  await t.see(page.getByRole("button", { name: "Follow again", exact: true }));
  await t.see(banner(page).getByText(`${OWN} Unlock to read the whole log.`));
  await settled(page);
  await lookAll(t, page, "logs-locked-after-follow");
}

/** A tab hidden for over a minute closes the follow; when it is seen again, the lines of the gap are read and none is missed. */
async function hiddenTab(t, page) {
  await unlockFromBanner(t, page);
  await page.getByRole("button", { name: "Follow again", exact: true }).click();
  await t.see(live(page));
  // Lines between the read and the start of a follow are not in the list, so the count begins at a line that came by the stream.
  const read = Math.max(0, ...(await ticksOf(page)));
  await ticksAbove(page, read, 2);
  const floor = Math.max(...(await ticksOf(page)));
  await hideTab(page);
  // The page closes the follow after a minute of being hidden: the pill goes, and Pause stays.
  await live(page).waitFor({ state: "hidden", timeout: 90_000 });
  await t.see(page.getByRole("button", { name: "Pause", exact: true }));
  await sleep(6000);
  await showTab(page);
  await t.see(live(page));
  // A minute of the stream, and the lines written after it closed, which only the read of the gap brings.
  const ticks = await ticksAbove(page, floor, 32, 60);
  contiguous(ticks, "the lines of a tab that was hidden for a minute");
  ensure(ticks[0] === floor + 1, `the first line after the tab was seen is tick ${ticks[0]}, not ${floor + 1}: the gap was not read`);
  const last = ticks.at(-1);
  contiguous(await ticksAbove(page, last, 2), "the lines that come after the gap");
  await page.getByRole("button", { name: "Pause", exact: true }).click();
}

// --- The command the page shows, run in the administrator's terminal ---


/** Waits until the focused pane's screen has a line that `pattern` matches, and gives the match. */
async function screenLine(page, pattern) {
  const handle = await page.waitForFunction(
    ([source]) => {
      const text = document.querySelector(".tr-pane.tr-focused .tr-screen")?.innerText ?? "";
      const found = text.split("\n").map((line) => line.trim()).find((line) => new RegExp(source).test(line));
      return found ?? false;
    },
    [pattern.source],
    { timeout: 60_000 },
  );
  return pattern.exec(await handle.jsonValue());
}

/** The command that the page shows, run in an administrator's terminal, says as many lines as the page shows. */
async function shownCommandRun(t, page, scope) {
  await goTo(t, page, addressFor(scope, { q: "lab-logs", priority: "warning" }));
  const rows = await shownRows(page);
  ensure(rows.length >= 6, `the page shows ${rows.length} lines of lab-logs at warning, fewer than the root steps wrote`);
  await showCommands(t, page);
  const shown = (await commandWith(page, ["lab-logs"])).trim();
  // The pane is root's already, which `elevate` is for.
  const command = shown.replace(/^elevate\s+/, "");

  await page.goto(new URL("/terminal", page.url()).href);
  // A menu opened before the page's own terminal is connected can close
  // as the page draws that terminal.
  await t.see(page.locator(".tr-pane").first());
  await t.see(page.getByRole("status").filter({ hasText: "Connected" }));
  await page.getByRole("button", { name: "More terminal actions" }).click();
  await page.getByRole("menuitem", { name: "New administrator terminal", exact: true }).click();
  const pane = page.getByRole("group", { name: /administrator$/ });
  await t.see(pane);
  await page.locator(".tr-pane .tr-body").first().click();
  await page.waitForFunction(() => document.querySelector(".tr-pane.tr-focused") !== null);
  await page.keyboard.type(`echo lab-count-$(${command} | wc -l)`);
  await page.keyboard.press("Enter");
  const [, count] = await screenLine(page, /^lab-count-(\d+)$/);
  ensure(Number(count) === rows.length, `the command "${shown}" says ${count} lines, and the page shows ${rows.length}`);

  // Lock asks first and ends the pane.
  const lock = page.getByRole("banner").getByRole("button", { name: "Lock", exact: true });
  await lock.click();
  const question = page.getByRole("dialog", { name: /^Lock and end \d+ administrator shell/ });
  await t.see(question);
  await press(page, question.getByRole("button", { name: "Lock and end", exact: true }), "/api/1/lock");
  await t.see(page.getByRole("banner").getByText("Limited access"));
}

// --- The people ---

/** An administrator: locked, unlocked, each filter, the zones, the ranges, older lines, the saved file, following and the command. */
async function administrator(t) {
  const name = adminOf(t.engine);
  const { page, seen } = await openConsole(t, "/", { timezoneId: ZONE });
  await signInAs(t, page, name);
  const session = await sessionOf(page);
  ensure(session.name === name && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);
  const now = Math.floor(Date.now() / 1000);
  const scope = { since: now - 3 * 3600, until: now };

  // Locked, the page says so, shows the administrator's own lines and none of the log's, and the API refuses the log.
  await goTo(t, page, addressFor(scope, { q: "lab-logs" }));
  await t.see(banner(page).getByText(`${OWN} Unlock to read the whole log.`));
  await t.see(banner(page).getByRole("button", { name: "Unlock", exact: true }));
  await t.see(page.getByText("No lines match. Try a longer time or less importance."));
  await sameAsApi(page, argsFor(scope, { grep: "lab-logs" }), "an administrator's own lines while locked", { all: false });
  ensure((await rowButtons(page).count()) === 0, "a locked administrator sees lines of lab-logs");
  mustBe(await fetchAsPage(page, "GET", `/api/1/logs/all?${new URLSearchParams(argsFor(scope))}`), 403, "ADM-106", "the whole log, locked");
  await lookAll(t, page, "logs-locked");

  await unlockFromBanner(t, page);
  await t.see(rowButtons(page).first());
  await sameAsApi(page, argsFor(scope, { grep: "lab-logs" }), "the whole log after Unlock");
  await lookAll(t, page, "logs-lines");

  await filters(t, page, scope);
  await sources(t, page, scope);
  await zones(t, page, scope);
  await summerTime(t, page);
  await olderAndSaved(t, page, scope);
  await keys(t, page, scope);
  await around(t, page);
  await following(t, page);
  await hiddenTab(t, page);
  await shownCommandRun(t, page, scope);
  mustHaveCalled(seen, Object.values(ROUTES));
  await signOut(t, page);
  await page.context().close();
}

/** Opens the terminal page of the reader, which makes its first terminal, and waits for the shell. */
async function readerShell(t, page, host, name) {
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.getByRole("tab", { name: `${name}@${host}`, exact: false }).first());
  await t.see(page.locator(".tr-pane").first());
  await page.locator(".tr-pane .tr-body").first().click();
  await page.waitForFunction(() => document.querySelector(".tr-pane.tr-focused") !== null);
}

/** A reader sees the lines of their own programs, which their terminal writes, and nothing of the machine's. */
async function reader(t) {
  const name = readerOf(t.engine);
  const { page, seen } = await openConsole(t, "/", { timezoneId: ZONE });
  await signInAs(t, page, name);
  const { host } = await sessionOf(page);
  const now = Math.floor(Date.now() / 1000);
  const before = { since: now - 3 * 3600, until: now };

  await goTo(t, page, addressFor(before, { q: "lab-logs" }));
  await t.see(banner(page).getByText(`${OWN} The machine's log is for administrators.`));
  await t.gone(banner(page).getByRole("button", { name: "Unlock", exact: true }));
  await sameAsApi(page, argsFor(before, { grep: "lab-logs" }), "a reader's lines of lab-logs", { all: false });
  ensure((await rowButtons(page).count()) === 0, "a reader sees a line of lab-logs");
  mustBe(await fetchAsPage(page, "GET", `/api/1/logs/all?${new URLSearchParams(argsFor(before))}`), 403, "ADM-122", "the whole log, for a reader");
  mustBe(await fetchAsPage(page, "GET", "/api/1/logs/follow?all=true"), 403, "ADM-122", "a follow of the whole log, for a reader");

  // The reader's own terminal writes a line, which is the only one the reader sees.
  await readerShell(t, page, host, name);
  await page.keyboard.type("logger -t lab-reader -p user.err 'lab-reader says hello'");
  await page.keyboard.press("Enter");
  await page.keyboard.type("echo lab-done-$((1 + 1))");
  await page.keyboard.press("Enter");
  await screenLine(page, /^lab-done-2$/);
  await sleep(1500);

  const scope = { since: now - 3600, until: Math.floor(Date.now() / 1000) + 5 };
  await goTo(t, page, addressFor(scope));
  const own = await sameAsApi(page, argsFor(scope), "a reader's own lines", { all: false });
  const texts = (await shownRows(page)).map((row) => row.text);
  ensure(texts.includes("lab-reader says hello"), `a reader's lines are ${JSON.stringify(texts)}, which do not hold the line the reader wrote`);
  ensure(own.every((record) => record.from !== "kernel"), "a reader sees a line of the kernel");
  ensure(texts.every((text) => !text.startsWith("lab-logs") && !text.startsWith("lab-follow")), "a reader sees a line that a root step wrote");
  await look(t, page, "logs-reader");
  mustHaveCalled(seen, [ROUTES.own, ROUTES.about, ROUTES.services]);
  await signOut(t, page);
  await page.context().close();
}


/** At 390 wide: the filters fold under a button, When and Importance stay in view, rows wrap, and nothing scrolls sideways. */
async function phone(t) {
  const name = adminOf(t.engine);
  const { page } = await openConsole(t, "/", { viewport: PHONE, timezoneId: ZONE });
  await signInAs(t, page, name, { phone: true });
  const now = Math.floor(Date.now() / 1000);
  const scope = { since: now - 3 * 3600, until: now };
  await goTo(t, page, addressFor(scope, { q: "lab-logs" }));
  await unlockFromBanner(t, page);
  await t.see(rowButtons(page).first());
  ensure((await sidewaysScroll(page)) === 0, `the phone's Logs scrolls ${await sidewaysScroll(page)} px sideways`);

  const toggle = page.getByRole("button", { name: "Filters", exact: true });
  await t.see(toggle);
  ensure((await toggle.getAttribute("aria-expanded")) === "false", "the filters of a phone are open before Filters is pressed");
  await t.see(field(page, "When"));
  await t.see(field(page, "Importance"));
  ensure(!(await field(page, "Words in the line").isVisible()), "Words is in view on a phone before Filters is pressed");
  await toggle.click();
  ensure((await toggle.getAttribute("aria-expanded")) === "true", "Filters did not open the filters");
  await t.see(field(page, "Words in the line"));
  await t.see(field(page, "Times in"));
  await look(t, page, "logs-phone-filters");
  await toggle.click();
  await t.gone(field(page, "Words in the line"));

  // An opened row's buttons fill the width.
  await rowButtons(page).first().click();
  const width = await page.evaluate(() => {
    const box = document.querySelector(".loglist .ldet:not([hidden]) .btns").getBoundingClientRect();
    const buttons = [...document.querySelectorAll(".loglist .ldet:not([hidden]) .btns .btn")].map((button) => button.getBoundingClientRect().width);
    return { box: box.width, narrowest: Math.min(...buttons) };
  });
  ensure(width.narrowest >= width.box - 2, `the buttons of an opened row are ${Math.round(width.narrowest)} px wide in ${Math.round(width.box)}, which they should fill`);
  const small = await smallTargets(page);
  ensure(small.length === 0, `these controls of the Logs page are under 44 px on a phone: ${small.join(", ")}`);
  ensure((await sidewaysScroll(page)) === 0, `the phone's Logs with a row open scrolls ${await sidewaysScroll(page)} px sideways`);
  await lookAll(t, page, "logs-phone");

  // Following works the same.
  await rowButtons(page).first().click();
  await goTo(t, page, `/logs?${new URLSearchParams({ when: "15m", priority: "error", q: "lab-follow tick" })}`);
  const floor = Math.max(0, ...(await ticksOf(page)));
  await followButton(page).click();
  await t.see(live(page));
  contiguous(await ticksAbove(page, floor, 2), "lines that came while following on a phone");
  await followButton(page).click();
  await t.gone(live(page));
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
