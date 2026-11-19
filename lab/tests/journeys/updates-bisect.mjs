// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The second browser part of journeys/updates: a start that failed, because a
// bad change came among four, and the owner finds which with the search that
// the Updates page starts (`changes bisect`, ADR 0475). What happened names the
// start that failed and the change that caused it, the search says what it
// will do, and the machine restarts by itself for each try while the page,
// which each restart cuts off, asks to sign in again. When the search ends the
// page names the change it found and says it was confirmed. updates.toml made
// the four changes, as web-console/updates does, and looks at the machine
// after.

import { ensure, fetchAsPage, lookAll, signInHeading, sleep } from "../web-console/console.mjs";
import { pressAndAnswer, unlockNow } from "../web-console/trial.mjs";
import { ROUTES, card, openUpdates, pathOf, pendingLine, signInInTry, signedIn, wordsOf } from "../web-console/updates-page.mjs";

const ADMIN = "journey-admin";

/** The most restarts that a search of `count` changes takes: the base 2 logarithm, rounded up. */
const mostRestarts = (count) => (count < 2 ? 0 : Math.ceil(Math.log2(count)));

/** `changes bisect --status --json`, through the console. */
async function statusNow(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.bisectStatus));
  ensure(answer.status === 200 && answer.json !== null, `${ROUTES.bisectStatus} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Signs in again if a restart took the session, and shows What happened. */
async function backAtThePage(t, page) {
  if (!(await signInHeading(page).isVisible().catch(() => false))) return;
  await signInInTry(t, page, ADMIN, async () => {
    await unlockNow(page);
    await openUpdates(t, page, "#happened");
  });
}

export default async function check(t) {
  const { page } = await signedIn(t, ADMIN);
  await unlockNow(page);
  await openUpdates(t, page, "#happened");

  // What happened says which start failed and why, and the search says what it will do.
  const box = card(page, "happened");
  await t.see(box.getByRole("heading", { level: 2, name: /^The start .* failed$/ }));
  await t.see(box.getByText(/io\.valkey\.valkey/).first());
  const status = await statusNow(page);
  ensure(status.running === false && Array.isArray(status.levers) && status.levers.length === 4, `the machine can leave out ${JSON.stringify(status.levers)}, not four changes`);
  const words = await wordsOf(box);
  const most = mostRestarts(status.levers.length);
  ensure(words.includes(`${status.levers.length} changes take at most ${most} restarts`), `the search does not say that 4 changes take at most ${most} restarts: ${words}`);
  await lookAll(t, page, "journey-bisect-before");

  await pressAndAnswer(t, page, box.getByRole("button", { name: "Start the search", exact: true }), pathOf(ROUTES.bisect));
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));

  // Two restarts, each a minute after the one before has a result.
  const until = Date.now() + 25 * 60_000;
  let found = "";
  for (;;) {
    await backAtThePage(t, page);
    found = await wordsOf(card(page, "happened")).catch(() => "");
    if (/(The start fails with|Found:) io\.valkey\.valkey/.test(found)) break;
    ensure(Date.now() < until, `the search did not end within 25 minutes: ${found}`);
    await sleep(5000);
  }
  const result = (await statusNow(page)).result;
  ensure(result?.end === "found" && /valkey/.test(result.what ?? "") && result.confirmed === true, `the search ends with ${JSON.stringify(result)}, not Valkey, confirmed`);
  ensure(found.includes("This start is a try of the search."), "the result does not say that this start is a try of the search");
  await lookAll(t, page, "journey-bisect-after");
}
