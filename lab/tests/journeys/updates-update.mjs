// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The first browser part of journeys/updates: one page of the console, open
// from the first look to the last, through two restarts of the machine. The
// owner reads what the console says of the machine, stages the update that the
// lab PC's update server offers (updated made the check before the page
// opened), restarts into it from the page, and the page, which the restart
// cut off, asks to sign in again by itself when the console answers. The
// machine runs the update, and the console and every service that was up are
// as before. The owner then goes back to the version before from the System
// card, restarts again, and the machine and the console are as they were at
// the start. updates.toml looks at the machine after each part.

import { ensure, fetchAsPage, lookAll, signInDone, signInHeading, sleep } from "../web-console/console.mjs";
import { PASSWORD, pressAndAnswer, unlockNow } from "../web-console/trial.mjs";
import { ROUTES, bannerOf, card, openUpdates, pathOf, pendingLine, read, signedIn, systemWords, offerOf } from "../web-console/updates-page.mjs";

const ADMIN = "journey-admin";

// The words the page says where this script looks for them.
const RESTARTED_WORDS = /Sign in again:? the console restarted\./;

/** `text` as a pattern's own words. */
const escaped = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The version a slot holds, as `GET /api/1/updates/slots` says. */
const versionIn = (slots, slot) => slots.slots?.find((one) => one.slot === slot)?.version;

// The health check gives its result and ends, so it is up at the start of a day and done or stopped later on.
const ENDS = ["sysupdate"];

/** The services that are enabled and up or done, as the Services page counts them, which a restart must leave so. */
async function servicesUp(page) {
  const answer = await fetchAsPage(page, "GET", "/api/1/services");
  ensure(answer.status === 200 && Array.isArray(answer.json?.services), `GET /api/1/services was answered ${answer.status}: ${answer.text}`);
  return answer.json.services.filter((service) => service.enabled === true && !ENDS.includes(service.name) && ["running", "listening", "done"].includes(service.state)).map((service) => service.name).sort();
}

/** What the console says of itself and the machine: the release it runs, the slot, the services that are up. */
async function consoleState(page) {
  const about = await fetchAsPage(page, "GET", "/api/1/console/about");
  ensure(about.status === 200 && typeof about.json?.release === "string", `GET /api/1/console/about was answered ${about.status}: ${about.text}`);
  const slots = await read(page, ROUTES.slots);
  ensure(slots.status === 200 && slots.json !== null, `${ROUTES.slots} was answered ${slots.status}: ${slots.text}`);
  return { version: about.json.release.trim().split(/\s+/).at(-1), slots: slots.json, services: await servicesUp(page) };
}

/**
 * Waits, for up to `minutes`, until the machine's answers hold `test`, signing
 * in again whenever the console asks to, as a person does at the page that
 * stayed open. The page is not reloaded: what it does by itself is what is
 * held. Gives the state.
 */
async function becomes(t, page, test, what, minutes) {
  const until = Date.now() + minutes * 60_000;
  let last = null;
  for (;;) {
    if (await signInHeading(page).isVisible().catch(() => false)) await signInDone(t, page, ADMIN, PASSWORD);
    try {
      await unlockNow(page);
      last = await consoleState(page);
      if (test(last)) return last;
    } catch {
      // The console is away, or the session is not signed in again yet.
    }
    ensure(Date.now() < until, `the machine did not ${what} within ${minutes} minutes: ${JSON.stringify(last && { version: last.version, running: last.slots.running, default: last.slots.default })}`);
    await sleep(5000);
  }
}

/** The restart that the System card sets, asked for apart as every restart is: its dialog says what it is for. */
async function restartFromCard(t, page, why) {
  await card(page, "system").getByRole("button", { name: "Restart now…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: /^Restart .+\?$/ });
  await t.see(dialog);
  await t.see(dialog.getByText(why));
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: "Restart in a minute", exact: true }), pathOf(ROUTES.restart));
  await t.gone(dialog);
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
}

/** Stage it stages the update the server offers, a task whose end the page says. Gives the version in the spare slot. */
async function stageIt(t, page) {
  const box = card(page, "system");
  const offered = await read(page, ROUTES.check);
  const system = offerOf(offered.json, "system");
  ensure(system?.state === "update", `the update server offers the system ${JSON.stringify(system)}, and updated said an update was ready`);
  await t.see(box.getByText(systemWords(system)));
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Stage it", exact: true }), pathOf(ROUTES.apply, { item: "system" }), "POST", [202]);
  // The staging fetches what the slot lacks, which takes as long as the lab network does.
  await page.waitForFunction(() => /is staged in slot [AB]/.test(document.querySelector("main section#system")?.textContent ?? ""), null, { timeout: 8 * 60 * 1000 });
  await t.see(bannerOf(page).getByText("Restart to finish the update"));
  const slots = (await read(page, ROUTES.slots)).json;
  ensure(slots.staged?.slot === "b", `${ROUTES.slots} says ${JSON.stringify(slots.staged)} is staged, not slot b`);
  return versionIn(slots, "b");
}

/** The page asks to sign in again once the restart is over, and the machine runs `slot` as the default. */
async function restartedInto(t, page, version, slot) {
  // The shell's banner says where the machine restarts into, and the console goes away.
  await page.getByText(new RegExp(`is restarting into ${escaped(version)}`)).waitFor({ state: "visible", timeout: 100_000 });
  // Nothing here reloads the page: it shows the sign-in page by itself, with the console's reason.
  const asked = await signInHeading(page).waitFor({ state: "visible", timeout: 12 * 60_000 }).then(() => true, () => false);
  ensure(asked, "the page that stayed open never asked to sign in again after the restart");
  await t.see(page.getByText(RESTARTED_WORDS));
  return becomes(t, page, (state) => state.slots.running === slot && state.slots.default === slot, `run slot ${slot} as the default`, 12);
}

/** The console and the machine as before: the same services up, and the Overview and Logs show. */
async function asBefore(t, page, before, now, what) {
  const missing = before.services.filter((name) => !now.services.includes(name));
  if (missing.length > 0) {
    const listed = (await fetchAsPage(page, "GET", "/api/1/services")).json.services;
    const states = missing.map((name) => `${name} is ${listed.find((service) => service.name === name)?.state ?? "not listed"}`);
    throw new Error(`${what}: these services were up before and are not: ${states.join(", ")}`);
  }
  for (const [path, heading] of [["/", "Overview"], ["/logs", "Logs"], ["/services", "Services"]]) {
    await page.goto(new URL(path, page.url()).href);
    await t.see(page.getByRole("heading", { name: heading, level: 1, exact: true }));
  }
  await openUpdates(t, page);
}

export default async function check(t) {
  const { page } = await signedIn(t, ADMIN);
  await unlockNow(page);
  await openUpdates(t, page);
  const before = await consoleState(page);
  ensure(before.slots.running === "a" && before.slots.default === "a", `the machine starts on ${JSON.stringify({ running: before.slots.running, default: before.slots.default })}, not slot a`);
  ensure(before.services.includes("admind") && before.services.includes("sshd"), `the services up at the start are ${before.services.join(", ")}`);
  await lookAll(t, page, "journey-updates-before");

  // The update is staged and the machine restarts into it from the page.
  const update = await stageIt(t, page);
  await restartFromCard(t, page, `To finish the update to ${update}.`);
  const inUpdate = await restartedInto(t, page, update, "b");
  ensure(versionIn(inUpdate.slots, "b") === update && versionIn(inUpdate.slots, "a") === before.version, `the slots hold ${JSON.stringify([versionIn(inUpdate.slots, "a"), versionIn(inUpdate.slots, "b")])}, not the version before and the update`);
  await asBefore(t, page, before, inUpdate, "after the update");
  await t.see(card(page, "system").getByText(`Kept for going back: ${before.version} in slot A`));
  await lookAll(t, page, "journey-updates-after");

  // Back to the version before, with today's settings.
  await card(page, "system").getByRole("button", { name: "Go back to it…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: new RegExp(`^Go back to ${escaped(before.version)}\\?$`) });
  await t.see(dialog);
  await dialog.getByRole("group", { name: "Settings", exact: true }).getByRole("radio", { name: /^Keep today's settings/ }).check();
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: "Go back and restart", exact: true }), pathOf(ROUTES.undo, { item: "system" }));
  await t.gone(dialog);
  const question = page.getByRole("dialog", { name: /^Restart .+\?$/ });
  await t.see(question);
  await t.see(question.getByText(`To go back to ${before.version}.`));
  await pressAndAnswer(t, page, question.getByRole("button", { name: "Restart in a minute", exact: true }), pathOf(ROUTES.restart));
  await t.gone(question);
  const back = await restartedInto(t, page, before.version, "a");
  ensure(back.version === before.version, `the console says it runs ${back.version}, and the version before was ${before.version}`);
  await asBefore(t, page, before, back, "after going back");
  await t.see(card(page, "system").getByText(`Kept for going back: ${update} in slot B`));
}
