// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/chart-to-logs drives in each engine, with the lab's
// browser runner (lab repository, docs/test-files.md, "Browser tests"): the
// view from a chart (design 6), at "/logs/chart/{chart}". An administrator
// unlocks, restarts metricsd from the console, which is a change in the
// journal and writes the history, and opens the CPU chart from the Usage card
// of the Overview. Two minutes of a machine at rest and then three of a
// service that spends a processor, lab-burn, are in the history that
// chart-to-logs.toml's root steps made. The arrow keys move the pick one
// point at a time, Enter chooses the minute of the burn, and the page says
// what the machine says of it: "Busiest at" ranks lab-burn first, "What
// moved most" lists it risen to that minute's CPU (a service that was busy
// as the machine started may rank above it, as the hour before is the few
// minutes since the boot), and the Logs card lists the warnings of that minute, each
// held to the API's read of the same minute (its time in the person's zone,
// importance, from and text), and so does "Open in Logs with this time". The
// restart is a mark on the chart at its minute; choosing the mark shows its
// row of the journal. A reader sees the same moment with the lines of their
// own programs, and a phone at 390 by 844 has the page in one column. Each
// look is dark, light, high contrast, forced colours and less motion, checked
// by axe.
//
// The minute of the burn is the one in which lab-burn's CPU was most, as
// `GET /api/1/metrics/around/{at}` says of the last hour; the key presses
// take the pick to that minute's point of `GET /api/1/metrics/usage/{range}`.

import {
  ensure,
  fetchAsPage,
  look,
  lookAll,
  mustBe,
  openConsole,
  press,
  sessionOf,
  sidewaysScroll,
  signOut,
} from "./console.mjs";
import {
  argsFor,
  commandWith,
  microsOf,
  OWN,
  PASSWORD,
  PHONE,
  readLog,
  rowsFor,
  sameRows,
  settled,
  showCommands,
  shownRows,
  signInAs,
  smallTargets,
  ZONE,
} from "./log-lines.mjs";

const adminOf = (engine) => `wc-chart-${engine}`;
const readerOf = (engine) => `wc-chart-reader-${engine}`;

// The service that chart-to-logs.toml starts, and the tag of its warnings.
const BURNER = "lab-burn";

// How far below the most CPU a minute of the burn may be and still count as
// one of its busiest, in percent of a processor.
const BURN_TIE = 1;
const MARK = "lab-mark burning";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them. `{range}` and `{at}` are a range's word and a Unix time.
const ROUTES = {
  usage: "GET /api/1/metrics/usage/{range}",
  around: "GET /api/1/metrics/around/{at}",
  changes: "GET /api/1/changes",
  about: "GET /api/1/console/about",
  own: "GET /api/1/logs",
  all: "GET /api/1/logs/all",
  show: "POST /api/1/show",
  unlock: "POST /api/1/unlock",
};

/** Throws unless the context has called each of `routes`, written as the table names them. */
function mustHaveCalled(seen, routes) {
  const missing = routes.filter((route) => {
    const pattern = new RegExp(`^${route.replace(/\{[a-z]+\}/g, "[^/]+")}$`);
    return ![...seen].some((call) => pattern.test(call));
  });
  ensure(missing.length === 0, `the test never called ${missing.join(", ")}`);
}

/** A Unix time in seconds as `log` reads it: "2026-11-14 09:30:00 UTC". */
const utcWords = (seconds) => `${new Date(seconds * 1000).toISOString().slice(0, 19).replace("T", " ")} UTC`;

// The minute of the burn, found by the administrator and used by the reader and the phone.
let burn = null;

// --- What the page shows ---

const heading = (page, words) => page.getByRole("heading", { name: words, level: 1 });
const slider = (page) => page.getByRole("slider", { name: "Choose a moment on the CPU chart" });
const chosen = (page) => page.getByText(/^Chosen: /);
const marks = (page) => page.locator("ul.marks button[data-at]");
const addressOf = (page) => new URL(page.url()).searchParams;

/** The rows of a table in the card `id`, as the words of their cells. */
const tableOf = (page, id) =>
  page.evaluate(
    (card) => [...document.querySelectorAll(`#${card} tbody tr`)].map((row) => [...row.children].map((cell) => cell.textContent.trim())),
    id,
  );

/** The card's items of the timeline, as the words of each. */
const timelineOf = (page) => page.evaluate(() => [...document.querySelectorAll("#ov-happened .tl-item")].map((item) => item.textContent.replace(/\s+/g, " ").trim()));

/** Unlocks from the header, as a person does, and waits for the header to say so. */
async function unlockFromHeader(t, page) {
  const header = page.getByRole("banner");
  await header.getByRole("button", { name: "Unlock", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes", exact: true });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.see(header.getByText("Unlocked ·"));
}

/** What `GET path` answers, which must be a list of series. */
async function seriesOf(page, path) {
  const answer = await fetchAsPage(page, "GET", path);
  ensure(answer.status === 200 && Array.isArray(answer.json), `${path} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The change that restarts metricsd, made by the console as the page does: its answer and its row of the journal. */
async function restartMetricsd(page, name) {
  const answer = await fetchAsPage(page, "POST", "/api/1/services/metricsd/restart", {});
  ensure(answer.status === 200 && answer.json?.done === true, `the restart of metricsd was answered ${answer.status}: ${answer.text}`);
  const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=20");
  ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
  const row = list.json.changes.find((one) => one.action === "service.restart" && one.who === name);
  ensure(row !== undefined && Number.isInteger(row.at), `the journal has no row of the restart made by ${name}: ${list.text}`);
  return row;
}

/**
 * The minute of the last hour in which the burner had the most CPU, as the
 * history says it: `{at, value}`. The burn keeps a processor busy for three
 * minutes, so several minutes have all of one, a hair apart; of those the
 * first is taken. A point is the reading taken in its minute, of the
 * minute before it, so the last of the burn may be read once the burn has
 * stopped, in a minute that holds no mark.
 */
async function burnMinute(page) {
  const now = Math.floor(Date.now() / 1000);
  const series = await seriesOf(page, `/api/1/metrics/around/${now}?range=1h`);
  const cpu = series.find((one) => one.series === `service.cpu{${BURNER}}`);
  ensure(cpu !== undefined, `the history has no CPU of ${BURNER}, only ${series.map((one) => one.series).join(", ")}: the root step that burns did not run, or metricsd did not write what it read`);
  const points = cpu.points.filter(([time, value]) => Number.isFinite(time) && typeof value === "number" && time <= now);
  const most = Math.max(...points.map(([, value]) => value));
  const [at, value] = points.find(([, value]) => value >= most - BURN_TIE);
  ensure(value >= 50, `${BURNER} had at most ${value} percent of a processor in the last hour, which spent all of one for three minutes`);
  return { at, value };
}

/** The times of the points of the CPU chart of the last hour, oldest first, as the chart draws them. */
async function chartTimes(page) {
  const cpu = (await seriesOf(page, "/api/1/metrics/usage/1h")).find((one) => one.series === "cpu");
  ensure(cpu !== undefined, "the usage of the last hour has no CPU");
  return [...new Set(cpu.points.filter(([, value]) => typeof value === "number" && Number.isFinite(value)).map(([time]) => time))].sort((a, b) => a - b);
}

// --- Steps ---

/** From the Overview's Usage card to the chart in its range, and from there to the last hour. */
async function openChart(t, page) {
  await page.goto(new URL("/", page.url()).href);
  const link = page.getByRole("link", { name: "Open CPU", exact: true });
  await t.see(link);
  await link.click();
  await t.see(heading(page, "CPU, last 24 hours"));
  await t.see(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Overview", exact: true }));
  await t.see(page.getByText("Choose a moment on the chart, with the arrow keys and Enter or a click, to see what happened around it."));
  await t.gone(chosen(page));
  await t.see(slider(page));
  await page.getByRole("button", { name: "1 h", exact: true }).click();
  await t.see(heading(page, "CPU, last hour"));
  ensure((await page.getByRole("button", { name: "1 h", exact: true }).getAttribute("aria-pressed")) === "true", "the 1 h button is not pressed after it is chosen");
  ensure(addressOf(page).get("range") === "1h", `the address says range ${addressOf(page).get("range")}, not 1h`);
  await t.see(slider(page));
}

/** The keys move the pick a point at a time, and Enter chooses the minute of the burn, whose cards the page then reads. */
async function pickTheBurn(t, page) {
  const times = await chartTimes(page);
  const index = times.indexOf(burn.at);
  ensure(index >= 0, `the chart of the last hour has no point at ${utcWords(burn.at)}, where ${BURNER} was busiest: its points are ${times.map(utcWords).join(", ")}`);
  const valueNow = async () => Number(await slider(page).getAttribute("aria-valuenow"));
  await slider(page).focus();
  await page.keyboard.press("End");
  ensure((await valueNow()) === times.length - 1, `End put the pick at ${await valueNow()}, not the last of ${times.length} points`);
  await page.keyboard.press("Home");
  ensure((await valueNow()) === 0, `Home put the pick at ${await valueNow()}, not the first point`);
  if (index > 0) {
    await page.keyboard.press("ArrowRight");
    ensure((await valueNow()) === 1, `ArrowRight put the pick at ${await valueNow()}, not one point on`);
  }
  for (let at = 2; at <= index; at += 1) await page.keyboard.press("ArrowRight");
  ensure((await valueNow()) === index, `the pick is at ${await valueNow()} after the keys, not at ${index}, the point of ${utcWords(burn.at)}`);
  ensure(/ percent$/.test((await slider(page).getAttribute("aria-valuetext")) ?? ""), `the slider says "${await slider(page).getAttribute("aria-valuetext")}", which does not end in a percent`);

  const around = page.waitForResponse((answer) => /^\/api\/1\/metrics\/around\/\d+$/.test(new URL(answer.url()).pathname), { timeout: 30_000 });
  const logs = page.waitForResponse((answer) => new URL(answer.url()).pathname === "/api/1/logs/all", { timeout: 30_000 });
  await page.keyboard.press("Enter");
  const asked = await around;
  ensure(asked.status() === 200, `metrics around was answered ${asked.status()}: ${await asked.text()}`);
  const place = new URL(asked.url());
  ensure(place.pathname === `/api/1/metrics/around/${burn.at}` && place.searchParams.get("range") === "1h", `the page asked ${place.pathname}${place.search}, not for the minute of the burn, ${burn.at}, in the last hour`);
  ensure((await logs).status() === 200, "the whole log was not answered for the minute of the burn");
  ensure(addressOf(page).get("at") === String(burn.at * 1000), `the address says at ${addressOf(page).get("at")} after Enter, not ${burn.at * 1000}`);
  await t.see(chosen(page));
  await t.see(page.locator("#ov-busy tbody tr").first());
  await t.see(page.locator("#ov-moved tbody tr").first());
  await settled(page);
}

/** "Busiest at" puts the burner first, and "What moved most" has it risen. */
async function ranked(t, page) {
  const busy = await tableOf(page, "ov-busy");
  ensure(busy.length > 0 && busy[0][0] === BURNER, `the busiest service at the minute of the burn is ${busy[0]?.[0]}, not ${BURNER}: ${JSON.stringify(busy)}`);
  const percent = Number.parseFloat(busy[0][1]);
  ensure(Math.abs(percent - burn.value) <= 0.6, `Busiest at says ${busy[0][1]} of CPU for ${BURNER}, and the history says ${burn.value}`);
  const changed = await tableOf(page, "ov-moved");
  const row = changed.find(([name]) => name === `${BURNER} CPU`);
  ensure(row !== undefined, `What moved most at the minute of the burn does not list ${BURNER} CPU: ${JSON.stringify(changed)}`);
  const words = /^(?:new, |from ([\d.]+)% to )([\d.]+)%$/.exec(row[1]);
  const risen = words !== null && Math.abs(Number(words[2]) - burn.value) <= 0.6 && (words[1] === undefined || Number(words[1]) < Number(words[2]));
  ensure(risen, `What moved most says "${row[1]}" of ${BURNER}, and the history says ${burn.value}`);
  await t.see(page.locator("#ov-busy h2").getByText(/^Busiest at /));
  await t.see(page.locator("#ov-moved h2").getByText(/^What moved most from /));
}

/** The Logs card lists the warnings of the minute, as the API's read of it does, in the person's zone, and the command it shows holds the minute's UTC times. */
async function minuteOfLogs(t, page) {
  const step = { from: burn.at, to: burn.at + 60 };
  await showCommands(t, page);
  await commandWith(page, [utcWords(step.from - 60), utcWords(step.to)]);
  const inMinute = (record) => {
    const seconds = Math.floor(microsOf(record.time) / 1_000_000);
    return seconds >= step.from && seconds < step.to;
  };
  const wanted = (await readLog(page, argsFor({ since: step.from - 60, until: step.to }, { priority: "warning" }), true)).filter(inMinute);
  ensure(wanted.some((record) => record.text.startsWith(MARK)), `the log holds no ${MARK} line in the minute of ${utcWords(step.from)}, which the root step wrote every ten seconds`);
  sameRows(await shownRows(page), rowsFor(wanted, ZONE, false), "the Logs card of the minute of the burn");
  const timeline = await timelineOf(page);
  ensure(timeline.some((item) => item.includes(MARK)), `"What happened around" has no ${MARK} line: ${JSON.stringify(timeline)}`);
  await lookAll(t, page, "chart-moment");

  // The same minute in the Logs page, which the link carries to with its times.
  const open = page.getByRole("link", { name: "Open in Logs with this time", exact: true });
  const target = new URL(await open.getAttribute("href"), page.url());
  ensure(target.pathname === "/logs" && target.searchParams.get("since") === String(step.from * 1000) && target.searchParams.get("until") === String(step.to * 1000), `Open in Logs with this time goes to ${target.pathname}${target.search}, not to the minute of ${utcWords(step.from)}`);
  await open.click();
  await t.see(page.getByRole("heading", { name: "Logs", level: 1 }));
  await settled(page);
  const same = (await readLog(page, argsFor({ since: step.from, until: step.to }, { priority: "warning" }), true));
  sameRows(await shownRows(page), rowsFor(same, ZONE, false), "the Logs page opened from the chart");
  await page.goBack();
  await t.see(heading(page, "CPU, last hour"));
  await t.see(chosen(page));
  await t.see(page.locator("#ov-busy tbody tr").first());
}

/** The restart is a mark at its minute; choosing it shows the row of the journal, and the pick goes to it. */
async function markOfChange(t, page, change, name) {
  const mark = page.locator(`ul.marks button[data-at="${change.at}"]`);
  await t.see(mark);
  const words = await mark.innerText();
  ensure(words.includes(`Restart a service by ${name}`) || (change.title && words.includes(`${change.title} by ${name}`)), `the mark of the restart says "${words}", which does not name the change and ${name}`);
  ensure((await marks(page).count()) >= 1, "the chart lists no mark");
  await mark.click();
  await page.waitForFunction((at) => new URLSearchParams(location.search).get("at") === at, String(change.at * 1000), { timeout: 20_000 });
  await t.see(chosen(page));
  await t.see(page.locator("#ov-happened .tl-item").filter({ hasText: `by ${name}` }).first());
  const timeline = await timelineOf(page);
  ensure(timeline.some((item) => item.includes(`by ${name}`) && item.includes("In the journal")), `"What happened around" does not show the restart as a row of the journal: ${JSON.stringify(timeline)}`);
  // A range that still holds the moment keeps it.
  await page.getByRole("button", { name: "24 h", exact: true }).click();
  await t.see(heading(page, "CPU, last 24 hours"));
  await t.see(chosen(page));
  ensure(addressOf(page).get("range") === "24h" && addressOf(page).get("at") === String(change.at * 1000), `after 24 h the address says ${addressOf(page)}, which does not keep the moment`);
  await look(t, page, "chart-restart");
}

// --- The people ---

/** An administrator: the chart from the Overview, the keys, the marks, the cards of the burn and the log of its minute. */
async function administrator(t) {
  const name = adminOf(t.engine);
  const { page, seen } = await openConsole(t, "/", { timezoneId: ZONE });
  await signInAs(t, page, name);
  const session = await sessionOf(page);
  ensure(session.name === name && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);
  await unlockFromHeader(t, page);

  // A chart that is not one of the four has no page.
  await page.goto(new URL("/logs/chart/no-such-chart", page.url()).href);
  await t.see(page.getByRole("heading", { name: "No page here", level: 1 }));

  const change = await restartMetricsd(page, name);
  burn = await burnMinute(page);
  await openChart(t, page);
  await pickTheBurn(t, page);
  await ranked(t, page);
  await minuteOfLogs(t, page);
  await markOfChange(t, page, change, name);
  // The administrator's reads are of the whole log; a reader's own reads are the reader's.
  mustHaveCalled(seen, Object.values(ROUTES).filter((route) => route !== ROUTES.own));
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the chart and the moment, and the lines of their own programs: none of the log's. */
async function reader(t) {
  const name = readerOf(t.engine);
  const { page, seen } = await openConsole(t, "/", { timezoneId: ZONE });
  await signInAs(t, page, name);
  const first = new URL(`/logs/chart/cpu?range=1h&at=${burn.at * 1000}`, page.url());
  await page.goto(first.href);
  await t.see(heading(page, "CPU, last hour"));
  await t.see(chosen(page));
  await t.see(page.locator("#ov-moment-logs"));
  await t.see(page.locator("#ov-moment-logs").getByText(`${OWN} The machine's log is for administrators.`));
  await t.gone(page.locator("#ov-moment-logs").getByRole("button", { name: "Unlock", exact: true }));
  await settled(page);
  // None of the cards says it could not read.
  ensure((await page.getByRole("button", { name: "Try again", exact: true }).count()) === 0, "a card of the view from a chart could not read for a reader");
  const step = { from: burn.at, to: burn.at + 60 };
  const own = (await readLog(page, argsFor({ since: step.from - 60, until: step.to }, { priority: "warning" }), false)).filter((record) => {
    const seconds = Math.floor(microsOf(record.time) / 1_000_000);
    return seconds >= step.from && seconds < step.to;
  });
  sameRows(await shownRows(page), rowsFor(own, ZONE, false), "a reader's lines of the minute of the burn");
  ensure(own.every((record) => !record.text.startsWith(MARK)), "a reader sees a line that a root step wrote");
  mustBe(await fetchAsPage(page, "GET", `/api/1/logs/all?${new URLSearchParams(argsFor({ since: step.from, until: step.to }))}`), 403, "ADM-122", "the whole log, for a reader");
  await look(t, page, "chart-reader");
  mustHaveCalled(seen, [ROUTES.own, ROUTES.around, ROUTES.about]);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: one column, the chart's keys and marks within reach of a thumb, and nothing scrolls sideways. */
async function phone(t) {
  const name = adminOf(t.engine);
  const { page } = await openConsole(t, "/", { viewport: PHONE, timezoneId: ZONE });
  await signInAs(t, page, name, { phone: true });
  await page.goto(new URL(`/logs/chart/cpu?range=1h&at=${burn.at * 1000}`, page.url()).href);
  await t.see(heading(page, "CPU, last hour"));
  await t.see(chosen(page));
  await t.see(page.locator("#ov-busy tbody tr").first());
  await settled(page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's view from a chart scrolls ${await sidewaysScroll(page)} px sideways`);
  const columns = await page.evaluate(() => {
    const cards = [...document.querySelectorAll("#ov-happened, #ov-busy")].map((card) => Math.round(card.getBoundingClientRect().left));
    return new Set(cards).size;
  });
  ensure(columns === 1, "the cards of the moment are not in one column on a phone");
  const small = await smallTargets(page);
  ensure(small.length === 0, `these controls of the view from a chart are under 44 px on a phone: ${small.join(", ")}`);
  await lookAll(t, page, "chart-phone");
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
