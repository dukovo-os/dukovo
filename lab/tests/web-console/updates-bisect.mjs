// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in one engine, with the machine restarting
// twice by the search that the page starts: Find which change broke it
// (`changes bisect`, ADR 0475) on the machine that startup/bisect.toml's first
// steps leave, and that updates.toml makes the same way. Four changes came
// between a good start and a failed one: an update, the boot option that
// update starts with, and two apps, Valkey and nginx; the start fails at
// Valkey, a setting it does not know, and the health check names it an
// essential service.
//
// What happened says that the start failed, with its error ID and the
// machine's own words, and lists the changes since the last good start. The
// search says what it does before it starts, in words and with the machine's
// own count ("4 changes take at most 2 restarts"), lists the changes that it
// can leave out and those it cannot (`GET /api/1/updates/bisect`'s `levers` and
// `cannot`), and Start the search (`POST /api/1/updates/bisect`) sets the
// restart that the page counts down and the shell's banner tells of. The
// machine restarts by itself 60 seconds after each try's result; the page
// shows each try with what it left out and how it went, and when the search
// ends it says in the machine's words which change it found, confirmed, and
// that this start is a try of the search. A search that has found its change
// by two tries has nothing left for Make sure to confirm.
//
// Stop the search, which only a search between tries has, and Make sure, which
// only a change named by elimination has (a power cut in the machine's second
// search), are not driven here: startup/bisect.toml has the second search, and
// its power cut is the boot test's.

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
import { ROUTES, card, openUpdates, pathOf, pendingLine, routeFor, signedIn, wordsOf } from "./updates-page.mjs";

const ADMIN = "wc-updates";

/** The most restarts that a search of `count` changes takes: the base 2 logarithm, rounded up. */
const mostRestarts = (count) => (count < 2 ? 0 : Math.ceil(Math.log2(count)));

/** `changes bisect --status --json`, through the console. */
async function statusNow(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.bisectStatus));
  ensure(answer.status === 200 && answer.json !== null, `${ROUTES.bisectStatus} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** What happened says that the start failed, and the search says what it will do before it does it. */
async function beforeTheSearch(t, page) {
  const box = card(page, "happened");
  await t.see(box);
  await t.see(box.getByRole("heading", { level: 2, name: /^The start .* failed$/ }));
  ensure((await box.locator('a[href^="/help/errors/"]').count()) >= 1, "What happened links no error ID");
  await t.see(box.getByText(/io\.valkey\.valkey/).first());
  await t.see(box.getByRole("link", { name: "Report it", exact: true }));

  const status = await statusNow(page);
  ensure(status.running === false && Array.isArray(status.levers) && status.levers.length === 4, `the machine can leave out ${JSON.stringify(status.levers)}, not four changes`);
  await t.see(box.getByRole("heading", { level: 3, name: "Find which change broke it", exact: true }));
  const words = await wordsOf(box);
  const most = mostRestarts(status.levers.length);
  ensure(words.includes(`${status.levers.length} changes take at most ${most} restarts`), `the search does not say that 4 changes take at most ${most} restarts: ${words}`);
  ensure(words.includes("The machine restarts 60 seconds after each try's result"), "the search does not say when the machine restarts between tries");
  ensure((await box.locator(".upd-search ul").first().locator("li").count()) === status.levers.length, "the search does not list each change that it can leave out");
  ensure(/app io\.valkey\.valkey/.test(words) && /app org\.nginx\.nginx/.test(words), `the search does not name the two apps: ${words}`);
  for (const one of status.cannot ?? []) ensure(words.includes(one.what), `the search does not say that it cannot leave out ${one.what}`);
  return status;
}

/** Signs in again if the restarts took the session, and shows What happened. */
async function backAtThePage(t, page) {
  if (await signInHeading(page).isVisible().catch(() => false)) {
    await signInDone(t, page, ADMIN, PASSWORD);
    await unlockNow(page);
    await openUpdates(t, page, "#happened");
  }
}

/** Waits for the search to end, for up to `minutes`, while the machine restarts for each try; gives the words of the search card. */
async function searchEnds(t, page, minutes) {
  const until = Date.now() + minutes * 60_000;
  for (;;) {
    await backAtThePage(t, page);
    const words = await wordsOf(card(page, "happened")).catch(() => "");
    if (/(The start fails with|Found:) io\.valkey\.valkey/.test(words)) return words;
    ensure(Date.now() < until, `the search did not end within ${minutes} minutes: ${words}`);
    await sleep(5000);
  }
}

async function afterTheSearch(t, page, words) {
  const box = card(page, "happened");
  const tries = box.locator(".upd-tries li");
  ensure((await tries.count()) === 2, `the search lists ${await tries.count()} tries and the machine made two`);
  ensure(/^Try 1: left out .+Try 1 of 2 failed as the first start did/.test(await wordsOf(tries.nth(0))), `try 1 says ${await wordsOf(tries.nth(0))}`);
  ensure(/^Try 2: left out .+Try 2 of 2 started well/.test(await wordsOf(tries.nth(1))), `try 2 says ${await wordsOf(tries.nth(1))}`);
  ensure(words.includes("This start is a try of the search."), "the result does not say that this start is a try of the search");
  for (const name of ["Make sure", "Stop the search", "Start the search"]) {
    ensure((await box.getByRole("button", { name, exact: true }).count()) === 0, `the page offers ${name} after a search that named its change by two tries`);
  }
  const status = await statusNow(page);
  ensure(status.result?.end === "found" && /valkey/.test(status.result.what ?? ""), `${ROUTES.bisectStatus} says ${JSON.stringify(status.result)}`);
  ensure(status.result.confirmed === true, "the machine named Valkey by elimination, and two tries confirm it");
}

export default async function check(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await unlockNow(page);
  await openUpdates(t, page, "#happened");
  await beforeTheSearch(t, page);
  await lookAll(t, page, "updates-search");

  const box = card(page, "happened");
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Start the search", exact: true }), pathOf(ROUTES.bisect));
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
  // Two restarts, each a minute after the one before has a result, and the second after a start that fails its check.
  const words = await searchEnds(t, page, 25);
  await afterTheSearch(t, page, words);
  await lookAll(t, page, "updates-searched");

  mustHaveCalled(a.seen, [routeFor(ROUTES.failed), routeFor(ROUTES.bisectStatus), routeFor(ROUTES.bisect), routeFor(ROUTES.power)]);
  await signOut(t, page);
  await page.context().close();
}
