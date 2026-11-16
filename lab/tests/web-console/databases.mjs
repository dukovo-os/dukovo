// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/databases drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the list at
// /databases, Add a database at /databases/add and one database's page at
// /databases/{name}. The list shows what `database list --json` says, which
// `GET /api/1/databases` runs, for an administrator whose console is
// unlocked: listing signs in to PostgreSQL as its superuser, so a locked
// console is asked to unlock and a reader is told who may.
//
// Add a database is opened from a site (`?site=shop-lab.example.org`), which
// gives the name shop_lab. Pressing Add the database makes a role and a
// database (`POST /api/1/databases/add`); the answer holds the password, which
// the page shows once in a dialog with the connection settings, Copy and Save
// as file. The dialog asks before the page is left (Back is pressed while it is
// open) and, once it is closed, the page holds the password nowhere. The
// database's page is held to what the machine says of it: its facts, who
// connects and its settings. Let other machines connect
// (`POST /api/1/databases/{name}/allow`) is a task that makes PostgreSQL listen
// on the network with this machine's certificate and lets one network of the
// documentation's range in; the page shows its steps, the machine says so, and
// the settings gain the way for other machines and the certificate to download.
// Remove beside the network (`POST /api/1/databases/{name}/deny`) puts it
// back. Make a new password (`POST /api/1/databases/{name}/password`) shows
// another password once. Remove… (`POST /api/1/databases/{name}/remove`) asks
// for the database's name; the second database, lab_busy, has a session open
// that databases.sh holds, which the machine refuses to end unless it is told
// to (DB-103) and which the dialog says it ends.
//
// A reader is told who may list the databases, has no button that works, and
// is refused when a route is asked. A phone at 390 by 844 shows the list and a
// database alone, with no sideways scroll and every control 44 px tall. Each
// look is the console's own: dark, light, high contrast, forced colours and
// less motion, each checked by axe (console.mjs's lookAll).
//
// The script runs once for each engine, databases.toml having made lab_busy and
// its session before it, and it removes both databases. The accounts are the
// two that databases.toml made, so no script needs the lab's password.

import { readFile } from "node:fs/promises";

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, pressAndAnswer, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-databases";
const READER = "wc-databases-reader";

// The database that the test adds, the site its name comes from, the one with
// a session open, a name that no machine has, and a network that no one uses.
const SHOP = "shop_lab";
const SITE = "shop-lab.example.org";
const BUSY = "lab_busy";
const NOBODY = "lab_nobody";
const NETWORK = "192.0.2.0/24";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with a
// database's name for {name}.
const ROUTES = {
  list: "GET /api/1/databases",
  add: "POST /api/1/databases/add",
  password: "POST /api/1/databases/{name}/password",
  allow: "POST /api/1/databases/{name}/allow",
  deny: "POST /api/1/databases/{name}/deny",
  remove: "POST /api/1/databases/{name}/remove",
};

/** The path of a route as the table names it, for the database `name`. */
const pathOf = (route, name = "") => route.slice(route.indexOf(" ") + 1).replace("{name}", encodeURIComponent(name));

/** The route as `track` writes it, for `name`. */
const routeFor = (route, name = "") => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, name)}`;

/** The address of a database's page. */
const hrefOf = (name) => `/databases/${encodeURIComponent(name)}`;

// The words the pages say where this test looks for them.
const READER_WORDS = "Only an administrator can list the databases: listing them signs in to PostgreSQL as its superuser.";
const LOCKED_WORDS = "Listing the databases signs in to PostgreSQL as its superuser, so the console has to be unlocked first.";
const ADD_READER_WORDS = "Only an administrator can add a database.";
const GUARD_WORDS = `The password of ${SHOP} is shown once. If you leave now it is lost, and you would have to make another.`;
const SECOND_STEP_WORDS = "Let other machines connect";

// --- Reading the machine ---

/** What `GET /api/1/databases` answers: `database list --json`; gives it whole. */
async function readDatabases(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(answer.status === 200 && Array.isArray(answer.json?.databases), `${ROUTES.list} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The database `name` of `read`, or undefined. */
const rowOf = (read, name) => read.databases.find((one) => one.name === name);

/** Reads the databases until `test(read)` holds, for up to `seconds`, and gives what was read. */
async function databasesBecome(page, test, what, seconds = 120) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const read = await readDatabases(page).catch(() => null);
    if (read !== null && test(read)) return read;
    ensure(Date.now() < until, `the machine did not ${what} within ${seconds} seconds`);
    await sleep(1500);
  }
}

/** What `GET /api/1/machine/name` says: the name other machines call this one by, `{name, first, ...}`. */
async function readMachine(page) {
  const answer = await fetchAsPage(page, "GET", "/api/1/machine/name");
  ensure(answer.status === 200 && typeof answer.json?.name === "string", `GET /api/1/machine/name was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. Chromium may read the clipboard back. */
async function signedIn(t, name) {
  const options = t.engine === "chromium" ? { permissions: ["clipboard-read", "clipboard-write"] } : {};
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The pages ---

const main = (page) => page.locator("main");
const cardOf = (page, heading) => page.locator("section.card").filter({ has: page.getByRole("heading", { level: 2, name: heading, exact: true }) });
const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
const dialogOf = (page, name) => page.getByRole("dialog", { name, exact: true });

/** The facts of the card "About this database", by their names: `{Owner: "shop_lab", ...}`. */
const factsOf = (page) =>
  cardOf(page, "About this database")
    .locator("dl.rows")
    .evaluate((dl) => {
      const facts = {};
      for (const term of dl.querySelectorAll("dt")) facts[term.textContent.trim()] = term.nextElementSibling.innerText.trim();
      return facts;
    });

/** Whether the unlock dialog came within a moment of a press, and answers it when it did. */
async function answerIfAsked(t, page) {
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  return asked;
}

/** Opens the list and waits for its rows, or for the words that stand in their place. */
async function openList(t, page) {
  await page.goto(new URL("/databases", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Databases", exact: true }));
  await page.waitForFunction(() => document.querySelectorAll(".db-list table, .db-list .db-empty, .db-place p").length > 0, null, { timeout: 30_000 });
}

/** Opens the page of the database `name` and waits for its facts. */
async function openDatabase(t, page, name) {
  await page.goto(new URL(hrefOf(name), page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name, exact: true }));
  await t.see(cardOf(page, "About this database"));
}

/** The rows of the page's table are the machine's: a link to each database's page, its owner, its sessions and the site it was made for. */
async function listAsAnswered(t, page, read) {
  const links = await page.locator('.db-list td[data-label="Database"] a').evaluateAll((all) => all.map((link) => link.getAttribute("href")));
  const wanted = read.databases.filter((one) => one.name !== undefined).map((one) => hrefOf(one.name));
  ensure(JSON.stringify([...links].sort()) === JSON.stringify([...wanted].sort()), `the page links ${JSON.stringify(links)} and ${ROUTES.list} gives ${JSON.stringify(wanted)}`);
  for (const one of read.databases) {
    const row = page.locator(".db-list tbody tr").filter({ has: page.locator(`td[data-label="Database"] a[href="${hrefOf(one.name)}"]`) });
    ensure((await row.locator('td[data-label="Owner"]').innerText()) === one.owner, `the row of ${one.name} does not say its owner ${one.owner}`);
    ensure((await row.locator('td[data-label="Sessions"]').innerText()) === String(one.sessions), `the row of ${one.name} does not say ${one.sessions} sessions`);
    if (typeof one.site === "string" && one.site !== "") ensure((await row.locator('td[data-label="Database"]').innerText()).includes(`for ${one.site}`), `the row of ${one.name} does not say its site`);
  }
  await t.see(page.getByText("PostgreSQL's own databases are not listed.", { exact: true }));
}

/** The line about the server says that it runs, and where it listens. */
async function serverLine(t, page, read) {
  await t.see(page.locator(".db-server").getByText("Running", { exact: true }));
  const line = await page.locator(".db-server").innerText();
  ensure(line.includes(read.server.listen === "localhost" ? "this machine only" : "network"), `the line about the server says ${JSON.stringify(line)}, and the machine says it listens on ${read.server.listen}`);
}

// --- Add a database ---

/** The words of the first row of what happens, and the form as it is filled from a site. */
async function formFromSite(t, page) {
  await page.goto(new URL(`/databases/add?site=${SITE}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a database", exact: true }));
  const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
  await t.see(crumbs.getByRole("link", { name: SITE, exact: true }));
  ensure((await crumbs.getByRole("link", { name: SITE, exact: true }).getAttribute("href")) === `/websites/${SITE}`, "the crumb of the site does not lead to its page");
  ensure((await page.getByLabel("Name", { exact: true }).inputValue()) === SHOP, `the name is ${await page.getByLabel("Name", { exact: true }).inputValue()}, not ${SHOP}`);
  ensure((await page.getByLabel("Owned by", { exact: true }).inputValue()) === SHOP, "the role is not named for the database");
  await t.see(page.getByText(`Makes the PostgreSQL role ${SHOP}, which signs in with a password, from this server only`, { exact: true }));
  await t.see(page.getByText("Restart: none; the database is ready at once.", { exact: false }));
  // Three ways to connect, the first chosen, and the one for other machines asks for networks.
  const cards = page.locator("label.radio-card");
  ensure((await cards.count()) === 3, `Who connects has ${await cards.count()} ways, not 3`);
  ensure(await cards.nth(0).locator("input").isChecked(), "the first way to connect is not chosen");
  await cards.nth(2).click();
  await t.see(page.getByLabel("Addresses", { exact: true }));
  await cards.nth(0).click();
  await t.gone(page.getByLabel("Addresses", { exact: true }));
  await lookAll(t, page, "databases-add");
}

/** A name that is taken is refused under its field before anything is sent. */
async function nameIsChecked(t, page) {
  const sent = [];
  const watch = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.add)) sent.push(request.url());
  };
  page.on("request", watch);
  const name = page.getByLabel("Name", { exact: true });
  await name.fill(BUSY);
  await buttonOf(page, "Add the database").click();
  await t.see(page.getByText(`A database or a role named ${BUSY} is there already.`, { exact: true }));
  await name.fill("Shop");
  await buttonOf(page, "Add the database").click();
  await t.see(page.getByText("Use lowercase letters, digits and _", { exact: false }));
  await sleep(1000);
  page.off("request", watch);
  ensure(sent.length === 0, `Add the database sent ${sent.join(", ")} with a name that is wrong`);
  await name.fill(SHOP);
}

/** The password of the dialog: the line "Password: …" of the text it shows. */
async function passwordOf(dialog) {
  const text = await dialog.locator("pre").first().innerText();
  const password = /^Password: (\S+)$/m.exec(text)?.[1];
  ensure(password !== undefined, `the dialog shows no password line: ${text}`);
  return { password, text };
}

/** The password is nowhere on the page but in the dialog that shows it. */
async function onlyInDialog(page, password) {
  const outside = await page.evaluate(
    (secret) => [...document.querySelectorAll("*")].filter((node) => !node.closest("dialog") && [...node.attributes].some((one) => one.value.includes(secret))).length +
      [...document.body.querySelectorAll("*")].filter((node) => node.children.length === 0 && !node.closest("dialog") && node.textContent.includes(secret)).length,
    password,
  );
  ensure(outside === 0, `the password is in ${outside} places of the page outside the dialog that shows it`);
}

/** Add a database: the password is shown once, can be copied and saved, and is guarded while it is shown. Gives the password that was shown. */
async function addTheDatabase(t, page) {
  const added = await pressAndAnswer(t, page, buttonOf(page, "Add the database"), pathOf(ROUTES.add));
  // A change that answers with a secret: {done, changed, output}, the database's JSON on one line of output.
  const answer = await added.json();
  const made = (answer.output ?? []).map((line) => { try { return JSON.parse(line); } catch { return null; } }).find((one) => typeof one?.password === "string");
  ensure(made !== undefined && made.database === SHOP && made.role === SHOP, `the answer of ${ROUTES.add} holds no password of ${SHOP}: ${JSON.stringify({ ...answer, output: "…" })}`);

  const shown = dialogOf(page, `The password of ${SHOP}`);
  await t.see(shown);
  const { password, text } = await passwordOf(shown);
  ensure(password === made.password, "the password that is shown is not the one the answer holds");
  ensure(/^Host: 127\.0\.0\.1$/m.test(text) && /^Port: 5432$/m.test(text) && text.includes(`Database: ${SHOP}`), `the settings that are shown with the password are ${JSON.stringify(text)}`);
  await t.see(shown.getByText("It is shown only now: copy it or save it.", { exact: false }));
  await onlyInDialog(page, password);
  await lookAll(t, page, "databases-password");

  // The steps behind the dialog say that the first is done.
  ensure((await page.locator('.flow-step[data-state="done"]').count()) === 1, "the step that makes the database is not marked done");

  // Copy: the text on the clipboard is the text shown, where the engine allows it to be read.
  await buttonOf(shown, "Copy").click();
  await t.see(shown.getByRole("status").filter({ hasText: /Copied\.|Press Ctrl\+C to copy\./ }));
  if (t.engine === "chromium") {
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    ensure(copied.trim() === text.trim(), "Copy put another text on the clipboard than the one shown");
  }

  // Save as file: the file is the settings with the password.
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), buttonOf(shown, "Save as file").click()]);
  ensure(download.suggestedFilename() === `${SHOP}-postgresql.txt`, `the file is saved as ${download.suggestedFilename()}, not ${SHOP}-postgresql.txt`);
  const file = await download.path();
  ensure(file !== null && (await readFile(file, "utf8")).trim() === text.trim(), "the saved file is not the text that was shown");

  // The leave guard: going back while the password is shown asks first, and staying keeps it shown.
  await page.evaluate(() => history.back());
  const guard = dialogOf(page, "Leave this page?");
  await t.see(guard);
  await t.see(guard.getByText(GUARD_WORDS, { exact: true }));
  await buttonOf(guard, "Stay on this page").click();
  await t.gone(guard);
  ensure(new URL(page.url()).pathname === "/databases/add", `the page was left for ${page.url()} though Stay on this page was pressed`);
  await t.see(shown);

  await buttonOf(shown, "I have kept it").click();
  await t.gone(shown);
  ensure(!(await page.content()).includes(password), "the password is still in the page after the dialog was closed");
  await t.see(page.getByText(`${SHOP} is added.`, { exact: true }));
  await t.see(page.getByText("Its password was shown once and is kept nowhere on this server.", { exact: true }));
  // The settings stay, and the examples read the password from where it is kept.
  await t.see(cardOf(page, "Connection settings"));
  await t.see(page.getByText(`dbname=${SHOP}`, { exact: false }).first());
  ensure((await cardOf(page, "Connection settings").innerText()).includes("$password"), "the example of the settings does not read the password from a variable");
  await lookAll(t, page, "databases-added");

  // With the guard let go of, a link leaves at once.
  const link = page.getByRole("link", { name: `Open ${SHOP}`, exact: true });
  ensure((await link.getAttribute("href")) === hrefOf(SHOP), `Open ${SHOP} leads to ${await link.getAttribute("href")}`);
  const read = await readDatabases(page);
  const one = rowOf(read, SHOP);
  ensure(one !== undefined && one.owner === SHOP && one.site === SITE, `${ROUTES.list} says ${JSON.stringify(one)} of ${SHOP}, which was added for ${SITE}`);
  return password;
}

// --- A database's page ---

/** The facts, who connects and the settings of a database's page are the machine's. */
async function databaseAsAnswered(t, page, name, read) {
  const one = rowOf(read, name);
  ensure(one !== undefined, `${ROUTES.list} does not list ${name}`);
  const facts = await factsOf(page);
  ensure(facts.Owner === one.owner, `the page says ${JSON.stringify(facts.Owner)} of the owner, and the machine ${one.owner}`);
  ensure(facts.Sessions === String(one.sessions), `the page says ${JSON.stringify(facts.Sessions)} of the sessions, and the machine ${one.sessions}`);
  if (typeof one.site === "string" && one.site !== "") ensure(facts.For === one.site, `the page says ${JSON.stringify(facts.For)} of the site`);
  await t.see(cardOf(page, "Who connects").getByText("Programs on this server, with the password", { exact: true }));
  const settings = cardOf(page, "Connection settings");
  await t.see(settings.getByRole("heading", { level: 3, name: "Programs on this server, with a password", exact: true }));
  const text = await settings.innerText();
  ensure(text.includes(`dbname=${name}`) && text.includes("$password") && text.includes("127.0.0.1"), `the settings of ${name} say ${JSON.stringify(text.slice(0, 200))}`);
}

/** Let other machines connect: a task that the page follows, and the machine says that PostgreSQL listens on the network, with TLS, for the network that was named. */
async function letThemConnect(t, page) {
  await buttonOf(cardOf(page, "Who connects"), "Let other machines connect").click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Let other machines connect to ${SHOP}`);
  await t.see(dialog);
  await t.see(dialog.getByText(/PostgreSQL (restarts once to listen on the network|is listening on the network already)/));
  await lookAll(t, page, "databases-allow");
  // Nothing is sent for a network that is not one, or that says every address.
  await buttonOf(dialog, "Let them connect").click();
  await t.see(dialog.getByText("Give at least one network", { exact: false }));
  await dialog.getByLabel("Addresses", { exact: true }).fill("192.0.2.0");
  await buttonOf(dialog, "Let them connect").click();
  await t.see(dialog.getByText("192.0.2.0 is not an address with its prefix length", { exact: false }));
  await dialog.getByLabel("Addresses", { exact: true }).fill(NETWORK);

  const path = pathOf(ROUTES.allow, SHOP);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === path);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path);
  await buttonOf(dialog, "Let them connect").click();
  await answerIfAsked(t, page);
  const body = (await sent).postData() ?? "";
  ensure(body.includes(NETWORK) && body.includes("this-machine"), `${ROUTES.allow} was sent ${JSON.stringify(body)}, not ${NETWORK} with this machine's certificate`);
  const status = (await answered).status();
  ensure(status === 200 || status === 202, `${ROUTES.allow} was answered ${status}`);

  // The page follows the task as a step, and ends it with the read of the machine.
  await t.see(page.locator(".db-flow").getByRole("heading", { level: 2, name: `Letting other machines connect to ${SHOP}`, exact: true }));
  await page.waitForSelector('.db-flow .flow-step[data-state="done"], .db-flow .flow-step[data-state="failed"]', { timeout: 240_000 });
  ensure((await page.locator('.db-flow .flow-step').first().getAttribute("data-state")) === "done", `the step "${SECOND_STEP_WORDS}" failed: ${await page.locator(".db-flow").innerText()}`);
  const read = await databasesBecome(page, (one) => one.server?.ssl === true && (one.access ?? []).some((line) => line.address === NETWORK && (line.databases ?? []).includes(SHOP)), `let ${NETWORK} in to ${SHOP}`);
  ensure(read.server.listen !== "localhost", `PostgreSQL listens on ${read.server.listen} after other machines were let in`);

  // The page says so: the line, who connects, and the settings for other machines with the certificate to download.
  await t.see(cardOf(page, "Who connects").getByText(`${NETWORK}, over TLS with the password`, { exact: true }));
  const settings = cardOf(page, "Connection settings");
  await t.see(settings.getByRole("heading", { level: 3, name: "Programs on other machines, with a password", exact: true }));
  ensure((await settings.innerText()).includes("sslmode=verify-full"), "the settings for other machines do not say sslmode=verify-full");
  const machine = await readMachine(page);
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), buttonOf(settings, `Download the certificate (${machine.first}.pem)`).click()]);
  ensure(download.suggestedFilename() === `${machine.first}.pem`, `the certificate is saved as ${download.suggestedFilename()}, not ${machine.first}.pem`);
  const pem = await readFile(await download.path(), "utf8");
  ensure(pem.includes("-----BEGIN CERTIFICATE-----"), "the file that was saved is not a certificate");
  await lookAll(t, page, "databases-network");
}

/** Remove beside the network stops it connecting: a task, and the machine goes back to listening on this machine only. */
async function stopThem(t, page) {
  await buttonOf(page, `Remove ${NETWORK}`).click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Stop ${NETWORK} connecting to ${SHOP}?`);
  await t.see(dialog);
  await t.see(dialog.getByText("goes back to listening on this machine only", { exact: false }));
  await lookAll(t, page, "databases-stop");
  const path = pathOf(ROUTES.deny, SHOP);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path);
  await buttonOf(dialog, `Stop ${NETWORK}`).click();
  await answerIfAsked(t, page);
  const status = (await answered).status();
  ensure(status === 200 || status === 202, `${ROUTES.deny} was answered ${status}`);
  await page.waitForSelector('.db-flow .flow-step[data-state="done"], .db-flow .flow-step[data-state="failed"]', { timeout: 240_000 });
  ensure((await page.locator(".db-flow .flow-step").first().getAttribute("data-state")) === "done", `the step that stops ${NETWORK} failed: ${await page.locator(".db-flow").innerText()}`);
  const read = await databasesBecome(page, (one) => !(one.access ?? []).some((line) => line.address === NETWORK), `stop ${NETWORK} connecting to ${SHOP}`);
  ensure(read.server.listen === "localhost", `PostgreSQL listens on ${read.server.listen} after the last network was stopped`);
  await t.gone(cardOf(page, "Who connects").getByText(NETWORK, { exact: false }));
  await t.gone(cardOf(page, "Connection settings").getByRole("heading", { level: 3, name: "Programs on other machines, with a password", exact: true }));
}

/** Make a new password asks first, and shows another password once, with nothing of it left in the page. */
async function newPassword(t, page, old) {
  await buttonOf(page, "Make a new password").click();
  await answerIfAsked(t, page);
  const question = dialogOf(page, `Make a new password for ${SHOP}?`);
  await t.see(question);
  await t.see(question.getByText("Programs that use the old one stop signing in until they have the new one.", { exact: true }));
  await lookAll(t, page, "databases-new-password");
  const path = pathOf(ROUTES.password, SHOP);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path);
  await buttonOf(question, "Make a new password").click();
  await answerIfAsked(t, page);
  ensure((await answered).status() === 200, `${ROUTES.password} was not answered 200`);
  const shown = dialogOf(page, `The password of ${SHOP}`);
  await t.see(shown);
  const { password } = await passwordOf(shown);
  ensure(password !== old, "the new password is the old one");
  await onlyInDialog(page, password);
  await buttonOf(shown, "I have kept it").click();
  await t.gone(shown);
  ensure(!(await page.content()).includes(password) && !(await page.content()).includes(old), "a password is still in the page after the dialog was closed");
}

/** Remove… asks for the name, and sends nothing without it; the role that the database made goes with it when it is ticked. */
async function removeShop(t, page) {
  const sent = [];
  const watch = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.remove, SHOP)) sent.push(request.postData() ?? "");
  };
  page.on("request", watch);
  await buttonOf(page, "Remove…").click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Remove ${SHOP}?`);
  await t.see(dialog);
  await t.see(dialog.getByText(`Removes the database ${SHOP} and everything in it.`, { exact: false }));
  ensure(await dialog.getByLabel(`Also remove the role ${SHOP}`).isChecked(), "the role that the database made is not ticked");
  const remove = buttonOf(dialog, "Remove the database");
  ensure(await remove.isDisabled(), "Remove the database works before the name is typed");
  const typed = dialog.getByLabel(`Type ${SHOP} to go on`);
  await typed.fill(SHOP.slice(0, -1));
  ensure(await remove.isDisabled(), "Remove the database works with a name that is not the database's");
  await lookAll(t, page, "databases-remove");
  await typed.fill(SHOP);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === pathOf(ROUTES.remove, SHOP));
  await remove.click();
  await answerIfAsked(t, page);
  ensure((await answered).status() === 200, `${ROUTES.remove} for ${SHOP} was not answered 200`);
  page.off("request", watch);
  ensure(sent.length === 1 && /"role"\s*:\s*true/.test(sent[0]) && !/"force"/.test(sent[0]), `${ROUTES.remove} was sent ${JSON.stringify(sent)}, not the database with its role and no force`);
  await page.waitForURL((url) => url.pathname === "/databases", { timeout: 30_000 });
  await databasesBecome(page, (one) => rowOf(one, SHOP) === undefined, `remove ${SHOP}`);
  await t.gone(page.locator(`.db-list a[href="${hrefOf(SHOP)}"]`));
}

/** A database with a session open is refused by the machine unless it is told to end it, and the page tells it to. */
async function removeBusy(t, page) {
  await openDatabase(t, page, BUSY);
  const read = await readDatabases(page);
  ensure((rowOf(read, BUSY)?.sessions ?? 0) >= 1, `${BUSY} has ${rowOf(read, BUSY)?.sessions} sessions: databases.sh holds one open`);
  ensure((await factsOf(page)).Sessions !== "0", `the page says no session is open on ${BUSY}`);
  await t.see(cardOf(page, "About this database").getByText(/\d+ programs? (is|are) connected now\./));
  // Without force the machine refuses, and changes nothing.
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.remove, BUSY), {}), 409, "DB-103", `the removal of ${BUSY} without force`);
  ensure(rowOf(await readDatabases(page), BUSY) !== undefined, `${BUSY} is gone though its removal was refused`);

  await buttonOf(page, "Remove…").click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Remove ${BUSY}?`);
  await t.see(dialog);
  await t.see(dialog.getByText(/Ends its \d+ open sessions?\./));
  await dialog.getByLabel(`Type ${BUSY} to go on`).fill(BUSY);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === pathOf(ROUTES.remove, BUSY));
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.remove, BUSY));
  await buttonOf(dialog, "Remove the database").click();
  await answerIfAsked(t, page);
  ensure(/"force"\s*:\s*true/.test((await sent).postData() ?? ""), `${ROUTES.remove} for ${BUSY} was not sent with force`);
  ensure((await answered).status() === 200, `${ROUTES.remove} for ${BUSY} was not answered 200`);
  await page.waitForURL((url) => url.pathname === "/databases", { timeout: 30_000 });
  await databasesBecome(page, (one) => rowOf(one, BUSY) === undefined, `remove ${BUSY}`);
}

/** A database that is not there says so and leads back to the list. */
async function notThere(t, page) {
  await page.goto(new URL(hrefOf(NOBODY), page.url()).href);
  await t.see(page.getByText(`There is no database named ${NOBODY}.`, { exact: true }));
  const link = page.getByRole("link", { name: "All the databases", exact: true });
  await t.see(link);
  ensure((await link.getAttribute("href")) === "/databases", "All the databases does not lead to /databases");
  await lookAll(t, page, "databases-missing");
}

// --- The people and the phone ---

/** An administrator: the list of a locked console, Add a database from a site, the password shown once, the settings, other machines let in and stopped, a new password, and both databases removed. */
async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;

  // A locked console is asked to unlock before the list is read.
  await page.goto(new URL("/databases", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Databases", exact: true }));
  await t.see(page.getByText(LOCKED_WORDS, { exact: true }));
  await lookAll(t, page, "databases-locked");
  await buttonOf(page, "Unlock").click();
  await answerUnlock(t, page);
  await page.waitForFunction(() => document.querySelectorAll(".db-list table").length > 0, null, { timeout: 30_000 });
  let read = await readDatabases(page);
  await listAsAnswered(t, page, read);
  await serverLine(t, page, read);
  ensure(rowOf(read, BUSY) !== undefined && rowOf(read, SHOP) === undefined, `${ROUTES.list} does not list ${BUSY} alone, which databases.toml made`);
  await lookAll(t, page, "databases-list");

  // Add a database, from the list.
  await page.getByRole("link", { name: "Add a database", exact: true }).first().click();
  await page.waitForURL((url) => url.pathname === "/databases/add");
  await formFromSite(t, page);
  await nameIsChecked(t, page);
  const old = await addTheDatabase(t, page);

  // Its page.
  await page.getByRole("link", { name: `Open ${SHOP}`, exact: true }).click();
  await page.waitForURL((url) => url.pathname === hrefOf(SHOP));
  await t.see(cardOf(page, "About this database"));
  read = await readDatabases(page);
  await databaseAsAnswered(t, page, SHOP, read);
  await lookAll(t, page, "databases-database");

  await letThemConnect(t, page);
  await stopThem(t, page);
  await newPassword(t, page, old);
  await removeShop(t, page);
  await removeBusy(t, page);
  await lookAll(t, page, "databases-list-after");
  await notThere(t, page);

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.list),
    routeFor(ROUTES.add),
    routeFor(ROUTES.password, SHOP),
    routeFor(ROUTES.allow, SHOP),
    routeFor(ROUTES.deny, SHOP),
    routeFor(ROUTES.remove, SHOP),
    routeFor(ROUTES.remove, BUSY),
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader is told who may list the databases, can press nothing that works, and is refused when a route is asked. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openList(t, page);
  await t.see(page.getByText(READER_WORDS, { exact: true }));
  ensure((await page.locator(".db-list table").count()) === 0, "a reader's list has a table");
  await lookAll(t, page, "databases-reader");
  await openDatabase(t, page, BUSY).catch(() => null);
  await t.see(page.getByText(READER_WORDS, { exact: true }));
  ensure((await page.getByRole("button", { name: "Remove…", exact: true }).count()) === 0, "a reader's page of a database has Remove…");
  await page.goto(new URL("/databases/add", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a database", exact: true }));
  await t.see(page.getByText(ADD_READER_WORDS, { exact: true }));
  ensure(await buttonOf(page, "Add the database").isDisabled(), "a reader's Add the database is not off");
  await lookAll(t, page, "databases-reader-add");
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.list)), 403, "ADM-122", `a reader's ${ROUTES.list}`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.add), { name: "lab_reader" }), 403, "ADM-122", `a reader's ${ROUTES.add}`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.remove, BUSY), { force: true }), 403, "ADM-122", `a reader's removal of ${BUSY}`);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list and a database alone with a way back, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const controls = "main .db-page button, main .db-page a.btn, main .db-list td[data-label=\"Database\"] a";
  await openList(t, page);
  await page.waitForFunction(() => document.querySelectorAll(".db-list table").length > 0, null, { timeout: 30_000 });
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, controls);
  ensure(small.length === 0, `these controls of the phone's list are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "databases-phone-list");

  await openDatabase(t, page, BUSY);
  await t.see(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Databases", exact: true }));
  ensure((await sidewaysScroll(page)) === 0, `the phone's database scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallOne = await smallControls(page, controls);
  ensure(smallOne.length === 0, `these controls of the phone's database are under 44 px tall: ${smallOne.join(", ")}`);
  await lookAll(t, page, "databases-phone-database");

  await page.goto(new URL("/databases/add", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a database", exact: true }));
  ensure((await sidewaysScroll(page)) === 0, `the phone's Add a database scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallAdd = await smallControls(page, controls);
  ensure(smallAdd.length === 0, `these controls of the phone's Add a database are under 44 px tall: ${smallAdd.join(", ")}`);
  await lookAll(t, page, "databases-phone-add");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The reader and the phone come first: the administrator takes both databases away.
  await reader(t);
  await phone(t);
  await administrator(t);
}
