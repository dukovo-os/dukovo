// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the two tests of changes that are tried (networking.mjs and
// firewall.mjs) share: the banner that counts a try down (design 8.4 to 8.9),
// Spark's document of the trials that run and the ones it undid, an unlock
// made through the console's own route, a press that answers the unlock when
// it is asked for, a connection from the lab PC to the console that is made
// the way a browser makes one, and the size of the controls on a phone. The
// scripts import it from beside themselves.

import net from "node:net";
import tls from "node:tls";

import { answerTo, ensure, fetchAsPage, sleep } from "./console.mjs";

/** The phone's window, as the other tests have it. */
export const PHONE = { width: 390, height: 844 };

/** The account's password, which the tests' own steps give the accounts they make. */
export const PASSWORD = "lab-console-password";

/** The words under the banner's heading are made of these; each is what the page says, whole. */
export const WORDS = {
  applied: "Applied",
  reached: "This browser reached the console over the new settings.",
  cannot: "This browser cannot reach the console over the new settings.",
  kept: "The change was kept.",
  undone: "The change was undone",
  reader: "Administrators keep or undo the change.",
};

/** The dialog that asks to unlock. */
export const unlockDialog = (page) => page.getByRole("dialog", { name: /^Unlock/ });

// --- The banner ---

/** The box that holds the banner of a trial, and the banner that says how it ended. */
export const boxOf = (page) => page.locator(".try-box");

/** The banner of a trial that runs. */
export const bannerOf = (page) => page.locator(".try-box .banner.try");

/** Keep this change, and Undo now. */
export const keepOf = (page) => bannerOf(page).getByRole("button", { name: "Keep this change", exact: true });
export const undoOf = (page) => bannerOf(page).getByRole("button", { name: "Undo now", exact: true });

/** What the banner shows now: whether a trial runs, its words, its clock and its reach line, and whether Keep is on and the sentence that says why not. */
export const readBanner = (page) =>
  page.evaluate(() => {
    const box = document.querySelector(".try-box");
    const live = box !== null && !box.hidden ? box.querySelector(".banner.try:not([hidden])") : null;
    const over = box !== null && !box.hidden ? box.querySelector(".try-over:not([hidden])") : null;
    const text = (parent, selector) => parent?.querySelector(selector)?.textContent.trim() ?? "";
    const keep = [...(live?.querySelectorAll("button") ?? [])].find((button) => button.textContent.trim() === "Keep this change");
    return {
      running: live !== null,
      heading: text(live, ".try-h"),
      words: text(live, ".try-words"),
      clock: text(live, ".count-down"),
      applied: [...(live?.querySelectorAll(".try-step") ?? [])].some((step) => !step.hidden && step.querySelector(".st") !== null),
      reach: [...(live?.querySelectorAll(".try-step") ?? [])].filter((step) => !step.hidden && !step.querySelector(".st")).map((step) => step.textContent.trim()).join(" "),
      keepOff: keep?.getAttribute("aria-disabled") === "true",
      reason: text(live, ".try-reason"),
      link: live?.querySelector(".try-link a")?.getAttribute("href") ?? null,
      note: text(live, ".try-reader"),
      buttons: [...(live?.querySelectorAll("button") ?? [])].filter((button) => !button.hidden).map((button) => button.textContent.trim()),
      over: text(over, ".try-over-words"),
      polite: box?.parentElement?.querySelector("[aria-live=polite]")?.textContent.trim() ?? "",
    };
  });

/** Waits until `test(banner)` holds of what `readBanner` gives, for up to `seconds`, and gives it. */
export async function bannerBecomes(page, test, what, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const banner = await readBanner(page).catch(() => null);
    if (banner !== null && test(banner)) return banner;
    ensure(Date.now() < until, `the banner did not ${what} within ${seconds} seconds: ${JSON.stringify(banner)}`);
    await sleep(500);
  }
}

/** Waits until the banner says how the trial ended, `kept` or `undone`, for up to `seconds`, and gives its words. */
export async function bannerEnds(page, how, seconds) {
  const wanted = how === "kept" ? WORDS.kept : WORDS.undone;
  const banner = await bannerBecomes(page, (shown) => !shown.running && shown.over.startsWith(wanted), `say that the change was ${how}`, seconds);
  return banner.over;
}

/** The countdown of the banner as seconds: "1:05" is 65. */
export const secondsOfClock = (clock) => {
  const found = /^(\d+):(\d\d)$/.exec(clock);
  return found === null ? null : Number(found[1]) * 60 + Number(found[2]);
};

// --- Spark's document ---

/** Spark's document, which `GET /api/1/services` answers: `{now_ms, services, trials, undone}`. */
export async function readDocument(page) {
  const answer = await fetchAsPage(page, "GET", "/api/1/services");
  ensure(answer.status === 200 && Array.isArray(answer.json?.services), `GET /api/1/services was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The running trial `id` of `document`, or null. */
export const trialIn = (document, id) => (document.trials ?? []).find((trial) => trial.id === id) ?? null;

/** The times at which Spark undid trials `id`, as its document says them. */
export const undoneIn = (document, id) => (document.undone ?? []).filter((entry) => entry.id === id).map((entry) => entry.at_ms);

/**
 * How many of the trials `id` that `document` lists as undone were undone
 * after all the times `before` (an earlier `undoneIn`) holds. Spark keeps
 * the last 8 undone trials of all, so a count of them stops growing once
 * the list is full: a new one is told by its time.
 */
export const undoneSince = (document, id, before) => {
  const last = Math.max(0, ...before);
  return undoneIn(document, id).filter((at) => at > last).length;
};

/** Reads the document until trial `id` is not in it, for up to `seconds`, tolerating a console that does not answer for a while. */
export async function noTrial(page, id, seconds) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const document = await readDocument(page).catch(() => null);
    if (document !== null && trialIn(document, id) === null) return document;
    ensure(Date.now() < until, `the trial ${id} still runs ${seconds} seconds after it should have ended`);
    await sleep(1000);
  }
}

// --- Unlocking and pressing ---

/** Unlocks the console for the session of `page` with the account's password, as the unlock dialog does. */
export async function unlockNow(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(answer.status === 200, `the unlock was answered ${answer.status}: ${answer.text}`);
  // The page follows the unlock by the console's stream: it knows within a moment.
  await sleep(1500);
}

/** Answers the dialog that asks to unlock with the account's password. */
export async function answerUnlock(t, page) {
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const answered = answerTo(page, "/api/1/unlock");
  await dialog.getByRole("button", { name: "Unlock", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `the unlock was answered ${answer.status()}`);
  await t.gone(dialog);
}

/**
 * Presses `control`, which asks the console to POST to `route` or `GET` it,
 * and gives the answer. A console that is locked asks to unlock first: the
 * dialog is answered when it comes. `statuses` are the answers that are
 * right: 200, or `[202]` for a long action, which starts a task.
 */
export async function pressAndAnswer(t, page, control, route, method = "POST", statuses = [200]) {
  const answered = page.waitForResponse((answer) => answer.request().method() === method && new URL(answer.url()).pathname === route);
  await control.click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  const answer = await answered;
  ensure(statuses.includes(answer.status()), `${method} ${route} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

/**
 * Presses `control` for a change that takes away the address the browser
 * comes by, and waits for its POST to `route` to end: answered with 200, or
 * given up by the page, since an answer has no way back by an address that
 * is gone (the console waits 15 seconds for it). Gives whether it was
 * answered.
 */
export async function pressAndMaybeAnswer(t, page, control, route) {
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === route, { timeout: 60_000 });
  await control.click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  const request = await sent;
  // Null when the request failed: the console gave it up.
  const answer = await Promise.race([request.response(), sleep(60_000).then(() => "waiting")]);
  ensure(answer !== "waiting", `POST ${route} neither was answered nor was given up in 60 s`);
  if (answer === null) return false;
  ensure(answer.status() === 200, `POST ${route} was answered ${answer.status()}: ${await answer.text()}`);
  return true;
}

/**
 * The journal's row of an undo after the row numbered `after`, or null.
 * admind gives Spark's undo its row in the journal's round, every 30
 * seconds, so the row is waited for as long as a round may take.
 */
export async function undoRowAfter(page, after, seconds = 45) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const journal = await fetchAsPage(page, "GET", "/api/1/changes?limit=50");
    ensure(journal.status === 200 && Array.isArray(journal.json?.changes), `GET /api/1/changes was answered ${journal.status}`);
    const row = journal.json.changes.find((one) => one.id > after && /undone/i.test(JSON.stringify(one)));
    if (row !== undefined) return row;
    if (Date.now() >= until) return null;
    await sleep(2000);
  }
}

/** Whether `control`, pressed, sends nothing to `route`: a button that says it is off. */
export async function sendsNothing(page, control, route) {
  let sent = 0;
  const count = (request) => {
    if (new URL(request.url()).pathname === route) sent += 1;
  };
  page.on("request", count);
  // Brought whole into the middle of the view first: a press that skips
  // Playwright's checks lands where the control's middle is, and a control
  // left half under the view's edge is not pressed at all.
  await control.evaluate((element) => element.scrollIntoView({ block: "center" }));
  // Pressed as a person may: Playwright would wait for it to come on.
  await control.click({ force: true });
  await sleep(1200);
  page.off("request", count);
  return sent === 0;
}

// --- The lab PC's own connection ---

/**
 * Whether a TLS connection from this computer to the console at `url` is
 * made within `ms` milliseconds: the connection a browser makes, over the
 * same route and from the same address, which a firewall rule or an address
 * that is gone stops.
 */
export function reaches(url, ms = 4000) {
  const { hostname, port } = new URL(url);
  const host = hostname.replace(/^\[|\]$/g, "");
  return new Promise((resolve) => {
    const socket = tls.connect(
      { host, port: Number(port) || 443, servername: net.isIP(host) ? undefined : host, rejectUnauthorized: false },
      () => {
        socket.destroy();
        resolve(true);
      },
    );
    socket.setTimeout(ms, () => {
      socket.destroy();
      resolve(false);
    });
    socket.on("error", () => resolve(false));
  });
}

/** Whether the console at `url` is reached within `seconds`: tries again every second. */
export async function reachesSoon(url, seconds) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    if (await reaches(url, 3000)) return true;
    if (Date.now() >= until) return false;
    await sleep(1000);
  }
}

// --- The phone ---

/** The controls of `selector`'s parts that are under 44 px tall, which a thumb needs, as "name (width by height)". */
export const smallControls = (page, selector) =>
  page.evaluate(
    (within) =>
      [...document.querySelectorAll(within)]
        .filter((element) => element.offsetParent !== null)
        .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
        .filter(({ box }) => box.height < 43.5)
        .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
    selector,
  );
