// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/unlock drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): an
// administrator unlocks the console with a password and with an
// authenticator app's code, watches the header count the 15 minutes down,
// resets them, locks it again, and sees it lock by itself when the
// session ends or its last page closes. An account that is no
// administrator is refused the unlock. Each engine has an account of its
// own, which unlock.toml made, so that an engine that fails halfway does
// not leave the next one an app whose secret it cannot know.

import {
  addApp,
  ensure,
  look,
  mustHaveCalled,
  mustRefuse,
  openConsole,
  postAsPage,
  press,
  sessionOf,
  signInDone,
  signInWithRecovery,
  signOut,
  sleep,
} from "./console.mjs";

// The password that unlock.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// An account in group console that is no administrator.
const VIEWER = "wc-unlock-viewer";

// The calls that lab/coverage.toml says this test covers, which the
// browser must have made by the end.
const ROUTES = [
  "GET /api/1/session",
  "POST /api/1/sign-out",
  "POST /api/1/unlock",
  "POST /api/1/unlock/extend",
  "POST /api/1/lock",
  "GET /api/1/events",
];

// What the console keeps an unlock for, in seconds.
const UNLOCK_SECONDS = 15 * 60;

// How long a session that has no page open stays unlocked, and the most
// this test waits to see it lock.
const LOCK_AFTER_SECONDS = 60;
const LOCK_WAIT_SECONDS = 150;

/** The header of the signed-in page, where the console's state and its buttons are. */
const header = (page) => page.getByRole("banner");

/** The header's button called `name`. */
const headerButton = (page, name) => header(page).getByRole("button", { name, exact: true });

/** The dialog that asks to unlock. */
const unlockDialog = (page) => page.getByRole("dialog", { name: "Unlock to make changes" });

/** The minutes that the header says are left, as the text of its counter. */
const minutesLeft = (page, minutes) => page.getByText(`${minutes} min`, { exact: true });

/** Waits for the header of `page` to say that the console is locked. */
async function seeLocked(t, page) {
  await t.see(header(page).getByText("Limited access"));
  await t.see(headerButton(page, "Unlock"));
}

/** Waits for the header of `page` to say that the console is unlocked, with 15 minutes left. */
async function seeUnlocked(t, page) {
  await t.see(header(page).getByText("Unlocked ·"));
  await t.see(headerButton(page, "Lock"));
  await t.see(minutesLeft(page, 15));
}

/** Throws unless `seconds`, what the console says is left of the unlock, is of an unlock made a moment ago. */
function ensureFresh(seconds, what) {
  ensure(
    seconds > UNLOCK_SECONDS - 60 && seconds <= UNLOCK_SECONDS,
    `${what}: the console says ${seconds} seconds are left of an unlock of ${UNLOCK_SECONDS}`,
  );
}

/** Opens the dialog, and gives it. */
async function openUnlock(t, page) {
  await headerButton(page, "Unlock").click();
  const dialog = unlockDialog(page);
  await t.see(dialog);
  return dialog;
}

/** Gives `proof` in the dialog's field `label` and presses "Unlock". Gives the answer. */
async function unlockWith(page, dialog, label, proof) {
  await dialog.getByLabel(label, { exact: true }).fill(proof);
  return press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
}

/** The error in the dialog that holds `id`. */
const dialogError = (dialog, id) => dialog.getByRole("alert").filter({ hasText: id });

/** A wrong password is refused and the right one unlocks; a second page follows by its stream. */
async function unlockWithPassword(t, page, other) {
  await seeLocked(t, page);
  await seeLocked(t, other);
  await look(t, page, "locked");

  const dialog = await openUnlock(t, page);
  await look(t, page, "unlock-dialog");
  await mustRefuse(await unlockWith(page, dialog, "Password", "not the password"), 401, "ADM-101");
  await t.see(dialogError(dialog, "ADM-101"));
  await look(t, page, "unlock-wrong-password");

  const unlocked = await unlockWith(page, dialog, "Password", PASSWORD);
  ensure(unlocked.status() === 200, `the console answered ${unlocked.status()} to the password`);
  ensureFresh((await unlocked.json()).unlock_seconds, "unlocked by the password");
  await t.gone(dialog);
  await seeUnlocked(t, page);
  await look(t, page, "unlocked");
  ensureFresh((await sessionOf(page)).unlock_seconds, "GET /api/1/session");

  // The other page learns of it from the session's stream, with no call of
  // its own.
  await seeUnlocked(t, other);
}

/** While the console is unlocked, a change asks for no password. */
async function changeWithoutPassword(t, page) {
  await page.getByRole("link", { name: "Manage the second step" }).click();
  await t.see(page.getByRole("heading", { name: "Second step", level: 1 }));
  await t.see(page.getByRole("heading", { name: "Trusted browsers" }));
  const { app, codes } = await addApp(t, page, null);
  await page.getByRole("button", { name: "I have saved them" }).click();
  await t.gone(page.getByRole("heading", { name: "Your recovery codes" }));
  await page.getByRole("link", { name: "Home" }).click();
  await t.see(page.getByText("Signed in as"));
  return { app, codes };
}

/** The header counts the unlock down by the minute, and "Reset to 15 min" makes it 15 again. */
async function countdownAndReset(t, page) {
  // The first minute has passed when the counter says 14.
  await minutesLeft(page, 14).waitFor({ timeout: 90_000 });
  await look(t, page, "counting-down");
  const reset = await press(page, headerButton(page, "Reset to 15 min"), "/api/1/unlock/extend");
  ensure(reset.status() === 200, `the console answered ${reset.status()} to the reset`);
  ensureFresh((await reset.json()).unlock_seconds, "reset to 15 minutes");
  await t.see(minutesLeft(page, 15));
}

/** "Lock" locks the console, in every page of the session. */
async function lock(t, page, other) {
  const locked = await press(page, headerButton(page, "Lock"), "/api/1/lock");
  ensure(locked.status() === 200 && (await locked.json()).unlock_seconds === null, "the lock did not lock");
  await seeLocked(t, page);
  await seeLocked(t, other);
  ensure((await sessionOf(page)).unlock_seconds === null, "GET /api/1/session says the console is unlocked after the lock");
}

/** Once the account has an app, the dialog asks for its code first. */
async function unlockWithCode(t, page, app) {
  // The dialog's ways to unlock are those of the session as the page read it.
  await page.reload();
  await seeLocked(t, page);
  const dialog = await openUnlock(t, page);
  await t.see(dialog.getByLabel("Six-digit code", { exact: true }));
  await look(t, page, "unlock-with-code");

  await mustRefuse(await unlockWith(page, dialog, "Six-digit code", await app.wrongCode(page)), 401, "ADM-102");
  await t.see(dialogError(dialog, "ADM-102"));
  const unlocked = await unlockWith(page, dialog, "Six-digit code", await app.nextCode(page));
  ensure(unlocked.status() === 200, `the console answered ${unlocked.status()} to the app's code`);
  ensureFresh((await unlocked.json()).unlock_seconds, "unlocked by the app's code");
  await seeUnlocked(t, page);
}

/**
 * The route that only the lab's build has runs a command as an
 * administrator for an unlocked session, and not for a locked one. An
 * image built without it has no such route, which is what a shipped
 * console is, and the check is left for one that has it.
 */
async function elevatedCommand(t, page) {
  const command = { command: ["/system/commands/echo", "hello"] };
  const ran = await postAsPage(page, "/api/1/test/elevate", command);
  if (ran.status() === 404) {
    console.log("this image has no POST /api/1/test/elevate: the elevated command is not tried");
    return;
  }
  const result = await ran.json();
  ensure(
    ran.status() === 200 && result.status === 0 && result.output.includes("hello"),
    `the command did not run as the unlocked session's: ${ran.status()} ${JSON.stringify(result)}`,
  );
  await press(page, headerButton(page, "Lock"), "/api/1/lock");
  await seeLocked(t, page);
  await mustRefuse(await postAsPage(page, "/api/1/test/elevate", command), 403, "ADM-106");
}

/** A session that is signed out is locked when it is signed in again. */
async function lockOnSignOut(t, page, name, codes) {
  await signOut(t, page);
  await signInWithRecovery(t, page, name, PASSWORD, codes[0]);
  await seeLocked(t, page);
  ensure((await sessionOf(page)).unlock_seconds === null, "a new session of the account is unlocked");
}

/** A session with no page open locks itself after a minute, and stays signed in. */
async function lockWhenTheLastPageCloses(t, page) {
  const context = page.context();
  const dialog = await openUnlock(t, page);
  await dialog.getByRole("button", { name: "Use your password" }).click();
  const unlocked = await unlockWith(page, dialog, "Password", PASSWORD);
  ensure(unlocked.status() === 200, `the console answered ${unlocked.status()} to the password`);
  await seeUnlocked(t, page);

  await Promise.all(context.pages().map((open) => open.close()));
  const closed = Date.now();
  const url = `${t.url}/api/1/session`;
  // The console waits a while before it locks, so that a reload is no
  // lock.
  await sleep(5000);
  ensure((await (await context.request.get(url)).json()).unlock_seconds > 0, "the console locked as soon as the last page closed");
  let seconds = null;
  while (Date.now() - closed < LOCK_WAIT_SECONDS * 1000) {
    const session = await (await context.request.get(url)).json();
    if (session.unlock_seconds === null) {
      seconds = (Date.now() - closed) / 1000;
      break;
    }
    await sleep(5000);
  }
  ensure(seconds !== null, `the console was not locked ${LOCK_WAIT_SECONDS} seconds after the last page closed`);
  ensure(seconds >= LOCK_AFTER_SECONDS - 10, `the console locked ${seconds} seconds after the last page closed, not after a minute`);

  const again = await context.newPage();
  await again.goto(`${t.url}/`);
  await seeLocked(t, again);
  return again;
}

/** An account that is no administrator sees the console, is offered no unlock, and is refused one. */
async function viewer(t, page) {
  await signOut(t, page);
  await signInDone(t, page, VIEWER, PASSWORD);
  await t.see(page.getByText("it is not an administrator"));
  await t.see(header(page).getByText("Limited access"));
  await t.gone(headerButton(page, "Unlock"));
  await look(t, page, "viewer");

  const refused = await postAsPage(page, "/api/1/unlock", { kind: "password", password: PASSWORD });
  const error = await mustRefuse(refused, 403, "ADM-122");
  ensure(error.detail.includes("is not an administrator"), `a viewer's unlock was refused as: ${error.detail}`);
  ensure((await sessionOf(page)).unlock_seconds === null, "a viewer is unlocked");
  await signOut(t, page);
}

export default async function check(t) {
  const name = `wc-unlock-${t.engine}`;
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, name, PASSWORD);

  // A second page of the same session.
  const other = await page.context().newPage();
  await other.goto(`${t.url}/`);

  await unlockWithPassword(t, page, other);
  const { app, codes } = await changeWithoutPassword(t, page);
  await countdownAndReset(t, page);
  await lock(t, page, other);
  await other.close();

  await unlockWithCode(t, page, app);
  await elevatedCommand(t, page);
  await lockOnSignOut(t, page, name, codes);
  const last = await lockWhenTheLastPageCloses(t, page);
  await viewer(t, last);

  mustHaveCalled(seen, ROUTES);
}
