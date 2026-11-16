// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/security drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Security
// page at /security (design 8). Its cards, each a link target of its own
// (#checks, #sign-in, #blocked, #ssh, #reach, #private), are held to what the
// commands print: the score and the findings to `dukovo-check --last --json`
// (`GET /api/1/checks/last`, and Check again, `POST /api/1/checks/run`), the
// sign-in rules and who may reach the console to `admin settings --json` (`GET
// /api/1/security/sign-in`, changed by `admin sign-in`, `POST
// /api/1/security/sign-in`), SSH to `ssh-server show --json` (`GET /api/1/ssh`,
// changed by `ssh-server set`, `on` and `off`, `POST /api/1/ssh`, `/ssh/on` and
// `/ssh/off`), private access to `wireguard show --json` (`GET
// /api/1/private`, changed by `wireguard serve`, `add` and `remove`, `POST
// /api/1/private/serve`, `/devices/add` and `/devices/remove`), and the
// addresses blocked after wrong sign-ins to `firewall blocked --json` (`GET
// /api/1/security/blocked`, `firewall unblock` and `firewall never-block`, which
// security-blocked.mjs drives, since it needs an address that guesses).
//
// What every engine does is the page's own: the score and each finding's
// button, the rules and their form, SSH's row and the refusals of its form, who
// may reach the console and the words of its two dialogs, and private access
// as far as the page goes on its own. A reader sees the score, the findings and
// the rows and none of the buttons, and the phone at 390 by 844 shows one
// column. Each look is the console's own: dark, light, high contrast, forced
// colours and less motion, each checked by axe (console.mjs's lookAll).
//
// What waits on the lead's parts is in functions of its own, which only the first
// engine runs, since they change what the machine does and what the lab PC
// reaches it by: `sshTrial` and `narrowerTrial` (the `ssh` trial,
// `ssh-server set --try` and `off --try`, `keep` and `undo`, and the lab PC's
// own `ssh` with a key), `privateDevice` (`wireguard add`, the device's file
// saved, the lab PC's WireGuard client brought up with it and the console
// reached through wg0, and Remove of that device from that browser refused,
// ADM-137, and from another browser allowed). `consoleTrials` (`POST
// /api/1/console/reach` with a try, `/console/trial/keep` and `/undo`) waits on
// the actions of the Security page alone. The lab PC's side of them is the
// script's: security.toml's first host step writes the machine's address and
// the ports it reaches it by to /tmp/lab-security-machine, and makes the key
// the lab PC signs in with.

import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { promisify } from "node:util";

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  sidewaysScroll,
  signIn,
  signOut,
  sleep,
} from "./console.mjs";
import {
  PASSWORD,
  PHONE,
  pressAndAnswer,
  reaches,
  reachesSoon,
  smallControls,
  unlockNow,
  WORDS,
} from "./trial.mjs";

const run = promisify(execFile);

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browsers must have called each by the end, security-
// blocked.mjs's among them.
export const ROUTES = {
  last: "GET /api/1/checks/last",
  check: "POST /api/1/checks/run",
  rules: "GET /api/1/security/sign-in",
  rulesSet: "POST /api/1/security/sign-in",
  blocked: "GET /api/1/security/blocked",
  unblock: "POST /api/1/security/unblock",
  never: "POST /api/1/security/never-block",
  neverRemove: "POST /api/1/security/never-block/remove",
  ssh: "GET /api/1/ssh",
  sshSet: "POST /api/1/ssh",
  sshOn: "POST /api/1/ssh/on",
  sshOff: "POST /api/1/ssh/off",
  sshKeep: "POST /api/1/ssh/trial/keep",
  sshUndo: "POST /api/1/ssh/trial/undo",
  reach: "POST /api/1/console/reach",
  consoleKeep: "POST /api/1/console/trial/keep",
  consoleUndo: "POST /api/1/console/trial/undo",
  private: "GET /api/1/private",
  serve: "POST /api/1/private/serve",
  add: "POST /api/1/private/devices/add",
  remove: "POST /api/1/private/devices/remove",
};

/** The path of a route as the table names it. */
export const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

// What the lab PC keeps for this test: the machine and the ports it reaches
// it by, written by security.toml, and the key it signs in with.
const MACHINE_FILE = "/tmp/lab-security-machine";
const KEY_FILE = "/tmp/lab-security-key";
const DEVICE_FILE = "/tmp/lab-security-device.conf";

// What the page changes the rules to and back, and the name of the device.
const IDLE_NOW = 30;
const IDLE_THEN = 45;
const DEVICE = "labpc";

// What the browsers called, over all of them, for the check at the end.
const called = new Set();
const remember = (seen) => {
  for (const route of seen) called.add(route);
};

/** The accounts of one engine: security.toml makes `admin` and `reader`. */
export const namesOf = (engine) => ({ admin: `wc-sec-${engine}`, reader: `wc-sec-r-${engine}` });

// --- The pages ---

export const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
export const dialogOf = (page, name) => page.getByRole("dialog", { name, exact: true });
export const cardOf = (page, id) => page.locator(`#${id}`);

/** Opens Security and waits until no card is busy. */
export async function openSecurity(t, page, hash = "") {
  await page.goto(new URL(`/security${hash}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Security", level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelector("main .card[aria-busy='true']") === null, null, { timeout: 30_000 });
}

/** What the console answers for the read `route` ("GET /path"), which must be 200. */
export async function readRoute(page, route) {
  const answer = await fetchAsPage(page, "GET", pathOf(route));
  ensure(answer.status === 200, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The words of a card, as a person reads them. */
const wordsOf = async (page, id) => (await cardOf(page, id).innerText()).replace(/\s+/g, " ").trim();

/** Waits until the words of the card `id` hold `pattern`, for up to `seconds`. */
export async function cardSays(page, id, pattern, what, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const words = await wordsOf(page, id).catch(() => "");
    if (pattern.test(words)) return words;
    ensure(Date.now() < until, `the ${id} card did not say ${what} within ${seconds} seconds: ${words}`);
    await sleep(500);
  }
}

// --- The lab PC ---

/** The machine and the ports the lab PC reaches it by, as security.toml wrote them: `{host, ssh, wireguard}`. */
function machine() {
  const [host, ssh, wireguard] = readFileSync(MACHINE_FILE, "utf8").trim().split("\n");
  return { host, ssh, wireguard };
}

/** Runs `command` on the lab PC, which this script runs on. Gives `{code, out, stdout}` and never throws: `out` has the command's words too when it fails, and `stdout` only what it printed. */
async function onLabPc(command, args) {
  try {
    const { stdout, stderr } = await run(command, args, { timeout: 90_000 });
    return { code: 0, out: `${stdout}${stderr}`, stdout };
  } catch (error) {
    return { code: typeof error.code === "number" ? error.code : 1, out: `${error.stdout ?? ""}${error.stderr ?? ""}${error.message}`, stdout: error.stdout ?? "" };
  }
}

/** The lab PC's `ssh` as `name`, with the key that security.toml made, on a terminal as a person's is, which authd records as a sign-in over SSH: gives `{code, out, stdout}`; `stdout` holds `lab-signed-in` when it signed in, which `out` holds anyway when it fails, in the command's words. */
function sshAs(name) {
  const { host, ssh } = machine();
  return onLabPc("ssh", [
    "-i", KEY_FILE, "-p", ssh, "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
    "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=15", "-tt", `${name}@${host}`, "echo", "lab-signed-in",
  ]);
}

// --- Signing in ---

/** Signs in as `name`, which must work, and gives the page and what it calls. */
export async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await leaveWelcome(t, page);
  return { page, seen };
}

// --- The score and the checks ---

/** The score ring, how many checks pass and each finding are what `dukovo-check --last --json` says; each finding has a button that goes where its check says, and Check again keeps a new result. */
async function theScore(t, page) {
  await openSecurity(t, page);
  const kept = await readRoute(page, ROUTES.last);
  const card = cardOf(page, "checks");
  const ring = await card.getByRole("img", { name: /^Health score \d+ of 100: / }).getAttribute("aria-label");
  const score = Number(/score (\d+) of 100/.exec(ring)[1]);
  const { passed, warning, failed } = kept.counts;
  const ran = passed + warning + failed;
  await t.see(card.getByText(passed === ran ? `All ${ran} checks pass` : `${passed} of ${ran} checks pass`, { exact: false }));
  const finds = card.locator("li.sec-find");
  ensure((await finds.count()) === warning + failed, `the card has ${await finds.count()} findings and the checks say ${warning + failed}`);
  ensure((warning + failed > 0) === (score < 100), `the score is ${score} with ${warning + failed} findings`);
  for (let at = 0; at < (await finds.count()); at += 1) {
    const find = finds.nth(at);
    const id = (await find.locator("a.mono").innerText()).trim();
    ensure((await find.locator("a.mono").getAttribute("href")) === `/help/errors/${id}`, `the finding ${id} does not link to its page`);
    ensure(/^\+\d+/.test((await find.locator(".pts").innerText()).trim()), `the finding ${id} does not say what it takes off`);
    const targets = find.locator("a.btn, button");
    ensure((await targets.count()) > 0, `the finding ${id} has no button`);
  }
  await lookAll(t, page, "security-score");

  // A link goes where it says; Show how says the words and the command, with Copy.
  const links = card.locator("li.sec-find a.btn");
  if ((await links.count()) > 0) {
    const href = await links.first().getAttribute("href");
    ensure(href.startsWith("/"), `a finding's button goes to ${href}, not to a page of the console`);
  }
  const how = card.getByRole("button", { name: "Show how", exact: true });
  if ((await how.count()) > 0) {
    await how.first().click();
    await t.see(card.locator(".sec-how:not([hidden]) pre").first());
    await t.see(card.getByRole("button", { name: "Copy", exact: true }).first());
  }

  // Check again runs the checks and the card shows the result that was kept.
  await unlockNow(page);
  await pressAndAnswer(t, page, buttonOf(card, "Check again"), pathOf(ROUTES.check));
  await t.see(card.getByText(/Checked today at /));
  const again = await readRoute(page, ROUTES.last);
  ensure(Date.parse(again.checked) >= Date.parse(kept.checked), "Check again did not keep a newer result");
}

// --- The sign-in rules ---

/** The rules are what `admin settings --json` says; the form refuses a number out of bounds under its field, changes only what is changed, and the rules are put back. */
async function theRules(t, page) {
  await openSecurity(t, page);
  const rules = (await readRoute(page, ROUTES.rules)).sign_in;
  const card = cardOf(page, "sign-in");
  await t.see(card.getByText(new RegExp(`A session lasts ${rules.session_hours} hours? after its sign-in, and ends after ${rules.idle_minutes} minutes? with no console page open`)));
  await t.see(card.getByText("SSH signs in by its own settings, below.", { exact: false }));

  await buttonOf(card, "Change…").click();
  const dialog = dialogOf(page, "Change the sign-in rules");
  await t.see(dialog);
  const idle = dialog.getByLabel("Minutes with no console page open before a session ends", { exact: true });
  await idle.fill("2");
  await t.see(dialog.getByText(/Write a whole number of minutes from 5 to 1440/));
  ensure(await buttonOf(dialog, "Save the rules").isDisabled(), "Save the rules is on for a number out of bounds");
  await idle.fill(String(IDLE_THEN));
  const saved = await pressAndAnswer(t, page, buttonOf(dialog, "Save the rules"), pathOf(ROUTES.rulesSet));
  ensure(!(await saved.text()).includes("session_hours"), "Save the rules changed what was not changed");
  await t.gone(dialog);
  await t.see(card.getByText(new RegExp(`ends after ${IDLE_THEN} minutes`)));
  const read = (await readRoute(page, ROUTES.rules)).sign_in;
  ensure(read.idle_minutes === IDLE_THEN && read.session_hours === rules.session_hours, `the rules after Save are ${JSON.stringify(read)}`);
  await lookAll(t, page, "security-rules");

  // Put back, so that the next engine reads what the image has.
  await buttonOf(card, "Change…").click();
  await t.see(dialog);
  await dialog.getByLabel("Minutes with no console page open before a session ends", { exact: true }).fill(String(IDLE_NOW));
  await pressAndAnswer(t, page, buttonOf(dialog, "Save the rules"), pathOf(ROUTES.rulesSet));
  await t.gone(dialog);
  const back = (await readRoute(page, ROUTES.rules)).sign_in;
  ensure(back.idle_minutes === IDLE_NOW, `the rules were not put back: ${JSON.stringify(back)}`);
}

// --- SSH ---

/** SSH's row is what `ssh-server show --json` says, and its form refuses what the command would, and offers only Try it to a change that takes a way in. */
async function theSsh(t, page) {
  await openSecurity(t, page);
  const shown = await readRoute(page, ROUTES.ssh);
  const card = cardOf(page, "ssh");
  await t.see(card.getByText(shown.on === false ? "Off" : `Port ${shown.port}`, { exact: false }));
  if (shown.on === false) return;
  await t.see(card.getByText("root cannot sign in", { exact: false }));

  await buttonOf(card, "Change…").click();
  const dialog = dialogOf(page, "Change SSH's settings");
  await t.see(dialog);
  ensure(await buttonOf(dialog, "Try it").isDisabled(), "Try it is on before anything is changed");
  // Neither passwords nor keys: nobody could sign in.
  const passwords = dialog.getByLabel("Passwords sign in", { exact: true });
  const keys = dialog.getByLabel("Keys sign in", { exact: true });
  if (await passwords.isChecked()) await passwords.uncheck();
  if (await keys.isChecked()) await keys.uncheck();
  await t.see(dialog.getByText("Nobody could sign in over SSH. Turn on passwords, keys or both."));
  ensure(await buttonOf(dialog, "Try it").isDisabled(), "Try it is on for a change that lets nobody in");
  // Keys only takes a way in from someone who has passwords: only Try it, for a window.
  await keys.check();
  if (shown.passwords === true) {
    ensure(!(await buttonOf(dialog, "Apply").isVisible()), "Apply is offered for a change that takes a way in");
    await t.see(dialog.getByText("Undo it after", { exact: true }));
  }
  await lookAll(t, page, "security-ssh-form");
  await buttonOf(dialog, "Cancel").click();
  await t.gone(dialog);
  await lookAll(t, page, "security-ssh");
}

// --- Who may reach the console ---

/** The row is what `admin settings --json` says, and each dialog says what admin.toml asks; nothing is changed here. */
async function theReach(t, page) {
  await openSecurity(t, page);
  const { console: reach } = await readRoute(page, ROUTES.rules);
  const card = cardOf(page, "reach");
  await t.see(card.getByText(`Port ${reach.listen}`, { exact: false }));
  const other = reach.reach === "any" ? "Only this network…" : "Make the console public…";
  await buttonOf(card, other).click();
  const dialog = dialogOf(page, reach.reach === "any" ? "Only this network" : "Make the console public");
  await t.see(dialog);
  await t.see(dialog.getByText(reach.reach === "any" ? /The console will answer this machine/ : /admin\.toml\(5\) asks for care/));
  await t.see(buttonOf(dialog, "Try it"));
  await lookAll(t, page, "security-reach");
  await buttonOf(dialog, "Cancel").click();
  await t.gone(dialog);
}

// --- Private access ---

/** Private access is set up with the defaults, which makes wg0 and opens its UDP port in the firewall; a server that has it says so. */
async function privateSetUp(t, page) {
  await openSecurity(t, page);
  const card = cardOf(page, "private");
  const have = (await readRoute(page, ROUTES.private)).interfaces?.length > 0;
  if (!have) {
    await t.see(card.getByText("A phone or laptop with the WireGuard app joins this server's own small network, wg0, from anywhere.", { exact: false }));
    await lookAll(t, page, "security-private-none");
    await buttonOf(card, "Set up private access…").click();
    const dialog = dialogOf(page, "Set up private access");
    await t.see(dialog);
    await t.see(dialog.getByText("Your router must send UDP port 51820 to this server."));
    await dialog.getByLabel("UDP port", { exact: true }).fill("80");
    await buttonOf(dialog, "Set up").click();
    await t.see(dialog.getByText("Write a port from 1024 to 65535."));
    await dialog.getByLabel("UDP port", { exact: true }).fill("51820");
    await pressAndAnswer(t, page, buttonOf(dialog, "Set up"), pathOf(ROUTES.serve));
    await t.gone(dialog);
  }
  await t.see(card.getByText(/wg0 · 10\.8\.0\.1\/24 · UDP port 51820/));
  await t.see(buttonOf(card, "Add a device…"));
  await lookAll(t, page, "security-private");
}

// --- The trials: a change that takes a way in is tried before it is kept ---

/** The banner of the trial in the card `id`, and its two buttons and the words of how it ended. */
const bannerIn = (page, id) => page.locator(`#${id} .try-box .banner.try`);
const keepIn = (page, id) => bannerIn(page, id).getByRole("button", { name: "Keep this change", exact: true });
const undoIn = (page, id) => bannerIn(page, id).getByRole("button", { name: "Undo now", exact: true });
const overIn = (page, id) => page.locator(`#${id} .try-box .try-over:not([hidden]) .try-over-words`);

/** Waits until the card `id` says that its trial ended `how`, `kept` or `undone`, for up to `seconds`. */
async function trialEnds(page, id, how, seconds) {
  const wanted = how === "kept" ? WORDS.kept : WORDS.undone;
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const words = (await overIn(page, id).innerText({ timeout: 1000 }).catch(() => "")).trim();
    if (words.startsWith(wanted)) return words;
    ensure(Date.now() < until, `the ${id} trial did not end ${how} within ${seconds} seconds: "${words}"`);
    await sleep(1000);
  }
}

/** Chooses `minutes` in the window of a form that offers one. */
const chooseWindow = (dialog, label) => dialog.getByLabel("Undo it after", { exact: true }).selectOption({ label });

/** Opens SSH's form. */
async function openSshForm(t, page) {
  const card = cardOf(page, "ssh");
  await buttonOf(card, "Change…").click();
  const dialog = dialogOf(page, "Change SSH's settings");
  await t.see(dialog);
  return dialog;
}

/** Puts the lab PC's key in the signed-in account, as the page does for its own: `POST /api/1/account/keys/add`. */
async function addLabKey(page) {
  const line = readFileSync(`${KEY_FILE}.pub`, "utf8").trim();
  const added = await fetchAsPage(page, "POST", "/api/1/account/keys/add", { line });
  ensure(added.status === 200, `the lab PC's key was not added: ${added.status} ${added.text}`);
}

/**
 * SSH turned on with its switch; then Keys only, tried: the lab PC's `ssh` with
 * the key signs in over the new settings and the card lists it; Keep. Waits on the lead's
 * `ssh` trial (`ssh-server set --try`, `keep` and `undo`, and its Spark side).
 */
async function sshTrial(t, page, who) {
  await openSecurity(t, page);
  const card = cardOf(page, "ssh");
  await addLabKey(page);

  // SSH was turned off by security.toml: the switch turns it on, which takes nothing away and is not tried.
  const shown = await readRoute(page, ROUTES.ssh);
  ensure(shown.on === false, `SSH is not off as security.toml left it: ${JSON.stringify(shown)}`);
  await pressAndAnswer(t, page, card.getByRole("switch", { name: "SSH", exact: true }), pathOf(ROUTES.sshOn));
  await t.see(card.getByText(/Port \d+ · /));
  const before = await sshAs(who.admin);
  ensure(before.code === 0 && before.stdout.includes("lab-signed-in"), `the lab PC did not sign in over SSH before the change: ${before.out}`);

  // Keys only: passwords stop, so the change is tried and cannot be applied.
  const dialog = await openSshForm(t, page);
  await dialog.getByLabel("Passwords sign in", { exact: true }).uncheck();
  ensure(!(await buttonOf(dialog, "Apply").isVisible()), "Apply is offered for Keys only");
  const sent = await pressAndAnswer(t, page, buttonOf(dialog, "Try it"), pathOf(ROUTES.sshSet));
  ensure((sent.request().postData() ?? "").includes("try"), "Try it did not send a try");
  await t.gone(dialog);
  await t.see(bannerIn(page, "ssh"));
  await t.see(card.getByText(/Open a new SSH connection from your computer now: ssh /));
  await t.see(card.getByText("Connections that were open stay open, so only a new one tells.", { exact: false }));
  await lookAll(t, page, "security-ssh-trial");

  // A new connection, as the card asks, signs in with its key, and the card lists it.
  const during = await sshAs(who.admin);
  ensure(during.code === 0 && during.stdout.includes("lab-signed-in"), `the lab PC did not sign in with its key while the change was tried: ${during.out}`);
  await cardSays(page, "ssh", new RegExp(`${who.admin} signed in over SSH at `), "that the account signed in over SSH", 30);
  await cardSays(page, "ssh", /1 account signed in over SSH since the change\./, "how many signed in", 10);

  await pressAndAnswer(t, page, keepIn(page, "ssh"), pathOf(ROUTES.sshKeep));
  await trialEnds(page, "ssh", "kept", 30);
  const kept = await readRoute(page, ROUTES.ssh);
  ensure(kept.passwords === false && kept.keys === true, `after Keep SSH says ${JSON.stringify({ passwords: kept.passwords, keys: kept.keys })}`);
}

/** SSH turned off, tried and taken back at once with Undo now: SSH goes on again. Waits on the lead's `off --try`, which restarts sshd. */
async function offTrial(t, page, who) {
  await openSecurity(t, page);
  const card = cardOf(page, "ssh");
  await card.getByRole("switch", { name: "SSH", exact: true }).click();
  const dialog = dialogOf(page, "Change SSH's settings");
  await t.see(dialog);
  await t.see(dialog.getByText("SSH stops. Nobody can sign in over SSH."));
  await chooseWindow(dialog, "2 minutes");
  await pressAndAnswer(t, page, buttonOf(dialog, "Try it"), pathOf(ROUTES.sshOff));
  await t.see(bannerIn(page, "ssh"));
  await pressAndAnswer(t, page, undoIn(page, "ssh"), pathOf(ROUTES.sshUndo));
  await trialEnds(page, "ssh", "undone", 30);
  const back = await readRoute(page, ROUTES.ssh);
  ensure(back.on !== false, "SSH is off after the try of turning it off was undone");
  // sshd was restarted twice; a new connection signs in again.
  let signed = await sshAs(who.admin);
  for (let at = 0; at < 10 && signed.code !== 0; at += 1) {
    await sleep(2000);
    signed = await sshAs(who.admin);
  }
  ensure(signed.code === 0, `the lab PC did not sign in after SSH came on again: ${signed.out}`);
}

/**
 * A narrower AllowUsers tried and not kept: while it is tried the lab PC's key is
 * refused, and Spark's timer undoes it, which the page says and the serial
 * console confirms (security.toml reads `ssh-server show`). Waits on the lead's
 * `ssh` trial.
 */
async function narrowerTrial(t, page, who) {
  await openSecurity(t, page);
  const dialog = await openSshForm(t, page);
  await dialog.getByText("These accounts", { exact: true }).click();
  // Each account has a group of its name: the box is the one under Accounts.
  await dialog.getByRole("group", { name: "Accounts", exact: true }).getByLabel(who.reader, { exact: true }).check();
  await t.see(dialog.getByText(new RegExp(`Over SSH, only ${who.reader} may sign in\\.`)));
  ensure(!(await buttonOf(dialog, "Apply").isVisible()), "Apply is offered for a narrower list");
  await chooseWindow(dialog, "2 minutes");
  await pressAndAnswer(t, page, buttonOf(dialog, "Try it"), pathOf(ROUTES.sshSet));
  await t.see(bannerIn(page, "ssh"));

  const refused = await sshAs(who.admin);
  ensure(refused.code !== 0 && !refused.stdout.includes("lab-signed-in"), `the lab PC signed in as ${who.admin} while only ${who.reader} was let in: ${refused.out}`);

  // Not kept: Spark undoes it within its window.
  await trialEnds(page, "ssh", "undone", 180);
  const back = await readRoute(page, ROUTES.ssh);
  ensure(back.allow_users.length === 0, `AllowUsers is ${JSON.stringify(back.allow_users)} after the try was undone`);
  let signed = await sshAs(who.admin);
  for (let at = 0; at < 10 && signed.code !== 0; at += 1) {
    await sleep(2000);
    signed = await sshAs(who.admin);
  }
  ensure(signed.code === 0, `the lab PC did not sign in after the narrower list was undone: ${signed.out}`);
}

/** Puts SSH back as the image has it: passwords on, which only adds a way in and is applied at once. */
async function sshBack(t, page) {
  await openSecurity(t, page);
  const dialog = await openSshForm(t, page);
  await dialog.getByLabel("Passwords sign in", { exact: true }).check();
  await pressAndAnswer(t, page, buttonOf(dialog, "Apply"), pathOf(ROUTES.sshSet));
  await t.gone(dialog);
}

/**
 * The console made public with its second step asked first, tried, reached by
 * this browser over the new settings and kept; from a browser on the lab's
 * outside network, where there is one, made private and not kept, which Spark
 * undoes by itself; made private with Apply from this browser, which is on the
 * network; and made public once more and taken back with Undo now. Waits on the
 * actions of the Security page (`POST /api/1/console/reach` with a try, and the
 * console's `keep` and `undo`).
 */
async function consoleTrials(t, page) {
  await openSecurity(t, page);
  const card = cardOf(page, "reach");
  const before = (await readRoute(page, ROUTES.rules)).sign_in.second_step;

  await buttonOf(card, "Make the console public…").click();
  const dialog = dialogOf(page, "Make the console public");
  await t.see(dialog);
  ensure(!(await buttonOf(dialog, "Apply").isVisible().catch(() => false)), "Apply is offered for making the console public");
  const second = dialog.getByLabel("Ask administrators for a second step first", { exact: true });
  if ((await second.count()) > 0) ensure(await second.isChecked(), "the second step is not asked for first by default");
  await pressAndAnswer(t, page, buttonOf(dialog, "Try it"), pathOf(ROUTES.reach));
  await t.see(bannerIn(page, "reach"));
  await t.see(card.getByText(WORDS.reached, { exact: false }));
  await lookAll(t, page, "security-reach-trial");
  await pressAndAnswer(t, page, keepIn(page, "reach"), pathOf(ROUTES.consoleKeep));
  await trialEnds(page, "reach", "kept", 30);
  ensure((await readRoute(page, ROUTES.rules)).console.reach === "any", "the console does not answer anyone after Keep");

  await madePrivateFromOutside(t);

  // Back to this network: this browser is on it, so Apply makes the change at once.
  await openSecurity(t, page);
  await buttonOf(card, "Only this network…").click();
  const network = dialogOf(page, "Only this network");
  await t.see(network);
  await pressAndAnswer(t, page, buttonOf(network, "Apply"), pathOf(ROUTES.reach));
  await t.gone(network);
  await cardSays(page, "reach", /this network and WireGuard only/, "that the console answers this network only", 30);

  // Public once more, and taken back with Undo now.
  await buttonOf(card, "Make the console public…").click();
  const again = dialogOf(page, "Make the console public");
  await t.see(again);
  await pressAndAnswer(t, page, buttonOf(again, "Try it"), pathOf(ROUTES.reach));
  await t.see(bannerIn(page, "reach"));
  await pressAndAnswer(t, page, undoIn(page, "reach"), pathOf(ROUTES.consoleUndo));
  await trialEnds(page, "reach", "undone", 30);
  ensure((await readRoute(page, ROUTES.rules)).console.reach === "private", "the console answers anyone after Undo now");

  // The second-step rule is as the image has it.
  const now = (await readRoute(page, ROUTES.rules)).sign_in.second_step;
  if (now !== before) {
    await buttonOf(cardOf(page, "sign-in"), "Change…").click();
    const rules = dialogOf(page, "Change the sign-in rules");
    await t.see(rules);
    await rules.getByText({ "if-set": "Whoever has added one", administrators: "Administrators, and whoever has added one", everyone: "Everyone" }[before], { exact: true }).click();
    await pressAndAnswer(t, page, buttonOf(rules, "Save the rules"), pathOf(ROUTES.rulesSet));
    await t.gone(rules);
  }
}

/**
 * While the console answers anyone, a browser on the lab's outside network,
 * which security.toml writes the console's URL for into /tmp/lab-security-outside,
 * is warned that making the console private would cut it off, is offered Try it
 * only, tries it and does not keep it: Spark undoes it by itself and the console
 * answers that browser again. The lab may have no such network, which is said and not failed.
 */
async function madePrivateFromOutside(t) {
  let url;
  try {
    url = readFileSync("/tmp/lab-security-outside", "utf8").trim();
  } catch {
    console.log("security: the lab has no outside network, so the change that would cut a browser off is not tried");
    return;
  }
  const { page, seen } = await openConsole(t, url);
  const answer = await signIn(page, namesOf(t.engine).admin, PASSWORD);
  ensure(answer.status() === 200, `the outside browser was not signed in: ${answer.status()}`);
  await leaveWelcome(t, page);
  await unlockNow(page);
  await openSecurity(t, page);
  const card = cardOf(page, "reach");
  await buttonOf(card, "Only this network…").click();
  const dialog = dialogOf(page, "Only this network");
  await t.see(dialog);
  await t.see(dialog.getByText(/This browser comes from .*, which is outside\. After this change it cannot reach the console: the change is undone at /));
  ensure(!(await buttonOf(dialog, "Apply").isVisible()), "Apply is offered to a browser that the change would cut off");
  await dialog.getByLabel("Undo it after", { exact: true }).selectOption({ index: 0 });
  await pressAndAnswer(t, page, buttonOf(dialog, "Try it"), pathOf(ROUTES.reach));
  // While it is tried this browser's address is not let in; when Spark undoes it, it is.
  let cut = false;
  for (let at = 0; at < 30 && !cut; at += 1) cut = !(await reaches(url, 3000));
  ensure(cut, "the console still answers the outside browser after the change to this network only was tried");
  ensure(await reachesSoon(url, 600), "the console does not answer the outside browser after the try ended");
  remember(seen);
  await page.context().close();
}

// --- Private access: a device, the lab PC's WireGuard client, and the guard ---

/** The settings that Add a device shows once, as a QR code and as the text under "Show the settings as text", and saved as NAME.conf; they are the device's file, with its private key. */
async function addDevice(t, page) {
  await openSecurity(t, page);
  const card = cardOf(page, "private");
  await buttonOf(card, "Add a device…").click();
  const dialog = dialogOf(page, "Add a device");
  await t.see(dialog);
  await dialog.getByLabel("Name of the device", { exact: true }).fill("my phone");
  await buttonOf(dialog, "Add the device").click();
  await t.see(dialog.getByText("Use 1 to 32 letters, digits, - and _, not starting with -."));
  await t.see(dialog.getByText("The device reaches this server through wg0", { exact: false }));
  await dialog.getByLabel("Name of the device", { exact: true }).fill(DEVICE);
  const added = await pressAndAnswer(t, page, buttonOf(dialog, "Add the device"), pathOf(ROUTES.add));
  // A change answers {done, changed, output}; `wireguard add NAME --json` printed the device as one line of output.
  const answer = await added.json();
  ensure(answer.changed === true, `Add a device says that it changed nothing: ${JSON.stringify({ ...answer, output: undefined })}`);
  const config = (answer.output ?? []).map((line) => { try { return JSON.parse(line); } catch { return null; } }).find((one) => typeof one?.config === "string")?.config;
  ensure(typeof config === "string" && config.includes("[Interface]") && config.includes("PrivateKey"), "the answer of Add a device is not the device's file");

  const shown = dialogOf(page, `Settings for ${DEVICE}`);
  await t.see(shown);
  await t.see(shown.getByText("Scan this with the WireGuard app", { exact: false }));
  // A device's file is about 240 bytes, which a QR code holds.
  ensure((await shown.locator("svg").count()) > 0, `the device's file of ${config.length} bytes is not shown as a QR code`);
  await buttonOf(shown, "Show the settings as text").click();
  const text = await shown.locator("pre").innerText();
  ensure(text.trim() === config.trim(), "the settings that are shown are not the device's file");
  await lookAll(t, page, "security-private-device");
  const saving = page.waitForEvent("download");
  await buttonOf(shown, "Save as file").click();
  const download = await saving;
  ensure(download.suggestedFilename() === `${DEVICE}.conf`, `the file is saved as ${download.suggestedFilename()}, not ${DEVICE}.conf`);
  await download.saveAs(DEVICE_FILE);
  ensure(readFileSync(DEVICE_FILE, "utf8").trim() === config.trim(), "the saved file is not the device's file");
  await buttonOf(shown, "I have kept it").click();
  await t.gone(shown);

  // The key is shown once: the server lists the device and no private key, and the page does not show it again.
  const listed = await fetchAsPage(page, "GET", pathOf(ROUTES.private));
  const key = /PrivateKey\s*=\s*(\S+)/.exec(config)[1];
  ensure(!listed.text.includes(key), "the list of private access holds the device's private key");
  ensure(!(await page.content()).includes(key), "the page still holds the device's private key");
  await t.see(card.getByText(DEVICE, { exact: true }));
  return config;
}

/**
 * The lab PC brings up WireGuard with the saved file, with the endpoint it
 * reaches the machine by, reaches the console through wg0, and a browser that
 * comes that way is refused Remove of its own device (ADM-137), while the
 * first browser removes it. Waits on the lead's `wireguard add`, its guard and
 * the lab PC having `wg-quick` and a way to run it as root.
 */
async function privateDevice(t, page, who) {
  const config = await addDevice(t, page);
  const { host, wireguard } = machine();
  const have = await onLabPc("sh", ["-c", "command -v wg-quick"]);
  if (have.code !== 0) {
    console.log("security: the lab PC has no wg-quick, so its WireGuard client is not brought up");
    await removeDevice(t, page);
    return;
  }
  const edited = config.replace(/^Endpoint\s*=.*$/m, `Endpoint = ${host}:${wireguard}`);
  await onLabPc("sh", ["-c", `printf '%s\\n' '${edited.replace(/'/g, "'\\''")}' > ${DEVICE_FILE}`]);
  const up = await onLabPc("sudo", ["-n", "wg-quick", "up", DEVICE_FILE]);
  ensure(up.code === 0, `the lab PC's WireGuard client did not come up: ${up.out}`);
  try {
    ensure(await reachesSoon("https://10.8.0.1:9090", 40), "the lab PC does not reach the console through wg0");
    await refusedThroughWg(t, who);
  } finally {
    await onLabPc("sudo", ["-n", "wg-quick", "down", DEVICE_FILE]);
  }
  await removeDevice(t, page);
}

/** A browser that comes through wg0 signs in, unlocks and cannot remove the device that it comes through: admind answers ADM-137 and the card says where to go. */
async function refusedThroughWg(t, who) {
  const { page, seen } = await openConsole(t, "https://10.8.0.1:9090/");
  const answer = await signIn(page, who.admin, PASSWORD);
  ensure(answer.status() === 200, `${who.admin} was not signed in through wg0: ${answer.status()}`);
  await leaveWelcome(t, page);
  await unlockNow(page);
  await openSecurity(t, page);
  const card = cardOf(page, "private");
  await card.getByRole("button", { name: `Remove ${DEVICE}`, exact: true }).click();
  const ask = dialogOf(page, `Remove ${DEVICE}?`);
  await t.see(ask);
  const refused = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === pathOf(ROUTES.remove));
  await buttonOf(ask, `Remove ${DEVICE}`).click();
  mustBe({ status: (await refused).status(), json: await (await refused).json() }, 409, "ADM-137", "Remove of the device this browser comes through");
  await t.see(card.getByText("This browser is connected through that device.", { exact: false }));
  await t.see(card.getByText(DEVICE, { exact: true }));
  remember(seen);
  await page.context().close();
}

/** Remove of the device from this browser, which does not come through it. */
async function removeDevice(t, page) {
  await openSecurity(t, page);
  const card = cardOf(page, "private");
  await card.getByRole("button", { name: `Remove ${DEVICE}`, exact: true }).click();
  const ask = dialogOf(page, `Remove ${DEVICE}?`);
  await t.see(ask);
  await t.see(ask.getByText(`${DEVICE} can no longer reach this server through wg0.`, { exact: false }));
  await pressAndAnswer(t, page, buttonOf(ask, `Remove ${DEVICE}`), pathOf(ROUTES.remove));
  await t.gone(card.getByText(DEVICE, { exact: true }));
}

// --- A locked administrator, a reader and the phone ---

/** An administrator who has not unlocked is asked to unlock before blocked addresses and private access are read. */
async function whileLocked(t, page) {
  await openSecurity(t, page);
  await t.see(cardOf(page, "blocked").getByText("Unlock to see blocked addresses."));
  await t.see(cardOf(page, "private").getByText("Unlock to see private access."));
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.blocked)), 403, "ADM-106", "a locked administrator's read of the blocked addresses");
  await lookAll(t, page, "security-locked");
}

/** A reader sees the rows and none of the buttons; the health score is for administrators, as dukovo-check keeps its result for them. */
async function reader(t, who) {
  const { page, seen } = await signedIn(t, who.reader);
  await openSecurity(t, page);
  for (const name of ["Check again", "Change…", "Make the console public…", "Only this network…", "Set up private access…", "Add a device…"]) {
    ensure((await buttonOf(page.locator("main"), name).count()) === 0, `a reader's Security page has ${name}`);
  }
  await t.see(cardOf(page, "checks").getByText("The health score is for administrators."));
  await t.see(cardOf(page, "blocked").getByText("Administrators see blocked addresses."));
  await t.see(cardOf(page, "private").getByText("Administrators see private access."));
  await t.see(cardOf(page, "sign-in").getByText(/Who gives a second step: /));
  await t.see(cardOf(page, "ssh").getByText(/root cannot sign in|Off/));
  await lookAll(t, page, "security-reader");
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.blocked)), 403, "ADM-122", "a reader's read of the blocked addresses");
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.private)), 403, "ADM-122", "a reader's read of private access");
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: one column, the findings as cards, the blocked table as cards, controls a thumb reaches and no sideways scroll. */
async function phone(t, who) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, who.admin, PASSWORD);
  ensure(answer.status() === 200, `${who.admin} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);
  await openSecurity(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's Security page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .sec button, main .sec a.btn");
  ensure(small.length === 0, `these controls of the phone's Security page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "security-phone");
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

// --- The administrator ---

async function administrator(t, who) {
  const { page, seen } = await signedIn(t, who.admin);
  await whileLocked(t, page);
  await unlockNow(page);
  await theScore(t, page);
  await theRules(t, page);
  await theSsh(t, page);
  await theReach(t, page);
  await privateSetUp(t, page);
  if (t.engine === "chromium") {
    await sshTrial(t, page, who);
    await offTrial(t, page, who);
    await narrowerTrial(t, page, who);
    await sshBack(t, page);
    await consoleTrials(t, page);
    await privateDevice(t, page, who);
  }
  remember(seen);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const who = namesOf(t.engine);
  await administrator(t, who);
  await reader(t, who);
  await phone(t, who);
  // Every route that this script drives was called by some browser; the blocked
  // addresses' routes are security-blocked.mjs's, and the first engine's
  // alone are the ones that change what the machine does.
  const everyEngine = [ROUTES.last, ROUTES.check, ROUTES.rules, ROUTES.rulesSet, ROUTES.blocked, ROUTES.ssh, ROUTES.private, ROUTES.serve];
  const firstEngine = [ROUTES.sshSet, ROUTES.sshOn, ROUTES.sshOff, ROUTES.sshKeep, ROUTES.sshUndo, ROUTES.reach, ROUTES.consoleKeep, ROUTES.add, ROUTES.remove];
  mustHaveCalled(called, everyEngine);
  if (t.engine === "chromium") mustHaveCalled(called, firstEngine);
}
