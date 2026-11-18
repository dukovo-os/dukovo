// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/alerts drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Alerts page at
// /alerts. A locked console reads nothing and asks to unlock; once unlocked, the
// Active card is what `alerts list` says (`GET /api/1/alerts`), Recent is what
// `alerts history` says (`GET /api/1/alerts/history`) and Where alerts go is
// what `alerts status` says (`GET /api/1/alerts/channels`), a line for the
// server's screens and a row for each channel with what it is told of and how it
// last did. The Email card sets the one email channel (`POST /api/1/alerts/email`):
// a field that is wrong is said where it is typed and nothing is sent. Its server
// is the lab PC's, at a port nobody listens at, so that a test alert fails
// there and says why. Add a webhook… adds a webhook of the kind that takes any
// address that takes a POST (`POST /api/1/alerts/webhooks`) to the receiver
// that the lab PC runs (alerts-hooks.sh of the monitoring tests), over TLS with
// a certificate that an authority of the test's own signed. Send a test alert
// (`POST /api/1/alerts/test`, which runs `alerts test`) shows a line for each
// channel, and the receiver has the test alert. Edit what it is told changes the
// kinds of alert a channel is told of (`POST /api/1/alerts/channels/about`) and,
// when no channel is told of a kind, says so; Remove takes email and the webhook
// away (`POST /api/1/alerts/email/remove`, `POST
// /api/1/alerts/webhooks/{number}/remove`). What was typed in a secret field,
// the mail password and the webhook's address and token, goes only in the
// body of its own call: it is not in the text of the page, in its storage, in
// an address, in the browser's console or after the form was sent in the field.
//
// This browser: Get alerts in this browser is pressed as far as the browser
// goes. The lab's browsers cannot subscribe to a push service, so the card says
// why: this browser does not trust the server's certificate (the two ways it
// then gives), or it was refused leave to show notices, or it could not subscribe
// or start its notice service, or it has no push at all. Then nothing was added
// and nothing was sent to `POST /api/1/alerts/push`. A subscription of the
// test's own (a P-256 key and an auth of its own, at an address of a push
// service that is not reachable) is handed to the machine by the route the page
// uses, and the page, which keeps only its ID, lists it as this browser: the
// card says that this browser gets alerts, with Send a test and Turn off in this
// browser, which ends it (`POST /api/1/alerts/push/{id}/remove`) and makes the
// page forget the ID. The machine's key is read (`GET /api/1/alerts/push/key`).
// The console serves the worker, the manifest and the icons of its home-screen
// app (`GET /sw.js`, `GET /manifest.webmanifest`, `GET /icons/app-192.png`,
// `GET /icons/app-512.png`), and its content security policy lets the page start
// a worker and read the manifest. Chromium's worker, where the browser started
// one, is driven through its handle (`context.serviceWorkers()`): a push shows a
// notice with the host and the title, and the page of its key; pressing it closes
// the notice. A browser that did not start the worker leaves its handlers to the
// Node tests, which run them on fakes.
//
// A reader is told that alerts are for administrators and every route is refused
// (ADM-122). A phone at 390 by 844 has Alerts as the fourth tab of its bar, one
// column, no sideways scroll, controls 44 px tall, no card of Set up a phone but
// the panel "Set up this phone" when the page is opened by `/alerts?phone=1`,
// which Not now puts away. On a computer the card Set up a phone has the QR
// code, which is held to the console's own encoder since the lab has no decoder,
// the address it holds, the fingerprint of the certificate the console serves,
// and the steps. Each look is the console's own: dark, light, high contrast,
// forced colours and less motion, each checked by axe (console.mjs's lookAll).
//
// alerts.toml raises two alerts before each engine, a warning of the Alerts page's
// own and a critical one about Hardware, clears them after, and leaves no channel
// and no certificate of the test's authority. The receiver, and where its log is,
// are what alerts.toml wrote to /tmp/lab-alerts-hooks. The accounts are the two
// that alerts.toml made, so no script needs the lab's password.

import { createECDH, randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  originOf,
  servedCertificate,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-alerts";
const READER = "wc-alerts-reader";

// What alerts.toml writes for the script: the receiver's address, its port and
// the log it keeps.
const HOOKS_FILE = "/tmp/lab-alerts-hooks";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end. `{number}` is
// the webhook's number in alerts.toml and `{id}` the ID of a browser's
// subscription, 16 hex digits.
const ROUTES = {
  list: "GET /api/1/alerts",
  history: "GET /api/1/alerts/history",
  channels: "GET /api/1/alerts/channels",
  test: "POST /api/1/alerts/test",
  email: "POST /api/1/alerts/email",
  emailRemove: "POST /api/1/alerts/email/remove",
  webhook: "POST /api/1/alerts/webhooks",
  webhookRemove: "POST /api/1/alerts/webhooks/{number}/remove",
  told: "POST /api/1/alerts/channels/about",
  pushKey: "GET /api/1/alerts/push/key",
  push: "POST /api/1/alerts/push",
  pushRemove: "POST /api/1/alerts/push/{id}/remove",
  about: "GET /api/1/console/about",
};

// The files of the console that the browser asks for, as the table names them.
const FILES = {
  worker: "GET /sw.js",
  manifest: "GET /manifest.webmanifest",
  small: "GET /icons/app-192.png",
  large: "GET /icons/app-512.png",
};

/** The path of a route as the table names it: `GET /api/1/alerts` is `/api/1/alerts`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

/** The method of a route as the table names it. */
const methodOf = (route) => route.slice(0, route.indexOf(" "));

/** The route `template` with its `{number}` or `{id}` filled in. */
const routeFor = (template, value) => template.replace(/\{(number|id)\}/, String(value));

// What the test types, and what it must never find on the page. The address of
// the webhook is built when the receiver is known.
const TO = "lab-ops@example.org";
const MAIL_USER = "lab-mail-user";
const MAIL_PASSWORD = "lab-mail-password-7391";
const MAIL_PORT = "465";
const HOOK_TOKEN = "lab-json-token";
const STAND_IN = "Lab stand-in";
const PUSH_ENDPOINT = "https://push.example.org/send/lab-1";

// The words the page says where this test looks for them.
const LOCKED_LINE = "Unlock to see the alerts.";
const READER_LINE = "Alerts are for administrators.";
const SCREENS_ONLY = "Alerts reach this server's screens only.";
const SCREENS_LINE = "On this server's screens and at the next sign-in";
const NOTHING_ACTIVE = "No alerts are active.";
const NOTHING_RECENT = "No alert has been raised or cleared.";
const NO_CHANNEL_TEST = "No channel is set up: alerts go to this server's screens only.";
const NO_ADDRESS = "Type at least one address.";
const NOT_AN_ADDRESS = "nobody is not an email address.";
const NO_SERVER = "Type the name of the mail server.";
const NO_PASSWORD = "Type the password: the mail server asks for one with a user name.";
const NO_NAME = "Give this browser a name, so that you know it in the list.";
const NO_WEB_ADDRESS = "Type the whole address, starting with https://.";
const NO_KIND = "Choose at least one kind of alert.";
const HARDWARE_LEFT = "Hardware alerts reach no channel: only this server's screens.";
const PRIVACY = /^A push notice goes from this server to the push service of your browser's maker/;
const UNSUPPORTED = "This browser cannot get push notices.";
const DENIED = "This browser said no. Allow notifications for this site in the browser's settings, then try again.";
const UNTRUSTED = "This browser does not trust this server's certificate, so it cannot get push notices. Two ways:";
const GETS = "This browser gets alerts.";

// The kinds of alert, in the words of their boxes.
const KINDS = ["Data at risk", "Something stopped", "Hardware"];

// What the card of This browser may say after Get alerts in this browser, by the
// words that show it.
const OUTCOMES = {
  certificate: UNTRUSTED,
  denied: DENIED,
  subscribe: "This browser could not subscribe:",
  service: "This browser could not start its notice service:",
  unsupported: UNSUPPORTED,
  got: GETS,
};

// --- Reading the machine ---

/** What `route` answers to the page's own request, whole: `{status, text, json}`. */
const askFor = (page, route, body = null) => fetchAsPage(page, methodOf(route), pathOf(route), body);

/** What a read route answers, which must be 200; gives its JSON. */
async function readOf(page, route) {
  const answer = await askFor(page, route);
  ensure(answer.status === 200, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** A list that a read route gives, which must be one. */
async function listOf(page, route) {
  const lines = await readOf(page, route);
  ensure(Array.isArray(lines), `${route} does not give a list: ${JSON.stringify(lines)}`);
  return lines;
}

/** Whether `value` is text with something in it. */
const isText = (value) => typeof value === "string" && value.trim() !== "";

/** Tries `test` until it stops throwing, for up to `seconds`: what the page draws follows what the machine says by a moment. */
async function eventually(test, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    try {
      return await test();
    } catch (error) {
      if (Date.now() >= until) throw error;
      await sleep(1000);
    }
  }
}

/** The line of a change's output that is JSON with `member`, parsed; null when none is. */
function outputJson(answer, member) {
  for (const line of Array.isArray(answer?.output) ? answer.output : []) {
    try {
      const found = JSON.parse(line);
      if (found !== null && typeof found === "object" && member in found) return found;
    } catch {
      // A line that is not JSON is not the one.
    }
  }
  return null;
}

// --- The lab PC's receiver ---

/** Where the receiver is, from alerts.toml: `{peer, port, log}`. */
async function hooksOf() {
  const [peer, port, log] = (await readFile(HOOKS_FILE, "utf8")).trim().split("\n");
  ensure(isText(peer) && /^\d+$/.test(port ?? "") && isText(log), `${HOOKS_FILE} does not give the receiver's address, its port and its log`);
  return { peer, port, log };
}

/** The lines of the receiver's log; none while it has taken nothing. */
async function hookLines(hooks) {
  try {
    return (await readFile(hooks.log, "utf8")).split("\n").filter(Boolean);
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

/** The lines the receiver took after its first `since` lines that have every one of `words`, waited for up to `seconds`. */
async function hookTook(hooks, since, words, seconds = 40) {
  return eventually(async () => {
    const found = (await hookLines(hooks)).slice(since).filter((line) => words.every((word) => line.includes(word)));
    ensure(found.length > 0, `the receiver took no request with ${words.join(" and ")} within ${seconds} seconds`);
    return found;
  }, seconds);
}

// --- Signing in ---

/** What a browser of `engine` is given to try notices with: the permission, where the browser can be given it (WebKit's cannot). */
const permissionsOf = (engine) => (engine === "webkit" ? {} : { permissions: ["notifications"] });

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

/**
 * What the page says and asks for while the script runs: the addresses it
 * requests and the lines of the browser's own console, to be held to the
 * secrets that were typed (`noneOf`).
 */
function watching(page) {
  const said = { urls: [], logs: [] };
  page.context().on("request", (request) => said.urls.push(request.url()));
  page.on("console", (message) => said.logs.push(message.text()));
  page.on("pageerror", (error) => said.logs.push(String(error?.message ?? error)));
  return said;
}

/** Throws when any of `secrets` is in the page's text and markup, its storage, its address, the addresses it asked for or its console. */
async function noneOf(page, said, secrets, when) {
  const text = await page.evaluate(() => [
    document.body.innerText,
    document.documentElement.outerHTML,
    location.href,
    document.cookie,
    JSON.stringify(Object.entries(localStorage)),
    JSON.stringify(Object.entries(sessionStorage)),
  ].join("\n"));
  const where = [text, ...said.urls, ...said.logs].join("\n");
  const found = secrets.filter((secret) => where.includes(secret));
  ensure(found.length === 0, `${when}: the page, its storage, its addresses or its console hold ${found.map((one) => JSON.stringify(one)).join(", ")}, which was typed in a secret field`);
}

// --- The page ---

const card = (page, id) => page.locator(`main section.card#${id}`);
const buttonIn = (scope, name) => scope.getByRole("button", { name, exact: true });
const headTest = (page) => buttonIn(page.locator("main .ph"), "Send a test alert");

/** Opens /alerts, with `after` (a hash or a query) after it, and waits until it has drawn what it read, or what it says in place of it. */
async function openAlerts(t, page, after = "") {
  await page.goto(new URL(`/alerts${after}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Alerts", exact: true }));
  await page.waitForFunction(() => {
    const grid = document.querySelector(".al-grid");
    if (grid === null) return document.querySelector("main .banner") !== null;
    if (grid.hidden) return (document.querySelector(".al > div")?.textContent ?? "").trim() !== "";
    return (document.querySelector("#active .al-body")?.textContent ?? "").trim() !== "";
  }, null, { timeout: 30_000 });
}

/**
 * Presses `control`, which asks the console to `method` `route`, and gives the
 * answer. A console that is locked asks to unlock first: the dialog is answered
 * when it comes.
 */
async function pressFor(t, page, control, route, method = "POST") {
  const answered = page.waitForResponse((answer) => answer.request().method() === method && new URL(answer.url()).pathname === route, { timeout: 120_000 });
  answered.catch(() => {});
  await control.click();
  const asked = unlockDialog(page).waitFor({ state: "visible", timeout: 60_000 }).then(() => "unlock", () => "never");
  const first = await Promise.race([answered.then(() => "answer"), asked]);
  if (first === "unlock") await answerUnlock(t, page);
  const answer = await answered;
  ensure(answer.status() === 200, `${method} ${route} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

/** Whether `control`, pressed, sends no request that is a POST to `route`. */
async function sendsNoPost(page, control, route) {
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === route) sent += 1;
  };
  page.on("request", count);
  // Pressed as a person may: Playwright would wait for it to come on.
  await control.click({ force: true });
  await sleep(1200);
  page.off("request", count);
  return sent === 0;
}

/** The request that the next press makes to `route`, which a press is about to make: `{url, body}`. */
function requestTo(page, route, method = "POST") {
  const made = page.waitForRequest((request) => request.method() === method && new URL(request.url()).pathname === route, { timeout: 120_000 });
  made.catch(() => {});
  return made.then((request) => ({ url: request.url(), body: request.postData() ?? "" }));
}

// --- Held to what the machine says ---

/** The channels of `alerts status`, which the page's rows are held to. */
const channelsOf = (page) => listOf(page, ROUTES.channels);

/** The name a channel goes by in the page's rows. */
const nameOf = (channel) => (channel.kind === "push" && isText(channel.name) ? `push · ${channel.name}` : channel.channel);

/** What a channel is told of, in the words of its row. */
function toldWords(channel) {
  const named = Array.isArray(channel.about) ? ["data", "stopped", "hardware"].filter((id) => channel.about.includes(id)) : [];
  const labels = { data: "data at risk", stopped: "something stopped", hardware: "hardware" };
  const kinds = named.length === 0 || named.length === 3 ? "every kind" : named.map((id) => labels[id]).join(", ");
  return `Told of: ${kinds} · ${channel.severity === "critical" ? "critical only" : "every alert"}${channel.cleared === true ? ", and when one clears" : ""}`;
}

/** The row of Where alerts go for `channel`. */
const rowOf = (page, channel) => card(page, "channels").locator("li.al-item").filter({ hasText: nameOf(channel) });

/** The active alerts are the lines of `alerts list`: a row for each with its title, or "No alerts are active.". */
async function activeAsAnswered(t, page) {
  await eventually(async () => {
    const lines = (await listOf(page, ROUTES.list)).filter((line) => line?.alert);
    const active = card(page, "active");
    if (lines.length === 0) {
      await t.see(active.getByText(NOTHING_ACTIVE, { exact: true }));
      return;
    }
    const rows = active.locator("li.al-item");
    ensure((await rows.count()) === lines.length, `the Active card has ${await rows.count()} rows and alerts list has ${lines.length} lines`);
    for (const line of lines) {
      ensure((await rows.filter({ hasText: line.alert.title }).count()) === 1, `the Active card has no row for "${line.alert.title}"`);
    }
  });
}

/** Recent is the newest lines of `alerts history`, at most 20, or "No alert has been raised or cleared.". */
async function recentAsAnswered(t, page) {
  await eventually(async () => {
    const kept = (await listOf(page, ROUTES.history)).filter((line) => Number.isFinite(line?.at) && line.alert).length;
    const recent = card(page, "recent");
    if (kept === 0) {
      await t.see(recent.getByText(NOTHING_RECENT, { exact: true }));
      return;
    }
    const rows = await recent.locator("li.al-item").count();
    ensure(rows === Math.min(20, kept), `the Recent card has ${rows} rows and the history has ${kept} lines`);
  });
}

/** Where alerts go is `alerts status`: the line of the screens first, and a row for each channel with its name, what it is told of and how it last did. */
async function channelsAsAnswered(t, page) {
  await eventually(async () => {
    const channels = await channelsOf(page);
    const where = card(page, "channels");
    await t.see(where.getByText(SCREENS_LINE, { exact: true }));
    if (channels.length === 0) {
      await t.see(where.getByText(SCREENS_ONLY, { exact: true }));
      return;
    }
    ensure((await where.locator("ul.al-list > li.al-item").count()) === channels.length + 1, `Where alerts go has ${await where.locator("ul.al-list > li.al-item").count()} rows, and alerts status has ${channels.length} channels and the screens`);
    for (const channel of channels) {
      const row = rowOf(page, channel);
      ensure((await row.count()) === 1, `Where alerts go has ${await row.count()} rows for ${nameOf(channel)}`);
      await t.see(row.getByText(toldWords(channel), { exact: true }));
      const last = Number.isFinite(channel.last_ok) ? "Last sent " : Array.isArray(channel.last_error) ? "Last failed " : "Nothing has been sent yet.";
      ensure((await row.innerText()).includes(last), `the row of ${nameOf(channel)} does not say "${last.trim()}"`);
    }
  });
}

// --- A locked console, and the unlock ---

/** A locked administrator reads nothing: the page says to unlock, asks nothing of the machine, and the routes refuse (ADM-106). The Unlock button opens the dialog, and the cards come when it is answered. */
async function whileLocked(t, page) {
  const asked = [];
  const note = (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith("/api/1/alerts")) asked.push(`${request.method()} ${pathname}`);
  };
  page.on("request", note);
  await openAlerts(t, page);
  await t.see(page.locator("main .al").getByText(LOCKED_LINE, { exact: true }));
  await sleep(500);
  page.off("request", note);
  ensure(asked.length === 0, `the page asked for ${asked.join(", ")} before the console was unlocked`);
  ensure((await page.locator(".al-grid").isHidden()), "the cards of a locked console are shown");
  mustBe(await askFor(page, ROUTES.list), 403, "ADM-106", "a locked administrator's alerts");
  mustBe(await askFor(page, ROUTES.history), 403, "ADM-106", "a locked administrator's history of alerts");
  mustBe(await askFor(page, ROUTES.channels), 403, "ADM-106", "a locked administrator's channels");
  await lookAll(t, page, "alerts-locked");

  await buttonIn(page.locator("main .al"), "Unlock").click();
  await answerUnlock(t, page);
  await t.see(card(page, "active"));
  await t.see(card(page, "channels"));
}

// --- Nothing set up ---

/** From no channel: the cards, held to what the machine says, with the two alerts that alerts.toml raised, and a test alert that says there is no channel to send it to. */
async function nothingSetUp(t, page) {
  await openAlerts(t, page);
  for (const [id, title] of [["active", "Active"], ["browser", "This browser"], ["channels", "Where alerts go"], ["email", "Email"], ["recent", "Recent"]]) {
    // A card's buttons sit in its heading, as on the console's other pages, so the heading's name begins with the title.
    await t.see(card(page, id).getByRole("heading", { name: new RegExp(`^${title}( |$)`), level: 2 }));
  }
  ensure((await channelsOf(page)).length === 0, "the machine has a channel before the test set one up");
  await activeAsAnswered(t, page);
  const active = card(page, "active");
  await t.see(active.getByText("Lab alert for the Alerts page", { exact: false }));
  await t.see(active.getByText("Lab temperature alert", { exact: false }));
  // The critical alert comes before the warning, with the way to the page it is about.
  // Each item begins with its level's word, then its title.
  const items = (await active.locator("li.al-item").allInnerTexts()).map((text) => text.replace(/\s+/g, " "));
  const critical = items.findIndex((text) => text.includes("Lab temperature alert"));
  const warning = items.findIndex((text) => text.includes("Lab alert for the Alerts page"));
  ensure(critical >= 0 && warning >= 0 && critical < warning, `the alerts are listed ${items.join(" | ")}: the critical one is not before the warning`);
  const hardware = active.locator("li.al-item").filter({ hasText: "Lab temperature alert" }).getByRole("link", { name: "Open Hardware", exact: true });
  await t.see(hardware);
  ensure((await hardware.getAttribute("href")) === "/hardware", `Open Hardware goes to ${await hardware.getAttribute("href")}`);
  await recentAsAnswered(t, page);
  // Engines run one after another on one machine and the history keeps the raises of the one before, so more than one row names the alert.
  await t.see(card(page, "recent").getByText("Lab temperature alert", { exact: false }).first());
  await t.see(card(page, "recent").getByRole("link", { name: "Show all on Reliability", exact: true }));
  await channelsAsAnswered(t, page);
  await lookAll(t, page, "alerts-none");

  // Refresh asks again.
  const again = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.list));
  await buttonIn(active, "Refresh").click();
  await again;

  // With no channel a test alert has nowhere to go, and the page says so.
  await pressFor(t, page, headTest(page), pathOf(ROUTES.test));
  await t.see(card(page, "channels").getByText(NO_CHANNEL_TEST, { exact: true }));
}

// --- Email ---

/** The Email card: each wrong field is said where it is typed and nothing is sent; with a server of the lab PC's at a port that nobody listens at, and a password, Save sets email, and the password is nowhere. */
async function emailSet(t, page, hooks, said) {
  const email = card(page, "email");
  await t.see(email.getByText("Send alerts to an email address through your mail provider.", { exact: true }));
  const field = (label) => email.getByLabel(label, { exact: true });
  const save = buttonIn(email, "Save");
  for (const label of ["Send to", "Mail server", "Port", "Sign in as", "Password", "From (optional)"]) await t.see(field(label));
  for (const label of KINDS) await t.see(email.getByRole("checkbox", { name: new RegExp(`^${label}`) }));

  ensure(await sendsNoPost(page, save, pathOf(ROUTES.email)), "Save with no address sent the email's settings");
  await t.see(email.getByText(NO_ADDRESS, { exact: true }));
  await field("Send to").fill("nobody");
  ensure(await sendsNoPost(page, save, pathOf(ROUTES.email)), "Save with a word that is no address sent the email's settings");
  await t.see(email.getByText(NOT_AN_ADDRESS, { exact: true }));
  await field("Send to").fill(TO);
  ensure(await sendsNoPost(page, save, pathOf(ROUTES.email)), "Save with no mail server sent the email's settings");
  await t.see(email.getByText(NO_SERVER, { exact: true }));
  await field("Mail server").fill(hooks.peer);
  await field("Port").selectOption(MAIL_PORT);
  await field("Sign in as").fill(MAIL_USER);
  ensure(await sendsNoPost(page, save, pathOf(ROUTES.email)), "Save with a user name and no password sent the email's settings");
  await t.see(email.getByText(NO_PASSWORD, { exact: true }));

  // The password is typed, and the command that the page shows has none of it.
  await field("Password").fill(MAIL_PASSWORD);
  await t.see(email.getByRole("heading", { name: "This runs", level: 3 }));
  await noneOf(page, said, [MAIL_PASSWORD], "with the password typed");
  await lookAll(t, page, "alerts-email-typed");

  const sent = requestTo(page, pathOf(ROUTES.email));
  await pressFor(t, page, save, pathOf(ROUTES.email));
  const request = await sent;
  ensure(request.body.includes(MAIL_PASSWORD), "the call that sets email does not carry the password in its body");
  ensure(!request.url.includes(MAIL_PASSWORD), "the password is in the address of the call that sets email");
  await t.see(email.getByText("Saved.", { exact: false }));
  await t.see(email.getByText(/^Email is set up\./));
  ensure((await field("Password").inputValue()) === "", "the password field still holds the password after Save");
  await noneOf(page, said, [MAIL_PASSWORD], "after Save");

  // Where alerts go lists email, told of every kind, and the machine says the same.
  const channels = await eventually(async () => {
    const list = await channelsOf(page);
    ensure(list.some((one) => one.kind === "email"), "alerts status lists no email channel after Save");
    return list;
  });
  ensure(isText(channels.find((one) => one.kind === "email").channel), "the email channel has no name in alerts status");
  await channelsAsAnswered(t, page);
}

// --- A webhook ---

/** Add a webhook…: a wrong address is said where it is typed; with the kind that takes any address that takes a POST, the receiver's address and a token, the webhook is added, and neither is on the page. */
async function webhookAdded(t, page, hooks, said) {
  const address = `https://${hooks.peer}:${hooks.port}/json/lab`;
  await buttonIn(card(page, "channels"), "Add a webhook…").click();
  const dialog = page.getByRole("dialog", { name: "Add a webhook", exact: true });
  await t.see(dialog);
  await dialog.getByLabel("Kind", { exact: true }).selectOption("json");
  const add = buttonIn(dialog, "Add the webhook");
  ensure(await sendsNoPost(page, add, pathOf(ROUTES.webhook)), "Add the webhook with no address sent a webhook");
  await t.see(dialog.getByText("Type the address.", { exact: true }));
  await dialog.getByLabel("Address", { exact: true }).fill("not an address");
  ensure(await sendsNoPost(page, add, pathOf(ROUTES.webhook)), "Add the webhook with a word that is no address sent a webhook");
  await t.see(dialog.getByText(NO_WEB_ADDRESS, { exact: true }));
  await dialog.getByLabel("Address", { exact: true }).fill(address);
  await dialog.getByLabel("Token", { exact: true }).fill(HOOK_TOKEN);
  await noneOf(page, said, [address, HOOK_TOKEN], "with the webhook's address and token typed");
  await lookAll(t, page, "alerts-webhook");

  const sent = requestTo(page, pathOf(ROUTES.webhook));
  await pressFor(t, page, add, pathOf(ROUTES.webhook));
  const request = await sent;
  ensure(request.body.includes(address) && request.body.includes(HOOK_TOKEN), "the call that adds the webhook does not carry the address and the token in its body");
  ensure(!request.url.includes(HOOK_TOKEN) && !request.url.includes("/json/lab"), "the webhook's address or token is in the address of its call");
  await t.gone(dialog);
  await noneOf(page, said, [address, HOOK_TOKEN], "after the webhook was added");

  const hook = await eventually(async () => {
    const found = (await channelsOf(page)).find((one) => one.kind !== "email" && one.kind !== "push");
    ensure(found !== undefined, "alerts status lists no webhook after Add the webhook");
    return found;
  });
  ensure(Number.isInteger(hook.number), `the webhook's line of alerts status has no number: ${JSON.stringify(hook)}`);
  await channelsAsAnswered(t, page);
  return hook;
}

// --- A test alert ---

/** The deliveries that the answer to Send a test alert holds, as the page reads them. */
function deliveriesIn(body) {
  const found = outputJson(body, "deliveries");
  ensure(found !== null && Array.isArray(found.deliveries), `the answer to the test alert has no line with the deliveries: ${JSON.stringify(body)}`);
  return found.deliveries;
}

/** A test alert: a line for each channel, the webhook's sent and email's failed with the server's words; the receiver took the test alert, with the token; the rows say how each last did. */
async function testSent(t, page, hooks) {
  const before = (await hookLines(hooks)).length;
  const answer = await pressFor(t, page, headTest(page), pathOf(ROUTES.test));
  const deliveries = deliveriesIn(await answer.json());
  const mail = deliveries.find((one) => /^email/.test(String(one.channel)));
  const hook = deliveries.find((one) => /^json/.test(String(one.channel)));
  ensure(deliveries.length === 2 && mail !== undefined && hook !== undefined, `the test alert went to ${deliveries.map((one) => one.channel).join(", ")}, not to email and json`);
  ensure(hook.ok === true, `the webhook did not take the test alert: ${JSON.stringify(hook)}`);
  ensure(mail.ok === false && isText(mail.error), `email, whose server nobody listens at, did not fail with words: ${JSON.stringify(mail)}`);

  const lines = card(page, "channels").locator("ul.al-test li");
  await eventually(async () => {
    ensure((await lines.count()) === deliveries.length, `the page shows ${await lines.count()} lines of the test, and the machine sent it to ${deliveries.length}`);
  });
  for (const one of deliveries) {
    const line = lines.filter({ hasText: `${one.channel}: ${one.ok ? "sent" : "failed"}` });
    ensure((await line.count()) === 1, `the page has no line "${one.channel}: ${one.ok ? "sent" : "failed"}"`);
    ensure((await line.getAttribute("class")) === (one.ok ? "al-ok" : "al-bad"), `the line of ${one.channel} is not marked ${one.ok ? "sent" : "failed"}`);
    if (!one.ok) ensure((await line.innerText()).includes(one.error), `the line of ${one.channel} does not say why: ${one.error}`);
  }

  const took = await hookTook(hooks, before, ["hook json POST /json/lab auth=bearer", "Test alert from alertd"]);
  ensure(took.length >= 1, "the receiver took no test alert");

  // The rows say how each did, and a channel that failed has a test of its own.
  await eventually(async () => {
    const channels = await channelsOf(page);
    ensure(Number.isFinite(channels.find((one) => one.kind !== "email")?.last_ok), "alerts status says the webhook sent nothing");
    ensure(Array.isArray(channels.find((one) => one.kind === "email")?.last_error), "alerts status says email did not fail");
  }, 30);
  await channelsAsAnswered(t, page);
  const email = card(page, "channels").locator("li.al-item").filter({ hasText: /^email/ });
  await t.see(email.getByText(/^Last failed /));
  await recentAsAnswered(t, page);
  await lookAll(t, page, "alerts-tested");

  const again = (await hookLines(hooks)).length;
  await pressFor(t, page, buttonIn(email, "Send a test alert"), pathOf(ROUTES.test));
  await hookTook(hooks, again, ["hook json POST /json/lab auth=bearer", "Test alert from alertd"]);
}

// --- What it is told of ---

/** Edit what it is told: no kind is refused where it is chosen; with Hardware left out of both channels the page says that Hardware alerts reach no channel, with the way to put it back, and puts it back. */
async function toldOf(t, page, hook) {
  const where = card(page, "channels");
  const edit = async (row, change) => {
    await buttonIn(row, "Edit what it is told").click();
    const dialog = page.getByRole("dialog", { name: "What it is told of", exact: true });
    await t.see(dialog);
    await change(dialog);
    return dialog;
  };
  const box = (dialog, label) => dialog.getByRole("checkbox", { name: new RegExp(`^${label}`) });
  const hookRow = () => where.locator("li.al-item").filter({ hasText: /^json/ });
  const mailRow = () => where.locator("li.al-item").filter({ hasText: /^email/ });

  // None chosen is refused, and the dialog is cancelled.
  let dialog = await edit(hookRow(), async (one) => {
    for (const label of KINDS) await box(one, label).uncheck();
    ensure(await sendsNoPost(page, buttonIn(one, "Save"), pathOf(ROUTES.told)), "Save with no kind chosen sent a change");
    await t.see(one.getByText(NO_KIND, { exact: true }));
  });
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);

  // Hardware left out of the webhook: what the machine keeps, and the row.
  const saveTold = async () => {
    const sent = requestTo(page, pathOf(ROUTES.told));
    await pressFor(t, page, buttonIn(page.getByRole("dialog", { name: "What it is told of", exact: true }), "Save"), pathOf(ROUTES.told));
    return JSON.parse((await sent).body);
  };
  await edit(hookRow(), async (one) => {
    await box(one, "Hardware").uncheck();
  });
  const body = await saveTold();
  ensure(body.channel === `webhook:${hook.number}` && body.about === "data,stopped", `the change was sent as ${JSON.stringify(body)}, not for webhook:${hook.number} and data,stopped`);
  await eventually(async () => {
    const found = (await channelsOf(page)).find((one) => one.number === hook.number);
    ensure(Array.isArray(found?.about) && found.about.join(",") === "data,stopped", `alerts status says the webhook is told of ${JSON.stringify(found?.about)}`);
  });
  await channelsAsAnswered(t, page);
  ensure((await where.locator(".banner.warn:not([hidden])").count()) === 0, "the page warns of uncovered kinds while email is told of every kind");

  // And of email: the page now says that Hardware reaches no channel.
  await edit(mailRow(), async (one) => {
    await box(one, "Hardware").uncheck();
  });
  const mailBody = await saveTold();
  ensure(mailBody.channel === "email" && mailBody.about === "data,stopped", `the change was sent as ${JSON.stringify(mailBody)}, not for email and data,stopped`);
  const warning = where.locator(".banner.warn:not([hidden])");
  await t.see(warning);
  await t.see(warning.getByText(HARDWARE_LEFT, { exact: true }));
  await t.see(buttonIn(warning, "Edit what email is told"));
  await channelsAsAnswered(t, page);
  await lookAll(t, page, "alerts-uncovered");

  // The warning's button puts Hardware back for email, and the warning is gone.
  await buttonIn(warning, "Edit what email is told").click();
  dialog = page.getByRole("dialog", { name: "What it is told of", exact: true });
  await t.see(dialog);
  ensure(!(await box(dialog, "Hardware").isChecked()), "the dialog shows Hardware chosen for email, which was left out");
  await box(dialog, "Hardware").check();
  const restored = await saveTold();
  ensure(restored.channel === "email" && restored.about === "all", `the change was sent as ${JSON.stringify(restored)}, not for email and all`);
  await t.gone(where.locator(".banner.warn:not([hidden])"));
}

// --- Removing ---

/** Remove asks first; with it the webhook, and email, are gone from the machine and from the page. */
async function removed(t, page, hook) {
  const where = card(page, "channels");
  const row = where.locator("li.al-item").filter({ hasText: /^json/ });
  await buttonIn(row, "Remove").click();
  let dialog = page.getByRole("dialog", { name: /^Remove json/ });
  await t.see(dialog);
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);
  ensure((await channelsOf(page)).some((one) => one.number === hook.number), "Cancel removed the webhook");

  await buttonIn(row, "Remove").click();
  dialog = page.getByRole("dialog", { name: /^Remove json/ });
  await t.see(dialog);
  await pressFor(t, page, buttonIn(dialog, "Remove"), pathOf(routeFor(ROUTES.webhookRemove, hook.number)));
  await t.gone(dialog);
  await eventually(async () => {
    ensure(!(await channelsOf(page)).some((one) => one.kind !== "email" && one.kind !== "push"), "alerts status still lists the webhook");
  });

  const email = card(page, "email");
  await buttonIn(email, "Remove email").click();
  dialog = page.getByRole("dialog", { name: /^Remove email/ });
  await t.see(dialog);
  await pressFor(t, page, buttonIn(dialog, "Remove"), pathOf(ROUTES.emailRemove));
  await t.gone(dialog);
  await eventually(async () => {
    ensure(!(await channelsOf(page)).some((one) => one.kind === "email"), "alerts status still lists email");
  });
  await channelsAsAnswered(t, page);
  await t.see(email.getByText("Send alerts to an email address through your mail provider.", { exact: true }));
}

// --- This browser ---

/** What the card of This browser says, which of `OUTCOMES` it is, waited for up to a minute and a half. */
async function outcomeOf(page) {
  const handle = await page.waitForFunction(
    (cases) => {
      const text = document.querySelector("#browser")?.innerText ?? "";
      return Object.entries(cases).find(([, words]) => text.includes(words))?.[0] ?? false;
    },
    OUTCOMES,
    { timeout: 90_000 },
  );
  return handle.jsonValue();
}

/**
 * Get alerts in this browser, as far as the browser goes: the privacy note; a
 * name that is empty is refused; and then the browser's own answer, said in
 * words. Nothing is added, and no subscription is sent to the machine. Gives
 * the answer, one of `OUTCOMES`' names, and whether the browser has push.
 */
async function getAlertsHere(t, page, seen) {
  const here = card(page, "browser");
  const hasPush = await page.evaluate(() => "serviceWorker" in navigator && "PushManager" in window);
  const get = buttonIn(here, "Get alerts in this browser");
  if (!hasPush) {
    await t.see(here.getByText(UNSUPPORTED, { exact: false }));
    ensure((await get.count()) === 0, "a browser that has no push is offered Get alerts in this browser");
    await lookAll(t, page, "alerts-browser-unsupported");
    return { outcome: "unsupported", hasPush };
  }
  await t.see(get);
  await t.see(here.getByText(PRIVACY));
  const name = here.getByLabel("Name", { exact: true });
  ensure(isText(await name.inputValue()), "the browser is offered no name for itself");
  await name.fill("");
  ensure(await sendsNoPost(page, get, pathOf(ROUTES.push)), "Get alerts in this browser with no name sent a subscription");
  await t.see(here.getByText(NO_NAME, { exact: true }));
  await name.fill("Lab browser");

  const posted = [];
  const watch = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.push)) posted.push(request.url());
  };
  page.on("request", watch);
  await get.click();
  const outcome = await outcomeOf(page);
  page.off("request", watch);
  if (outcome === "got") {
    // A lab that reaches its browser's push service gets a real subscription: it is listed as this browser, and turned off here.
    console.log(`alerts: ${t.engine} subscribed to its push service, so the page's own subscription was listed and is turned off`);
    ensure(posted.length === 1, `the page sent ${posted.length} subscriptions, not one`);
    const id = JSON.parse(await keptId(page));
    await t.see(card(page, "channels").locator("li.al-item").filter({ hasText: "Lab browser" }).getByText("This browser", { exact: true }));
    await pressFor(t, page, buttonIn(here, "Turn off in this browser"), pathOf(routeFor(ROUTES.pushRemove, id)));
    await eventually(async () => {
      ensure(!(await channelsOf(page)).some((one) => one.kind === "push"), "alerts status still lists the browser after Turn off in this browser");
    });
    return { outcome, hasPush };
  }
  ensure(posted.length === 0, `the page sent a subscription to ${posted.join(", ")} though the browser did not subscribe`);

  if (outcome === "certificate") {
    const banner = here.locator(".banner.warn");
    await t.see(banner.getByText(UNTRUSTED, { exact: true }));
    const letsEncrypt = banner.getByRole("link", { name: "Give the console a certificate from Let's Encrypt", exact: true });
    await t.see(letsEncrypt);
    ensure((await letsEncrypt.getAttribute("href")) === "/certificates", "the way to a certificate from Let's Encrypt does not go to /certificates");
    await t.see(banner.getByText("Trust this server's own certificate", { exact: true }));
    await t.see(banner.getByText("Until then, email and webhooks such as ntfy reach your phone.", { exact: true }));
  } else {
    await t.see(here.locator(".al-errors .msg").getByText(OUTCOMES[outcome], { exact: false }));
  }
  if (outcome === "subscribe") ensure(seen.has(ROUTES.pushKey), "the browser tried to subscribe and the page never read the machine's key");
  ensure(!(await channelsOf(page)).some((one) => one.kind === "push"), "the machine lists a browser that never subscribed");
  await lookAll(t, page, `alerts-browser-${outcome}`);
  return { outcome, hasPush };
}

/** A subscription as a browser gives it, `PushSubscription.toJSON()`, of an address of a push service that this lab cannot reach, with a P-256 key and an auth of its own. */
function subscriptionOfOwn() {
  const key = createECDH("prime256v1");
  key.generateKeys();
  const base64url = (bytes) => Buffer.from(bytes).toString("base64url");
  return JSON.stringify({ endpoint: PUSH_ENDPOINT, keys: { p256dh: base64url(key.getPublicKey()), auth: base64url(randomBytes(16)) } });
}

/** The machine's key is a point of P-256 as a browser takes it: base64url of 65 bytes, the first one 4. */
async function machineKey(page) {
  const key = await readOf(page, ROUTES.pushKey);
  const text = key?.public_key;
  ensure(typeof text === "string" && /^[A-Za-z0-9_-]{87}=?$/.test(text), `${ROUTES.pushKey} gives no key a browser can take: ${JSON.stringify(key)}`);
  const bytes = Buffer.from(text.replace(/=$/, ""), "base64url");
  ensure(bytes.length === 65 && bytes[0] === 4, `${ROUTES.pushKey} gives ${bytes.length} bytes that start with ${bytes[0]}, not the 65 of a point that start with 4`);
}

/** The key that the page keeps this browser's ID under, as the page's store writes it. */
const ID_KEY = "dukovo-admin.alerts.push.id";

/** What the browser keeps of its ID: the text of its value, or null. */
const keptId = (page) => page.evaluate((key) => localStorage.getItem(key), ID_KEY);

/**
 * A subscription of the test's own is handed to the machine by the route the
 * page uses. The page keeps only the ID: with it kept, the card says this
 * browser gets alerts, the row is this browser's, and Turn off in this browser
 * (or Remove, for a browser that has no push) ends it and the page forgets the
 * ID. A note of an ID that the machine does not list is forgotten too.
 */
async function subscriptionListed(t, page, hasPush) {
  await machineKey(page);
  const added = await askFor(page, ROUTES.push, { subscription: subscriptionOfOwn(), name: STAND_IN, severity: "warning", cleared: true });
  ensure(added.status === 200, `${ROUTES.push} was answered ${added.status}: ${added.text}`);
  const id = outputJson(added.json, "id")?.id;
  ensure(/^[0-9a-f]{16}$/.test(id ?? ""), `${ROUTES.push} gave no ID of 16 hex digits in its output: ${added.text}`);
  ensure(!added.text.includes("send/lab-1"), "the answer of the subscription's call holds its address");

  await page.evaluate(([key, value]) => localStorage.setItem(key, JSON.stringify(value)), [ID_KEY, id]);
  await openAlerts(t, page);
  const where = card(page, "channels");
  const row = where.locator("li.al-item").filter({ hasText: STAND_IN });
  await t.see(row);
  await t.see(row.getByText("This browser", { exact: true }));
  await channelsAsAnswered(t, page);
  await noneOf(page, { urls: [], logs: [] }, [PUSH_ENDPOINT, "send/lab-1"], "with a subscription listed");

  if (hasPush) {
    const here = card(page, "browser");
    await t.see(here.getByText(GETS, { exact: true }));
    await t.see(buttonIn(here, "Send a test"));
    await lookAll(t, page, "alerts-browser-on");
    await pressFor(t, page, buttonIn(here, "Turn off in this browser"), pathOf(routeFor(ROUTES.pushRemove, id)));
    await t.see(buttonIn(here, "Get alerts in this browser"));
  } else {
    // A browser with no push can still remove what the machine lists.
    await buttonIn(row, "Remove").click();
    const dialog = page.getByRole("dialog", { name: /^Remove / });
    await t.see(dialog);
    await pressFor(t, page, buttonIn(dialog, "Remove"), pathOf(routeFor(ROUTES.pushRemove, id)));
    await t.gone(dialog);
  }
  await eventually(async () => {
    ensure(!(await channelsOf(page)).some((one) => one.kind === "push"), "alerts status still lists the subscription after it was turned off");
    ensure((await keptId(page)) === null, "the page still keeps the ID of a subscription that was turned off");
  });
  await t.gone(row);

  // An ID that the machine does not list is forgotten when the page reads the channels.
  await page.evaluate(([key, value]) => localStorage.setItem(key, JSON.stringify(value)), [ID_KEY, "0123456789abcdef"]);
  await openAlerts(t, page);
  await eventually(async () => {
    ensure((await keptId(page)) === null, "the page keeps the ID of a subscription that the machine does not list");
  }, 20);
}

// --- The console's files ---

/** What the page's own fetch of `path` gets: `{status, headers, size, head, text}`; a binary file has no text. */
const fileAsPage = (page, path) =>
  page.evaluate(async (route) => {
    const answer = await fetch(route, { credentials: "same-origin" });
    const bytes = new Uint8Array(await answer.arrayBuffer());
    const binary = (answer.headers.get("content-type") ?? "").startsWith("image/");
    return {
      status: answer.status,
      headers: Object.fromEntries(answer.headers),
      size: bytes.length,
      head: Array.from(bytes.slice(0, 32)),
      text: binary ? "" : new TextDecoder().decode(bytes),
    };
  }, path);

/** The worker, the manifest and its icons are served as what they are; the worker is never kept by the browser; the policy lets the page start a worker and read the manifest. */
async function filesServed(page) {
  const worker = await fileAsPage(page, pathOf(FILES.worker));
  ensure(worker.status === 200, `${FILES.worker} was answered ${worker.status}`);
  ensure(/javascript/.test(worker.headers["content-type"] ?? ""), `${FILES.worker} is served as ${worker.headers["content-type"]}, not as JavaScript`);
  ensure(/no-cache/.test(worker.headers["cache-control"] ?? ""), `${FILES.worker} has Cache-Control "${worker.headers["cache-control"]}", not no-cache: a browser would keep an old worker`);
  ensure(worker.headers["service-worker-allowed"] === undefined, `${FILES.worker} widens the worker's scope with Service-Worker-Allowed: ${worker.headers["service-worker-allowed"]}`);
  ensure(/addEventListener\(\s*['"]push['"]/.test(worker.text) && /addEventListener\(\s*['"]notificationclick['"]/.test(worker.text), `${FILES.worker} does not handle push and notificationclick`);
  ensure(!/addEventListener\(\s*['"]fetch['"]/.test(worker.text), `${FILES.worker} handles fetch: the console keeps no cache`);

  const manifest = await fileAsPage(page, pathOf(FILES.manifest));
  ensure(manifest.status === 200, `${FILES.manifest} was answered ${manifest.status}`);
  ensure(/application\/manifest\+json/.test(manifest.headers["content-type"] ?? ""), `${FILES.manifest} is served as ${manifest.headers["content-type"]}, not as application/manifest+json`);
  const app = JSON.parse(manifest.text);
  ensure(app.name === "Dukovo Admin" && app.start_url === "/" && app.display === "standalone", `the manifest names ${JSON.stringify([app.name, app.start_url, app.display])}`);

  for (const [route, size] of [[FILES.small, 192], [FILES.large, 512]]) {
    const icon = await fileAsPage(page, pathOf(route));
    ensure(icon.status === 200 && icon.headers["content-type"] === "image/png", `${route} was answered ${icon.status} as ${icon.headers["content-type"]}`);
    ensure(icon.head.slice(0, 8).join(",") === "137,80,78,71,13,10,26,10", `${route} is not a PNG`);
    // The picture's width and height are the 4 bytes each after the name of its header's chunk.
    const read = (at) => ((icon.head[at] << 24) | (icon.head[at + 1] << 16) | (icon.head[at + 2] << 8) | icon.head[at + 3]) >>> 0;
    ensure(read(16) === size && read(20) === size, `${route} is ${read(16)} by ${read(20)} px, not ${size} by ${size}`);
    ensure(app.icons.some((one) => one.src === pathOf(route) && one.sizes === `${size}x${size}`), `the manifest does not list ${route} at ${size}x${size}`);
  }

  const home = await page.request.get(`${originOf(page)}/`);
  const meta = /<meta[^>]+http-equiv=["']Content-Security-Policy["'][^>]*content=["']([^"']+)["']/i.exec(await home.text())?.[1] ?? "";
  const policy = `${home.headers()["content-security-policy"] ?? ""};${meta}`;
  ensure(policy.includes("worker-src 'self'"), `the console's content security policy has no worker-src 'self': ${policy}`);
  ensure(policy.includes("manifest-src 'self'"), `the console's content security policy has no manifest-src 'self': ${policy}`);
}

/** Chromium's worker, where the browser started it: a push shows a notice with the host and the title, and the page its key is about; pressing the notice closes it. */
async function theWorker(page) {
  const context = page.context();
  let worker = context.serviceWorkers().find((one) => new URL(one.url()).pathname === "/sw.js");
  worker ??= await context.waitForEvent("serviceworker", { timeout: 3000 }).catch(() => undefined);
  if (worker === undefined) {
    console.log("alerts: this Chromium started no worker for /sw.js, so its push and notificationclick are the Node tests' (sw.test.mjs)");
    return;
  }
  const tag = "hardwarememory-test";
  const message = {
    v: 1,
    host: "lab-host",
    count: 1,
    first: { state: "raised", severity: "warning", title: "Memory test found a fault", detail: "Run the memory test again.", source: "hardware", key: "memory-test", at_ms: Date.now() },
  };
  const notices = () => worker.evaluate(async (wanted) => (await self.registration.getNotifications({ tag: wanted })).map((one) => ({ title: one.title, body: one.body, tag: one.tag, data: one.data })), tag);
  await worker.evaluate((json) => { self.dispatchEvent(new PushEvent("push", { data: json })); }, JSON.stringify(message));
  const shown = await eventually(async () => {
    const found = await notices();
    ensure(found.length === 1, `the worker shows ${found.length} notices for the push, not one`);
    return found[0];
  }, 20);
  ensure(shown.title === "lab-host · Memory test found a fault", `the notice is titled "${shown.title}"`);
  ensure(shown.body === "Run the memory test again.", `the notice says "${shown.body}"`);
  ensure(shown.data?.path === "/hardware", `the notice opens ${shown.data?.path}, not /hardware`);

  await worker.evaluate(async (wanted) => {
    const [notification] = await self.registration.getNotifications({ tag: wanted });
    self.dispatchEvent(new NotificationEvent("notificationclick", { notification }));
  }, tag);
  await eventually(async () => {
    ensure((await notices()).length === 0, "pressing the notice did not close it");
  }, 20);
}

// --- Set up a phone, on a computer ---

/** The card of steps on a computer: the QR code is the console's own encoder's of the address it shows, the address is the console's at /alerts?phone=1, and the fingerprint is the certificate the console serves. */
async function phoneCard(t, page) {
  const about = await readOf(page, ROUTES.about);
  const phone = card(page, "phone");
  await t.see(phone.getByRole("heading", { name: "Set up a phone", level: 2, exact: true }));
  await t.see(phone.getByRole("img", { name: "QR code of this console's address", exact: true }));
  const steps = phone.locator("ol.al-steps > li");
  await t.see(steps.nth(4));
  ensure((await steps.count()) === 5, `the card has ${await steps.count()} steps, not five`);
  await t.see(steps.nth(0).getByText("Scan this with the phone's camera.", { exact: false }));
  await t.see(steps.nth(1).getByText("Check the certificate.", { exact: false }));
  await t.see(steps.nth(2).getByText("Sign in on the phone with your account.", { exact: false }));

  const address = (await phone.locator("p.cmdtext").first().innerText()).trim();
  const found = /^https:\/\/(\[[0-9a-f:]+\]|[^/:]+)(?::(\d+))?\/alerts\?phone=1$/i.exec(address);
  ensure(found !== null, `the card shows ${JSON.stringify(address)}, not https://ADDRESS:PORT/alerts?phone=1`);
  if (Number.isInteger(about.port)) ensure(found[2] === String(about.port), `the address has port ${found[2]} and the console's is ${about.port}`);
  const host = found[1].replace(/^\[|\]$/g, "");
  ensure((about.addresses ?? []).includes(host) || host === new URL(page.url()).hostname, `the address names ${host}, which is none of the console's: ${(about.addresses ?? []).join(", ")}`);

  // The picture is the encoder's own, of the address as written.
  const shown = await phone.locator(".qr-row svg path").first().getAttribute("d");
  const made = await page.evaluate(async (text) => {
    const { qrSvg } = await import("/qr.js");
    return qrSvg(text, "check").querySelector("path").getAttribute("d");
  }, address);
  ensure(shown === made, "the picture on the card is not the QR code of the address it shows");

  // The fingerprint is the certificate the console serves, in four groups of eight bytes.
  const groups = (await phone.locator("p.cmdtext").nth(1).innerText()).trim().split(/\s+/);
  const served = (await servedCertificate(originOf(page))).fingerprint256.replaceAll(":", "").toUpperCase();
  ensure(groups.join(":").replaceAll(":", "").toUpperCase() === served, `the card shows the fingerprint ${groups.join(" ")}, and the console's certificate has ${served}`);
  ensure(groups.length === 4 && groups.every((group) => /^([0-9A-F]{2}:){7}[0-9A-F]{2}$/.test(group)), `the fingerprint is not in four groups of eight bytes: ${groups.join(" ")}`);

  // Without a certificate that a phone trusts, the fourth step gives the two ways instead of the page's own steps.
  const fourth = await steps.nth(3).innerText();
  ensure(fourth.includes("Turn on alerts on the phone") || fourth.includes("Push notices need a certificate the phone trusts"), `the fourth step says "${fourth.split("\n")[0]}"`);
  await t.see(steps.nth(4).getByText("Add it to the home screen from the browser's menu", { exact: false }));
  await lookAll(t, page, "alerts-phone-card");
}

// --- The administrator ---

async function administrator(t) {
  const hooks = await hooksOf();
  const { page, seen } = await signedIn(t, ADMIN, permissionsOf(t.engine));
  const said = watching(page);
  await whileLocked(t, page);
  await nothingSetUp(t, page);
  await phoneCard(t, page);
  await emailSet(t, page, hooks, said);
  const hook = await webhookAdded(t, page, hooks, said);
  await testSent(t, page, hooks);
  await toldOf(t, page, hook);
  await removed(t, page, hook);
  await noneOf(page, said, [MAIL_PASSWORD, HOOK_TOKEN, `https://${hooks.peer}:${hooks.port}/json/lab`], "at the end");

  const here = await getAlertsHere(t, page, seen);
  await subscriptionListed(t, page, here.hasPush);
  await filesServed(page);
  if (t.engine === "chromium") await theWorker(page);

  mustHaveCalled(seen, [...Object.values(ROUTES).filter((route) => !/\{/.test(route)), ...Object.values(FILES)]);
  const calledWith = (template, value) => ensure(seen.has(routeFor(template, value)), `the test never called ${routeFor(template, value)}`);
  calledWith(ROUTES.webhookRemove, hook.number);
  ensure([...seen].some((call) => /^POST \/api\/1\/alerts\/push\/[0-9a-f]{16}\/remove$/.test(call)), `the test never called ${ROUTES.pushRemove}`);
  await signOut(t, page);
  await page.context().close();
}

// --- A reader ---

/** A reader is told that alerts are for administrators, has none of the page's buttons, and is refused by every route. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await page.goto(new URL("/alerts", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Alerts", exact: true }));
  await t.see(page.getByText(READER_LINE, { exact: true }));
  const link = page.getByRole("link", { name: "Who are the administrators?", exact: true });
  await t.see(link);
  ensure((await link.getAttribute("href")) === "/accounts", "the reader's way to the administrators does not go to /accounts");
  for (const name of ["Send a test alert", "Add a webhook…", "Get alerts in this browser", "Save"]) {
    ensure((await buttonIn(page.locator("main"), name).count()) === 0, `a reader's Alerts page has ${name}`);
  }
  ensure((await page.locator("main section.card").count()) === 0, "a reader's Alerts page has cards");
  await lookAll(t, page, "alerts-reader");

  const refused = async (route, body = null) => mustBe(await askFor(page, route, body), 403, "ADM-122", `a reader's ${route}`);
  for (const route of [ROUTES.list, ROUTES.history, ROUTES.channels, ROUTES.pushKey]) await refused(route);
  // Arguments are checked before the person (ADR 0515), so each change is
  // asked with arguments it takes, and only the person is left to refuse it.
  const subscription = JSON.stringify({
    endpoint: "https://push.example.net/send/lab",
    keys: { p256dh: "BLabLabLabLabLabLabLabLabLabLabLabLabLab", auth: "LabLabLabLabLabLab" },
  });
  const changes = [
    [ROUTES.test, {}],
    [ROUTES.email, { to: "ops@example.org", server: "mail.example.org", user: "alerts@example.org", password: "lab-password" }],
    [ROUTES.emailRemove, {}],
    [ROUTES.webhook, { kind: "json", url: "http://10.0.2.2:8080/alerts" }],
    [ROUTES.told, { channel: "email", about: "data,stopped" }],
    [ROUTES.push, { subscription, name: "Lab browser" }],
    [routeFor(ROUTES.webhookRemove, 1), {}],
    [routeFor(ROUTES.pushRemove, "0123456789abcdef"), {}],
  ];
  for (const [route, body] of changes) await refused(route, body);
  await signOut(t, page);
  await page.context().close();
}

// --- A phone ---

/** At 390 wide: Alerts is the fourth tab, the page is one column with no sideways scroll and targets of 44 px, the card of Set up a phone is not there, and the panel is, from the code's address, until Not now. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE, ...permissionsOf(t.engine) });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const tabs = await page.locator("nav.tabbar a").evaluateAll((links) => links.map((one) => one.textContent.trim()));
  ensure(tabs.join("|") === "Overview|Services|Logs|Alerts", `the phone's tab bar holds ${tabs.join(", ")}, not Overview, Services, Logs and Alerts`);
  await page.locator("nav.tabbar").getByRole("link", { name: "Alerts", exact: true }).click();
  await t.see(page.getByRole("heading", { level: 1, name: "Alerts", exact: true }));
  await page.waitForFunction(() => (document.querySelector("#active .al-body")?.textContent ?? "").trim() !== "", null, { timeout: 30_000 });
  ensure((await page.locator("nav.tabbar a[aria-current=page]").innerText()).trim() === "Alerts", "the Alerts tab is not the one marked as this page");
  ensure((await page.locator("#phone").count()) === 0, "the phone is offered the card of Set up a phone");
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .ph button, main .al button, main .al a.btn, main .al-item a, main .al select, main .al summary, main .al .check, main .al input:not([type=checkbox]):not([type=radio])");
  ensure(small.length === 0, `these controls of the phone's Alerts page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "alerts-phone");

  // The code's address opens the panel of steps for the phone itself.
  await openAlerts(t, page, "?phone=1");
  const panel = card(page, "phone");
  await t.see(panel.getByRole("heading", { name: "Set up this phone", level: 2, exact: true }));
  await t.see(panel.getByText("Signed in", { exact: false }));
  await t.see(buttonIn(panel, "Get alerts on this phone"));
  const home = buttonIn(panel, "Add to the home screen");
  if ((await home.count()) === 0) await t.see(panel.getByText("Use the browser's menu: Add to the home screen.", { exact: true }));
  await t.see(buttonIn(panel, "Not now"));
  ensure((await sidewaysScroll(page)) === 0, `the phone's panel scrolls ${await sidewaysScroll(page)} px sideways`);
  const panelSmall = await smallControls(page, "#phone button, #phone a");
  ensure(panelSmall.length === 0, `these controls of the phone's panel are under 44 px tall: ${panelSmall.join(", ")}`);
  await lookAll(t, page, "alerts-phone-panel");

  await buttonIn(panel, "Not now").click();
  await t.gone(panel);
  await openAlerts(t, page, "?phone=1");
  ensure((await page.locator("#phone").count()) === 0, "the panel comes back after Not now");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The administrator's run comes first: it leaves no channel, as the reader and the phone expect.
  await administrator(t);
  await reader(t);
  await phone(t);
}
