// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/accounts drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Accounts
// page at /accounts and one account at /accounts/{name} (design 7), and Your
// account at /account, which draws the same rows for the person's own
// account. The list shows what `user list --json` says (`GET
// /api/1/accounts`) and, to an administrator who has unlocked, what `user
// list --json` says as root, with which accounts are disabled (`GET
// /api/1/accounts/list/all`), and what `lastlog --json` says (`GET
// /api/1/sign-ins/last`). An account is read with `user
// show --json` (`GET /api/1/accounts/{name}`) and changed by Add an account
// (`user add`, `POST /api/1/accounts/add`, the password on the command's
// input), the rows' controls (`user set`, `POST /api/1/accounts/{name}/set`),
// Change… (`passwd`, `POST /api/1/accounts/{name}/password`), Disable and
// Enable (`user disable` and `user enable`, `POST
// /api/1/accounts/{name}/disable` and `/enable`), Remove the account…
// (`user remove`, `POST /api/1/accounts/{name}/remove`), the SSH keys (`user
// key add` and `user key remove`, `POST /api/1/accounts/{name}/keys/add` and
// `/keys/remove`), Revoke (`user token remove`, `POST
// /api/1/accounts/{name}/tokens/remove`), Remove second steps… (`user steps
// reset`, `POST /api/1/accounts/{name}/second-step/reset`) and End for a
// session at the machine (`user end`, `POST /api/1/accounts/{name}/end`). The
// person's own account is read by `GET /api/1/account/show` and changed by
// `POST /api/1/account/name`, `/password`, `/keys/add` and `/keys/remove`,
// which the person makes whatever their standing.
//
// The machine has the accounts that accounts.toml made. The page makes two
// more: an account with a name, a full name, a password and a key, which
// accounts.toml reads on the machine after (the key file, its modes and its
// owner), and an account with no password and one key, whose key is its only
// way in and is removed only with the account's name typed. Two browsers are
// used at once, as people are: the administrator's, and the browser of the
// account the administrator changes, whose session ends when the account is
// disabled and whose sign-in is refused while its console right is off or the
// account is disabled. The administrator's own account is held by the
// guards of design 7.6: the console does not disable it, remove it or take its
// administrator right away, and its page says where that is done; the routes
// answer ADM-136. The password goes in the call that sets it and nowhere else:
// not in an answer and not in the page. A reader sees Account and Can, the row
// of another account and their own account whole; a phone at 390 by 844 shows
// each page in one column. Each look is the console's own: dark, light, high
// contrast, forced colours and less motion, each checked by axe (console.mjs's
// lookAll). The sessions and terminals of an account, which wait on the
// sessions route (13.3), are accounts-sessions.mjs, a step of their own.

import { createHash, generateKeyPairSync } from "node:crypto";

import {
  addApp,
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  openSecondStep,
  sessionOf,
  sidewaysScroll,
  signIn,
  signOut,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, smallControls, unlockNow } from "./trial.mjs";

// The passwords that the page gives: the first is the one that Add an account
// gives, the second the one that Change… sets and that the reader changes to.
const FIRST = "lab-first-password-1";
const SECOND = "lab-second-password-2";

// The full name that Add an account gives a person, and the one that the
// reader gives themselves on their own page.
const FRESH_FULL = "Lab New";
const READER_FULL = "Lab Reader";

// The comments of the keys, which accounts.toml looks for in the files after.
const KEY_ONE = "lab-key-one";
const KEY_TWO = "lab-key-two";
const KEY_ONLY = "lab-key-only";
const KEY_READER = "lab-key-reader";

/**
 * The accounts of one engine. accounts.toml makes `admin`, `reader` and
 * `withApp`; the page makes `fresh` and `keyOnly`. Each engine has accounts
 * of its own so that an engine that fails halfway does not leave the next
 * with a changed password.
 */
const namesOf = (engine) => ({
  admin: `wc-acc-${engine}`,
  reader: `wc-acc-r-${engine}`,
  withApp: `wc-acc-s-${engine}`,
  fresh: `wc-acc-n-${engine}`,
  keyOnly: `wc-acc-k-${engine}`,
});

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browsers must have called each by the end, with any
// name for {name}.
const ROUTES = {
  list: "GET /api/1/accounts",
  listAll: "GET /api/1/accounts/list/all",
  last: "GET /api/1/sign-ins/last",
  show: "GET /api/1/accounts/{name}",
  add: "POST /api/1/accounts/add",
  set: "POST /api/1/accounts/{name}/set",
  password: "POST /api/1/accounts/{name}/password",
  disable: "POST /api/1/accounts/{name}/disable",
  enable: "POST /api/1/accounts/{name}/enable",
  remove: "POST /api/1/accounts/{name}/remove",
  keyAdd: "POST /api/1/accounts/{name}/keys/add",
  keyRemove: "POST /api/1/accounts/{name}/keys/remove",
  revoke: "POST /api/1/accounts/{name}/tokens/remove",
  reset: "POST /api/1/accounts/{name}/second-step/reset",
  end: "POST /api/1/accounts/{name}/end",
  ownShow: "GET /api/1/account/show",
  ownName: "POST /api/1/account/name",
  ownPassword: "POST /api/1/account/password",
  ownKeyAdd: "POST /api/1/account/keys/add",
  ownKeyRemove: "POST /api/1/account/keys/remove",
};

/** The path of a route as the table names it, for the account `name`. */
const pathOf = (route, name = "") => route.slice(route.indexOf(" ") + 1).replace("{name}", encodeURIComponent(name));

/** A pattern for what `track` writes of `route`, with any account for {name} ("last" is the route's own word, no account's). */
const patternOf = (route) => {
  const escaped = route.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${escaped.replace("\\{name\\}", "(?!last(?:/|$))[^/]+")}$`);
};

// What the browsers called, over all of them, for the check at the end.
const called = new Set();
const remember = (seen) => {
  for (const route of seen) called.add(route);
};

// --- Keys ---

/** A public key as ssh-keygen writes it for a key made now, with its fingerprint whole and as the page shortens it. */
function makeKey(comment) {
  const raw = generateKeyPairSync("ed25519").publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  const part = (bytes) => Buffer.concat([Buffer.from([0, 0, 0, bytes.length]), bytes]);
  const blob = Buffer.concat([part(Buffer.from("ssh-ed25519")), part(raw)]);
  const whole = `SHA256:${createHash("sha256").update(blob).digest("base64").replace(/=+$/, "")}`;
  return { line: `ssh-ed25519 ${blob.toString("base64")} ${comment}`, whole, short: `${whole.slice(0, 11)}…${whole.slice(-3)}` };
}

// --- Signing in ---

/** Opens the console and signs in as `name` with `password`. Gives the page, what it calls and the answer. */
async function signInAs(t, name, password, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  const answer = await signIn(page, name, password);
  return { page, seen, answer };
}

/** Signs in as `name`, which must work, and gives the time zone that the welcome page asks for. */
async function signedIn(t, name, password = PASSWORD) {
  const { page, seen, answer } = await signInAs(t, name, password);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await leaveWelcome(t, page);
  return { page, seen };
}

/** Signs in as `name` with `password` in a browser of its own, which the console must refuse. Closes the browser. */
async function mustBeRefused(t, name, password) {
  const { page, answer } = await signInAs(t, name, password);
  ensure(answer.status() !== 200, `${name} was signed in with a password that the console should have refused`);
  await page.context().close();
}

/** What the console answers a signed-in page that asks for its session: 200, or 401 once it has ended. */
const statusOf = async (page) => (await page.request.get(new URL("/api/1/session", page.url()).href)).status();

// --- The pages ---

const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
const dialogOf = (page, name) => page.getByRole("dialog", { name, exact: true });

/** Opens the list of accounts, or the page of the account `name`, and waits until it has been read. `heading` is the page's own when it is not the name. */
async function openAccounts(t, page, name = null, heading = name) {
  await page.goto(new URL(name === null ? "/accounts" : `/accounts/${encodeURIComponent(name)}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: name === null ? "Accounts" : heading, level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelector(".acc-loading") === null, null, { timeout: 30_000 });
}

/** Opens Your account and waits until it has been read. */
async function openYours(t, page) {
  await page.goto(new URL("/account", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Your account", level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelector(".acc-loading") === null && document.querySelector("#display") !== null, null, { timeout: 30_000 });
}

/** The row of the list that is `name`'s. */
const rowOf = (page, name) => page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });

/** The words of the row `label` of an account's list of rows. */
async function rowText(page, label) {
  const row = page.locator("dl.rows dt").filter({ hasText: new RegExp(`^${label}$`) }).locator("xpath=following-sibling::dd[1]");
  return (await row.innerText()).replace(/\s+/g, " ").trim();
}

/** What the console answers for `GET /api/1/accounts/NAME`: what `user show --json` says. */
async function readAccount(page, name) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.show, name));
  ensure(answer.status === 200 && answer.json?.account !== undefined, `${ROUTES.show} for ${name} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Reads the account until `test(shown)` holds, for up to `seconds`: the page and the machine are a moment apart. */
async function accountBecomes(page, name, test, what, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const shown = await readAccount(page, name).catch(() => null);
    if (shown !== null && test(shown)) return shown;
    ensure(Date.now() < until, `${name} did not ${what} within ${seconds} seconds`);
    await sleep(1000);
  }
}

/** Whether the account is disabled, as `user show` says it: `disabled` is a value that says since when, or false. */
const isDisabled = (shown) => shown.account.disabled != null && shown.account.disabled !== false;

/** Presses the confirming button `button` of `dialog` and gives the answer to the POST to `route` that it makes. */
async function confirm(page, dialog, button, route) {
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === route);
  await buttonOf(dialog, button).click();
  return answered;
}

/** Throws unless `answer`, a response, has status 200; gives it. */
async function mustSucceed(answer, what) {
  ensure(answer.status() === 200, `${what} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

// --- The list ---

/** The list holds what `GET /api/1/accounts` says, and, once the console is unlocked, when each was last active. */
async function theList(t, page, who) {
  await openAccounts(t, page);
  const read = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(read.status === 200 && Array.isArray(read.json), `${ROUTES.list} was answered ${read.status}: ${read.text}`);
  const names = read.json.map((account) => account.name);
  for (const name of [who.admin, who.reader, who.withApp]) ensure(names.includes(name), `${ROUTES.list} has no ${name}: ${names.join(", ")}`);
  for (const name of names) await t.see(page.getByRole("link", { name, exact: true }).first());
  await t.see(rowOf(page, who.admin).getByText("Your account", { exact: true }));
  await t.see(rowOf(page, who.admin).getByText("Administrator", { exact: true }));
  await t.see(page.getByRole("columnheader", { name: "Last active", exact: true }));
  await t.see(rowOf(page, who.admin).getByText("Now, this browser", { exact: true }));
  await t.see(page.getByText(/A session lasts \d+ hours?\./));
  await t.see(page.getByText("Service accounts made by apps (postgres, nginx and others) are on each app's page."));
  // The groups are chips with their counts, and no link: no page manages them.
  const groups = page.getByRole("group", { name: "Groups", exact: true });
  await t.see(groups.getByText(/^admins · \d+$/));
  ensure((await groups.getByRole("link").count()) === 0, "a group of the line of groups is a link");
  const last = await fetchAsPage(page, "GET", pathOf(ROUTES.last));
  ensure(last.status === 200 && Array.isArray(last.json), `${ROUTES.last} was answered ${last.status}: ${last.text}`);
  // The list as root says which accounts are disabled and have a password, which the person's list leaves null.
  const all = await fetchAsPage(page, "GET", pathOf(ROUTES.listAll));
  ensure(all.status === 200 && Array.isArray(all.json), `${ROUTES.listAll} was answered ${all.status}: ${all.text}`);
  const mine = all.json.find((account) => account.name === who.admin);
  ensure(mine?.disabled === false && mine?.password === "set", `${ROUTES.listAll} has ${JSON.stringify(mine)} for ${who.admin}, which is enabled and has a password`);
  await lookAll(t, page, "accounts-list");
}

// --- An account is added ---

/** Add an account… makes an account with a full name and a password, refuses what it should first, and offers a key for it. */
async function addFresh(t, page, who, key) {
  await openAccounts(t, page);
  await buttonOf(page, "Add an account…").click();
  const dialog = dialogOf(page, "Add an account");
  await t.see(dialog);
  const name = dialog.getByLabel("Name", { exact: true });

  // A name that cannot be one, and one that is one already, are told under the field and ask nothing.
  await name.fill("Bad Name");
  await buttonOf(dialog, "Add").click();
  await t.see(dialog.getByText("Use lowercase letters, digits, _, - and ., starting with a letter."));
  await name.fill(who.admin);
  await buttonOf(dialog, "Add").click();
  await t.see(dialog.getByText(`An account is already called ${who.admin}.`));

  await name.fill(who.fresh);
  await dialog.getByLabel("Full name", { exact: true }).fill(FRESH_FULL);
  // Two passwords that differ ask nothing either.
  await dialog.getByLabel("Password", { exact: true }).fill(FIRST);
  await dialog.getByLabel("Password, again", { exact: true }).fill(SECOND);
  await buttonOf(dialog, "Add").click();
  await t.see(dialog.getByText("The two are not the same"));
  await dialog.getByLabel("Password, again", { exact: true }).fill(FIRST);
  const added = await mustSucceed(await confirm(page, dialog, "Add", pathOf(ROUTES.add)), ROUTES.add);
  ensure(!(await added.text()).includes(FIRST), "the answer of Add an account holds the password");

  // The dialog that offers a key checks the line as authd reads it before it asks.
  const offer = dialogOf(page, `Add an SSH key for ${who.fresh}`);
  await t.see(offer);
  await offer.getByLabel("Public key", { exact: true }).fill("ssh-rsa AAAAB3NzaC1yc2E");
  await buttonOf(offer, "Add the key").click();
  await t.see(offer.getByText(/Dukovo takes Ed25519 and ECDSA keys/));
  await offer.getByLabel("Public key", { exact: true }).fill(key.line);
  await mustSucceed(await confirm(page, offer, "Add the key", pathOf(ROUTES.keyAdd, who.fresh)), ROUTES.keyAdd);
  await t.gone(offer);
  await t.see(page.getByRole("heading", { name: who.fresh, level: 1, exact: true }));

  // What the machine has: the full name, the one key, the console right.
  const shown = await accountBecomes(page, who.fresh, (one) => one.keys.length === 1, "have its key");
  ensure(shown.account.full_name === FRESH_FULL, `${who.fresh} has the full name "${shown.account.full_name}"`);
  ensure(shown.account.console === true, `${who.fresh} has no console right after Add an account with Can sign in on`);
  ensure(shown.keys[0].fingerprint === key.whole, `the key of ${who.fresh} is ${shown.keys[0].fingerprint}, not ${key.whole}`);
}

/** The new account signs in to the console, in a browser of its own, with the password that it was given. */
async function firstSignIn(t, who) {
  const { page, answer } = await signInAs(t, who.fresh, FIRST);
  ensure(answer.status() === 200, `${who.fresh} was not signed in with the password it was given: ${answer.status()}`);
  const session = await sessionOf(page);
  ensure(session.name === who.fresh, `the session of the first sign-in is ${session.name}'s, not ${who.fresh}'s`);
  await page.context().close();
}

/** One account's rows and sections, held to `user show`. */
async function oneAccount(t, page, who, key) {
  await openAccounts(t, page, who.fresh);
  const shown = await readAccount(page, who.fresh);
  await t.see(page.getByText(new RegExp(`^Account ${shown.account.uid}`)));
  ensure((await rowText(page, "Full name")).startsWith(FRESH_FULL), "the row Full name does not say the full name");
  ensure((await rowText(page, "Second step")).startsWith("None"), "the row Second step does not say None for an account with none");
  ensure((await rowText(page, "Password")).includes("Never expires. Dukovo never forces a change."), "the row Password does not say it never expires");
  await t.see(page.locator("#keys").getByText(KEY_ONE, { exact: true }));
  await t.see(page.locator("#keys").getByText(key.short));
  await t.see(page.locator("#tokens").getByText(`${who.fresh} has no API token.`));
  await lookAll(t, page, "accounts-one");
}

// --- The console right and the password ---

/** Can sign in to Dukovo Admin off: a new sign-in is refused; on again, it is let in. */
async function consoleRight(t, page, who) {
  await unlockNow(page);
  await openAccounts(t, page, who.fresh);
  const toggle = page.getByRole("switch", { name: "Can sign in to Dukovo Admin", exact: true });
  await t.see(toggle);
  ensure((await toggle.getAttribute("aria-checked")) === "true", `Can sign in to Dukovo Admin is off for ${who.fresh}, which Add an account gave it`);
  const off = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.set, who.fresh));
  await toggle.click();
  await mustSucceed(await off, ROUTES.set);
  await accountBecomes(page, who.fresh, (one) => one.account.console === false, "lose its console right");
  await mustBeRefused(t, who.fresh, FIRST);

  await openAccounts(t, page, who.fresh);
  const on = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.set, who.fresh));
  await page.getByRole("switch", { name: "Can sign in to Dukovo Admin", exact: true }).click();
  await mustSucceed(await on, ROUTES.set);
  await accountBecomes(page, who.fresh, (one) => one.account.console === true, "have its console right back");
  const again = await signInAs(t, who.fresh, FIRST);
  ensure(again.answer.status() === 200, `${who.fresh} was not let in with its console right on: ${again.answer.status()}`);
  await again.page.context().close();
}

/** Change… sets a password for another account, typed twice: the old one is refused, the new one works, and the page holds neither. */
async function newPassword(t, page, who) {
  await unlockNow(page);
  await openAccounts(t, page, who.fresh);
  await buttonOf(page, `Change the password of ${who.fresh}`).click();
  const dialog = dialogOf(page, `Change the password of ${who.fresh}`);
  await t.see(dialog);
  await t.see(dialog.getByText(`${who.fresh} is not told the new password: give it to them yourself.`));
  await dialog.getByLabel("New password", { exact: true }).fill(SECOND);
  await dialog.getByLabel("New password, again", { exact: true }).fill(SECOND);
  const changed = await mustSucceed(await confirm(page, dialog, "Change the password", pathOf(ROUTES.password, who.fresh)), ROUTES.password);
  ensure(!(await changed.text()).includes(SECOND), "the answer of the password change holds the password");
  await t.gone(dialog);
  const text = await page.locator("body").innerText();
  ensure(!text.includes(SECOND) && !text.includes(FIRST), "the page shows a password");
  await mustBeRefused(t, who.fresh, FIRST);
  const works = await signInAs(t, who.fresh, SECOND);
  ensure(works.answer.status() === 200, `${who.fresh} was not signed in with the new password: ${works.answer.status()}`);
  await works.page.context().close();
}

// --- Disable and Enable ---

/**
 * Disable the account… ends its sessions and refuses its sign-in; Enable lets
 * it in with the same password. Gives the session that signed in after Enable.
 */
async function disableAndEnable(t, page, who) {
  const victim = await signedIn(t, who.fresh, SECOND);
  ensure((await statusOf(victim.page)) === 200, `${who.fresh}'s session was not answered 200 before it was disabled`);

  await unlockNow(page);
  await openAccounts(t, page, who.fresh);
  await buttonOf(page, "Disable the account…").click();
  const dialog = dialogOf(page, `Disable ${who.fresh}?`);
  await t.see(dialog);
  await t.see(dialog.getByText(`Everything ${who.fresh} runs ends now`, { exact: false }));
  await mustSucceed(await confirm(page, dialog, "Disable the account", pathOf(ROUTES.disable, who.fresh)), ROUTES.disable);
  await accountBecomes(page, who.fresh, isDisabled, "be disabled");
  await t.see(page.getByText(`${who.fresh} is disabled: nothing signs in to the account.`));
  const next = await statusOf(victim.page);
  ensure(next === 401, `the session of the account that was disabled was not ended: it was answered ${next}`);
  await victim.page.context().close();
  await mustBeRefused(t, who.fresh, SECOND);
  await lookAll(t, page, "accounts-disabled");

  await openAccounts(t, page);
  await t.see(rowOf(page, who.fresh).getByText("Disabled", { exact: true }));
  await openAccounts(t, page, who.fresh);
  const enabled = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.enable, who.fresh));
  await buttonOf(page, "Enable").click();
  await mustSucceed(await enabled, ROUTES.enable);
  await accountBecomes(page, who.fresh, (one) => !isDisabled(one), "be enabled");
  return signedIn(t, who.fresh, SECOND);
}

// --- Keys ---

/** A key with options is refused, a second key is added, and the first is removed without a question since another is left. */
async function theKeys(t, page, who, second) {
  await unlockNow(page);
  await openAccounts(t, page, who.fresh);
  await buttonOf(page, "Add a key…").click();
  const dialog = dialogOf(page, "Add an SSH key");
  await t.see(dialog);
  await dialog.getByLabel("Public key", { exact: true }).fill(`command="ls" ${second.line}`);
  await buttonOf(dialog, "Add the key").click();
  await t.see(dialog.getByText(/options before the key/));
  await dialog.getByLabel("Public key", { exact: true }).fill(second.line);
  await mustSucceed(await confirm(page, dialog, "Add the key", pathOf(ROUTES.keyAdd, who.fresh)), ROUTES.keyAdd);
  await accountBecomes(page, who.fresh, (one) => one.keys.length === 2, "have two keys");
  await t.see(page.locator("#keys").getByText(KEY_TWO, { exact: true }));

  // The fingerprint is whole while it has the focus.
  await page.locator("#keys").getByText(second.short).focus();
  await t.see(page.locator("#keys").getByText(second.whole, { exact: true }));

  const removed = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.keyRemove, who.fresh));
  await buttonOf(page, `Remove the key ${KEY_ONE}`).click();
  await mustSucceed(await removed, ROUTES.keyRemove);
  await accountBecomes(page, who.fresh, (one) => one.keys.length === 1 && one.keys[0].comment === KEY_TWO, `have only ${KEY_TWO}`);
}

/** An account with no password and one key: the key is its only way in, so removing it asks for the account's name. */
async function onlyWayIn(t, page, who) {
  await unlockNow(page);
  const key = makeKey(KEY_ONLY);
  await openAccounts(t, page);
  await buttonOf(page, "Add an account…").click();
  const dialog = dialogOf(page, "Add an account");
  await dialog.getByLabel("Name", { exact: true }).fill(who.keyOnly);
  // No password and no console: a key is the account's only way in.
  await dialog.getByRole("switch", { name: "Can sign in to this console", exact: true }).uncheck();
  await mustSucceed(await confirm(page, dialog, "Add", pathOf(ROUTES.add)), `${ROUTES.add} without a password`);
  const offer = dialogOf(page, `Add an SSH key for ${who.keyOnly}`);
  await t.see(offer);
  await offer.getByLabel("Public key", { exact: true }).fill(key.line);
  await mustSucceed(await confirm(page, offer, "Add the key", pathOf(ROUTES.keyAdd, who.keyOnly)), ROUTES.keyAdd);
  await t.see(page.getByRole("heading", { name: who.keyOnly, level: 1, exact: true }));
  await accountBecomes(page, who.keyOnly, (one) => one.keys.length === 1, "have its key");

  // The list says so: the account signs in with a key and nothing else.
  await openAccounts(t, page);
  await t.see(rowOf(page, who.keyOnly).getByText("SSH key only", { exact: true }));

  await openAccounts(t, page, who.keyOnly);
  await buttonOf(page, `Remove the key ${KEY_ONLY}`).click();
  const ask = dialogOf(page, `Remove the key ${KEY_ONLY}?`);
  await t.see(ask);
  await t.see(ask.getByText(`This is the only way ${who.keyOnly} signs in: after this it cannot sign in at all.`));
  await buttonOf(ask, "Remove the key").click();
  await t.see(ask.getByText(`Type ${who.keyOnly} exactly as it is written`));
  ensure((await readAccount(page, who.keyOnly)).keys.length === 1, "the key was removed before the account's name was typed");
  await ask.getByLabel(`Type ${who.keyOnly} to go on`, { exact: true }).fill(who.keyOnly);
  await mustSucceed(await confirm(page, ask, "Remove the key", pathOf(ROUTES.keyRemove, who.keyOnly)), ROUTES.keyRemove);
  await accountBecomes(page, who.keyOnly, (one) => one.keys.length === 0, "have no key");
}

// --- Tokens ---

/** A token that the account made is listed with what it may do; Revoke asks first, and the token's next call is refused. */
async function revoke(t, page, who, later) {
  const made = await fetchAsPage(later.page, "POST", "/api/1/account/tokens/add", { proof: SECOND, name: "lab-token", scopes: ["*"], days: 1 });
  ensure(made.status === 200 && typeof made.json?.token === "string", `the token was not made: ${made.status} ${made.text}`);
  const headers = { Authorization: `Bearer ${made.json.token}` };
  const bare = await openConsole(t, "/");
  const url = new URL("/api/1/actions", bare.page.url()).href;
  const before = await bare.page.request.get(url, { headers });
  ensure(before.status() === 200, `the token's own call was answered ${before.status()}, not 200`);

  await unlockNow(page);
  await openAccounts(t, page, who.fresh);
  const row = page.locator("#tokens").getByRole("row").filter({ hasText: "lab-token" });
  await t.see(row);
  await t.see(row.getByText(`Everything ${who.fresh} can do`, { exact: false }));
  await buttonOf(page, "Revoke the token lab-token").click();
  const dialog = dialogOf(page, "Revoke the token lab-token?");
  await t.see(dialog);
  await t.see(dialog.getByText("Whatever uses it stops working."));
  ensure((await bare.page.request.get(url, { headers })).status() === 200, "the token stopped before Revoke was confirmed");
  await mustSucceed(await confirm(page, dialog, "Revoke", pathOf(ROUTES.revoke, who.fresh)), ROUTES.revoke);
  const after = await bare.page.request.get(url, { headers });
  ensure(after.status() === 401, `the revoked token's call was answered ${after.status()}, not 401`);
  await bare.page.context().close();
}

// --- Second steps ---

/** An account with an authenticator app: an administrator removes its second steps, and it signs in with its password alone. */
async function removeSteps(t, page, who) {
  const mine = await signedIn(t, who.withApp);
  await openSecondStep(t, mine.page);
  await addApp(t, mine.page, PASSWORD);
  await mine.page.context().close();

  await unlockNow(page);
  await openAccounts(t, page, who.withApp);
  ensure((await rowText(page, "Second step")).startsWith("Authenticator app"), "the row Second step does not say the app");
  await buttonOf(page, "Remove second steps…").click();
  const dialog = dialogOf(page, `Remove ${who.withApp}'s second steps?`);
  await t.see(dialog);
  await t.see(dialog.getByText(`Use this when ${who.withApp} has lost the phone or key.`, { exact: false }));
  await mustSucceed(await confirm(page, dialog, "Remove second steps", pathOf(ROUTES.reset, who.withApp)), ROUTES.reset);
  const shown = await accountBecomes(page, who.withApp, (one) => one.steps?.app === false, "lose its app");
  ensure(!(shown.steps.recovery_left > 0), `${who.withApp} still has ${shown.steps.recovery_left} recovery codes`);
  await openAccounts(t, page, who.withApp);
  ensure((await rowText(page, "Second step")).startsWith("None"), "the row Second step does not say None after Remove second steps");

  // Its password alone signs it in now.
  const alone = await signedIn(t, who.withApp);
  await alone.page.context().close();
}

// --- The guards of the account in use ---

/** The account in use is not disabled, removed, stripped of its administrator right or ended by the console: the page says where it is done, and the routes answer ADM-136. */
async function ownGuards(t, page, who) {
  await unlockNow(page);
  await openAccounts(t, page, who.admin, "Your account");
  for (const name of ["Disable the account…", "Remove the account…"]) {
    ensure((await buttonOf(page, name).count()) === 0, `the page of the account in use has ${name}`);
  }
  ensure((await page.getByRole("switch", { name: "Administrator", exact: true }).count()) === 0, "the page of the account in use has a switch for its administrator right");
  await t.see(page.getByText("Do this from another administrator's account, or at a terminal with").first());
  await t.see(page.locator("code").getByText("elevate user …").first());

  for (const [action, body] of [["disable", {}], ["remove", { home: "keep" }], ["set", { admin: false }], ["end", { session: "1-1" }]]) {
    const answer = await fetchAsPage(page, "POST", `/api/1/accounts/${encodeURIComponent(who.admin)}/${action}`, body);
    mustBe(answer, 409, "ADM-136", `${action} of the account in use`);
  }
  // A session that is not there is the command's refusal, as for any other account.
  const gone = await fetchAsPage(page, "POST", pathOf(ROUTES.end, who.reader), { session: "1-1" });
  ensure(gone.status === 422, `End of a session at the machine that is not there was answered ${gone.status}, not 422: ${gone.text}`);
}

// --- Your account ---

/** A person's own full name and key, which they change whatever their standing, and a password, which asks for the one they have. */
async function ownAccount(t, page, who) {
  await openYours(t, page);
  for (const id of ["display", "zone", "second-step", "keys", "tokens", "terminal-settings"]) await t.see(page.locator(`#${id}`));

  await buttonOf(page, "Change your full name").click();
  const name = dialogOf(page, "Change your full name");
  await name.getByLabel("Full name", { exact: true }).fill(READER_FULL);
  await mustSucceed(await confirm(page, name, "Save", pathOf(ROUTES.ownName)), ROUTES.ownName);
  await t.see(page.locator("dl.rows").getByText(READER_FULL));

  const key = makeKey(KEY_READER);
  await buttonOf(page, "Add a key…").click();
  const add = dialogOf(page, "Add an SSH key");
  await add.getByLabel("Public key", { exact: true }).fill(key.line);
  await mustSucceed(await confirm(page, add, "Add the key", pathOf(ROUTES.ownKeyAdd)), ROUTES.ownKeyAdd);
  await t.see(page.locator("#keys").getByText(KEY_READER, { exact: true }));

  // The password asks for the one they have: a wrong one is refused and the dialog stays.
  await buttonOf(page, "Change your password").click();
  const password = dialogOf(page, "Change your password");
  const fill = async (current) => {
    await password.getByLabel("Current password", { exact: true }).fill(current);
    await password.getByLabel("New password", { exact: true }).fill(SECOND);
    await password.getByLabel("New password, again", { exact: true }).fill(SECOND);
  };
  await fill("not-the-password");
  const refused = await confirm(page, password, "Change the password", pathOf(ROUTES.ownPassword));
  ensure(refused.status() >= 400, `a wrong current password was answered ${refused.status()}`);
  await t.see(password);
  await fill(PASSWORD);
  await mustSucceed(await confirm(page, password, "Change the password", pathOf(ROUTES.ownPassword)), ROUTES.ownPassword);
  await t.gone(password);
  const works = await signInAs(t, who.reader, SECOND);
  ensure(works.answer.status() === 200, `${who.reader} was not signed in with the password it changed to: ${works.answer.status()}`);
  await works.page.context().close();

  // The reader has a password and the console, so the key is not its only way in and comes off without a question.
  const gone = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.ownKeyRemove));
  await buttonOf(page, `Remove the key ${KEY_READER}`).click();
  await mustSucceed(await gone, ROUTES.ownKeyRemove);
  await t.gone(page.locator("#keys").getByText(KEY_READER, { exact: true }));
  await lookAll(t, page, "account-own");
}

// --- The reader ---

/** A reader sees Account and Can, another account's row and what only administrators see, and their own account whole. */
async function reader(t, who) {
  const { page, seen } = await signedIn(t, who.reader);
  await openAccounts(t, page);
  ensure((await page.getByRole("columnheader", { name: "Last active", exact: true }).count()) === 0, "a reader's list has Last active");
  ensure((await buttonOf(page, "Add an account…").count()) === 0, "a reader's list has Add an account…");
  await t.see(page.getByRole("columnheader", { name: "Can", exact: true }));
  await t.see(rowOf(page, who.reader).getByText("Your account", { exact: true }));
  await lookAll(t, page, "accounts-reader");

  const blocked = await fetchAsPage(page, "GET", pathOf(ROUTES.last));
  mustBe(blocked, 403, "ADM-122", "a reader's read of when each account was last active");
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.listAll)), 403, "ADM-122", "a reader's read of the list as root");
  await openAccounts(t, page, who.admin);
  await t.see(page.getByText("Administrators see the rest."));
  ensure((await page.locator("dl.rows").count()) === 0, "a reader is shown the rows of another account");
  await ownAccount(t, page, who);
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

// --- The phone ---

/** At 390 wide: the list is cards and one account is one column, with controls a thumb reaches and no sideways scroll. */
async function phone(t, who) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, who.admin, PASSWORD);
  ensure(answer.status() === 200, `${who.admin} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  await openAccounts(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  await t.see(page.locator(".acc-list .tbl.stack td[data-label='Can']").first());
  const add = await buttonOf(page, "Add an account…").boundingBox();
  ensure(add !== null && add.height >= 43.5, `Add an account… is ${add?.height} px tall on a phone`);
  let small = await smallControls(page, "main .acc-list button, main .acc-list a.btn");
  ensure(small.length === 0, `these controls of the phone's list are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "accounts-phone-list");

  await openAccounts(t, page, who.fresh);
  ensure((await sidewaysScroll(page)) === 0, `the phone's account scrolls ${await sidewaysScroll(page)} px sideways`);
  small = await smallControls(page, "main .acc-account button, main .acc-account a.btn");
  ensure(small.length === 0, `these controls of the phone's account are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "accounts-phone-account");
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

// --- The administrator ---

/** Remove the account… with the home folder kept and the name typed. */
async function removeAccount(t, page, name) {
  await unlockNow(page);
  await openAccounts(t, page, name);
  await buttonOf(page, "Remove the account…").click();
  const dialog = dialogOf(page, `Remove ${name}?`);
  await t.see(dialog);
  await t.see(dialog.getByText(`Keep it in /users/${name}`, { exact: true }));
  await buttonOf(dialog, "Remove the account").click();
  await t.see(dialog.getByText(`Type ${name} exactly as it is written`));
  await dialog.getByLabel(`Type ${name} to go on`, { exact: true }).fill(name);
  await mustSucceed(await confirm(page, dialog, "Remove the account", pathOf(ROUTES.remove, name)), ROUTES.remove);
  await t.see(page.getByRole("heading", { name: "Accounts", level: 1, exact: true }));
  await t.gone(page.getByRole("link", { name, exact: true }));
}

async function administrator(t, who) {
  const { page, seen } = await signedIn(t, who.admin);
  await unlockNow(page);
  const first = makeKey(KEY_ONE);
  const second = makeKey(KEY_TWO);

  await theList(t, page, who);
  await addFresh(t, page, who, first);
  await firstSignIn(t, who);
  await oneAccount(t, page, who, first);
  await consoleRight(t, page, who);
  await newPassword(t, page, who);
  const later = await disableAndEnable(t, page, who);
  await theKeys(t, page, who, second);
  await revoke(t, page, who, later);
  await later.page.context().close();
  remember(later.seen);
  await onlyWayIn(t, page, who);
  await removeAccount(t, page, who.keyOnly);
  await removeSteps(t, page, who);
  await ownGuards(t, page, who);
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const who = namesOf(t.engine);
  await administrator(t, who);
  await reader(t, who);
  await phone(t, who);
  // Every route that the table names was called by some browser.
  const missing = Object.values(ROUTES).filter((route) => ![...called].some((seen) => patternOf(route).test(seen)));
  ensure(missing.length === 0, `no browser called ${missing.join(", ")}`);
  mustHaveCalled(called, [ROUTES.list, ROUTES.listAll, ROUTES.last, ROUTES.ownShow]);
}
