// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/overview drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Overview
// (design 10), the board at "/". An administrator signs in and finds its
// cards, each held to what the API answers the same page at the same time:
// Health, Finish setting up, What changed, Usage, This server and Using the
// most memory, and the notices above them. The states a lab machine is not in
// are answered by the test's own routes in a second page of the same browser
// (a score in each band, checks that are not on the machine, a notice, usage
// of a known size over each range), so that the words and the colours of each
// are seen whatever the machine is doing. Finish setting up lists the steps
// that pages offer, the lab's and the alert email's among them, and the table
// of "How the score works" has a row for SEC-170, Alerts reach nobody.
//
// The one thing that needs the machine is a hand edit: overview.toml's root
// step before each engine has added a comment line naming the engine to a
// service file of the engine's own (diskhealth's for Chromium, nfsd's for
// Firefox, smbd's for WebKit). The console's round that looks for hand edits
// runs every 30 seconds, so the script asks for GET /api/1/changes every few
// seconds, for up to 90, until the newest `by hand` row whose diff of the file
// adds the engine's line is there. The row says that `lab` did it: the lab
// runs a root step as `elevate -S sh -c ...`, which authd's audit log has with
// the file's path as a word of the command. The Overview then lists the edit
// under Health and under What changed, "See the change" shows its diff, "Keep
// it" asks to unlock and marks it seen, which takes it out of Health, and
// "Put back" asks what it will write back, writes it, and has a row of its
// own. A reader sees the same cards and none of the buttons that change
// something. A phone at 390 by 844 shows the short Overview.
//
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll). The accounts
// are the two that overview.toml made, so no script needs the lab's password.

import {
  answerTo,
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
  lookAll,
  mustHaveCalled,
  openConsole,
  sessionOf,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";

// The password that overview.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";
const ADMIN = "wc-overview";
const READER = "wc-overview-reader";

const PHONE = { width: 390, height: 844 };

// The service whose file overview.toml changes for each engine: services
// every image has, whose files a comment line leaves as they were.
const SERVICES = { chromium: "diskhealth", firefox: "nfsd", webkit: "smbd" };

// The account that runs the lab's root steps, and so the person whose
// elevated command names the file.
const EDITOR = "lab";

// How long a hand edit is waited for, and how often the journal is asked
// meanwhile: the round that finds hand edits runs every 30 seconds.
const WAIT_SECONDS = 90;
const ASK_EVERY_MILLISECONDS = 3000;

// How long the Health card is given to list a hand edit that the journal has:
// the card reads again on the `change` event, at most every 5 seconds.
const LISTED_SECONDS = 20;

// The action and the title of the step that Finish setting up shows on a lab
// image, which is built with systools' `lab` feature.
const LAB_ACTION = "lab.long";
const STEP_TITLE = "Run the lab's long test";
const ALERT_TITLE = "Get an email when something needs you";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with the
// number of a change for {id} and a range's word for {range}.
const ROUTES = {
  checks: "GET /api/1/checks/last",
  about: "GET /api/1/console/about",
  notices: "GET /api/1/console/notices",
  now: "GET /api/1/metrics/now",
  usage: "GET /api/1/metrics/usage/{range}",
  changes: "GET /api/1/changes",
  change: "GET /api/1/changes/{id}",
  keep: "POST /api/1/changes/{id}/keep",
  putBack: "POST /api/1/changes/{id}/put-back",
};

const routeOf = (template, values) =>
  Object.entries(values).reduce((text, [name, value]) => text.replace(`{${name}}`, String(value)), template);

/** The path of a route as the table names it: `GET /api/1/changes` is `/api/1/changes`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

// The four charts of Usage, in the order of the card, and the ranges with
// the words that name them and the length of a point, in seconds.
const CHARTS = ["CPU", "Memory used", "Disk reads and writes", "Network in and out"];
const RANGES = {
  "1h": { words: "the last hour", heading: "last hour", label: "1 h", span: 3600, every: 60 },
  "24h": { words: "the last 24 hours", heading: "last 24 hours", label: "24 h", span: 86400, every: 900 },
  "7d": { words: "the last 7 days", heading: "last 7 days", label: "7 days", span: 7 * 86400, every: 7200 },
  "1y": { words: "the last year", heading: "last year", label: "1 year", span: 365 * 86400, every: 86400 },
};

/** What the test changes for `engine`: the service, its file, the line the root step added, and the words of the row. */
function targetOf(engine) {
  const service = SERVICES[engine];
  ensure(service !== undefined, `overview.toml changes no file for ${engine}`);
  return {
    service,
    file: `/system/settings/spark/services/${service}.toml`,
    line: `# lab overview ${engine}`,
    title: `${service}.toml changed outside this console`,
  };
}

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

/** Waits for the Overview to be shown and for every card in view to have read: none says "Loading…". */
async function overviewLoaded(t, page) {
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await page.waitForFunction(
    () => {
      const bodies = [...document.querySelectorAll(".ov .card-body")].filter((body) => body.offsetParent !== null);
      return bodies.length > 0 && bodies.every((body) => body.textContent.trim() !== "Loading…");
    },
    null,
    { timeout: 30_000 },
  );
}

/** Shows the Overview again. */
async function reopen(t, page) {
  await page.goto(new URL("/", page.url()).href);
  await overviewLoaded(t, page);
}

/** What a route of the test answers with: `body` as JSON, with `status`. */
const answerOf = (status, body) => ({ status, contentType: "application/json", body: JSON.stringify(body) });

/** An error as the console answers it. */
const failure = (status, id, title, detail = "") => answerOf(status, { error: { id, title, detail, fix: "" } });

/** The aria labels of the charts of the Usage card, in the order they are drawn. */
const chartLabels = (card) => card.locator("svg.chart").evaluateAll((pictures) => pictures.map((picture) => picture.getAttribute("aria-label")));

/** A dialog's own close button, which the dialogs have in their foot. */
const closeButton = (dialog) => dialog.getByRole("button", { name: "Close", exact: true });

// --- The cards, as the machine answers ---

/** Which state the kept checks are in, from what `GET /api/1/checks/last` answered: ok, missing, none or error. */
function checksState(answer) {
  if (answer.status === 200 && Array.isArray(answer.json?.results)) return "ok";
  const id = answer.json?.error?.id;
  if (id === "ADM-129") return "missing";
  if (id === "ADM-123") return "none";
  return "error";
}

/** The words each state of the checks has on the card, in the line under the ring. */
const CHECK_WORDS = {
  missing: "Checks are not on this server yet.",
  none: "Not checked yet.",
  error: "The checks could not be read.",
};

/** The Health card says what the checks say: a score and its band, or why there is none. */
async function healthAsAnswered(t, page) {
  const card = page.locator("#ov-health");
  const before = checksState(await fetchAsPage(page, "GET", pathOf(ROUTES.checks)));
  const line = (await card.locator(".st").innerText()).trim();
  const label = await card.locator(".score").getAttribute("aria-label");
  const shown = Object.entries(CHECK_WORDS).find(([, words]) => words === line)?.[0] ?? (/^Health score \d+ out of 100, /.test(label) ? "ok" : "unknown");
  const after = checksState(await fetchAsPage(page, "GET", pathOf(ROUTES.checks)));
  // The checks may have run between the two reads: the card shows one of the two.
  ensure(shown === before || shown === after, `the Health card shows "${line}", which is not the state of ${ROUTES.checks}: ${before}, then ${after}`);
  if (shown === "ok") {
    const band = /^Health score \d+ out of 100, (Good|Could be better|Needs you)$/.exec(label);
    ensure(band !== null && band[1] === line, `the score's label is "${label}" and the line under it "${line}"`);
  }
  if (shown === "none") await t.see(card.getByText("elevate dukovo-check", { exact: true }));
}

/** The notices at the top are the ones the machine keeps, none of them hidden in a browser that is new. */
async function noticesAsAnswered(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.notices));
  ensure(answer.status === 200 && Array.isArray(answer.json?.notices), `${ROUTES.notices} was answered ${answer.status}: ${answer.text}`);
  const names = await page.locator(".ov-notices [data-notice]").evaluateAll((banners) => banners.map((banner) => banner.getAttribute("data-notice")));
  const kept = answer.json.notices.filter((notice) => Array.isArray(notice.lines) && notice.lines.length > 0).map((notice) => notice.name);
  ensure(JSON.stringify(names) === JSON.stringify(kept), `the notices on the page are ${names.join(", ")} and the machine keeps ${kept.join(", ")}`);
}

/** Usage draws four charts when `GET /api/1/metrics/usage/24h` has numbers, and says so when it has none. */
async function usageAsAnswered(t, page) {
  const card = page.locator("#ov-usage");
  const read = async () => {
    const answer = await fetchAsPage(page, "GET", pathOf(routeOf(ROUTES.usage, { range: "24h" })));
    const none = answer.status === 422 && answer.json?.error?.id === "ADM-123";
    ensure(answer.status === 200 || none, `the usage was answered ${answer.status}: ${answer.text}`);
    return answer.status === 200 && Array.isArray(answer.json) && answer.json.length > 0;
  };
  const before = await read();
  const shown = await card.locator(".usage-grid").isVisible();
  const after = await read();
  // The first numbers may have come between the two reads, and never go away.
  ensure(shown ? after : !before, `the Usage card ${shown ? "draws charts" : "has none"} and the usage had numbers ${before}, then ${after}`);
  if (!shown) {
    await t.see(card.getByText(/^No numbers yet/));
    return false;
  }
  const labels = await chartLabels(card);
  ensure(
    labels.length === CHARTS.length && CHARTS.every((name, at) => labels[at].startsWith(`${name} over the last 24 hours: `)),
    `the charts are labelled ${JSON.stringify(labels)}`,
  );
  ensure(labels.some((label) => !label.endsWith(": no numbers yet")), "no chart has numbers although the usage has some");
  return true;
}

/**
 * This server says what `GET /api/1/console/about` says. The command runs as
 * the person, so an account that may not run it (`lenient`) has the card say
 * so in its place, which is all that is checked of it.
 */
async function serverAsAnswered(page, { lenient = false } = {}) {
  const about = await fetchAsPage(page, "GET", pathOf(ROUTES.about));
  if (lenient && about.status !== 200) return null;
  ensure(about.status === 200 && typeof about.json?.host === "string", `${ROUTES.about} was answered ${about.status}: ${about.text}`);
  const rows = await page.locator("#ov-server dl.rows").evaluate((list) => {
    const found = {};
    for (const term of list.querySelectorAll("dt")) found[term.textContent.trim()] = term.nextElementSibling.textContent.replace(/\s+/g, " ").trim();
    return found;
  });
  const facts = about.json;
  const wanted = [["Name", facts.host], ["System", facts.release], ["Time", facts.zone], ["SSH host key", facts.ssh_host_key]];
  for (const [label, value] of wanted) {
    if (typeof value !== "string" || value === "") continue;
    ensure(rows[label]?.startsWith(value), `This server says "${rows[label]}" for ${label}, and the machine says "${value}"`);
  }
  if (Number.isFinite(facts.up)) ensure(rows["Up for"] !== undefined, "This server has no Up for row although the machine says how long it has been up");
  return facts;
}

/** Using the most memory lists services that `GET /api/1/metrics/now` gives memory for, or says there are no numbers. */
async function topAsAnswered(t, page) {
  const card = page.locator("#ov-top");
  const named = async () => {
    const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.now));
    if (answer.status !== 200 || answer.json === null || typeof answer.json !== "object") return [];
    return Object.entries(answer.json)
      .filter(([key, value]) => /^service\.memory\{.+\}$/.test(key) && typeof value === "number" && value > 0)
      .map(([key]) => key.slice("service.memory{".length, -1));
  };
  const before = await named();
  const found = await card.locator("tbody tr td:first-child b").allInnerTexts();
  const after = await named();
  if (found.length === 0) {
    ensure(before.length === 0, `the card lists no service and the machine gives memory for ${before.join(", ")}`);
    await t.see(card.getByText(/^No numbers yet/));
    return;
  }
  ensure(found.length <= 5, `the card lists ${found.length} services, and five is the most`);
  const known = new Set([...before, ...after]);
  ensure(found.every((name) => known.has(name)), `the card lists ${found.join(", ")}, and the machine gives memory for ${[...known].join(", ")}`);
}

/** Finish setting up is the card of the lab's step on a lab image, and "Set up" leads to /setup with the step chosen. */
async function setupAsAnswered(t, page, host) {
  const card = page.locator("#ov-setup");
  const listed = await fetchAsPage(page, "GET", "/api/1/actions");
  ensure(listed.status === 200 && Array.isArray(listed.json), `GET /api/1/actions was answered ${listed.status}: ${listed.text}`);
  if (!listed.json.some((each) => each.id === LAB_ACTION)) {
    // A build without the lab feature has no step, and the card is not there.
    await t.gone(card);
    return;
  }
  const tasks = await fetchAsPage(page, "GET", "/api/1/tasks");
  ensure(tasks.status === 200 && Array.isArray(tasks.json?.tasks), `GET /api/1/tasks was answered ${tasks.status}: ${tasks.text}`);
  const done = tasks.json.tasks.some((task) => task.action === LAB_ACTION && task.state === "done");
  await t.see(card);
  // The steps are every page's that offers one here, so the count is the card's own rows.
  const total = await card.locator("ul.todo > li").count();
  ensure(total >= 2, `Finish setting up lists ${total} steps, and the lab's and the alert email's are two`);
  await t.see(card.getByText(new RegExp(`^\\d+ of ${total} done$`)));
  const step = card.locator("li").filter({ hasText: STEP_TITLE });
  await t.see(step);

  // The alert email step is listed, and leads to /setup with the step chosen, unless email is set up.
  const email = card.locator("li").filter({ hasText: ALERT_TITLE });
  await t.see(email);
  const setUp = email.getByRole("link", { name: `Set up ${ALERT_TITLE}`, exact: true });
  if ((await setUp.count()) === 0) ensure((await email.innerText()).includes("Done"), "the alert email step has no Set up and does not say it is done");
  else ensure((await setUp.getAttribute("href")) === "/setup?step=alert-email", `Set up of the alert email step goes to ${await setUp.getAttribute("href")}`);
  const link = card.getByRole("link", { name: `Set up ${STEP_TITLE}` });
  if (done) {
    await t.gone(link);
    ensure((await step.innerText()).includes("Done"), "the step ran to its end and the card does not say it is done");
    return;
  }
  await link.click();
  await t.see(page.getByRole("heading", { name: `Make ${host} safe in a few minutes`, level: 1 }));
  ensure(new URL(page.url()).searchParams.get("step") === "lab-long", `"Set up" led to ${page.url()}, which does not choose the step`);
  await page.goBack();
  await overviewLoaded(t, page);
}

/** The head of the page and its cards, by their headings. */
async function head(t, page, host) {
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await seeFocusOn(page, "Overview");
  ensure((await page.title()) === `Overview · ${host} · Dukovo Admin`, `the tab is called "${await page.title()}"`);
  const wanted = [["ov-health", "Health"], ["ov-changed", "What changed"], ["ov-usage", "Usage, last 24 hours"], ["ov-server", "This server"], ["ov-top", "Using the most memory"]];
  for (const [id, words] of wanted) {
    const heading = page.locator(`#${id} > h2`);
    await t.see(heading);
    ensure((await heading.innerText()).startsWith(words), `the card ${id} is headed "${await heading.innerText()}", not "${words}"`);
  }
}

// --- The states the machine may not be in ---

/** A finding of the kept checks, as `dukovo-check` keeps it. */
const finding = (id, result, area, check, detail, fix) => ({ area, check, result, detail, id, fix });

const FIREWALL = finding("SEC-101", "FAILED", "firewall", "open ports", "a firewall rule lets every port in from everywhere", "Remove the rule that lets every port in.");
const PASSWORDS = finding("SEC-110", "warning", "ssh", "passwords", "passwords sign in over SSH", "Use a key and turn passwords off.");
const WAITING = finding("SEC-130", "warning", "updates", "waiting", "a newer release is waiting", "Install it from Updates.");
const STALE = finding("SEC-131", "warning", "updates", "recent check", "no check for updates in 8 days", "Check for updates.");
const CLOCK = finding("SEC-140", "warning", "time", "clock", "the clock has not synchronized in a day", "Look at the time source.");
const EARLY = finding("MET-001", "warning", "health", "early warning", "memory is above 90%", "Close what uses the most of it.");

/** The result `dukovo-check` keeps for `list`, checked an hour ago. */
const kept = (list) =>
  answerOf(200, {
    version: 1,
    checked: new Date(Date.now() - 3_600_000).toISOString().replace(/\.\d+Z$/, "Z"),
    release: "lab",
    results: list,
    counts: { passed: 0, warning: list.length, failed: 0, skipped: 0 },
  });

// What each score is, from the weights of the "How the score works" table:
// 100 less 20 (a FAILED weighs twice) and 6, 4, 4 and 4 is 62; 100 less 6, 6
// and 4 is 84; 100 less 4 is 96.
const BANDS = [
  { list: [FIREWALL, PASSWORDS, STALE, CLOCK, EARLY], label: "Health score 62 out of 100, Needs you", line: "Needs you" },
  { list: [PASSWORDS, WAITING, CLOCK], label: "Health score 84 out of 100, Could be better", line: "Could be better" },
  { list: [STALE], label: "Health score 96 out of 100, Good", line: "Good" },
];

const NOTICES = answerOf(200, {
  notices: [
    {
      name: "failed-start",
      lines: ["This server went back to the earlier version.", "The update installed, but a service failed a required check after the restart."],
      at: Math.floor(Date.now() / 1000) - 7 * 3600,
    },
    { name: "lab-note", lines: ["A note the machine kept for the next sign-in."], at: Math.floor(Date.now() / 1000) - 3600 },
  ],
});

/** `GET /api/1/metrics/usage/{range}` for a machine whose numbers never change, so that the words of each chart are known. */
function usageOf(range) {
  const { span, every } = RANGES[range];
  const now = Math.floor(Date.now() / 1000);
  const count = Math.floor(span / every);
  const series = (name, value) => ({ series: name, points: Array.from({ length: count + 1 }, (_, at) => [now - (count - at) * every, value]) });
  return [
    series("cpu", 20),
    series("memory.used", 4_000_000_000),
    series("memory.available", 12_000_000_000),
    series("disk.read{vda}", 1_000_000),
    series("disk.write{vda}", 500_000),
    series("net.rx{eth0}", 1_250_000),
    series("net.tx{eth0}", 125_000),
  ];
}

/** What a chart's label says of `range` when usageOf's numbers are drawn: each pattern starts a label. */
function chartPatterns(range) {
  const { words } = RANGES[range];
  const at = " at [^;]+";
  return [
    new RegExp(`^CPU over ${words}: average 20%, peak 20%${at}$`),
    new RegExp(`^Memory used over ${words}: average 4 GB, peak 4 GB${at}$`),
    new RegExp(`^Disk reads and writes over ${words}: read: average 1 MB/s, peak 1 MB/s${at}; written: average 500 KB/s, peak 500 KB/s${at}$`),
    new RegExp(`^Network in and out over ${words}: in: average 10 Mbit/s, peak 10 Mbit/s${at}; out: average 1 Mbit/s, peak 1 Mbit/s${at}$`),
  ];
}

/**
 * A second page of the browser's context, with the answers of `checks`, the
 * notices and the usage given by the test's own routes. Gives the page and a
 * function that sets the answer of the checks.
 */
async function statesPage(t, page) {
  const states = await page.context().newPage();
  const answers = { checks: kept(BANDS[0].list) };
  await states.route("**/api/1/checks/last", (route) => route.fulfill(answers.checks));
  await states.route("**/api/1/console/notices", (route) => route.fulfill(NOTICES));
  await states.route("**/api/1/metrics/usage/*", (route) => {
    const range = new URL(route.request().url()).pathname.split("/").pop();
    return route.fulfill(RANGES[range] ? answerOf(200, usageOf(range)) : failure(400, "ADM-125", "The usage range is 1h, 24h, 7d or 1y"));
  });
  await states.goto(new URL("/", page.url()).href);
  await overviewLoaded(t, states);
  return { states, setChecks: (answer) => { answers.checks = answer; } };
}

/** The Health card for a score: its ring and its band in words, and what the list says. */
async function scoreShown(t, page, band) {
  const card = page.locator("#ov-health");
  await t.see(card.locator(".score"));
  ensure((await card.locator(".score").getAttribute("aria-label")) === band.label, `the ring says "${await card.locator(".score").getAttribute("aria-label")}", not "${band.label}"`);
  ensure((await card.locator(".st").innerText()).trim() === band.line, `the line under the ring is "${await card.locator(".st").innerText()}", not "${band.line}"`);
}

/** The list of what needs a look: at most four rows and, when there are more, "See all N" with a dialog that names every one. */
async function seeAll(t, page) {
  const card = page.locator("#ov-health");
  const count = Number((/^(\d+) things to look at$/.exec((await card.locator(".look").innerText()).trim()) ?? [])[1]);
  ensure(count >= 5, `the card says "${await card.locator(".look").innerText()}" for a result with five findings`);
  ensure((await card.locator("ul.need > li").count()) === 4, `the card lists ${await card.locator("ul.need > li").count()} rows, not four`);
  // Changes that come in draw the card again, and may change its count:
  // the dialog lists as many things as the card had when it was opened.
  const open = card.getByRole("button", { name: /^See all \d+$/ });
  await open.click();
  const dialog = page.getByRole("dialog", { name: /^\d+ things to look at$/ });
  await t.see(dialog);
  const listed = Number(/^\d+/.exec(await dialog.getByRole("heading").first().innerText())[0]);
  ensure(listed >= 5, `the dialog says ${listed} things for a result with five findings`);
  ensure((await dialog.locator("li.all-row").count()) === listed, `the dialog lists ${await dialog.locator("li.all-row").count()} things, not ${listed}`);
  for (const one of BANDS[0].list) {
    const title = one.detail.charAt(0).toUpperCase() + one.detail.slice(1);
    await t.see(dialog.getByText(title));
  }
  await t.see(dialog.getByText("Needs you: ").first());
  await look(t, page, "overview-see-all");
  await closeButton(dialog).click();
  await t.gone(dialog);
  // The card may be drawn again while the dialog is open, as changes come
  // in, and its count with it: focus goes to the See all that is there.
  const back = card.getByRole("button", { name: /^See all \d+$/ });
  ensure(await back.evaluate((element) => element === document.activeElement), "focus did not go back to See all when its dialog closed");
}

/** "How the score works": the rules, the weights and when the checks last ran. */
async function scoreExplained(t, page) {
  await page.locator("#ov-health").getByRole("button", { name: "How the score works", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "How the score works", exact: true });
  await t.see(dialog);
  await t.see(dialog.getByText("The score starts at 100.", { exact: false }));
  await t.see(dialog.getByText("Good is 90 and over; Could be better is 70 to 89; Needs you is under 70.", { exact: false }));
  const table = dialog.getByRole("region", { name: "What each check takes off" });
  await t.see(table);
  for (const id of ["SEC-101", "SEC-150", "SEC-170", "MET-…", "HW-…"]) await t.see(table.getByRole("rowheader", { name: id, exact: true }));
  // Alerts that reach nobody take off 6.
  const alerts = (await table.locator("tbody tr").filter({ hasText: "SEC-170" }).locator("th, td").allInnerTexts()).map((cell) => cell.trim());
  ensure(alerts.join(" | ") === "SEC-170 | Alerts reach nobody | 6", `the row of SEC-170 says ${alerts.join(" | ")}, not SEC-170 | Alerts reach nobody | 6`);
  await t.see(dialog.getByText(/^Checked (today at|yesterday at) /));
  await look(t, page, "overview-score");
  await page.keyboard.press("Escape");
  await t.gone(dialog);
}

/** A notice is dismissed in this browser, and the same one does not come back. */
async function noticeDismissed(t, page) {
  const banner = page.locator('.ov-notices [data-notice="failed-start"]');
  await t.see(banner);
  await t.see(banner.getByText("This server went back to the earlier version."));
  await t.see(page.locator('.ov-notices [data-notice="lab-note"]'));
  await banner.getByRole("button", { name: "Dismiss", exact: true }).click();
  await t.gone(banner);
  await t.see(page.locator('.ov-notices [data-notice="lab-note"]'));
  await page.reload();
  await overviewLoaded(t, page);
  await t.gone(page.locator('.ov-notices [data-notice="failed-start"]'));
  await t.see(page.locator('.ov-notices [data-notice="lab-note"]'));
}

/** The charts, drawn from numbers of a known size: their labels, the range control and its keys, the address, and "Show the numbers". */
async function usageDrawn(t, page) {
  const card = page.locator("#ov-usage");
  const labelsMatch = async (range) => {
    const patterns = chartPatterns(range);
    await page.waitForFunction(
      (wanted) => [...document.querySelectorAll("#ov-usage svg.chart")].map((picture) => picture.getAttribute("aria-label") ?? "")
        .every((label, at) => new RegExp(wanted[at]).test(label)) && document.querySelectorAll("#ov-usage svg.chart").length === wanted.length,
      patterns.map((pattern) => pattern.source),
      { timeout: 15_000 },
    );
  };
  const radio = (range) => card.getByRole("radio", { name: RANGES[range].label, exact: true });
  await t.see(card.getByRole("radiogroup", { name: "Range" }));
  ensure((await radio("24h").getAttribute("aria-checked")) === "true", "the range is not 24 h to begin with");
  await labelsMatch("24h");
  ensure((await card.locator('[data-chart="cpu"] .stat b').innerText()) === "20%", "the CPU chart's number is not 20%");

  // A range is chosen by a press, and is kept in the address.
  await radio("7d").click();
  await t.see(card.getByRole("heading", { name: "Usage, last 7 days" }));
  await labelsMatch("7d");
  ensure(new URL(page.url()).searchParams.get("range") === "7d", `the address is ${page.url()}, which does not name the range 7d`);
  ensure((await radio("7d").getAttribute("aria-checked")) === "true" && (await radio("24h").getAttribute("aria-checked")) === "false", "the range control does not mark 7 days alone");

  // The arrow keys move between the ranges, and the card follows.
  await radio("7d").focus();
  await page.keyboard.press("ArrowRight");
  ensure(await radio("1y").evaluate((element) => element === document.activeElement), "ArrowRight did not move focus to 1 year");
  await t.see(card.getByRole("heading", { name: "Usage, last year" }));
  await labelsMatch("1y");

  // The range is in the address, so a page that is read again has it.
  await page.reload();
  await overviewLoaded(t, page);
  ensure((await radio("1y").getAttribute("aria-checked")) === "true", "the range 1 year was not kept by the address");
  await labelsMatch("1y");

  // The numbers behind a chart are a table that opens and closes.
  const cpu = card.locator('[data-chart="cpu"]');
  const show = cpu.getByRole("button", { name: "Show the numbers", exact: true });
  await show.click();
  const numbers = cpu.getByRole("region", { name: "Numbers of CPU over the last year" });
  await t.see(numbers);
  await t.see(numbers.getByRole("columnheader", { name: "CPU", exact: true }));
  ensure((await numbers.locator("tbody tr").count()) > 300, "the table of a year of CPU has fewer than 300 rows");
  await look(t, page, "overview-numbers");
  await cpu.getByRole("button", { name: "Hide the numbers", exact: true }).click();
  await t.gone(numbers);
  await radio("24h").click();
  await labelsMatch("24h");
}

/** The other states of Health, each answered by the test's route: a score in each band, checks that are not on the machine, none kept and one that cannot be read. */
async function otherStates(t, states, setChecks) {
  const card = states.locator("#ov-health");
  const change = async (answer) => {
    setChecks(answer);
    await states.reload();
    await overviewLoaded(t, states);
  };
  for (const band of BANDS.slice(1)) {
    await change(kept(band.list));
    await scoreShown(t, states, band);
  }
  await change(failure(501, "ADM-129", "This server does not have that command yet", "The command `dukovo-check` is not on this machine."));
  await t.see(card.getByText(CHECK_WORDS.missing, { exact: true }));
  ensure((await card.locator(".score").getAttribute("aria-label")) === "No health score", "the ring has a score for checks that are not on the machine");
  await look(t, states, "overview-no-checks");

  await change(failure(422, "ADM-123", "The command did not work", "dukovo-check: no result is kept yet."));
  await t.see(card.getByText(CHECK_WORDS.none, { exact: true }));
  await t.see(card.getByText("elevate dukovo-check", { exact: true }));
  await look(t, states, "overview-not-checked");

  await change(failure(500, "ADM-111", "authd did not answer"));
  await t.see(card.getByText(CHECK_WORDS.error, { exact: true }));
}

/** All of the above, on a page that is told what to answer. */
async function states(t, page) {
  const { states: other, setChecks } = await statesPage(t, page);
  await scoreShown(t, other, BANDS[0]);
  await lookAll(t, other, "overview-states");
  await seeAll(t, other);
  await scoreExplained(t, other);
  await noticeDismissed(t, other);
  await usageDrawn(t, other);
  await otherStates(t, other, setChecks);
  await other.close();
}

// --- A hand edit ---

/** Change `id` as `GET /api/1/changes/{id}` gives it, with the diff of each file. */
async function readChange(page, id) {
  const answer = await fetchAsPage(page, "GET", `/api/1/changes/${id}`);
  ensure(answer.status === 200, `GET /api/1/changes/${id} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The lines that the diff of `path` in `change` adds, for `sign` `+`, or removes, for `-`, without their sign. */
function linesOf(change, path, sign) {
  const file = change.files.find((each) => each.path === path);
  ensure(file !== undefined, `change ${change.id} does not name ${path}: ${JSON.stringify(change.files)}`);
  return (file.diff ?? "")
    .split("\n")
    .filter((text) => text.startsWith(sign) && !text.startsWith(`${sign.repeat(3)} `))
    .map((text) => text.slice(1));
}

/** The newest row made `by hand` whose diff of `target`'s file adds its line, waited for up to `WAIT_SECONDS`. */
async function handEditAdding(page, { file: path, line }) {
  const until = Date.now() + WAIT_SECONDS * 1000;
  const others = new Set();
  for (;;) {
    const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
    ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
    for (const row of list.json.changes) {
      if (row.how !== "by hand" || others.has(row.id) || !row.files.some((file) => file.path === path)) continue;
      const change = await readChange(page, row.id);
      if (linesOf(change, path, "+").includes(line)) return change;
      others.add(row.id);
    }
    ensure(Date.now() < until, `no row made by hand adds "${line}" to ${path} within ${WAIT_SECONDS} seconds`);
    await sleep(ASK_EVERY_MILLISECONDS);
  }
}

/** The selector of the row that the hand edit `edit` is in the Health list and in the dialog that "See all" opens. Other tests leave unseen hand edits of the same file, so a title can name two rows; the row's id names one. */
const needOf = (edit) => `[data-need="change-${edit.id}"]`;

/**
 * The thing to look at that the edit is, in the Health list or, when four
 * others come first, in the dialog that "See all" opens, waited for up to
 * `LISTED_SECONDS`: a dialog that opened on the list as it was before the card
 * read again is closed and opened once more.
 */
async function listedInHealth(t, page, edit, title) {
  const card = page.locator("#ov-health");
  const until = Date.now() + LISTED_SECONDS * 1000;
  for (;;) {
    if ((await card.locator(`ul.need ${needOf(edit)}`).count()) > 0) return;
    const more = card.getByRole("button", { name: /^See all \d+$/ });
    if ((await more.count()) > 0) {
      await more.click();
      const dialog = page.getByRole("dialog", { name: /^\d+ things to look at$/ });
      await t.see(dialog);
      const listed = (await dialog.locator(needOf(edit)).count()) > 0;
      await closeButton(dialog).click();
      await t.gone(dialog);
      if (listed) return;
    }
    ensure(Date.now() < until, `the Health card does not list "${title}" within ${LISTED_SECONDS} seconds, in its list or in See all`);
    await sleep(ASK_EVERY_MILLISECONDS);
  }
}

/** The edit in What changed, found by its change's id since other tests leave edits of the same file with the same title, with the dot that asks for a look, who did it, and the buttons that fit it. */
async function editListed(t, page, edit, title) {
  const item = page.locator(`#ov-changed .tl-item[data-change="${edit.id}"]`);
  await t.see(item);
  ensure((await item.locator("time").getAttribute("datetime")) !== null, "the row has no time");
  await t.see(item.getByText("Needs a look"));
  await t.see(item.getByText(`${EDITOR} edited it by hand`));
  for (const words of ["Keep it", "Put back", "See the change"]) await t.see(item.getByRole("button", { name: `${words}: ${title}`, exact: true }));
  return item;
}

/** "See the change": a dialog with who and when, and the diff of the file with the line that was added. */
async function changeSeen(t, page, item, target) {
  const open = item.getByRole("button", { name: `See the change: ${target.title}`, exact: true });
  await open.click();
  const dialog = page.getByRole("dialog", { name: target.title, exact: true });
  await t.see(dialog);
  await t.see(dialog.getByText(`${EDITOR} edited it by hand`));
  const diff = dialog.getByLabel(`Change of ${target.file}`);
  await t.see(diff);
  ensure((await diff.innerText()).includes(target.line), `the diff of ${target.file} does not add "${target.line}"`);
  await look(t, page, "overview-change");
  await closeButton(dialog).click();
  await t.gone(dialog);
  ensure(await open.evaluate((element) => element === document.activeElement), "focus did not go back to See the change when its dialog closed");
}

/** "Keep it" asks to unlock, and then marks the edit seen, which takes it out of Health. */
async function editKept(t, page, item, edit, target) {
  await item.getByRole("button", { name: `Keep it: ${target.title}`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes", exact: true });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const answered = answerTo(page, `/api/1/changes/${edit.id}/keep`);
  await dialog.getByRole("button", { name: "Unlock", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `Keep it was answered ${answer.status()}`);
  await t.see(page.getByText(`Kept: ${target.title}. It stays as it is.`));
  await t.see(item.getByText(`kept by ${ADMIN}`));
  await t.gone(item.getByRole("button", { name: `Keep it: ${target.title}`, exact: true }));
  await t.gone(page.locator(`#ov-health ul.need ${needOf(edit)}`));
  const row = await readChange(page, edit.id);
  ensure(row.seen?.by === ADMIN, `the journal says ${JSON.stringify(row.seen)} after Keep it, not that ${ADMIN} saw it`);
}

/** "Put back" says what it writes back and the command it runs, can be cancelled, and writes the file back when it is not. */
async function editPutBack(t, page, item, edit, target) {
  let posted = 0;
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/put-back")) posted += 1;
  });
  const openAsk = () => item.getByRole("button", { name: `Put back: ${target.title}`, exact: true }).click();
  const ask = page.getByRole("dialog", { name: `Put back: ${target.title}`, exact: true });

  await openAsk();
  await t.see(ask);
  await t.see(ask.getByText(target.file, { exact: true }));
  await t.see(ask.getByText(`elevate admin put-back ${edit.id}`, { exact: true }));
  await look(t, page, "overview-put-back");
  await ask.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(ask);
  ensure(posted === 0, "Cancel put the change back");

  await openAsk();
  await t.see(ask);
  const answered = answerTo(page, `/api/1/changes/${edit.id}/put-back`);
  await ask.getByRole("button", { name: "Put it back", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `Put it back was answered ${answer.status()}: ${await answer.text()}`);
  const told = await answer.json();
  ensure(told.done === true && told.command === `elevate admin put-back ${edit.id}` && Number.isInteger(told.change), `the put back was answered ${JSON.stringify(told)}`);
  await t.see(page.getByText(`Put back: ${target.title}.`, { exact: true }));

  // The put back is a change of the console's, with a row of its own that names the one it puts back.
  const row = await readChange(page, told.change);
  ensure(row.action === "changes.put-back" && row.note === `puts back change ${edit.id}`, `the put back's own row is ${JSON.stringify(row)}`);
  ensure(linesOf(row, target.file, "-").includes(target.line), `the put back's row does not remove "${target.line}" from ${target.file}`);
  await t.see(page.locator("#ov-changed .tl-item").filter({ hasText: "Put a change back" }).first());
}

/** The hand edit that overview.toml made: found, listed, looked at, kept and put back. */
async function handEdit(t, page, target) {
  const edit = await handEditAdding(page, target);
  ensure(edit.who === EDITOR, `the row of the hand edit names ${edit.who}, not ${EDITOR}: admind could not tell who wrote ${target.file} from authd's audit lines`);
  ensure(edit.seen === null, `the hand edit is marked seen before anyone looked: ${JSON.stringify(edit.seen)}`);
  await reopen(t, page);
  await listedInHealth(t, page, edit, target.title);
  const item = await editListed(t, page, edit, target.title);
  await lookAll(t, page, "overview-hand-edit");
  await changeSeen(t, page, item, target);
  await editKept(t, page, item, edit, target);
  await editPutBack(t, page, item, edit, target);
  return edit;
}

// --- The people ---

/** An administrator: the cards as the machine answers, the states, and the hand edit. */
async function administrator(t) {
  const target = targetOf(t.engine);
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, ADMIN, PASSWORD);
  await overviewLoaded(t, page);
  const session = await sessionOf(page);
  ensure(session.name === ADMIN && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);

  await head(t, page, session.host);
  await healthAsAnswered(t, page);
  await noticesAsAnswered(page);
  await usageAsAnswered(t, page);
  await serverAsAnswered(page);
  await topAsAnswered(t, page);
  await setupAsAnswered(t, page, session.host);
  mustHaveCalled(seen, [ROUTES.checks, ROUTES.about, ROUTES.notices, ROUTES.now, routeOf(ROUTES.usage, { range: "24h" }), ROUTES.changes]);

  await states(t, page);
  const edit = await handEdit(t, page, target);
  mustHaveCalled(seen, [
    routeOf(ROUTES.usage, { range: "7d" }),
    routeOf(ROUTES.usage, { range: "1y" }),
    routeOf(ROUTES.change, { id: edit.id }),
    routeOf(ROUTES.keep, { id: edit.id }),
    routeOf(ROUTES.putBack, { id: edit.id }),
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the cards, a score that is for administrators, and no button that changes something. */
async function reader(t) {
  const { page } = await openConsole(t, "/");
  await signInDone(t, page, READER, PASSWORD);
  await overviewLoaded(t, page);
  const { host } = await sessionOf(page);
  await head(t, page, host);
  const health = page.locator("#ov-health");
  await t.see(health.getByText("The health score is for administrators.", { exact: true }));
  ensure((await health.locator(".score").getAttribute("aria-label")) === "No health score", "a reader has a health score");
  await t.gone(page.locator("#ov-setup"));
  const changed = page.locator("#ov-changed");
  ensure((await changed.getByRole("button", { name: /^(Keep it|Put back|Undo)/ }).count()) === 0, "a reader has a button that changes something under What changed");
  const rows = await changed.locator(".tl-item").count();
  ensure((await changed.getByRole("button", { name: /^See the change/ }).count()) === rows, "a reader cannot look at each change");
  await serverAsAnswered(page, { lenient: true });
  await look(t, page, "overview-reader");
  await signOut(t, page);
  await page.context().close();
}

/** The targets of the phone that are under 44 px each way, which a thumb needs. */
const smallTargets = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("main button, main a[href], main input, main summary")]
      .filter((element) => element.offsetParent !== null && !element.classList.contains("skip"))
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && (box.height < 43.5 || box.width < 43.5))
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );

/** At 390 wide: the notices, Health, two charts and how long the machine has been up, and nothing else. */
async function phone(t) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  // The account menu is in the More sheet here, so the page is signed in when the tab bar is there.
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await overviewLoaded(t, page);
  await seeFocusOn(page, "Overview");
  ensure((await sidewaysScroll(page)) === 0, `the phone's Overview scrolls ${await sidewaysScroll(page)} px sideways`);

  await t.see(page.locator("#ov-health"));
  for (const id of ["ov-setup", "ov-changed", "ov-server", "ov-top"]) await t.gone(page.locator(`#${id}`));
  const about = await fetchAsPage(page, "GET", pathOf(ROUTES.about));
  ensure(about.status === 200, `${ROUTES.about} was answered ${about.status}: ${about.text}`);
  if (Number.isFinite(about.json.up)) {
    await t.see(page.locator("p.ov-up"));
    ensure(/^Up for /.test(await page.locator("p.ov-up").innerText()), `the line under the charts is "${await page.locator("p.ov-up").innerText()}"`);
  }
  if (await page.locator("#ov-usage .usage-grid").isVisible()) {
    await t.see(page.locator('[data-chart="cpu"]'));
    await t.see(page.locator('[data-chart="memory"]'));
    await t.gone(page.locator('[data-chart="disk"]'));
    await t.gone(page.locator('[data-chart="network"]'));
  }
  ensure((await smallTargets(page)).length === 0, `these controls of the Overview are under 44 px on a phone: ${(await smallTargets(page)).join(", ")}`);
  await lookAll(t, page, "overview-phone");

  // The same with a score that needs you, two notices and charts: the most the phone shows.
  const { states: other } = await statesPage(t, page);
  await t.see(other.locator("#ov-health"));
  ensure((await sidewaysScroll(other)) === 0, `the phone's Overview with notices scrolls ${await sidewaysScroll(other)} px sideways`);
  ensure((await smallTargets(other)).length === 0, `these controls are under 44 px on a phone, with notices and a long list: ${(await smallTargets(other)).join(", ")}`);
  await lookAll(t, other, "overview-phone-states");
  await other.close();

  await signOut(t, page);
  mustHaveCalled(seen, [ROUTES.checks, ROUTES.notices, routeOf(ROUTES.usage, { range: "24h" })]);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
