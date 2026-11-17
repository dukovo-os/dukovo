// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/certificates drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the
// Certificates page at /certificates, and its deep link /certificates/{name}.
// The list is what `cert list --json` says (`GET /api/1/certificates`), each
// panel what `cert show` says (`GET /api/1/certificates/{name}`), and the
// Renewals card what `cert history` says (`GET /api/1/certificates/history/all`).
// The form that gets a certificate shows the checks of `cert get --check`
// (`GET /api/1/certificates/check/{names}`): with port 80 closed the check
// fails and offers Open port 80, a trial of the firewall that is undone or
// kept with its banner. The order itself (`POST /api/1/certificates/{name}/get`)
// is the one thing that is not made here: it orders from Let's Encrypt and
// from no other authority, so the lab's Pebble cannot be reached by it; an
// order that cannot finish is told, and the certificates Pebble made (one
// proved on port 80, one proved by DNS) are made by toml's steps, as
// services/acme.toml makes them, and then renewed, revoked and removed by the
// page (`POST /api/1/certificates/{name}/renew`, `POST
// /api/1/certificates/{name}/revoke`, `POST /api/1/certificates/{name}/remove`).
// The one proved by DNS cannot be renewed by certd: the page learns it from
// the refusal and offers Renew by DNS, which fills the form. An order that
// cannot finish is kept for certd to ask again every hour: the list has a row
// of its own kind for it (`GET /api/1/certificates/wanted/all`, what `cert
// wanted --json` prints), which says when certd tries next, how many tries
// there were and what the last said; a reader sees the row and has no Forget,
// and an administrator's Forget asks first and stops certd trying (`POST
// /api/1/certificates/{name}/forget`), which takes the row away. Upload your own
// (`POST /api/1/certificates/{name}/add`) sends the key and the chain once, as
// one secret, and the key is nowhere else: not the page, not a command shown,
// not a request but the one, not the browser's storage. Use for this console
// (`POST /api/1/console/certificate`) is refused for a certificate that does
// not name the address the browser comes by (ADM-141), is a trial for one that
// does, which the page keeps (`POST /api/1/console/trial/keep`) once it came
// back on it (`GET /api/1/console/fresh/{trial}`) or undoes (`POST
// /api/1/console/trial/undo`). Change the name (`GET /api/1/machine/name`, `POST
// /api/1/machine/name`) says what changes for the address the browser comes
// by, and once the machine answers the page asks for nothing: no read, and the
// link to the address that keeps working. A reader sees every fact and no
// button, and is refused when a route is asked. A phone at 390 by 844 shows the
// page with no sideways scroll and controls 44 px tall. Each look is the
// console's own: dark, light, high contrast, forced colours and less motion,
// each checked by axe (console.mjs's lookAll).
//
// certificates.toml makes the certificates Pebble gives and puts the machine
// back before and after each engine: its name, its firewall, the certificate
// the console serves, and a list with none of the test's certificates. The
// accounts are the two that certificates.toml made, so no script needs the
// lab's password.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import {
  ensure,
  fetchAsPage,
  isAddress,
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
import {
  PASSWORD,
  PHONE,
  WORDS,
  answerUnlock,
  pressAndAnswer,
  readDocument,
  smallControls,
  trialIn,
  unlockDialog,
  unlockNow,
} from "./trial.mjs";

const ADMIN = "wc-cert";
const READER = "wc-cert-reader";

// The certificates Pebble gave, which certificates.toml made, and the ones this
// script makes and uploads.
const STANDALONE = "standalone.acme-lab.example";
const BY_DNS = "dns.acme-lab.example";
const BY_DNS_NAMES = `${BY_DNS} *.${BY_DNS}`;
const OTHER = "wc-cert-other";
const SERVED = "wc-cert-console";
const NEW_NAME = "wc-cert-lab";
const ORDERED = "fails.acme-lab.example";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end.
const ROUTES = {
  list: "GET /api/1/certificates",
  show: "GET /api/1/certificates/{name}",
  history: "GET /api/1/certificates/history/all",
  check: "GET /api/1/certificates/check/{names}",
  get: "POST /api/1/certificates/{name}/get",
  renew: "POST /api/1/certificates/{name}/renew",
  add: "POST /api/1/certificates/{name}/add",
  revoke: "POST /api/1/certificates/{name}/revoke",
  remove: "POST /api/1/certificates/{name}/remove",
  wanted: "GET /api/1/certificates/wanted/all",
  forget: "POST /api/1/certificates/{name}/forget",
  machine: "GET /api/1/machine/name",
  rename: "POST /api/1/machine/name",
  serve: "POST /api/1/console/certificate",
  consoleKeep: "POST /api/1/console/trial/keep",
  consoleUndo: "POST /api/1/console/trial/undo",
  fresh: "GET /api/1/console/fresh/{trial}",
  open: "POST /api/1/firewall/add",
  portKeep: "POST /api/1/firewall/trial/keep",
  portUndo: "POST /api/1/firewall/trial/undo",
};

/** The path of a route as the table names it: `GET /api/1/certificates` is `/api/1/certificates`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

/** The method of a route as the table names it. */
const methodOf = (route) => route.slice(0, route.indexOf(" "));

/** The route `template` with its `{name}` or `{trial}` given. */
const routeFor = (template, name) => template.replace(/\{[a-z]+\}/, name);

// The words the page says where this test looks for them.
const READER_LEAD = "An administrator can get, upload, renew, revoke and remove certificates and change this machine's name.";
const PORT_CLOSED = "The firewall closes port 80, which Let's Encrypt must reach";
const PORT_BANNER = "Port 80 is open for a while so that Let's Encrypt can reach this server.";
const DNS_LINE = "You add a TXT record for each name at your DNS provider; this server waits up to 10 minutes for them to show.";
const UNTRUSTED = "It is not from an authority this server trusts (a private authority)";
const NOT_TRUSTED = "Not from an authority this server trusts";
const SERVING = (name) => `This console now serves ${name}'s certificate. Keep it once this page has come back on it.`;
const AGAIN = "Use this server's own certificate again";
const NOT_FINISHED = "The order did not finish.";
const FORGET_WORDS = (name) => `Stop trying to get ${name}? certd will not ask the authority for it again, and its alert goes away.`;

// --- Reading the machine ---

/** What `route` answers to the page's own request, whole: `{status, text, json}`. */
const askFor = (page, route, body = null) => fetchAsPage(page, methodOf(route), pathOf(route), body);

/** What a read route answers, which must be 200; gives its JSON. */
async function readOf(page, route) {
  const answer = await askFor(page, route);
  ensure(answer.status === 200, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** `cert list --json`: the certificates, which must be a list. */
async function readList(page) {
  const list = await readOf(page, ROUTES.list);
  ensure(Array.isArray(list), `${ROUTES.list} does not give a list: ${JSON.stringify(list)}`);
  return list;
}

/** `cert show NAME --json`, from the page's own route. */
const readShown = (page, name) => readOf(page, routeFor(ROUTES.show, encodeURIComponent(name)));

/** The certificate called `name` of `cert list`, or null. */
const foundIn = (list, name) => list.find((one) => one.name === name) ?? null;

/** The fingerprint as hex in capitals, which is the same for each way the console and a browser write it. */
const hexOf = (text) => String(text ?? "").replace(/[^0-9a-fA-F]/g, "").toUpperCase();

/** The fingerprint as a browser shows it: AB:CD:…. */
const colonsOf = (text) => hexOf(text).match(/.{1,2}/g)?.join(":") ?? "";

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

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The page ---

const card = (page, id) => page.locator(`main section.card#${id}`);
const row = (page, name) => page.locator(`#certificates tr[data-name="${name}"]`);
const buttonIn = (scope, name) => scope.getByRole("button", { name, exact: true });
const namesBox = (page) => page.locator("#get-names");
const checkRows = (page) => page.locator("#get .certs-checks li");

/** Opens /certificates, with `rest` after it, and waits for the cards, which come when the page has read what it shows. */
async function openCertificates(t, page, rest = "") {
  await page.goto(new URL(`/certificates${rest}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Certificates", exact: true }));
  await t.see(card(page, "name"));
  await page.waitForFunction(() => {
    const words = document.querySelector("#certificates .certs-body")?.textContent ?? "";
    return words !== "" && !words.includes("Reading the certificates");
  }, null, { timeout: 30_000 });
}

/** The button `words` of the row `name`, whether it is in the row or under its More. */
async function rowButton(page, name, words) {
  const one = buttonIn(row(page, name), words);
  if (await one.isVisible()) return one;
  await row(page, name).locator("summary").click();
  return one;
}

/**
 * Presses `control`, which asks the console to `method` `route`, and gives the
 * answer whatever it is. A console that is locked asks to unlock first: the
 * dialog is answered when it comes.
 */
async function pressAnswer(t, page, control, route, method = "POST") {
  const answered = page.waitForResponse((answer) => answer.request().method() === method && new URL(answer.url()).pathname === route, { timeout: 180_000 });
  answered.catch(() => {});
  await control.click();
  const asked = unlockDialog(page).waitFor({ state: "visible", timeout: 60_000 }).then(() => "unlock", () => "never");
  const first = await Promise.race([answered.then(() => "answer"), asked]);
  if (first === "unlock") await answerUnlock(t, page);
  return answered;
}

/** Presses `control` and gives the answer of `route`, which must be 200 or 202. */
async function pressFor(t, page, control, route) {
  const answer = await pressAnswer(t, page, control, route);
  ensure(answer.status() === 200 || answer.status() === 202, `POST ${route} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

/** Presses `control`, which asks a question before it changes anything, and waits for `dialog`; a locked console asks to unlock first. */
async function opensDialog(t, page, control, dialog) {
  await control.click();
  const first = await Promise.race([
    dialog.waitFor({ state: "visible", timeout: 30_000 }).then(() => "dialog", () => "never"),
    unlockDialog(page).waitFor({ state: "visible", timeout: 30_000 }).then(() => "unlock", () => "never"),
  ]);
  if (first === "unlock") await answerUnlock(t, page);
  await t.see(dialog);
}

// --- The banner of a trial this page tries ---

/** The box that holds the banner of trial `id`, which the page makes for the console's certificate and for port 80. */
const boxOf = (page, id) => page.locator(".try-box", { has: page.locator(`#try-reason-${id}`) });

/** What the banner of trial `id` shows now, as trial.mjs's `readBanner` gives it for the one banner of another page. */
const readTrial = (page, id) =>
  page.evaluate((trial) => {
    const box = document.querySelector(`#try-reason-${trial}`)?.closest(".try-box") ?? null;
    const live = box !== null && !box.hidden ? box.querySelector(".banner.try:not([hidden])") : null;
    const over = box !== null && !box.hidden ? box.querySelector(".try-over:not([hidden])") : null;
    const text = (parent, selector) => parent?.querySelector(selector)?.textContent.trim() ?? "";
    const keep = [...(live?.querySelectorAll("button") ?? [])].find((button) => button.textContent.trim() === "Keep this change");
    return {
      running: live !== null,
      heading: text(live, ".try-h"),
      words: text(live, ".try-words"),
      reach: [...(live?.querySelectorAll(".try-step") ?? [])].filter((step) => !step.hidden && !step.querySelector(".st")).map((step) => step.textContent.trim()).join(" "),
      keepOff: keep?.getAttribute("aria-disabled") === "true",
      over: text(over, ".try-over-words"),
    };
  }, id);

/** Waits until `test(banner)` holds of what `readTrial` gives, for up to `seconds`, and gives it. */
async function trialBecomes(page, id, test, what, seconds = 40) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const banner = await readTrial(page, id).catch(() => null);
    if (banner !== null && test(banner)) return banner;
    ensure(Date.now() < until, `the banner of ${id} did not ${what} within ${seconds} seconds: ${JSON.stringify(banner)}`);
    await sleep(500);
  }
}

/** Waits until the banner of trial `id` says how the trial ended, `kept` or `undone`. */
const trialEnds = (page, id, how, seconds = 30) =>
  trialBecomes(page, id, (banner) => !banner.running && banner.over.startsWith(how === "kept" ? WORDS.kept : WORDS.undone), `say that the change was ${how}`, seconds);

const keepOf = (page, id) => boxOf(page, id).getByRole("button", { name: "Keep this change", exact: true });
const undoOf = (page, id) => boxOf(page, id).getByRole("button", { name: "Undo now", exact: true });

// --- The list ---

/** Each certificate of `cert list` has its row, with who made it, what uses it and the end; and the history's lines name the certificates it is about. */
async function listAsAnswered(t, page, { pebble = true } = {}) {
  const list = await readList(page);
  ensure(list.length > 0, `${ROUTES.list} gives no certificate, and this server has its own`);
  for (const one of list) {
    const line = row(page, one.name);
    await t.see(line);
    const words = (await line.innerText()).replace(/\s+/g, " ");
    ensure(words.includes(one.names?.[0] ?? one.name), `the row of ${one.name} does not say ${one.names?.[0]}: ${words}`);
    for (const user of one.used_by ?? []) ensure(words.includes(user), `the row of ${one.name} does not say that ${user} uses it: ${words}`);
    if (one.kind === "self-signed" || one.name === "this-machine") ensure(words.includes("This server"), `the row of this server's certificate does not say who made it: ${words}`);
    if (one.revoked === true) ensure(words.includes("revoked"), `the row of ${one.name} does not say it is revoked: ${words}`);
  }
  const shown = await page.locator("#certificates tbody tr[data-name]").count();
  ensure(shown === list.length, `the list shows ${shown} certificates and ${ROUTES.list} gives ${list.length}`);
  if (!pebble) return;
  const made = list.filter((one) => one.name === STANDALONE || one.name === BY_DNS);
  ensure(made.length === 2, `the machine has ${made.length} of the two certificates Pebble made, not 2`);
  for (const one of made) ensure(/pebble/i.test((await row(page, one.name).innerText())), `the row of ${one.name} does not say that Pebble's authority made it`);

  const history = await readOf(page, ROUTES.history);
  ensure(Array.isArray(history), `${ROUTES.history} does not give a list: ${JSON.stringify(history)}`);
  const lines = await card(page, "renewals").locator("li").evaluateAll((items) => items.map((item) => item.textContent.replace(/\s+/g, " ")));
  for (const name of new Set(history.filter((one) => Date.now() - one.at_ms < 90 * 86_400_000).map((one) => one.name))) {
    ensure(lines.some((words) => words.includes(name)), `the Renewals card has no line about ${name}, which the history has: ${lines.join(" | ")}`);
  }
}

/** A link to /certificates/{name} opens the panel of the certificate, and a name that is not one says so. */
async function deepLink(t, page) {
  await openCertificates(t, page, `/${STANDALONE}`);
  await t.see(page.getByRole("dialog", { name: STANDALONE, exact: true }));
  await page.keyboard.press("Escape");
  await t.gone(page.getByRole("dialog", { name: STANDALONE, exact: true }));
  await openCertificates(t, page, "/nothing.acme-lab.example");
  await t.see(card(page, "certificates").getByText("There is no certificate called nothing.acme-lab.example."));
}

// --- Get one from Let's Encrypt ---

/** Types the names and leaves the box, as a person does, so that the checks are read. */
async function typeNames(page, words) {
  const box = namesBox(page);
  await box.fill(words);
  await box.press("Tab");
}

/** With port 80 closed the check says so and offers Open port 80; the order is off until it is put right. */
async function portIsClosed(t, page, name) {
  await typeNames(page, name);
  const closed = checkRows(page).filter({ hasText: PORT_CLOSED });
  await t.see(closed);
  await t.see(closed.locator("span.mono", { hasText: "CRT-103" }));
  await t.see(buttonIn(card(page, "get"), "Open port 80"));
  ensure((await card(page, "get").locator("button.default").getAttribute("aria-disabled")) === "true", "the button that gets the certificate is on while port 80 is closed");
  await lookAll(t, page, "certificates-checks");
}

/**
 * Open port 80 is a trial of the firewall: its request carries the window,
 * its banner says why, and the check passes once the rule is in. Gives nothing;
 * the trial is left for the caller to keep or undo.
 */
async function opensPort(t, page) {
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.open), { timeout: 120_000 });
  sent.catch(() => {});
  await pressFor(t, page, buttonIn(card(page, "get"), "Open port 80"), pathOf(ROUTES.open));
  const body = JSON.parse((await sent).postData() ?? "{}");
  ensure(body.action === "allow" && body.protocol === "tcp" && body.ports === "80" && body.try === 600, `Open port 80 sent ${JSON.stringify(body)}, not an allow of tcp 80 tried for 600 seconds`);
  const shown = await trialBecomes(page, "firewall", (banner) => banner.running && banner.heading.startsWith(PORT_BANNER), "show that port 80 is open for a while");
  ensure(/^The change is undone at \d{1,2}:\d\d [AP]M unless it is kept\./.test(shown.words), `the banner says "${shown.words}" about when the change is undone`);
  await eventually(async () => {
    ensure((await checkRows(page).filter({ hasText: PORT_CLOSED }).count()) === 0, "the check still says that the firewall closes port 80");
    ensure((await checkRows(page).count()) > 0 && (await checkRows(page).locator(".st.bad").count()) === 0, `a check failed or none is shown: ${(await checkRows(page).allInnerTexts()).join(" | ")}`);
  }, 30);
}

/** Open port 80 undone: the banner says so and the check fails again, with the button. */
async function undoesPort(t, page) {
  await pressAndAnswer(t, page, undoOf(page, "firewall"), pathOf(ROUTES.portUndo));
  await trialEnds(page, "firewall", "undone", 30);
  ensure(trialIn(await readDocument(page), "firewall") === null, "Spark still tries the firewall after Undo now");
  await t.see(checkRows(page).filter({ hasText: PORT_CLOSED }));
  await t.see(buttonIn(card(page, "get"), "Open port 80"));
}

/** Open port 80 kept once this browser reached the console: the rule stays, and the check passes. */
async function keepsPort(t, page) {
  await trialBecomes(page, "firewall", (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 40);
  await pressAndAnswer(t, page, keepOf(page, "firewall"), pathOf(ROUTES.portKeep));
  await trialEnds(page, "firewall", "kept", 30);
  ensure(trialIn(await readDocument(page), "firewall") === null, "Spark still tries the firewall after it was kept");
}

/** An order that cannot finish is told in words, and nothing is added to the list. */
async function orderCannotFinish(t, page) {
  const before = await readList(page);
  await typeNames(page, ORDERED);
  await buttonIn(card(page, "get"), "Check again").click();
  await eventually(async () => {
    ensure((await checkRows(page).count()) > 0, "no check is shown");
    ensure((await checkRows(page).locator(".st.bad").count()) === 0, `a check failed: ${(await checkRows(page).allInnerTexts()).join(" | ")}`);
  }, 60);
  const get = card(page, "get").locator("button.default");
  ensure((await get.getAttribute("aria-disabled")) !== "true", "the button that gets the certificate is off while every check passes");
  const answer = await pressAnswer(t, page, get, routeFor(pathOf(ROUTES.get), ORDERED));
  ensure(answer.status() === 200 || answer.status() === 202, `${ROUTES.get} was answered ${answer.status()}: ${await answer.text()}`);
  await page.waitForFunction((words) => (document.querySelector("#get .certs-progress")?.textContent ?? "").includes(words), NOT_FINISHED, { timeout: 240_000 });
  ensure((await get.getAttribute("aria-disabled")) !== "true", "the button that gets the certificate stays off after the order ended");
  const after = await readList(page);
  ensure(after.length === before.length, `an order that did not finish changed the list from ${before.length} to ${after.length} certificates`);
}

// --- What certd tries again ---

/** What `GET /api/1/certificates/wanted/all` says: `cert wanted --json`, `{format, wanted: [...]}`. */
async function readWanted(page) {
  const answer = await askFor(page, ROUTES.wanted);
  ensure(answer.status === 200 && Array.isArray(answer.json?.wanted), `${ROUTES.wanted} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The record of `name` in what certd asks for again, or null. */
const wantedOne = (read, name) => read.wanted.find((one) => one.name === name) ?? null;

/** The row of the list that says certd tries to get `name` again. */
const wantedRow = (page, name) => page.locator(`#certificates tr.certs-wanted[data-wanted="${name}"]`);

/**
 * The order that did not finish was kept: the machine has its record, and the
 * list has a row for it that says when certd tries next, how many tries there
 * were and what the last said, as the record has it. Gives the record.
 */
async function wantedIsShown(t, page) {
  const read = await eventually(async () => {
    const found = await readWanted(page);
    ensure(wantedOne(found, ORDERED) !== null, `the order of ${ORDERED} that did not finish left no record in ${ROUTES.wanted}: ${JSON.stringify(found.wanted)}`);
    return found;
  }, 30);
  const record = wantedOne(read, ORDERED);
  ensure(record.given_up === false && record.tries >= 1 && record.names?.[0] === ORDERED && record.next_ms > record.first_ms, `the record of ${ORDERED} is ${JSON.stringify(record)}`);

  // The card reads the records beside the certificates, so it is opened again.
  await openCertificates(t, page);
  const line = wantedRow(page, ORDERED);
  await t.see(line);
  const words = (await line.innerText()).replace(/\s+/g, " ");
  ensure(words.includes("Trying again: next at "), `the row of ${ORDERED} does not say that certd tries again: ${words}`);
  ensure(words.includes(record.tries === 1 ? "1 try" : `${record.tries} tries`), `the row of ${ORDERED} does not say ${record.tries} tries: ${words}`);
  ensure(words.includes("last: "), `the row of ${ORDERED} does not say what the last try said: ${words}`);
  const id = /\(([A-Z]{2,4}-\d{3})\)/.exec(record.why)?.[1];
  if (id !== undefined) ensure(words.includes(id), `the row of ${ORDERED} does not say ${id}, which the last try ended with: ${words}`);
  await t.see(buttonIn(line, "Forget"));
  ensure((await row(page, ORDERED).count()) === 0, `${ORDERED} is listed among the certificates, which it is not`);
  await lookAll(t, page, "certificates-wanted");
  return record;
}

/** A reader sees the row, whose words are the machine's, and has no Forget; the route refuses it, and the list of records is open to it. */
async function readerSeesWanted(t, record) {
  const { page } = await signedIn(t, READER);
  await openCertificates(t, page);
  const line = wantedRow(page, ORDERED);
  await t.see(line);
  ensure((await line.innerText()).replace(/\s+/g, " ").includes("Trying again: next at "), "a reader's row of what certd tries again does not say so");
  ensure((await buttonIn(line, "Forget").count()) === 0, "a reader's row of what certd tries again has Forget");
  ensure(wantedOne(await readWanted(page), ORDERED)?.tries === record.tries, `a reader's ${ROUTES.wanted} does not give the record of ${ORDERED}`);
  mustBe(await askFor(page, routeFor(ROUTES.forget, ORDERED), {}), 403, "ADM-122", `a reader's ${ROUTES.forget}`);
  ensure(wantedOne(await readWanted(page), ORDERED) !== null, `a reader's ${ROUTES.forget} took the record of ${ORDERED} away`);
  await signOut(t, page);
  await page.context().close();
}

/** Forget asks first, and Cancel leaves the record; Forget it stops certd trying and takes the row away. */
async function forgets(t, page) {
  const dialog = page.getByRole("dialog", { name: `Forget ${ORDERED}`, exact: true });
  await opensDialog(t, page, buttonIn(wantedRow(page, ORDERED), "Forget"), dialog);
  await t.see(dialog.getByText(FORGET_WORDS(ORDERED), { exact: true }));
  await lookAll(t, page, "certificates-forget");
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);
  ensure(wantedOne(await readWanted(page), ORDERED) !== null, `Cancel took the record of ${ORDERED} away`);
  await t.see(wantedRow(page, ORDERED));

  await opensDialog(t, page, buttonIn(wantedRow(page, ORDERED), "Forget"), dialog);
  await pressFor(t, page, buttonIn(dialog, "Forget it"), routeFor(pathOf(ROUTES.forget), ORDERED));
  await t.gone(wantedRow(page, ORDERED));
  ensure(wantedOne(await readWanted(page), ORDERED) === null, `${ROUTES.wanted} still has ${ORDERED} after Forget it`);
  // Forgetting what certd has no record of is refused, and the list stays as it is.
  const again = await askFor(page, routeFor(ROUTES.forget, ORDERED), {});
  ensure(again.status >= 400, `${ROUTES.forget} for ${ORDERED} was answered ${again.status} when certd had no record of it`);
}

// --- Renew, revoke, remove ---

/** Renew now asks the authority the certificate came from and says when the new one is good until; its fingerprint is new. */
async function renewsStandalone(t, page) {
  const before = (await readShown(page, STANDALONE)).fingerprint_sha256;
  await pressFor(t, page, buttonIn(row(page, STANDALONE), "Renew now"), routeFor(pathOf(ROUTES.renew), STANDALONE));
  await t.see(card(page, "certificates").getByText(/Renewed: good until /));
  const after = (await readShown(page, STANDALONE)).fingerprint_sha256;
  ensure(hexOf(before) !== hexOf(after), `the fingerprint of ${STANDALONE} is the same after Renew now`);
}

/** A certificate proved by DNS cannot be renewed by certd: the refusal says so, and the row then says Renew by DNS, which fills the form. */
async function renewsByDns(t, page) {
  const line = row(page, BY_DNS);
  ensure((await line.innerText()).replace(/\s+/g, " ").includes("and 1 more"), "the row of the certificate proved by DNS does not say that it has a second name");
  const answer = await pressAnswer(t, page, buttonIn(line, "Renew now"), routeFor(pathOf(ROUTES.renew), BY_DNS));
  ensure(answer.status() >= 400, `${ROUTES.renew} was answered ${answer.status()} for a certificate proved by DNS: ${await answer.text()}`);
  ensure((await answer.text()).includes("proved by DNS"), `the refusal of a renewal by certd of a certificate proved by DNS does not say so: ${await answer.text()}`);
  await t.see(buttonIn(line, "Renew by DNS…"));
  ensure((await buttonIn(line, "Renew now").count()) === 0, "Renew now is still offered for a certificate that was proved by DNS");
  await t.see(line.getByText("cannot renew by itself", { exact: false }));
  const checked = page.waitForRequest((request) => new URL(request.url()).pathname.startsWith(`${pathOf(ROUTES.check).replace("{names}", "")}`) && request.url().includes("dns=true"), { timeout: 60_000 });
  checked.catch(() => {});
  await buttonIn(line, "Renew by DNS…").click();
  await checked;
  ensure((await namesBox(page).inputValue()).split(/\s+/).join(" ") === BY_DNS_NAMES, `Renew by DNS filled the names with ${JSON.stringify(await namesBox(page).inputValue())}, not ${BY_DNS_NAMES}`);
  ensure(await card(page, "get").locator("input[type=radio][value=dns]").isChecked(), "Renew by DNS did not choose the proof by DNS");
  await t.see(card(page, "get").getByText(DNS_LINE));
  ensure((await card(page, "get").locator(".certs-checks li").filter({ hasText: PORT_CLOSED }).count()) === 0, "the checks of a name proved by DNS speak of port 80");
  await lookAll(t, page, "certificates-dns");
}

/** Show fingerprint: the fingerprint is the one `cert show` gives, and Save the certificate downloads its chain. */
async function fingerprintPanel(t, page, name) {
  const shown = await readShown(page, name);
  const dialog = page.getByRole("dialog", { name, exact: true });
  await opensDialog(t, page, await rowButton(page, name, "Show fingerprint"), dialog);
  const said = (await dialog.locator(".certs-fingerprint").innerText()).trim();
  ensure(said === colonsOf(shown.fingerprint_sha256), `the panel of ${name} says the fingerprint ${said}, and ${ROUTES.show} says ${colonsOf(shown.fingerprint_sha256)}`);
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), buttonIn(dialog, "Save the certificate").click()]);
  ensure(download.suggestedFilename() === `${name}.crt`, `Save the certificate offers the file ${download.suggestedFilename()}, not ${name}.crt`);
  ensure(readFileSync(await download.path(), "utf8") === shown.chain_pem, `the file Save the certificate gives is not the chain of ${name} that ${ROUTES.show} gives`);
  await buttonIn(dialog, "Close").click();
  await t.gone(dialog);
}

/** Revoke asks why, says what it does not do, and the row says revoked; the machine says the same. */
async function revokes(t, page, name) {
  const dialog = page.getByRole("dialog", { name: `Revoke ${name}`, exact: true });
  await opensDialog(t, page, await rowButton(page, name, "Revoke…"), dialog);
  await t.see(dialog.getByText("Services keep using the certificate until a new one replaces it", { exact: false }));
  await dialog.getByLabel("A new certificate replaces it", { exact: false }).check();
  await lookAll(t, page, "certificates-revoke");
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === routeFor(pathOf(ROUTES.revoke), name), { timeout: 120_000 });
  sent.catch(() => {});
  await pressFor(t, page, buttonIn(dialog, "Revoke it"), routeFor(pathOf(ROUTES.revoke), name));
  ensure(JSON.parse((await sent).postData() ?? "{}").reason === "superseded", "Revoke sent a reason that is not the one chosen");
  await t.gone(dialog);
  await t.see(row(page, name).getByText("revoked", { exact: false }));
  ensure((await readShown(page, name)).revoked === true, `${ROUTES.show} does not say that ${name} is revoked`);
}

/** Remove is offered where no service uses the certificate, and the certificate is gone from the list. */
async function removes(t, page, name) {
  const dialog = page.getByRole("dialog", { name: `Remove ${name}`, exact: true });
  await opensDialog(t, page, await rowButton(page, name, "Remove…"), dialog);
  await pressFor(t, page, buttonIn(dialog, "Remove it"), routeFor(pathOf(ROUTES.remove), name));
  await t.gone(row(page, name));
  ensure(foundIn(await readList(page), name) === null, `${ROUTES.list} still lists ${name} after Remove`);
}

// --- Upload your own ---

/** The certificates this script makes: its own authority, a certificate for a name, and one for the address the browser comes by. */
function makeCertificates(host) {
  const dir = mkdtempSync(path.join(tmpdir(), "wc-cert-"));
  const openssl = (...args) => execFileSync("openssl", args, { cwd: dir, stdio: "pipe" });
  const key = (file) => openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-out", file);
  key("ca.key");
  openssl("req", "-new", "-x509", "-key", "ca.key", "-subj", "/CN=wc-cert-lab-ca", "-days", "30", "-out", "ca.pem");
  let serial = 10;
  const leaf = (id, san) => {
    serial += 1;
    key(`${id}.key`);
    openssl("req", "-new", "-key", `${id}.key`, "-subj", `/CN=${id}`, "-out", `${id}.csr`);
    writeFileSync(path.join(dir, `${id}.ext`), `subjectAltName=${san}\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n`);
    openssl("x509", "-req", "-in", `${id}.csr`, "-CA", "ca.pem", "-CAkey", "ca.key", "-set_serial", String(serial), "-days", "30", "-extfile", `${id}.ext`, "-out", `${id}.crt`);
    writeFileSync(path.join(dir, `${id}.chain`), readFileSync(path.join(dir, `${id}.crt`), "utf8") + readFileSync(path.join(dir, "ca.pem"), "utf8"));
  };
  leaf("other", "DNS:other.acme-lab.example");
  leaf("console", `${isAddress(host) ? "IP" : "DNS"}:${host.replace(/^\[|\]$/g, "")},DNS:console.acme-lab.example`);
  const file = (name) => path.join(dir, name);
  const text = (name) => readFileSync(file(name), "utf8");
  return {
    dir,
    other: { chain: file("other.chain"), key: file("other.key"), chainText: text("other.chain"), keyText: text("other.key") },
    console: { chain: file("console.chain"), key: file("console.key"), chainText: text("console.chain"), keyText: text("console.key") },
    clean: () => rmSync(dir, { recursive: true, force: true }),
  };
}

/** The line of the key's body, which is what no request, text or store but the upload may carry. */
const markOf = (keyText) => keyText.split("\n").find((line) => line !== "" && !line.startsWith("-----"));

const uploadDialog = (page) => page.getByRole("dialog", { name: "Upload your own certificate", exact: true });
const boxesOf = (dialog) => ({
  name: dialog.locator("#upload-name"),
  chain: dialog.locator("textarea").nth(0),
  key: dialog.locator("textarea").nth(1),
  chainFile: dialog.locator("input[type=file]").nth(0),
  keyFile: dialog.locator("input[type=file]").nth(1),
  untrusted: dialog.getByLabel(UNTRUSTED, { exact: true }),
});

/** Opens the upload dialog from the head's button. */
async function opensUpload(t, page) {
  const dialog = uploadDialog(page);
  await opensDialog(t, page, page.getByRole("button", { name: "Upload your own…", exact: true }), dialog);
  return dialog;
}

/** What the page holds of a secret `mark`: its text and attributes, its browser stores and its address. */
const holdsSecret = (page, mark) =>
  page.evaluate((wanted) => {
    const stores = [localStorage, sessionStorage].map((store) => JSON.stringify(Object.entries(store)));
    const attributes = [...document.querySelectorAll("*")].flatMap((one) => [...one.attributes].map((attribute) => attribute.value));
    return [document.documentElement.textContent, ...attributes, ...stores, location.href].some((words) => words.includes(wanted));
  }, mark);

/**
 * An upload whose key is not the certificate's is refused with its words,
 * the key leaves its box, the chain stays, and Cancel empties the rest. Then
 * the right key is chosen as a file and the certificate is added: the key is
 * in one request, which is the add, and nowhere the page holds words.
 */
async function uploads(t, page, made, requests) {
  const mark = markOf(made.other.keyText);
  let dialog = await opensUpload(t, page);
  let boxes = boxesOf(dialog);
  await boxes.name.fill(OTHER);
  await boxes.untrusted.check();
  await boxes.chain.fill(made.other.chainText);
  await boxes.key.fill(made.console.keyText);
  await lookAll(t, page, "certificates-upload");
  const refused = await pressAnswer(t, page, buttonIn(dialog, "Upload"), routeFor(pathOf(ROUTES.add), OTHER));
  ensure(refused.status() >= 400, `${ROUTES.add} with a key that is not the certificate's was answered ${refused.status()}: ${await refused.text()}`);
  await eventually(async () => ensure(/\b[A-Z]{3}-\d{3}\b/.test(await dialog.innerText()), "the dialog does not say the number of the refusal"), 20);
  ensure((await boxes.key.inputValue()) === "", "the key waits in its box after it was sent and refused");
  ensure((await boxes.chain.inputValue()) !== "", "the chain was taken out of its box by a refusal");
  ensure(!(await holdsSecret(page, markOf(made.console.keyText))), "the refused key is in the page");
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);
  ensure(requests.filter((one) => one.body.includes(markOf(made.console.keyText))).length === 1, "the refused key was in more than one request");

  dialog = await opensUpload(t, page);
  boxes = boxesOf(dialog);
  await boxes.name.fill(OTHER);
  await boxes.untrusted.check();
  await boxes.chainFile.setInputFiles(made.other.chain);
  await boxes.keyFile.setInputFiles(made.other.key);
  ensure(!(await holdsSecret(page, mark)), "a key that was chosen as a file is in the page before it is sent");
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === routeFor(pathOf(ROUTES.add), OTHER), { timeout: 120_000 });
  sent.catch(() => {});
  await pressFor(t, page, buttonIn(dialog, "Upload"), routeFor(pathOf(ROUTES.add), OTHER));
  const body = JSON.parse((await sent).postData() ?? "{}");
  ensure(body.pem === `${made.other.keyText.trim()}\n${made.other.chainText.trim()}\n`, "the document that was sent is not the key and then the chain");
  ensure(Object.keys(body).every((member) => ["pem", "replace", "untrusted"].includes(member)), `the upload sent ${Object.keys(body).join(", ")}, which is more than the document and its two choices`);
  await t.gone(dialog);
  ensure(requests.filter((one) => one.body.includes(mark)).length === 1, `the key was in ${requests.filter((one) => one.body.includes(mark)).length} requests, and the add is the one`);
  ensure(!(await holdsSecret(page, mark)), "the key is in the page after it was sent");
  await t.see(row(page, OTHER));
  await t.see(row(page, OTHER).getByText(NOT_TRUSTED));
  const listed = foundIn(await readList(page), OTHER);
  ensure(listed !== null && listed.kind === "added", `${ROUTES.list} does not list ${OTHER} as added: ${JSON.stringify(listed)}`);
  ensure(!JSON.stringify(await readShown(page, OTHER)).includes(mark), `${ROUTES.show} gives the key of ${OTHER}`);
}

/** Replace… of a certificate added by hand opens the upload with its name, and a new certificate takes its place. */
async function replaces(t, page, made) {
  const dialog = page.getByRole("dialog", { name: `Replace ${OTHER}`, exact: true });
  await opensDialog(t, page, await rowButton(page, OTHER, "Replace…"), dialog);
  const boxes = boxesOf(dialog);
  ensure((await boxes.name.inputValue()) === OTHER, "Replace… does not give the name of the certificate");
  await boxes.untrusted.check();
  await boxes.chain.fill(made.other.chainText);
  await boxes.key.fill(made.other.keyText);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === routeFor(pathOf(ROUTES.add), OTHER), { timeout: 120_000 });
  sent.catch(() => {});
  await pressFor(t, page, buttonIn(dialog, "Replace"), routeFor(pathOf(ROUTES.add), OTHER));
  ensure(JSON.parse((await sent).postData() ?? "{}").replace === true, "Replace sent no replace");
  await t.gone(dialog);
}

// --- The certificate the console serves ---

/** The fingerprint of the certificate the console presents to a new connection, as hex. */
async function servedNow(page) {
  return hexOf((await servedCertificate(originOf(page))).fingerprint256);
}

/** Waits until the console presents the certificate with fingerprint `hex`, for up to `seconds`. */
async function servedBecomes(page, hex, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  let seen = "";
  for (;;) {
    seen = await servedNow(page).catch(() => "");
    if (seen === hex) return;
    ensure(Date.now() < until, `the console does not present ${what} ${seconds} seconds on: it presents ${seen}, not ${hex}`);
    await sleep(1000);
  }
}

/** The console's certificate: refused for one that does not name the address, a trial for one that does, kept once the page came back on it, and undone. */
async function consoleCertificate(t, page) {
  const own = hexOf((await readShown(page, "this-machine")).fingerprint_sha256);
  const named = hexOf((await readShown(page, SERVED)).fingerprint_sha256);
  await servedBecomes(page, own, "this server's certificate", 10);

  // The guard: the certificate that does not name this browser's address is refused, and nothing changes.
  await unlockNow(page);
  mustBe(await askFor(page, ROUTES.serve, { name: OTHER, try: 300 }), 409, "ADM-141", `the console's certificate set to ${OTHER}, which does not name the address this browser comes by`);
  ensure((await servedNow(page)) === own, "the console serves another certificate after the guard refused it");
  ensure((await row(page, OTHER).getByRole("button", { name: "Use for this console" }).count()) === 0, `${OTHER} is offered for the console, and it does not name this browser's address`);

  // A trial of the one that does, kept once the page came back on it.
  await pressFor(t, page, await rowButton(page, SERVED, "Use for this console"), pathOf(ROUTES.serve));
  const began = await trialBecomes(page, "console", (banner) => banner.running && banner.heading === SERVING(SERVED), "show that the console serves the certificate", 40);
  ensure(/^The change is undone at \d{1,2}:\d\d [AP]M unless it is kept\./.test(began.words), `the banner says "${began.words}" about when the change is undone`);
  await t.see(row(page, SERVED).locator(".pill", { hasText: "this console" }));
  await servedBecomes(page, named, `${SERVED}'s certificate`);
  await trialBecomes(page, "console", (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 60);
  await lookAll(t, page, "certificates-console-trial");
  await pressAndAnswer(t, page, keepOf(page, "console"), pathOf(ROUTES.consoleKeep));
  await trialEnds(page, "console", "kept", 30);
  ensure(trialIn(await readDocument(page), "console") === null, "Spark still tries the console's certificate after it was kept");
  ensure((await readOf(page, "GET /api/1/security/sign-in")).console?.certificate === SERVED, "the sign-in rules do not say that the console serves the certificate that was kept");

  // This server's own again: a trial, undone, and the one that was kept is served again.
  await pressFor(t, page, await rowButton(page, "this-machine", AGAIN), pathOf(ROUTES.serve));
  await trialBecomes(page, "console", (banner) => banner.running && banner.heading === SERVING("this-machine"), "show that the console serves its own certificate", 40);
  await pressAndAnswer(t, page, undoOf(page, "console"), pathOf(ROUTES.consoleUndo));
  await trialEnds(page, "console", "undone", 30);
  await servedBecomes(page, named, `${SERVED}'s certificate again`);

  // And kept, so that the console serves its own at the end.
  await pressFor(t, page, await rowButton(page, "this-machine", AGAIN), pathOf(ROUTES.serve));
  await trialBecomes(page, "console", (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 60);
  await pressAndAnswer(t, page, keepOf(page, "console"), pathOf(ROUTES.consoleKeep));
  await trialEnds(page, "console", "kept", 30);
  await servedBecomes(page, own, "this server's certificate again");
}

// --- The machine's name ---

/** The requests the page itself makes to read what it shows, which stop once the machine is renamed. */
const isPageRead = (request) => {
  const { pathname } = new URL(request.url());
  return request.method() === "GET" && (pathname.startsWith("/api/1/certificates") || pathname === "/api/1/machine/name" || pathname === "/api/1/console/about" || pathname === "/api/1/security/sign-in");
};

/** Changes the name in the dialog: what it says changes for this browser's address, the answer, and the page asking for nothing after it. */
async function renames(t, page, { first, domain = "", wait }) {
  const original = await readOf(page, ROUTES.machine);
  const host = new URL(page.url()).hostname.replace(/^\[|\]$/g, "");
  const dialog = page.getByRole("dialog", { name: "Change the machine's name", exact: true });
  await opensDialog(t, page, buttonIn(card(page, "name"), "Change…"), dialog);
  ensure((await dialog.locator("#rename-first").inputValue()) === original.first, `the dialog starts with the name ${await dialog.locator("#rename-first").inputValue()}, not ${original.first}`);
  await dialog.locator("#rename-first").fill(first);
  await dialog.locator("#rename-domain").fill(domain);
  const said = (await dialog.innerText()).replace(/\s+/g, " ");
  ensure(said.includes("This server makes its own certificate anew for the new name."), `the dialog does not say that this server makes its certificate anew: ${said}`);
  ensure(said.includes(`You reach this console as ${host}.`), `the dialog does not say how this browser reaches the console: ${said}`);
  if (isAddress(host)) ensure(said.includes("That address keeps working."), `the dialog does not say that the address keeps working: ${said}`);
  await lookAll(t, page, "certificates-rename");
  const before = await servedNow(page);
  const full = domain === "" ? first : `${first}.${domain}`;
  // The answer is the last the page asks for: what it reads is counted from the press on, and the press reads nothing.
  const reads = [];
  const count = (request) => {
    if (isPageRead(request)) reads.push(`${request.method()} ${new URL(request.url()).pathname}`);
  };
  page.on("request", count);
  await pressFor(t, page, buttonIn(dialog, "Change the name"), pathOf(ROUTES.rename));
  await t.gone(dialog);
  await t.see(card(page, "name").getByText(`The machine is called ${full} now.`, { exact: false }));
  const nameCard = (await card(page, "name").innerText()).replace(/\s+/g, " ");
  if (isAddress(host)) {
    ensure(nameCard.includes(`This page keeps working at ${host}, and asks for nothing more.`), `the Name card does not say that the page keeps working at ${host}: ${nameCard}`);
    const link = card(page, "name").getByRole("link", { name: "Reload this page", exact: true });
    await t.see(link);
    ensure(new URL(await link.getAttribute("href")).hostname.replace(/^\[|\]$/g, "") === host, "the link of the Name card goes to another address than the one that keeps working");
  } else {
    const link = card(page, "name").getByRole("link", { name: /^Open the console at / });
    await t.see(link);
    ensure(new URL(await link.getAttribute("href")).port === new URL(page.url()).port, "the link of the Name card goes to another port");
  }
  for (const id of ["certificates", "get", "renewals"]) ensure(!(await card(page, id).isVisible()), `the ${id} card is shown after the rename`);
  const shown = /SHA-?256[^A-F0-9]*((?:[0-9A-F]{2}:){31}[0-9A-F]{2})/i.exec(nameCard) ?? /((?:[0-9A-F]{2}:){31}[0-9A-F]{2})/i.exec(nameCard);
  await sleep(wait * 1000);
  page.off("request", count);
  ensure(reads.length === 0, `the page asked for ${reads.join(", ")} after the machine was renamed`);
  const now = await readOf(page, ROUTES.machine);
  ensure(now.name.toLowerCase() === full.toLowerCase(), `${ROUTES.machine} says ${now.name} after the rename, not ${full}`);
  // The machine's own certificate is made again for the new name, and the card says which it is.
  await eventually(async () => ensure((await servedNow(page)) !== before, "the console still presents the certificate it had before the rename"), 90);
  if (shown) ensure(hexOf(shown[1]) === (await servedNow(page)), `the Name card says the fingerprint ${shown[1]}, which is not the one the console presents`);
  return { full, fingerprintShown: shown !== null };
}

// --- The people and the phone ---

/** The administrator: each card, as the sections above have it, in the order the machine allows. */
async function administrator(t, made) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const requests = [];
  page.on("request", (request) => {
    if (request.method() === "POST") requests.push({ path: new URL(request.url()).pathname, body: request.postData() ?? "" });
  });
  await openCertificates(t, page);
  await listAsAnswered(t, page);
  await lookAll(t, page, "certificates-first");
  await deepLink(t, page);
  await openCertificates(t, page);

  // The form: port 80 closed, opened for a while and undone, opened again and kept.
  await portIsClosed(t, page, ORDERED);
  await opensPort(t, page);
  await undoesPort(t, page);
  await opensPort(t, page);
  await keepsPort(t, page);
  await orderCannotFinish(t, page);
  const record = await wantedIsShown(t, page);
  await readerSeesWanted(t, record);
  await forgets(t, page);

  // What Pebble made: renewed, shown, the one proved by DNS refused its renewal, revoked, removed.
  await renewsStandalone(t, page);
  await fingerprintPanel(t, page, STANDALONE);
  await renewsByDns(t, page);
  await revokes(t, page, STANDALONE);
  await removes(t, page, STANDALONE);
  await removes(t, page, BY_DNS);

  // Upload your own, and the one that names this browser's address.
  await uploads(t, page, made, requests);
  await replaces(t, page, made);
  const serving = await opensUpload(t, page);
  const second = boxesOf(serving);
  await second.name.fill(SERVED);
  await second.untrusted.check();
  await second.chain.fill(made.console.chainText);
  await second.key.fill(made.console.keyText);
  await pressFor(t, page, buttonIn(serving, "Upload"), routeFor(pathOf(ROUTES.add), SERVED));
  await t.gone(serving);
  await consoleCertificate(t, page);
  await removes(t, page, SERVED);
  await removes(t, page, OTHER);

  // The machine's name, changed and put back.
  const original = await readOf(page, ROUTES.machine);
  await renames(t, page, { first: NEW_NAME, wait: t.engine === "chromium" ? 70 : 8 });
  await openCertificates(t, page);
  await t.see(card(page, "name").getByText(NEW_NAME, { exact: true }));
  await renames(t, page, { first: original.first, domain: original.domain ?? "", wait: 5 });

  const concrete = (route, name) => `${methodOf(route)} ${routeFor(pathOf(route), name)}`;
  mustHaveCalled(a.seen, [
    ROUTES.list, ROUTES.history, ROUTES.wanted, ROUTES.machine, ROUTES.rename, ROUTES.serve, ROUTES.consoleKeep, ROUTES.consoleUndo, ROUTES.open, ROUTES.portKeep, ROUTES.portUndo,
    concrete(ROUTES.show, STANDALONE), concrete(ROUTES.get, ORDERED), concrete(ROUTES.forget, ORDERED), concrete(ROUTES.renew, STANDALONE), concrete(ROUTES.revoke, STANDALONE),
    concrete(ROUTES.remove, STANDALONE), concrete(ROUTES.add, OTHER), concrete(ROUTES.fresh, "firewall"), concrete(ROUTES.fresh, "console"),
  ]);
  ensure([...a.seen].some((route) => route.startsWith(`${methodOf(ROUTES.check)} ${pathOf(ROUTES.check).replace("{names}", "")}`)), `the test never called ${ROUTES.check}`);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees every fact and no button but the panel's, is told who may change things, and is refused when a route is asked. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openCertificates(t, page);
  await t.see(page.getByText(READER_LEAD, { exact: true }));
  await listAsAnswered(t, page, { pebble: false });
  const buttons = await page.locator("main button").evaluateAll((all) => all.filter((one) => one.offsetParent !== null).map((one) => one.textContent.trim()));
  const allowed = new Set(["Show fingerprint", "Check again", "Show all"]);
  const other = buttons.filter((words) => !allowed.has(words));
  ensure(other.length === 0, `a reader's page has these buttons: ${other.join(", ")}`);
  ensure((await page.getByRole("button", { name: "Upload your own…" }).count()) === 0, "a reader has Upload your own");
  ensure((await buttonIn(card(page, "name"), "Change…").count()) === 0, "a reader has Change… for the machine's name");
  for (const route of [ROUTES.renew, ROUTES.add, ROUTES.revoke, ROUTES.remove, ROUTES.get, ROUTES.forget]) {
    mustBe(await askFor(page, routeFor(route, "wc-cert-x"), {}), 403, "ADM-122", `a reader's ${route}`);
  }
  mustBe(await askFor(page, ROUTES.serve, { name: "this-machine" }), 403, "ADM-122", `a reader's ${ROUTES.serve}`);
  mustBe(await askFor(page, ROUTES.rename, { name: "wc-cert-x" }), 403, "ADM-122", `a reader's ${ROUTES.rename}`);
  await lookAll(t, page, "certificates-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the page in one column, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  await openCertificates(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main button, main summary, main input[type=text], main input[type=email], main label.radio-card, main a.btn");
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "certificates-phone");

  // The dialogs are read on a phone too, and closed.
  const dialog = await opensUpload(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's upload dialog scrolls ${await sidewaysScroll(page)} px sideways`);
  const inDialog = await smallControls(page, "dialog button, dialog input[type=text], dialog textarea, dialog input[type=file]");
  ensure(inDialog.length === 0, `these controls of the phone's upload dialog are under 44 px tall: ${inDialog.join(", ")}`);
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The address the browser comes by is the one the certificate for the console names.
  const made = makeCertificates(new URL(t.url).hostname);
  try {
    await administrator(t, made);
    await reader(t);
    await phone(t);
  } finally {
    made.clean();
  }
}
