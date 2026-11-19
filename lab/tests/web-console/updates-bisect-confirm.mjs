// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in one engine, with the machine restarting
// by itself for each try and once more for the page's Make sure: the search of
// startup/bisect.toml's second half, begun at the machine's own terminal by
// updates.toml (a page that stopped a search cannot begin another; the stop is
// updates-bisect-stop.mjs's). The failure is the boot option, which a required
// check refuses: try 1 leaves out the system and the options and starts well,
// try 2 leaves out the system alone and fails as the first start did, so the
// option is named by elimination, and no try left out only it.
//
// What happened says in the machine's words that the search found the boot
// options by elimination, lists its two tries, and offers Make sure and
// nothing else (`GET /api/1/updates/bisect`, updates.bisect-status). Make sure
// (`POST /api/1/updates/bisect/confirm`, updates.bisect-confirm) sets the
// restart for one more try, which leaves out only the option: the page counts
// it down, the machine restarts, the third try, which says it confirms, starts
// well, and the search says that the start fails with the boot options and
// starts without them. Nothing is left for Make sure then.

import { ensure, lookAll, mustHaveCalled, signOut, sleep } from "./console.mjs";
import { pressAndAnswer, unlockNow } from "./trial.mjs";
import { backAtThePage, searchEnds } from "./updates-bisect.mjs";
import { ROUTES, card, openUpdates, pathOf, pendingLine, read, routeFor, signedIn, wordsOf } from "./updates-page.mjs";

const ADMIN = "wc-updates";

/** The search as `changes bisect --status --json` says it, through the console; null while the machine restarts. */
async function searchNow(page) {
  const answer = await read(page, ROUTES.bisectStatus).catch(() => null);
  return answer?.status === 200 ? answer.json : null;
}

/** Signs in and opens What happened, again after a restart that came in between: the first try's start has just come up when the page is opened. */
async function openedAtThePage(t) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      const a = await signedIn(t, ADMIN);
      await unlockNow(a.page);
      await openUpdates(t, a.page, "#happened");
      return a;
    } catch (error) {
      ensure(attempt < 8, `the page was not opened in 8 tries: ${error.message}`);
      await sleep(15_000);
    }
  }
}

/** Waits for the third try, the one that confirms, to start well and the search to say so; gives the words of the search card. */
async function confirmEnds(t, page, minutes) {
  const until = Date.now() + minutes * 60_000;
  for (;;) {
    await backAtThePage(t, page);
    const words = await wordsOf(card(page, "happened")).catch(() => "");
    if (/Confirming try: left out .+started well/.test(words) && /and starts without it\./.test(words)) return words;
    ensure(Date.now() < until, `the confirming try did not end the search within ${minutes} minutes: ${words}`);
    await sleep(5000);
  }
}

export default async function check(t) {
  const a = await openedAtThePage(t);
  const { page } = a;
  const box = card(page, "happened");

  // The two tries name the option by elimination, and Make sure is all that is left to press.
  const words = await searchEnds(t, page, 25, /the boot options/);
  ensure(words.includes("named by elimination"), `the search does not say that it named the boot options by elimination: ${words}`);
  const named = await searchNow(page);
  ensure(named?.result?.end === "found" && named.result.confirmed === false && /boot options/.test(named.result.what ?? ""), `${ROUTES.bisectStatus} says ${JSON.stringify(named?.result)}`);
  ensure(named.tries.length === 2 && named.tries.every((one) => one.confirm !== true), `the search made ${JSON.stringify(named.tries)}, two tries that confirm nothing`);
  const tries = box.locator(".upd-tries li");
  ensure((await tries.count()) === 2, `the search lists ${await tries.count()} tries and the machine made two`);
  ensure(/^Try 1: left out .+started well/.test(await wordsOf(tries.nth(0))), `try 1 says ${await wordsOf(tries.nth(0))}`);
  ensure(/^Try 2: left out .+failed as the first start did/.test(await wordsOf(tries.nth(1))), `try 2 says ${await wordsOf(tries.nth(1))}`);
  for (const name of ["Stop the search", "Start the search"]) {
    ensure((await box.getByRole("button", { name, exact: true }).count()) === 0, `the page offers ${name} after the search ended`);
  }
  const makeSure = box.getByRole("button", { name: "Make sure", exact: true });
  await t.see(makeSure);
  await lookAll(t, page, "updates-eliminated");

  await pressAndAnswer(t, page, makeSure, pathOf(ROUTES.bisectConfirm));
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
  const confirmed = await confirmEnds(t, page, 15);
  ensure((await box.getByRole("button", { name: "Make sure", exact: true }).count()) === 0, "the page offers Make sure after the change was confirmed");
  ensure(confirmed.includes("This start is a try of the search."), "the result does not say that this start is a try of the search");
  const done = await searchNow(page);
  ensure(done?.result?.end === "found" && done.result.confirmed === true, `${ROUTES.bisectStatus} says ${JSON.stringify(done?.result)} after Make sure`);
  ensure(done.tries.length === 3 && done.tries[2].confirm === true && done.tries[2].verdict === "good", `the search made ${JSON.stringify(done.tries)}, and the third try confirms and starts well`);
  await lookAll(t, page, "updates-confirmed");

  mustHaveCalled(a.seen, [routeFor(ROUTES.bisectStatus), routeFor(ROUTES.bisectConfirm), routeFor(ROUTES.power)]);
  await signOut(t, page);
  await page.context().close();
}
