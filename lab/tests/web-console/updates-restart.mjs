// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in one engine, with the machine restarting
// once by the page's own button: an update that cannot prove itself, taken
// back. updates.toml stages the lab's `broken` test update (the one
// updates/fallback.toml uses: its system log service cannot start, so the
// health check fails it) and puts a setting in place that the fallback takes
// back, and this script is the person at the page.
//
// The System card says the update is staged in a slot, and so does the banner
// "Restart to finish the update". Restart now… asks first, and says what the
// restart is for ("To finish the update to …") in the dialog; "Restart in a
// minute" runs `power.restart` (`POST /api/1/power/restart`) and the System
// card says that the machine restarts in a number of seconds, which counts
// down and is the machine's own (`GET /api/1/power`). Cancel (`POST
// /api/1/power/cancel`) takes the restart back, and nothing is set. Pressed
// again, the restart is left to come: the shell's banner says the machine is
// restarting into the update, the machine tries it, the health check fails it
// and the machine falls back to the version before, with the settings from
// before the update put back (ADRs 0453 and 0454). The console answers again,
// and the shell's banner says "Back, on …: the update was taken back." with
// "See what happened", which opens /updates#happened. What happened says that
// the update was taken back, with the start and its error ID, the machine's
// own words for what failed, what the machine did, and Report it
// (`/report?about=failure`), and no search for a change (an update that fell
// back is not one the search can try). The release that fell back is held
// (`[hold]` in updates.toml): the Held card lists it, and Release lets it go.
// The Overview's notice of the failed start has the same two buttons.
//
// The look of the page with a card of What happened is checked in each way the
// console can look (console.mjs's lookAll).

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
  consoleReturns,
  openUpdates,
  pathOf,
  pendingLine,
  read,
  routeFor,
  settingsNow,
  signedIn,
  wordsOf,
} from "./updates-page.mjs";

const ADMIN = "wc-updates";

// The words the page and the shell say where this script looks for them.
const RESTART_BUTTON = "Restart now…";
const RESTART_CONFIRM = "Restart in a minute";
const BACK_WORDS = /^Back, on .*: the update was taken back\./;
const WENT_BACK = /The machine went back to .* with the settings from before the update\. Nothing was lost\./;
const NOT_OFFERED = "The search is not offered after an update was taken back";

/** The seconds the System card's pending line says are left, or null when it says none. */
async function secondsLeft(page) {
  const found = /restarts in (\d+) seconds?/.exec(await wordsOf(pendingLine(page)));
  return found === null ? null : Number(found[1]);
}

/** The version of the slot that holds the staged update, as `GET /api/1/updates/slots` says. */
async function stagedVersion(page) {
  const slots = await read(page, ROUTES.slots);
  ensure(slots.status === 200 && slots.json?.staged?.slot !== undefined, `${ROUTES.slots} says nothing is staged: ${slots.status} ${slots.text}`);
  const slot = slots.json.slots.find((one) => one.slot === slots.json.staged.slot);
  ensure(typeof slot?.version === "string", `${ROUTES.slots} gives no version for the staged slot: ${slots.text}`);
  return slot.version;
}

/** The machine's own say of what is waiting to restart, `shutdown` of `GET /api/1/power`. */
async function powerNow(page) {
  const answer = await read(page, ROUTES.power);
  ensure(answer.status === 200, `${ROUTES.power} was answered ${answer.status}: ${answer.text}`);
  return answer.json.shutdown ?? null;
}

/** Restart now… asks, and Restart in a minute sets the restart; gives the dialog's lead. */
async function restartSet(t, page, version) {
  await card(page, "system").getByRole("button", { name: RESTART_BUTTON, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: /^Restart .+\?$/ });
  await t.see(dialog);
  await t.see(dialog.getByText(`To finish the update to ${version}.`));
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: RESTART_CONFIRM, exact: true }), pathOf(ROUTES.restart));
  await t.gone(dialog);
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
}

/** The countdown counts down, the machine says the same, and Cancel takes the restart back. */
async function countdownAndCancel(t, page, version) {
  await restartSet(t, page, version);
  const first = await secondsLeft(page);
  ensure(first !== null && first <= 60, `the pending line says ${first} seconds, and a restart waits a minute`);
  const set = await powerNow(page);
  ensure(set?.end === "reboot" && set.in_seconds <= 60, `${ROUTES.power} says ${JSON.stringify(set)} for a restart that was just set`);
  await sleep(4000);
  const second = await secondsLeft(page);
  ensure(second !== null && second < first, `the countdown went from ${first} to ${second} seconds in 4 seconds`);
  await pressAndAnswer(t, page, pendingLine(page).getByRole("button", { name: "Cancel", exact: true }), pathOf(ROUTES.cancel));
  await t.gone(pendingLine(page));
  ensure((await powerNow(page)) === null, "the machine still has a restart set after Cancel");
  await t.see(card(page, "system").getByText(/is staged in slot [AB]/));
}

/** The restart is left to come: the banner says where the machine restarts into, and then the console is away. */
async function restartAndWait(t, page, version) {
  await restartSet(t, page, version);
  await page.getByText(new RegExp(`is restarting into ${version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\. It has three tries to prove itself`)).waitFor({ state: "visible", timeout: 100_000 });
  console.log("the shell's banner says the machine is restarting into the update");
  // The machine falls back by itself, which takes a few minutes: the update's tries, the menu, the version before.
  const away = await page.getByText(BACK_WORDS).waitFor({ state: "visible", timeout: 15 * 60_000 }).then(() => true, () => false);
  ensure(away, "the banner never said that the console is back on the version before");
}

/** After the console is back: a session that did not survive the restart signs in again, and the console is unlocked again. */
async function signedBack(t, page) {
  await consoleReturns(page, 120);
  if (await signInHeading(page).isVisible().catch(() => false)) await signInDone(t, page, ADMIN, PASSWORD);
  await unlockNow(page);
}

/** What happened says that the update was taken back, in the machine's words, and offers Report it and no search. */
async function whatHappened(t, page, version) {
  const box = card(page, "happened");
  await t.see(box);
  await t.see(box.getByRole("heading", { level: 2, name: `The update to ${version} was taken back`, exact: true }));
  const words = await wordsOf(box);
  ensure(/Start \d+/.test(words) && /slot [AB]/.test(words), `What happened gives no start and slot: ${words}`);
  ensure((await box.locator('a[href^="/help/errors/"]').count()) >= 1, `What happened links no error ID: ${words}`);
  ensure(/What failed/.test(words) && /logd/.test(words), `What happened does not say that logd failed: ${words}`);
  await t.see(box.getByText(WENT_BACK));
  const report = box.getByRole("link", { name: "Report it", exact: true });
  ensure((await report.getAttribute("href")) === "/report?about=failure", "Report it does not lead to /report?about=failure");
  await t.see(box.getByText(NOT_OFFERED));
  ensure((await box.getByRole("button", { name: "Start the search", exact: true }).count()) === 0, "What happened offers a search after an update was taken back");
}

/** The release that fell back is held: the Held card lists it with the machine's own words, and Release lets it go. */
async function heldAndReleased(t, page, version) {
  const held = await settingsNow(page);
  ensure(held.hold?.system === version, `the machine holds ${held.hold?.system} and the update that fell back was ${version}`);
  const box = card(page, "held");
  await t.see(box.getByText(`System ${version}`));
  await t.see(box.getByText("It was taken back, so it is not staged again until you release it."));
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Release", exact: true }), pathOf(ROUTES.unhold, { item: "system" }));
  await t.see(box.getByText("Nothing is held."));
  ensure((await settingsNow(page)).hold?.system == null, "the machine still holds the system after Release");
}

/** The Overview's notice of the failed start has the page's two buttons. */
async function overviewNotice(t, page) {
  await page.goto(new URL("/", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Overview", level: 1, exact: true }));
  await t.see(page.locator('a[href="/updates#happened"]').first());
  await t.see(page.locator('a[href="/report?about=failure"]').first());
}

export default async function check(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await unlockNow(page);
  await openUpdates(t, page);
  const version = await stagedVersion(page);

  // The staged update, before anything is pressed.
  await t.see(card(page, "system").getByText(`${version} is staged in slot`));
  await t.see(bannerOf(page).getByText("Restart to finish the update"));
  await lookAll(t, page, "updates-staged");

  await countdownAndCancel(t, page, version);
  await restartAndWait(t, page, version);

  // "See what happened" opens What happened, which takes the focus.
  const link = page.getByRole("link", { name: "See what happened", exact: true });
  await link.click({ timeout: 4000 }).catch(() => console.log("the banner's link was gone before it was pressed"));
  await signedBack(t, page);
  await openUpdates(t, page, "#happened");
  await page.waitForFunction(() => document.activeElement?.id === "happened-h", null, { timeout: 20_000 });

  await whatHappened(t, page, version);
  await heldAndReleased(t, page, version);
  await lookAll(t, page, "updates-happened");
  await overviewNotice(t, page);

  // A start that fell back is the machine's own record: it is read again, and says the same.
  const failed = await fetchAsPage(page, "GET", pathOf(ROUTES.failed));
  ensure(failed.status === 200 && failed.json?.failed?.result === "fell-back", `${ROUTES.failed} says ${JSON.stringify(failed.json?.failed)}, not a fall back`);

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.slots),
    routeFor(ROUTES.power),
    routeFor(ROUTES.restart),
    routeFor(ROUTES.cancel),
    routeFor(ROUTES.failed),
    routeFor(ROUTES.unhold, { item: "system" }),
  ]);
  await signOut(t, page);
  await page.context().close();
}
