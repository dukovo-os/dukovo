// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in one engine, with the machine restarting
// once by the page's own button: going back to the version before the last
// update. updates.toml stages the lab's `good` test update, restarts the
// machine and waits until the update has proved itself, so that slot B runs
// it and is the default, slot A keeps the version before it, and this script
// is the person at the page.
//
// The System card says which version is kept ("Kept for going back: ... in
// slot A") and has "Go back to it…". That asks first, and runs `update undo
// system` (`POST /api/1/updates/{item}/undo`): the dialog is titled "Go back
// to ...?" with the version kept, and its Settings are two choices, the
// settings from before the update, which are put back at the next start as a
// fallback puts them back, and today's. The choice that is on is the one the
// machine's last update calls for (`last_update` of `GET /api/1/updates/slots`):
// the settings from before, unless a fallback has put them back already.
// Keep today's settings is chosen, so the settings stay as updates.toml made
// them for the steps after this one. Go back and restart sends that choice,
// and the slots then say that slot A is the default and starts next while
// slot B still runs.
//
// The restart is asked for apart, as every restart is: "Restart ...?" says
// what it is for ("To go back to ..."), and Restart in a minute runs
// `power.restart` (`POST /api/1/power/restart`). The System card counts the
// minute, and the shell's banner says the machine is restarting into the
// version before. When the machine answers again it runs slot A, which the
// slots and the console's own answer (`GET /api/1/console/about`) both say,
// and the System card keeps the update for coming back to ("Kept for going
// back: ... in slot B"). updates.toml reads `sysupdate status` after it.
//
// The look of the question is checked in each way the console can look
// (console.mjs's lookAll).

import {
  ensure,
  fetchAsPage,
  lookAll,
  mustHaveCalled,
  signInDone,
  signInHeading,
  signOut,
  sleep,
} from "./console.mjs";
import { PASSWORD, pressAndAnswer, unlockNow } from "./trial.mjs";
import {
  ROUTES,
  bannerOf,
  card,
  openUpdates,
  pathOf,
  pendingLine,
  read,
  routeFor,
  signedIn,
} from "./updates-page.mjs";

const ADMIN = "wc-updates";

// The words the page says where this script looks for them.
const GO_BACK = "Go back to it…";
const SUBMIT = "Go back and restart";
const RESTART_CONFIRM = "Restart in a minute";
const ABOUT = "/api/1/console/about";

/** `text` as a pattern's own words. */
const escaped = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The version that slot `slot`, `a` or `b`, holds, as `GET /api/1/updates/slots` says. */
const versionIn = (slots, slot) => slots.slots?.find((one) => one.slot === slot)?.version;

/**
 * What `GET /api/1/updates/slots` says, `sysupdate status --json`, or null
 * while the console does not answer. A restart ends the unlock, which is made
 * again.
 */
async function slotsNow(page) {
  try {
    let answer = await read(page, ROUTES.slots);
    if (answer.json?.error?.id === "ADM-106") {
      await unlockNow(page);
      answer = await read(page, ROUTES.slots);
    }
    return answer.status === 200 ? answer.json : null;
  } catch {
    // The console is away, or the session is not signed in again yet.
    return null;
  }
}

/** The machine as updates.toml leaves it: the update runs in slot B, which is the default, and slot A keeps the version before. */
async function updateProved(page) {
  const slots = await slotsNow(page);
  ensure(slots !== null, `${ROUTES.slots} was not answered`);
  const now = { running: slots.running, default: slots.default, previous: slots.previous, staged: slots.staged ?? null };
  ensure(JSON.stringify(now) === JSON.stringify({ running: "b", default: "b", previous: "a", staged: null }), `the machine says ${JSON.stringify(now)}, and the update was proved in slot B`);
  const update = versionIn(slots, "b");
  const before = versionIn(slots, "a");
  ensure(typeof update === "string" && update.endsWith("-test.good") && typeof before === "string" && before !== update, `the slots hold ${JSON.stringify([before, update])}, the version before and the update`);
  // The page puts the settings from before on while the machine says no fallback put them back.
  return { update, before, fromBeforeOn: slots.last_update?.settings_restored === false };
}

/** The question: its title names the version kept, and it offers the two settings, with the one that the last update calls for on. */
async function questionAsked(t, page, { before, fromBeforeOn }) {
  const box = card(page, "system");
  await t.see(box.getByText(`Kept for going back: ${before} in slot A`));
  await box.getByRole("button", { name: GO_BACK, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: new RegExp(`^Go back to ${escaped(before)}\\?$`) });
  await t.see(dialog);
  await t.see(dialog.getByText(new RegExp(`^Go back to ${escaped(before)}\\. The machine restarts into it`)));
  const group = dialog.getByRole("group", { name: "Settings", exact: true });
  await t.see(group);
  ensure((await group.getByRole("radio").count()) === 2, `the Settings of the question have ${await group.getByRole("radio").count()} choices, and there are two`);
  const fromBefore = group.getByRole("radio", { name: /^The settings from before the update to / });
  const keep = group.getByRole("radio", { name: /^Keep today's settings/ });
  await t.see(fromBefore);
  await t.see(keep);
  await t.see(group.getByText("They are put back at the next start, as a fallback does, and today's are kept apart."));
  await t.see(group.getByText("The settings stay as they are."));
  ensure((await fromBefore.isChecked()) === fromBeforeOn && (await keep.isChecked()) === !fromBeforeOn, `the question has the settings from before ${(await fromBefore.isChecked()) ? "on" : "off"}, and the machine's last update calls for them ${fromBeforeOn ? "on" : "off"}`);
  await lookAll(t, page, "updates-back");
  await keep.check();
  ensure(!(await fromBefore.isChecked()), "the settings from before are still chosen after Keep today's settings");
  return dialog;
}

/** Go back and restart sends the settings that were chosen, and the machine goes back to the version before at its next start. */
async function goneBack(t, page, dialog, { update, before }) {
  const path = pathOf(ROUTES.undo, { item: "system" });
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === path);
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: SUBMIT, exact: true }), path);
  const body = (await sent).postData() ?? "";
  ensure(/"settings"\s*:\s*"keep"/.test(body), `${ROUTES.undo} was sent ${JSON.stringify(body)}, not today's settings kept`);
  await t.gone(dialog);
  // The machine runs the update until the restart, and starts the version before then.
  const slots = await slotsNow(page);
  ensure(slots !== null, `${ROUTES.slots} was not answered after Go back`);
  const now = { running: slots.running, default: slots.default, previous: slots.previous, next: slots.next };
  ensure(JSON.stringify(now) === JSON.stringify({ running: "b", default: "a", previous: "b", next: "a" }), `the machine says ${JSON.stringify(now)} after Go back, and ${before} starts at the next restart in place of ${update}`);
}

/** The restart is asked for apart: the question says what it is for, and the minute is counted. */
async function restartAsked(t, page, { before }) {
  const question = page.getByRole("dialog", { name: /^Restart .+\?$/ });
  await t.see(question);
  await t.see(question.getByText(`To go back to ${before}.`));
  await pressAndAnswer(t, page, question.getByRole("button", { name: RESTART_CONFIRM, exact: true }), pathOf(ROUTES.restart));
  await t.gone(question);
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
}

/**
 * The shell's banner says where the machine restarts into, and the console is
 * away. It answers again before the machine does when it is still going down,
 * so the slots are asked until they say that slot A runs, signing in again
 * if the restart ended the session. Gives the slots.
 */
async function backOn(t, page, { before }) {
  await page.getByText(new RegExp(`is restarting into ${escaped(before)}\\.`)).waitFor({ state: "visible", timeout: 100_000 });
  console.log("the shell's banner says the machine is restarting into the version before");
  const seconds = 12 * 60;
  const until = Date.now() + seconds * 1000;
  for (;;) {
    if (await signInHeading(page).isVisible().catch(() => false)) await signInDone(t, page, ADMIN, PASSWORD);
    const slots = await slotsNow(page);
    if (slots?.running === "a") return slots;
    ensure(Date.now() < until, `the machine did not run slot A again within ${seconds} seconds: ${JSON.stringify(slots?.running ?? null)}`);
    await sleep(5000);
  }
}

/** The machine runs the version before, and the page says so: the console's own answer, and the System card keeping the update for coming back. */
async function runsVersionBefore(t, page, { update, before }, slots) {
  ensure(slots.default === "a" && slots.previous === "b" && slots.next === "a" && slots.staged == null, `the machine says ${JSON.stringify({ default: slots.default, previous: slots.previous, next: slots.next, staged: slots.staged })} after the restart`);
  ensure(versionIn(slots, "a") === before && versionIn(slots, "b") === update, `the slots hold ${JSON.stringify([versionIn(slots, "a"), versionIn(slots, "b")])} after the restart`);
  await openUpdates(t, page);
  const about = await fetchAsPage(page, "GET", ABOUT);
  ensure(about.status === 200 && typeof about.json?.release === "string", `${ABOUT} was answered ${about.status}: ${about.text}`);
  const running = about.json.release.trim().split(/\s+/).at(-1);
  ensure(running === before, `the console says it runs ${running}, and the version before was ${before}`);
  const box = card(page, "system");
  await t.see(box.getByText(`Kept for going back: ${update} in slot B`));
  await t.see(box.getByRole("button", { name: GO_BACK, exact: true }));
  // Nothing waits for a restart, and nothing is staged.
  await t.gone(pendingLine(page));
  await t.gone(bannerOf(page));
}

export default async function check(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await unlockNow(page);
  await openUpdates(t, page);
  const proved = await updateProved(page);

  const dialog = await questionAsked(t, page, proved);
  await goneBack(t, page, dialog, proved);
  await restartAsked(t, page, proved);
  const slots = await backOn(t, page, proved);
  await runsVersionBefore(t, page, proved, slots);

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.slots),
    routeFor(ROUTES.undo, { item: "system" }),
    routeFor(ROUTES.restart),
    routeFor(ROUTES.power),
  ]);
  await signOut(t, page);
  await page.context().close();
}
