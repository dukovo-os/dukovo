// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/services drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Services
// pages (design 4): the list at /services, and the page of one service at
// /services/{name} (with the service's name for {name}). An administrator
// signs in and finds the list held to what `GET /api/1/services` answers the
// same page at the same time, with the failed service first; the filter and
// the state choice narrow it and put themselves in the address; a name Spark
// does not know gets the page's own words.
//
// lab-broken, the service that fails at once (services/lab-broken.service),
// is the failed one: services.toml puts it in place and waits for Spark to give
// up on it. Its page, with the console locked, says why in the sentence that
// `GET /api/1/services/{name}/explain` gives, offers Run now, and asks to
// unlock before it runs it (a cancel sends nothing). Its recent log asks to
// unlock too, and then shows the line the service wrote.
//
// diskhealth is the safe service: every image runs it, nothing needs it. Its
// page shows what `service diskhealth status --json` says, which is what
// `GET /api/1/services/{name}` runs, after each press: Restart (its process
// changes, and the journal has a row), Stop (the explain banner appears), Run
// now, and the switch Starts at boot off and on, with the journal's row of
// each holding the line that `service disable` and `service enable` leave in
// the service's file. With Show commands on, each button has under it the
// command that `POST /api/1/show` answers. services.toml saves diskhealth's
// file first and puts it back after.
//
// admind, which the console runs on, has no Stop and no Restart on its page,
// a switch that does nothing, and the words that say why, before anyone tries;
// the route refuses it as well. A reader sees the list and the page, no
// button, and the line that says who may start and stop services. A phone at
// 390 by 844 shows a failed service's page with its sentence, Run now, Open its
// log and the switch, each 44 px tall and as wide as the page.
//
// The page of one service has the cards that Spark's answer decides: Use (its
// numbers and, over a range the person picks, the charts that `GET
// /api/1/services/{name}/usage/{range}` has points for, or the words that say
// the machine has kept none), Limits (the rows of the service's `holds`, its
// file and the command that changes them), Sockets (only a service that holds
// some) and Timer (only a service that a timer starts). The Scheduled tasks
// page at /services/scheduled is for an administrator who has unlocked the
// console: a locked one is asked to unlock, and a cancel reads nothing; after
// the unlock its table is held to what `GET /api/1/timers` answers. lab-timer,
// which services.toml installs with a timer of 30 3 * * *, is its row and has
// the Timer card, which says the same locked (it asks to unlock), unlocked
// (its times) and to a reader (who is told it is an administrator's). Each
// look is the console's own: dark, light, high contrast, forced colours and
// less motion, each checked by axe (console.mjs's lookAll). The accounts are
// the two that services.toml made, so no script needs the lab's password.

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
  sessionOf,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";

// The password that services.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";
const ADMIN = "wc-services";
const READER = "wc-services-reader";

const PHONE = { width: 390, height: 844 };

// The safe service, the failed one and the one the console runs on, and the
// words of the line the failed one writes each time it starts: the log shows
// them after its name, which the line begins with too.
const SAFE = "diskhealth";
const BROKEN = "lab-broken";
const CONSOLE = "admind";
const BROKEN_WORDS = "cannot open /shared/lab: no such folder";

// The safe service's file, which the switch Starts at boot writes.
const FILE = `/system/settings/spark/services/${SAFE}.toml`;

// A name no service has.
const NOBODY = "no-such-service";

// The service that services.toml gives a timer, and what the page says of it.
const TIMED = "lab-timer";
const TIMED_WORDS = "Every day at 3:30 AM";
const TIMED_SCHEDULE = "30 3 * * *";

// What the pages of the timers say where the console is locked and to a reader.
const TIMERS_LOCKED = "Unlock to see the scheduled tasks.";
const TIMERS_READER = "Administrators see the scheduled tasks.";
const TIMER_LOCKED = "Unlock to see its schedule.";
const TIMER_READER = "Administrators see its schedule.";
const NO_HISTORY = "No history yet: the machine keeps it from its first 10 minutes on.";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with a
// service's name for {name}.
const ROUTES = {
  list: "GET /api/1/services",
  status: "GET /api/1/services/{name}", // service.status
  explain: "GET /api/1/services/{name}/explain", // service.explain
  start: "POST /api/1/services/{name}/start",
  stop: "POST /api/1/services/{name}/stop",
  restart: "POST /api/1/services/{name}/restart",
  enable: "POST /api/1/services/{name}/enable",
  disable: "POST /api/1/services/{name}/disable",
  logs: "GET /api/1/logs",
  logsAll: "GET /api/1/logs/all",
  usage: "GET /api/1/services/{name}/usage/{range}",
  timers: "GET /api/1/timers", // service.timers
};

const routeFor = (template, name) => template.replace("{name}", name);

/** The path of a route as the table names it: `GET /api/1/services` is `/api/1/services`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

/** The address of the page of the service `name`: a dot is written out, which a path's last part may not hold. */
const addressOfService = (name) => `/services/${encodeURIComponent(name).replace(/\./g, "%2E")}`;

// The words the pages say for each state Spark names, and for how a service starts.
const STATE_WORDS = {
  running: "Running",
  listening: "Ready",
  starting: "Starting",
  done: "Done",
  waiting: "Waiting",
  pending: "Waiting",
  stopping: "Stopping",
  failed: "Failed",
  stopped: "Stopped",
  disabled: "Stopped",
};
const wordOf = (state) => STATE_WORDS[state] ?? `${state.charAt(0).toUpperCase()}${state.slice(1)}`;
const bootOf = (service) => {
  if (service.enabled !== true) return "Off";
  if (service.timer === true) return "By its timer";
  return service.start === "on-connection" ? "On connection" : "Starts at boot";
};
const isUp = (service) => ["running", "listening", "starting"].includes(service.state);

// How the Runs card says a service's restart rule.
const RESTART_WORDS = {
  always: "Starts again whenever it ends",
  "on-failure": "Starts again when it fails",
  never: "Does not start again when it ends",
};

// What the pages say to a reader, and where a card has nothing to show.
const READER_LINE = "Administrators can start and stop services.";
const LOG_LOCKED = "You see the lines of your own programs. Unlock to see the service's log.";
const LOG_READER = "Administrators see the service's log.";
const CONSOLE_WORDS = `The console runs on ${CONSOLE}, so this page does not stop it, restart it or turn off its start at boot: that is done at a terminal.`;

/** Waits for focus to be on `heading`, which is where a page puts it. */
const seeFocusOn = (page, heading) =>
  page.waitForFunction(
    (wanted) => {
      const element = document.activeElement;
      return element !== null && /^H[12]$/.test(element.tagName) && element.textContent.trim() === wanted;
    },
    heading,
    { timeout: 15_000 },
  );

/** The notice that the shell shows when a change is done. */
const noticeOf = (page, words) => page.locator(".notice-words", { hasText: words }).first();

const unlockDialog = (page) => page.getByRole("dialog", { name: /^Unlock/ });

// --- Reading the machine ---

/** The document `GET /api/1/services` answers: Spark's `show`. */
async function readServices(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(answer.status === 200 && Array.isArray(answer.json?.services), `${ROUTES.list} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** What `GET /api/1/services/{name}` answers, which is `service NAME status --json`: `{service, processes, listening, trial}`. */
async function readStatus(page, name) {
  const answer = await fetchAsPage(page, "GET", pathOf(routeFor(ROUTES.status, name)));
  ensure(
    answer.status === 200 && answer.json !== null && typeof answer.json.service === "object",
    `${routeFor(ROUTES.status, name)} was answered ${answer.status}: ${answer.text}`,
  );
  return answer.json;
}

/** Reads the status of `name` until `test(service)` holds, for up to 30 seconds, and gives it. */
async function becomes(page, name, test, what) {
  const until = Date.now() + 30_000;
  for (;;) {
    const status = await readStatus(page, name);
    if (test(status.service)) return status;
    ensure(Date.now() < until, `${name} did not ${what} within 30 seconds: it is ${status.service.state}, and starts at boot is ${status.service.enabled}`);
    await sleep(1000);
  }
}

/** The number of the newest row of the journal, or 0. */
async function lastChange(page) {
  const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
  ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
  return list.json.changes.reduce((most, row) => Math.max(most, row.id), 0);
}

/** The row of `action` that the console made for ADMIN after row `after`, as `GET /api/1/changes/{id}` gives it with its diffs. */
async function changeOf(page, action, after) {
  const until = Date.now() + 30_000;
  for (;;) {
    const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
    ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
    const row = list.json.changes.filter((each) => each.id > after && each.action === action && each.how === "console").sort((a, b) => b.id - a.id)[0];
    if (row !== undefined) {
      const change = await fetchAsPage(page, "GET", `/api/1/changes/${row.id}`);
      ensure(change.status === 200, `GET /api/1/changes/${row.id} was answered ${change.status}: ${change.text}`);
      ensure(change.json.who === ADMIN, `the row of ${action} names ${change.json.who}, not ${ADMIN}`);
      return change.json;
    }
    ensure(Date.now() < until, `the journal has no row of ${action} after row ${after}`);
    await sleep(1000);
  }
}

/** The lines that the diff of `path` in `change` adds, without their sign. */
function addedBy(change, path) {
  const file = change.files.find((each) => each.path === path);
  ensure(file !== undefined, `change ${change.id} does not name ${path}: ${JSON.stringify(change.files)}`);
  return (file.diff ?? "")
    .split("\n")
    .filter((text) => text.startsWith("+") && !text.startsWith("+++ "))
    .map((text) => text.slice(1));
}

// --- The list ---

/** Waits for the list to be shown and read: its table is not busy and has a row. */
async function listLoaded(t, page) {
  await t.see(page.getByRole("heading", { name: "Services", level: 1, exact: true }));
  await page.waitForFunction(
    () => {
      const table = document.querySelector("table.svc-tbl");
      return table !== null && !table.hasAttribute("aria-busy") && table.querySelector("tbody tr[data-name]") !== null;
    },
    null,
    { timeout: 30_000 },
  );
}

/** Opens the list at `address`. */
async function openList(t, page, address = "/services") {
  await page.goto(new URL(address, page.url()).href);
  await listLoaded(t, page);
}

/** The rows of the table as drawn, in order: name, the words of the state and of the start, the description and the link. */
const rowsShown = (page) =>
  page.locator("table.svc-tbl tbody tr[data-name]").evaluateAll((rows) =>
    rows.map((row) => ({
      name: row.dataset.name,
      state: row.querySelector("td[data-label='Now'] .st")?.textContent.trim() ?? null,
      boot: row.querySelector("td[data-label='At boot'] .pill")?.textContent.trim() ?? null,
      description: row.querySelector("td[data-label='What it does']")?.textContent.trim() ?? null,
      href: row.querySelector("th a")?.getAttribute("href") ?? null,
    })),
  );

/** The table's caption, which says how many rows are shown of how many there are. */
const captionOfList = (page) => page.locator("table.svc-tbl caption").evaluate((caption) => caption.textContent.trim());

/**
 * The list says what the machine says: the same services, the failed ones
 * first and then by name, each with its state, how it starts, what it does and
 * the address of its page. The document is read before and after the rows, and a
 * row may say what either said, since a service may have moved between.
 */
async function listAsAnswered(t, page) {
  const before = await readServices(page);
  const rows = await rowsShown(page);
  const after = await readServices(page);
  const named = (document) => JSON.stringify(document.services.map((service) => service.name).sort());
  const shown = JSON.stringify(rows.map((row) => row.name).sort());
  ensure(shown === named(before) || shown === named(after), `the list shows ${shown} and ${ROUTES.list} gives ${named(before)}, then ${named(after)}`);
  for (const name of [SAFE, CONSOLE, "authd"]) ensure(rows.some((row) => row.name === name), `the list does not show ${name}`);

  const rank = (row) => (row.state === "Failed" ? 0 : 1);
  rows.forEach((row, at) => {
    if (at === 0) return;
    const last = rows[at - 1];
    ensure(rank(last) < rank(row) || (rank(last) === rank(row) && last.name < row.name), `${last.name} comes before ${row.name}, and failed services come first, then the names in order`);
  });
  ensure(rows[0].name === BROKEN && rows[0].state === "Failed", `the list starts with ${rows[0].name}, ${rows[0].state}, not the failed ${BROKEN}`);

  for (const row of rows) {
    const entries = [before, after].map((document) => document.services.find((service) => service.name === row.name)).filter((service) => service !== undefined);
    ensure(entries.length > 0, `the list shows ${row.name}, which the machine does not list`);
    ensure(entries.some((service) => wordOf(service.state) === row.state), `${row.name} is ${row.state} in the list and ${entries.map((service) => service.state).join(", then ")} on the machine`);
    ensure(entries.some((service) => bootOf(service) === row.boot), `${row.name} starts at boot: "${row.boot}" in the list, and ${entries.map(bootOf).join(", then ")} on the machine`);
    ensure(entries.some((service) => (service.description ?? "") === row.description), `${row.name} does "${row.description}" in the list`);
    ensure(row.href === addressOfService(row.name), `the link of ${row.name} is ${row.href}, not ${addressOfService(row.name)}`);
  }
  const caption = await captionOfList(page);
  ensure(caption === `Services, ${rows.length} shown of ${rows.length}`, `the table's caption is "${caption}"`);
  return rows;
}

/** The filter and the state choice narrow the rows, say so in the address, and are read back from it. */
async function listFiltered(t, page, all) {
  const row = (name) => page.locator(`table.svc-tbl tr[data-name="${name}"]`);
  const query = () => new URL(page.url()).searchParams;

  await page.locator("#svc-q").fill("diskh");
  await t.see(row(SAFE));
  await t.gone(row(BROKEN));
  ensure(query().get("q") === "diskh", `the address is ${page.url()} after the filter`);
  const narrowed = await rowsShown(page);
  ensure(narrowed.length >= 1 && narrowed.length < all.length, `the filter "diskh" leaves ${narrowed.length} of ${all.length}`);
  ensure(narrowed.some((one) => one.name === SAFE), `the filter "diskh" does not leave ${SAFE}`);
  const caption = await captionOfList(page);
  ensure(caption === `Services, ${narrowed.length} shown of ${all.length}`, `the caption after the filter is "${caption}"`);

  await page.locator("#svc-q").fill("zzzz-nothing-like-it");
  await t.see(page.getByText("No service matches the filter."));
  await page.getByRole("button", { name: "Show every service", exact: true }).click();
  await t.see(row(BROKEN));
  ensure((await page.locator("#svc-q").inputValue()) === "" && query().get("q") === null, "Show every service left the filter in place");
  ensure((await rowsShown(page)).length === all.length, "Show every service did not bring every row back");

  await page.locator("#svc-state").selectOption("failed");
  await t.gone(row(SAFE));
  ensure(query().get("state") === "failed", `the address is ${page.url()} after the state choice`);
  const failed = (await readServices(page)).services.filter((service) => service.state === "failed").map((service) => service.name).sort();
  const listed = (await rowsShown(page)).map((one) => one.name).sort();
  ensure(JSON.stringify(failed) === JSON.stringify(listed), `Failed shows ${listed.join(", ")} and the machine has ${failed.join(", ")} failed`);

  await page.locator("#svc-state").selectOption("off-at-boot");
  await t.see(row(BROKEN));
  const off = await rowsShown(page);
  ensure(off.length > 0 && off.every((one) => one.boot === "Off"), `Off at boot shows ${off.map((one) => `${one.name} (${one.boot})`).join(", ")}`);
  await page.locator("#svc-state").selectOption("any");
  await t.see(row(SAFE));

  // The address holds the filter: opened by it, the page has it in its fields.
  await openList(t, page, "/services?q=diskh&state=running");
  ensure((await page.locator("#svc-q").inputValue()) === "diskh", "the address's filter is not in the field");
  ensure((await page.locator("#svc-state").inputValue()) === "running", "the address's state is not in the choice");
  await t.see(row(SAFE));
  await t.gone(row(BROKEN));
}

/** A name that is no service gets the page's own words and a way to the list. */
async function unknownService(t, page) {
  await page.goto(new URL(addressOfService(NOBODY), page.url()).href);
  await t.see(page.getByText(`There is no service called ${NOBODY}`).first());
  await t.see(page.getByText("Open the list to see the services this server has.").first());
  await look(t, page, "services-unknown");
  await page.getByRole("link", { name: "Open the list", exact: true }).click();
  await listLoaded(t, page);
  ensure(new URL(page.url()).pathname === "/services", `Open the list led to ${page.url()}`);
}

// --- The page of one service ---

/** Opens the page of `name` and waits for it to have read the service. */
async function openService(t, page, name, host) {
  await page.goto(new URL(addressOfService(name), page.url()).href);
  await t.see(page.getByRole("heading", { name, level: 1, exact: true }));
  await page.waitForFunction(
    () => {
      const root = document.querySelector(".svc");
      return root !== null && !root.hasAttribute("aria-busy");
    },
    null,
    { timeout: 30_000 },
  );
  await seeFocusOn(page, name);
  const title = await page.title();
  ensure(title.startsWith(`${name} · `) && title.endsWith(`${host} · Dukovo Admin`), `the tab is called "${title}"`);
}

/** What the page of a service shows: its state, how it starts, the buttons by name, and whether the switch is on (null: no switch). */
const shownOf = (page) =>
  page.evaluate(() => {
    const root = document.querySelector(".svc");
    const text = (selector) => root.querySelector(selector)?.textContent.trim() ?? null;
    const toggle = root.querySelector("[role=switch]");
    return {
      state: text(".svc-pills .st"),
      atBoot: text(".svc-pills .pill"),
      buttons: [...root.querySelectorAll(".svc-actions [data-action]")].map((button) => button.dataset.action),
      on: toggle === null ? null : toggle.getAttribute("aria-checked") === "true",
    };
  });

/** What the page should show of `service`, as `service NAME status --json` says it. */
function expectedOf(service) {
  let buttons = ["start"];
  if (isUp(service)) buttons = service.name === CONSOLE ? [] : ["stop", "restart"];
  else if (service.state === "stopping") buttons = [];
  return { state: wordOf(service.state), atBoot: bootOf(service), buttons, on: service.enabled === true };
}

/** The page of `name` says what its status says, waited for up to 30 seconds. Gives the status. */
async function agrees(page, name) {
  const until = Date.now() + 30_000;
  for (;;) {
    const status = await readStatus(page, name);
    const shown = await shownOf(page);
    const wanted = expectedOf(status.service);
    if (JSON.stringify(shown) === JSON.stringify(wanted)) return status;
    ensure(Date.now() < until, `the page of ${name} shows ${JSON.stringify(shown)}, and its status says ${JSON.stringify(wanted)}`);
    await sleep(1000);
  }
}

/** `text` with each "how long ago" of a time taken out, as two readings a moment apart differ in it: "04:59:47, 2 s ago" is "04:59:47, ago". */
const sinceLeftOut = (text) => text.replace(/, \d+ (?:s|min|h(?: \d+ min)?|days?) ago/g, ", ago");

/** The banner above the cards says what `GET /api/1/services/{name}/explain` says, or Spark's own words when that has none. */
async function bannerAsExplained(t, page, name) {
  const explained = await fetchAsPage(page, "GET", pathOf(routeFor(ROUTES.explain, name)));
  const banner = page.locator(".svc-banners .banner").first();
  await t.see(banner);
  const text = await banner.innerText();
  const sentence = explained.status === 200 ? explained.json?.sentence : null;
  if (typeof sentence === "string" && sentence !== "") {
    // The page and the test each read the explanation, a moment apart: the
    // times are the same second, and how long ago they were may differ.
    ensure(sinceLeftOut(text).includes(sinceLeftOut(sentence)), `the banner of ${name} says "${text}", and the explanation is "${sentence}"`);
    if (typeof explained.json.fix === "string" && explained.json.fix !== "") ensure(text.includes(explained.json.fix), `the banner of ${name} leaves out the fix "${explained.json.fix}"`);
  } else {
    ensure(text.includes(`${name} is not running.`), `the banner of ${name} says "${text}"`);
  }
  return text;
}

/** The labels and values of the rows of a card, as the page draws them. */
const rowsOfCard = (card) =>
  card.locator("dl.svc-rows").evaluate((list) => Object.fromEntries([...list.querySelectorAll("dt")].map((term) => [term.textContent.trim(), term.nextElementSibling.textContent.trim()])));

/** A number in the way the page reads it: 4,096. */
const written = (count) => count.toLocaleString("en-US");

/** Limits shows what the service's `holds` says, its file and the command that changes them; a service with no `holds` has no such card. */
async function limitsAsAnswered(t, page, service) {
  const card = page.locator("#limits");
  const holds = service.holds;
  if (holds === undefined || holds === null) {
    ensure(!(await card.isVisible()), `${service.name} has no holds in its status, and the page shows Limits`);
    return;
  }
  await t.see(card.getByRole("heading", { name: "Limits", level: 2, exact: true }));
  const rows = await rowsOfCard(card);
  ensure(Number.isFinite(holds.memory_bytes) ? rows.Memory !== "No limit" : rows.Memory === "No limit", `Limits says Memory is "${rows.Memory}", and the status has ${holds.memory_bytes}`);
  ensure(rows["CPU weight"] === written(holds.cpu_weight), `Limits says CPU weight is "${rows["CPU weight"]}", and the status has ${holds.cpu_weight}`);
  ensure(rows.Threads === `At most ${written(holds.threads)}`, `Limits says Threads is "${rows.Threads}", and the status has ${holds.threads}`);
  ensure(rows["Disk weight"] === written(holds.io_weight), `Limits says Disk weight is "${rows["Disk weight"]}", and the status has ${holds.io_weight}`);
  ensure(rows["Its file"] === `/system/settings/spark/services/${service.name}.toml`, `Limits names the file "${rows["Its file"]}"`);
  ensure(typeof service.stop_signal === "string" && service.stop_signal !== "" ? rows["Stop signal"] === service.stop_signal : rows["Stop signal"] === undefined, `Limits says the stop signal is "${rows["Stop signal"]}", and the status has ${service.stop_signal}`);
  const command = card.locator("pre.cmdtext");
  ensure((await command.innerText()) === `elevate service try ${service.name}`, `the command under Limits is "${await command.innerText()}"`);
  await t.see(card.getByRole("button", { name: "Copy", exact: true }));
}

/** Sockets lists what Spark holds for a service that holds any, and is left out for one that holds none. */
async function socketsAsAnswered(t, page, service) {
  const card = page.locator("#sockets");
  const names = (list) => (Array.isArray(list) ? list.filter((one) => typeof one === "string" && one !== "") : []);
  const sockets = names(service.holds?.sockets);
  const groups = names(service.holds?.groups);
  if (sockets.length === 0 && groups.length === 0) {
    ensure(!(await card.isVisible()), `${service.name} holds no socket, and the page shows Sockets`);
    return;
  }
  await t.see(card.getByRole("heading", { name: "Sockets", level: 2, exact: true }));
  const rows = await card.locator("dl.svc-rows dd").evaluateAll((items) => items.map((item) => [...item.querySelectorAll("li")].map((line) => line.textContent.trim())));
  const shown = rows.flat();
  for (const name of [...sockets, ...groups]) ensure(shown.includes(name), `Sockets does not list ${name}: ${shown.join(", ")}`);
}

/** Timer is the card of a service that a timer starts, and is left out for one that none starts. */
async function timerCardShown(t, page, service) {
  const card = page.locator("#timer");
  if (service.timer !== true) {
    ensure(!(await card.isVisible()), `${service.name} has no timer, and the page shows Timer`);
    return null;
  }
  await t.see(card.getByRole("heading", { name: "Timer", level: 2, exact: true }));
  const link = card.getByRole("link", { name: "Open Scheduled tasks", exact: true });
  ensure((await link.getAttribute("href")) === "/services/scheduled", `Open Scheduled tasks leads to ${await link.getAttribute("href")}`);
  return card;
}

/** What `GET /api/1/services/{name}/usage/{range}` answers: whether the machine has kept points for the service over the range. */
async function hasHistory(page, name, range) {
  const answer = await fetchAsPage(page, "GET", pathOf(routeFor(ROUTES.usage, name).replace("{range}", range)));
  const none = answer.status === 422 && answer.json?.error?.id === "ADM-123";
  ensure(answer.status === 200 || none, `${routeFor(ROUTES.usage, name)} for ${range} was answered ${answer.status}: ${answer.text}`);
  return answer.status === 200 && Array.isArray(answer.json) && answer.json.length > 0;
}

/**
 * Use says what Spark's `usage` of the service says, and draws a chart of its
 * processor and one of its memory when the machine has kept points, or says it
 * has kept none; the range control draws them again and puts the range in the
 * address.
 */
async function useAsAnswered(t, page, service) {
  const card = page.locator("#use");
  await t.see(card.getByRole("heading", { name: "Use", level: 2, exact: true }));
  if (service.usage && Number.isFinite(service.usage.memory_bytes)) await t.see(card.locator("dl.svc-rows dt", { hasText: "Memory now" }));
  const settled = () => page.waitForFunction(() => {
    const grid = document.querySelector("#use .svc-charts");
    const words = document.querySelector("#use [role=status]");
    return (grid !== null && !grid.hidden && grid.querySelectorAll("svg[role=img]").length === 2) || (words !== null && !words.hidden && words.textContent.trim() !== "");
  }, null, { timeout: 30_000 });
  await settled();
  const before = await hasHistory(page, service.name, "24h");
  const drawn = await card.locator(".svc-charts").isVisible();
  const after = await hasHistory(page, service.name, "24h");
  // The first points may have come between the reads, and never go away.
  ensure(drawn ? after : !before, `the Use card ${drawn ? "draws charts" : "has none"} and the machine had points ${before}, then ${after}`);
  if (!drawn) {
    await t.see(card.getByText(NO_HISTORY, { exact: true }));
    return;
  }
  const labels = () => card.locator(".svc-charts svg[role=img]").evaluateAll((pictures) => pictures.map((picture) => picture.getAttribute("aria-label")));
  const first = await labels();
  ensure(first.length === 2 && first[0].startsWith("CPU over the last 24 hours: ") && first[1].startsWith("Memory over the last 24 hours: "), `the charts of Use are labelled ${JSON.stringify(first)}`);
  await card.locator('button[data-range="7d"]').click();
  await page.waitForFunction(() => document.querySelector("#use .svc-charts svg[role=img]")?.getAttribute("aria-label")?.includes("over the last 7 days") ?? false, null, { timeout: 30_000 });
  ensure(new URL(page.url()).searchParams.get("range") === "7d", `the address is ${page.url()} after choosing 7 days`);
  ensure((await card.locator('button[data-range="7d"]').getAttribute("aria-checked")) === "true", "7 days is not the range that is chosen");
  await card.locator('button[data-range="24h"]').click();
  await page.waitForFunction(() => document.querySelector("#use .svc-charts svg[role=img]")?.getAttribute("aria-label")?.includes("over the last 24 hours") ?? false, null, { timeout: 30_000 });
}

/** The cards that every service has, and the four that Spark's answer decides: Use, Limits, Sockets and Timer. */
async function cardsAsAnswered(t, page, status) {
  for (const [id, words] of [["runs", "Runs"], ["needs", "Needs"]]) {
    const heading = page.locator(`#${id} > h2`);
    await t.see(heading);
    ensure((await heading.innerText()) === words, `the card ${id} is headed "${await heading.innerText()}", not "${words}"`);
  }
  const rule = RESTART_WORDS[status.service.restart];
  if (rule !== undefined) await t.see(page.locator("#runs dd").filter({ hasText: rule }).first());
  await useAsAnswered(t, page, status.service);
  await limitsAsAnswered(t, page, status.service);
  await socketsAsAnswered(t, page, status.service);
  await timerCardShown(t, page, status.service);
}

/** The button `id` (start, stop or restart) of the page. */
const buttonOf = (page, id) => page.locator(`.svc-actions [data-action="${id}"]`);

/** The switch Starts at boot. */
const switchOf = (page) => page.getByRole("switch", { name: "Starts at boot", exact: true });

/** Answers the dialog that asks to unlock with the account's password. */
async function unlockHere(t, page) {
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const answered = answerTo(page, "/api/1/unlock");
  await dialog.getByRole("button", { name: "Unlock", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `the unlock was answered ${answer.status()}`);
  await t.gone(dialog);
}

/**
 * Presses `control`, which runs the action whose route is `route`, and gives the
 * answer. A console that is locked asks to unlock first: the dialog is answered
 * when it comes.
 */
async function pressAction(t, page, control, route) {
  const answered = answerTo(page, route);
  await control.click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await unlockHere(t, page);
  const answer = await answered;
  ensure(answer.status() === 200, `${route} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

/** A locked console asks to unlock before `control` runs its action, and a cancel sends nothing. */
async function askedFirst(t, page, control, route, words) {
  let posted = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === route) posted += 1;
  };
  page.on("request", count);
  await control.click();
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText(words, { exact: true }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  page.off("request", count);
  ensure(posted === 0, `a cancelled unlock still sent ${route}`);
}

/** The command under `button` is the one that `POST /api/1/show` answers for the action. */
async function commandUnder(t, page, action, name) {
  const shown = await fetchAsPage(page, "POST", "/api/1/show", { action, args: { name } });
  ensure(shown.status === 200 && typeof shown.json?.command === "string", `POST /api/1/show for ${action} was answered ${shown.status}: ${shown.text}`);
  ensure(shown.json.command.includes(`service ${name}`), `the command of ${action} is "${shown.json.command}"`);
  await t.see(page.getByRole("button", { name: `Show the command: ${shown.json.command}`, exact: true }));
}

/** A failed service, with the console locked: why, Run now behind the unlock, and a log that asks to unlock and then has its line. */
async function failedService(t, page, host) {
  await openService(t, page, BROKEN, host);
  const status = await agrees(page, BROKEN);
  ensure(status.service.state === "failed", `${BROKEN} is ${status.service.state}: services.toml waits for Spark to give up on it`);
  const text = await bannerAsExplained(t, page, BROKEN);
  ensure(text.includes(BROKEN), `the banner of ${BROKEN} does not name it: "${text}"`);
  await t.see(page.locator(".svc-banners .banner.bad").first());

  await askedFirst(t, page, buttonOf(page, "start"), routeFor(pathOf(ROUTES.start), BROKEN), `start ${BROKEN}`);

  const card = page.locator("#log");
  await t.see(card.getByRole("heading", { name: "Recent log", level: 2, exact: true }));
  await t.see(card.getByText(LOG_LOCKED, { exact: true }));
  const link = card.getByRole("link", { name: "Open in Logs", exact: true });
  ensure((await link.getAttribute("href")) === `/logs?service=${BROKEN}`, `Open in Logs leads to ${await link.getAttribute("href")}`);
  await card.getByRole("button", { name: "Unlock", exact: true }).click();
  await unlockHere(t, page);
  await t.see(card.locator(".lrow").filter({ hasText: BROKEN }).filter({ hasText: BROKEN_WORDS }).first());
  await t.gone(card.getByText(LOG_LOCKED, { exact: true }));
  const read = await fetchAsPage(page, "GET", `${pathOf(ROUTES.logsAll)}?service=${BROKEN}&lines=20`);
  ensure(read.status === 200 && read.text.includes(BROKEN_WORDS), `${ROUTES.logsAll} for ${BROKEN} was answered ${read.status}: ${read.text}`);
  await lookAll(t, page, "services-failed");
}

/** Restart: a new process, the notice, and a row of the journal that the console made. */
async function restarted(t, page) {
  const before = await readStatus(page, SAFE);
  // The process's number and the time it began: one of them is new after a restart.
  const mark = (service) => `${service.pid ?? ""}/${service.since_ms ?? ""}`;
  const after = await lastChange(page);
  await pressAction(t, page, buttonOf(page, "restart"), pathOf(routeFor(ROUTES.restart, SAFE)));
  await t.see(noticeOf(page, `${SAFE} restarted.`));
  const now = await becomes(page, SAFE, (service) => service.state === "running" && mark(service) !== mark(before.service), "run as a new process");
  ensure(isUp(now.service), `${SAFE} is ${now.service.state} after Restart`);
  await agrees(page, SAFE);
  const row = await changeOf(page, "service.restart", after);
  ensure(row.how === "console", `the journal's row of the restart was made ${row.how}`);
}

/** Stop: the state, the buttons and the explain banner follow. */
async function stopped(t, page) {
  await pressAction(t, page, buttonOf(page, "stop"), pathOf(routeFor(ROUTES.stop, SAFE)));
  await t.see(noticeOf(page, `${SAFE} stopped.`));
  await becomes(page, SAFE, (service) => !isUp(service), "stop");
  await agrees(page, SAFE);
  await bannerAsExplained(t, page, SAFE);
  await t.gone(buttonOf(page, "stop"));
  await t.see(buttonOf(page, "start"));
  await lookAll(t, page, "services-stopped");
}

/** Run now: it runs again, and the banner goes. */
async function started(t, page) {
  await pressAction(t, page, buttonOf(page, "start"), pathOf(routeFor(ROUTES.start, SAFE)));
  await t.see(noticeOf(page, `${SAFE} started.`));
  await becomes(page, SAFE, (service) => service.state === "running", "run");
  await agrees(page, SAFE);
  await t.gone(page.locator(".svc-banners .banner"));
}

/**
 * Starts at boot off, which stops the service as well, and on, which starts it:
 * the page agrees with the status each time, and the journal's row of each holds
 * the line of the file that `service disable` and `service enable` leave.
 */
async function startsAtBoot(t, page) {
  const control = switchOf(page);
  ensure((await control.getAttribute("aria-checked")) === "true", `${SAFE} does not start at boot before the test`);

  const afterOff = await lastChange(page);
  await pressAction(t, page, control, pathOf(routeFor(ROUTES.disable, SAFE)));
  await t.see(noticeOf(page, `${SAFE} no longer starts at boot, and is stopped.`));
  await becomes(page, SAFE, (service) => service.enabled !== true && !isUp(service), "stop and stop starting at boot");
  await agrees(page, SAFE);
  const disabled = await changeOf(page, "service.disable", afterOff);
  ensure(addedBy(disabled, FILE).includes("enabled = false"), `the row of the disable does not add "enabled = false" to ${FILE}`);

  const afterOn = await lastChange(page);
  await pressAction(t, page, switchOf(page), pathOf(routeFor(ROUTES.enable, SAFE)));
  await t.see(noticeOf(page, `${SAFE} starts at boot, and runs now.`));
  await becomes(page, SAFE, (service) => service.enabled === true && service.state === "running", "run and start at boot");
  await agrees(page, SAFE);
  const enabled = await changeOf(page, "service.enable", afterOn);
  ensure(addedBy(enabled, FILE).includes("enabled = true"), `the row of the enable does not add "enabled = true" to ${FILE}`);
}

/** The safe service's page: as the machine says, with Show commands on, and each button's press. */
async function safeService(t, page, host) {
  const saved = await fetchAsPage(page, "POST", "/api/1/account/preferences", { commands: "shown" });
  ensure(saved.status === 200, `Show commands was not kept: the console answered ${saved.status}: ${saved.text}`);
  await openService(t, page, SAFE, host);
  const status = await agrees(page, SAFE);
  ensure(isUp(status.service) && status.service.enabled === true, `${SAFE} is ${status.service.state} and starts at boot ${status.service.enabled}, and the test needs it running`);
  await cardsAsAnswered(t, page, status);
  for (const action of ["service.stop", "service.restart", "service.disable"]) await commandUnder(t, page, action, SAFE);
  await lookAll(t, page, "services-running");

  await restarted(t, page);
  await stopped(t, page);
  await started(t, page);
  await startsAtBoot(t, page);
  await look(t, page, "services-running-again");
}

/** The console's own service: no Stop or Restart, a switch that does nothing, the words that say why, and a route that refuses. */
async function consoleService(t, page, host) {
  await openService(t, page, CONSOLE, host);
  const status = await agrees(page, CONSOLE);
  ensure(status.service.state === "running", `${CONSOLE} is ${status.service.state}`);
  await t.see(page.getByText(CONSOLE_WORDS, { exact: true }));
  await t.see(page.locator("pre.cmdtext", { hasText: `elevate service ${CONSOLE} restart` }));

  // Pressing the switch sends nothing and asks nothing.
  const control = switchOf(page);
  ensure((await control.getAttribute("aria-disabled")) === "true", "the switch of the console's service is not disabled");
  ensure((await control.getAttribute("aria-checked")) === "true", "the switch of the console's service is not on");
  let posted = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname.startsWith(`/api/1/services/${CONSOLE}/`)) posted += 1;
  };
  page.on("request", count);
  // Brought whole into the middle of the view first: a press that skips
  // Playwright's checks lands where the control's middle is, and a control
  // left half under the view's edge is not pressed at all.
  await control.evaluate((element) => element.scrollIntoView({ block: "center" }));
  // A switch that says it is off is pressed all the same, as a person may:
  // Playwright would wait for it to come on.
  await control.click({ force: true });
  // What is waited for here does not happen: nothing is sent, and no dialog comes.
  await sleep(1000);
  page.off("request", count);
  ensure(posted === 0, `the switch of ${CONSOLE} sent a request`);
  await t.gone(unlockDialog(page));

  // The route says no, as the words do.
  for (const route of [ROUTES.stop, ROUTES.restart, ROUTES.disable]) {
    const refused = await fetchAsPage(page, "POST", pathOf(routeFor(route, CONSOLE)), {});
    ensure(
      refused.status === 400 && refused.json?.error?.id === "ADM-125" && refused.json.error.field === "name" && refused.text.includes(`The console runs on ${CONSOLE}`),
      `${routeFor(route, CONSOLE)} was answered ${refused.status}: ${refused.text}`,
    );
  }
  await look(t, page, "services-console");
}

// --- Scheduled tasks ---

/** Opens /services/scheduled and waits for the page to have read. */
async function openScheduled(t, page) {
  await page.goto(new URL("/services/scheduled", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Scheduled tasks", level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelector(".svc-body") !== null && !document.querySelector(".svc-body").hasAttribute("aria-busy"), null, { timeout: 30_000 });
}

/** Counts the requests the page makes to `route` (as `METHOD /path`) until `stop()`. */
function watchRoute(page, route) {
  let counted = 0;
  const note = (request) => {
    if (`${request.method()} ${new URL(request.url()).pathname}` === route) counted += 1;
  };
  page.on("request", note);
  return { count: () => counted, stop: () => page.off("request", note) };
}

/** A locked console: the Scheduled tasks page and the Timer card ask to unlock and have read nothing, and a cancelled unlock reads nothing either. */
async function timersLocked(t, page, host) {
  const watch = watchRoute(page, ROUTES.timers);
  await openScheduled(t, page);
  const note = page.locator(".svc-body .read-note");
  await t.see(note.getByText(TIMERS_LOCKED, { exact: true }));
  await note.getByRole("button", { name: "Unlock", exact: true }).click();
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText("see the scheduled tasks", { exact: true }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  await t.see(note.getByText(TIMERS_LOCKED, { exact: true }));
  await look(t, page, "services-scheduled-locked");

  await openService(t, page, TIMED, host);
  const status = await readStatus(page, TIMED);
  ensure(status.service.timer === true, `${TIMED} has no timer in its status: services.toml installs it with one`);
  const card = await timerCardShown(t, page, status.service);
  await t.see(card.getByText(TIMER_LOCKED, { exact: true }));
  await t.see(card.getByRole("button", { name: "Unlock", exact: true }));
  ensure(watch.count() === 0, `the pages of the timers asked for ${ROUTES.timers} ${watch.count()} times while the console was locked`);
  watch.stop();
  // service.timers is an administrator's read: the route refuses a locked one as the page does not ask.
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.timers)), 403, "ADM-106", "a locked administrator's read of the scheduled tasks");
}

/** The words of a result that `last_result` stands for. */
const resultWords = (result) => ({ ok: "Worked", failed: "Failed" })[result] ?? "Not known";

/** The rows of the Scheduled tasks table as drawn: the task, when in words and as the file writes it, the times as ISO, and the result. */
const timerRowsShown = (page) =>
  page.locator("table.svc-tbl tbody tr[data-name]").evaluateAll((rows) =>
    rows.map((row) => ({
      name: row.dataset.name,
      href: row.querySelector("th a")?.getAttribute("href") ?? null,
      when: row.querySelector("td[data-label='When']").childNodes[0]?.textContent.trim() ?? "",
      written: row.querySelector("td[data-label='When'] .svc-written")?.textContent.trim() ?? null,
      next: row.querySelector("td[data-label='Next'] time")?.getAttribute("datetime") ?? null,
      last: row.querySelector("td[data-label='Last'] time")?.getAttribute("datetime") ?? null,
      lastWords: row.querySelector("td[data-label='Last']").textContent.trim(),
      result: row.querySelector("td[data-label='Result']").textContent.trim(),
    })),
  );

/** What `GET /api/1/timers` answers: a row for each service that has a timer. */
async function readTimers(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.timers));
  ensure(answer.status === 200 && Array.isArray(answer.json), `${ROUTES.timers} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The time of a timer as ISO, which is how the page names it in its tag; null when it has none. */
const isoOf = (ms) => (Number.isFinite(ms) ? new Date(ms).toISOString() : null);

/** The timers as the machine lists them, soonest next run first and a timer with none last, then by name. */
const inTimerOrder = (timers) =>
  [...timers].sort((a, b) => (Number.isFinite(a.next_ms) ? a.next_ms : Infinity) - (Number.isFinite(b.next_ms) ? b.next_ms : Infinity) || a.service.localeCompare(b.service));

/** Unlocked, the Scheduled tasks table says what `GET /api/1/timers` says, and the Timer card of lab-timer says its row. */
async function timersAsAnswered(t, page, host) {
  await openScheduled(t, page);
  await t.see(page.locator("table.svc-tbl"));
  const before = await readTimers(page);
  const rows = await timerRowsShown(page);
  const after = await readTimers(page);
  const wanted = (timers) => JSON.stringify(inTimerOrder(timers).map((timer) => timer.service));
  ensure(JSON.stringify(rows.map((row) => row.name)) === wanted(before) || JSON.stringify(rows.map((row) => row.name)) === wanted(after), `the table lists ${rows.map((row) => row.name).join(", ")} and ${ROUTES.timers} gives ${JSON.parse(wanted(before)).join(", ")}`);
  ensure(rows.some((row) => row.name === TIMED), `the table has no row for ${TIMED}`);
  for (const row of rows) {
    const timer = [...before, ...after].filter((one) => one.service === row.name);
    ensure(timer.length > 0, `the table has ${row.name}, which ${ROUTES.timers} does not list`);
    ensure(row.href === addressOfService(row.name), `the link of ${row.name} is ${row.href}`);
    ensure(timer.some((one) => (one.next_ms === null || one.next_ms === undefined ? row.next === null : row.next === isoOf(one.next_ms))), `${row.name} runs next at ${row.next} in the table, and ${ROUTES.timers} says ${timer.map((one) => isoOf(one.next_ms)).join(" or ")}`);
    ensure(timer.some((one) => (Number.isFinite(one.last_ms) ? row.last === isoOf(one.last_ms) : row.last === null && row.lastWords === "Never")), `${row.name} ran last at ${row.last} in the table, and ${ROUTES.timers} says ${timer.map((one) => isoOf(one.last_ms)).join(" or ")}`);
    ensure(timer.some((one) => row.result === resultWords(one.last_result)), `${row.name}'s last run is "${row.result}" in the table, and ${ROUTES.timers} says ${timer.map((one) => one.last_result).join(" or ")}`);
    ensure(timer.some((one) => (typeof one.schedule === "string" && one.schedule !== "" ? row.written === one.schedule || row.written === null : row.when === "Its file does not read")), `${row.name} is written "${row.written}" in the table, and ${ROUTES.timers} says ${timer.map((one) => one.schedule).join(" or ")}`);
  }
  const timed = rows.find((row) => row.name === TIMED);
  ensure(timed.when === TIMED_WORDS && timed.written === TIMED_SCHEDULE, `${TIMED} is "${timed.when}" written "${timed.written}", not "${TIMED_WORDS}" written "${TIMED_SCHEDULE}"`);
  ensure(timed.next !== null, `${TIMED} has no next time in the table`);
  ensure(timed.last === null && timed.lastWords === "Never", `${TIMED} last ran "${timed.lastWords}" and has not run`);
  const caption = await page.locator("table.svc-tbl caption").evaluate((one) => one.textContent.trim());
  ensure(caption === `Scheduled tasks, ${rows.length}, soonest first`, `the table's caption is "${caption}"`);
  const server = [...before, ...after].some((timer) => typeof timer.zone === "string" && timer.zone !== "" && timer.zone !== "UTC");
  const heads = await page.locator("table.svc-tbl thead th").allInnerTexts();
  ensure(heads[2].startsWith("Next") && heads[2].includes("server's clock") === server, `the column of the next time is "${heads[2]}", and a timer on another clock is ${server}`);
  await t.see(page.getByText("Spark keeps the result of a run only since the machine started", { exact: false }));
  const again = answerToGet(page, ROUTES.timers);
  await page.getByRole("button", { name: "Read again", exact: true }).click();
  ensure((await again).status() === 200, "Read again was not answered 200");
  await lookAll(t, page, "services-scheduled");

  // The Timer card of the service says the row of the table.
  await openService(t, page, TIMED, host);
  const status = await readStatus(page, TIMED);
  const card = await timerCardShown(t, page, status.service);
  await t.see(card.locator("dl.svc-rows"));
  const facts = await rowsOfCard(card);
  ensure(facts.When === `${TIMED_WORDS} (${TIMED_SCHEDULE})`, `the Timer card says When is "${facts.When}"`);
  ensure(facts.Last === "Never" && facts.Result === "Not known", `the Timer card says Last is "${facts.Last}" and Result "${facts.Result}"`);
  ensure(facts.Next !== "–", "the Timer card has no next time");
  await t.gone(card.getByText(TIMER_LOCKED, { exact: true }));
  await look(t, page, "services-timer");
}

/** The answer of the console to the next GET of `route` (as `GET /path`), which a press is about to make. */
function answerToGet(page, route) {
  return page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(route));
}

// --- The people ---

/** An administrator: the list, a failed service, the safe service and the console's own. */
async function administrator(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, ADMIN, PASSWORD);
  const session = await sessionOf(page);
  ensure(session.name === ADMIN && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);

  await openList(t, page);
  const rows = await listAsAnswered(t, page);
  await lookAll(t, page, "services-list");
  await listFiltered(t, page, rows);
  await unknownService(t, page);

  // The timers while the console is locked, which a cancelled unlock leaves so.
  await timersLocked(t, page, session.host);

  // The failed service next, while the console is locked; its log is where the console is unlocked.
  await failedService(t, page, session.host);
  await timersAsAnswered(t, page, session.host);
  await safeService(t, page, session.host);
  await consoleService(t, page, session.host);

  mustHaveCalled(seen, [
    ROUTES.list,
    routeFor(ROUTES.status, SAFE),
    routeFor(ROUTES.explain, SAFE),
    routeFor(ROUTES.explain, BROKEN),
    routeFor(ROUTES.restart, SAFE),
    routeFor(ROUTES.stop, SAFE),
    routeFor(ROUTES.start, SAFE),
    routeFor(ROUTES.disable, SAFE),
    routeFor(ROUTES.enable, SAFE),
    ROUTES.logs,
    ROUTES.logsAll,
    routeFor(ROUTES.usage, SAFE).replace("{range}", "24h"),
    routeFor(ROUTES.status, TIMED),
    ROUTES.timers,
    "POST /api/1/show",
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the list and the page of a service, no button that changes anything, and the words that say who may. */
async function reader(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, READER, PASSWORD);
  const { host } = await sessionOf(page);

  await openList(t, page);
  const rows = await rowsShown(page);
  ensure(rows.length > 0 && rows.every((row) => row.href !== null), "a reader's list has no link to the page of each service");
  ensure((await page.locator("main .svc-tbl button, main .svc-tbl input").count()) === 0, "a reader's list has a control in its rows");

  await openService(t, page, SAFE, host);
  await t.see(page.getByText(READER_LINE, { exact: true }));
  ensure((await page.locator(".svc-controls button, .svc-controls [role=switch]").count()) === 0, "a reader has a button under the state of a service");
  const shown = await shownOf(page);
  ensure(shown.buttons.length === 0 && shown.on === null, `a reader's page of ${SAFE} offers ${JSON.stringify(shown)}`);
  const card = page.locator("#log");
  await t.see(card.getByText(LOG_READER, { exact: true }));
  await t.gone(card.getByRole("button", { name: "Unlock", exact: true }));
  const status = await readStatus(page, SAFE);
  ensure(shown.state === wordOf(status.service.state) && shown.atBoot === bootOf(status.service), `a reader's page of ${SAFE} says ${shown.state}, ${shown.atBoot}, and the status ${status.service.state}, ${status.service.enabled}`);
  await look(t, page, "services-reader");

  // The timers are an administrator's: a reader is told so and asks for none.
  const timers = watchRoute(page, ROUTES.timers);
  await openScheduled(t, page);
  const note = page.locator(".svc-body .read-note");
  await t.see(note.getByText(TIMERS_READER, { exact: true }));
  ensure((await note.getByRole("button").count()) === 0, "a reader's Scheduled tasks page has a button");
  await openService(t, page, TIMED, host);
  const timerCard = await timerCardShown(t, page, (await readStatus(page, TIMED)).service);
  await t.see(timerCard.getByText(TIMER_READER, { exact: true }));
  ensure((await timerCard.getByRole("button").count()) === 0, "a reader's Timer card has a button");
  ensure(timers.count() === 0, `a reader's pages asked for ${ROUTES.timers} ${timers.count()} times`);
  timers.stop();
  // service.timers: the route refuses a reader, as the pages tell it.
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.timers)), 403, "ADM-122", "a reader's read of the scheduled tasks");
  await look(t, page, "services-timer-reader");

  mustHaveCalled(seen, [ROUTES.list, routeFor(ROUTES.status, SAFE), routeFor(ROUTES.status, TIMED), ROUTES.logs]);
  await signOut(t, page);
  await page.context().close();
}

/** The controls of the page of a service that are under 44 px tall, which a thumb needs. */
const smallControls = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("main .svc button, main .svc a.btn, main .svc-list button")]
      .filter((element) => element.offsetParent !== null)
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.height < 43.5)
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );

/** The width of the control `name` of a phone's page, and the width of the page's content. */
const widthsOf = (control) =>
  control.evaluate((element) => ({ control: element.getBoundingClientRect().width, content: element.closest(".svc").getBoundingClientRect().width }));

/** At 390 wide: the list as cards, and a failed service's page with its sentence, Run now, Open its log and the switch. */
async function phone(t) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  // The account menu is in the More sheet here, so the page is signed in when the tab bar is there.
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  const { host } = await sessionOf(page);

  await openList(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  const rows = await rowsShown(page);
  ensure(rows[0].name === BROKEN, `the phone's list starts with ${rows[0].name}`);
  await t.see(page.locator(`table.svc-tbl tr[data-name="${BROKEN}"] td[data-label="Now"]`));
  await lookAll(t, page, "services-phone-list");

  await openService(t, page, BROKEN, host);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page of a service scrolls ${await sidewaysScroll(page)} px sideways`);
  const text = await bannerAsExplained(t, page, BROKEN);
  ensure(text.includes(BROKEN), `the phone's banner does not name ${BROKEN}: "${text}"`);
  const run = buttonOf(page, "start");
  const toLog = page.getByRole("link", { name: "Open its log", exact: true });
  await t.see(run);
  await t.see(toLog);
  await t.see(switchOf(page));
  for (const [name, control] of [["Run now", run], ["Open its log", toLog]]) {
    const { control: width, content } = await widthsOf(control);
    ensure(width >= content - 4 && width >= 300, `${name} is ${Math.round(width)} px wide in a page ${Math.round(content)} px wide`);
  }
  const small = await smallControls(page);
  ensure(small.length === 0, `these controls of the page of a service are under 44 px tall on a phone: ${small.join(", ")}`);
  await lookAll(t, page, "services-phone");

  // Open its log goes to the card, which is far below the buttons.
  await toLog.click();
  await page.waitForFunction(
    () => {
      const box = document.querySelector("#log").getBoundingClientRect();
      return box.top < window.innerHeight && box.bottom > 0;
    },
    null,
    { timeout: 15_000 },
  );
  ensure(new URL(page.url()).hash === "#log", `Open its log led to ${page.url()}`);

  // The page of the timers, locked: its words and its Unlock button fit the phone.
  await openScheduled(t, page);
  await t.see(page.locator(".svc-body .read-note").getByText(TIMERS_LOCKED, { exact: true }));
  ensure((await sidewaysScroll(page)) === 0, `the phone's Scheduled tasks page scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallTimers = await smallControls(page);
  ensure(smallTimers.length === 0, `these controls of Scheduled tasks are under 44 px tall on a phone: ${smallTimers.join(", ")}`);
  await look(t, page, "services-phone-scheduled");

  mustHaveCalled(seen, [ROUTES.list, routeFor(ROUTES.status, BROKEN), routeFor(ROUTES.explain, BROKEN), ROUTES.logs]);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
