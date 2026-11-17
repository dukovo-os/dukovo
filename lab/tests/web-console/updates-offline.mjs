// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives with the update server not answering, in one
// engine: Check now (`update check --json`, `GET /api/1/updates/check`) when
// the machine's updates.toml names a server that is not there. The command
// still answers (exit 0), and says in `unchecked` which part it could not
// check and why; the page says "Could not check: WHY." on each such card, in
// the machine's words, and never says that a part it could not check is up to
// date. The old check, the one the daily check kept, stays on the page until
// the next one is made. updates.toml points the machine at a port where
// nothing listens before this script and back at the lab PC's server after it.

import { ensure, lookAll, mustHaveCalled, signOut } from "./console.mjs";
import { ROUTES, card, checkedLine, openUpdates, pathOf, routeFor, signedIn, wordsOf } from "./updates-page.mjs";

const ADMIN = "wc-updates";

/** The parts of the page that a check's `unchecked` names, by the card that says it. */
const CARD_OF = { system: "system", firmware: "firmware", security: "security", apps: "apps" };

export default async function check(t) {
  const { page, seen } = await signedIn(t, ADMIN);
  await openUpdates(t, page);
  const before = await wordsOf(checkedLine(page));

  const answered = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.check));
  await page.getByRole("button", { name: "Check now", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `${ROUTES.check} was answered ${answer.status()} with the update server away: ${await answer.text()}`);
  const doc = await answer.json();
  ensure(Array.isArray(doc.unchecked) && doc.unchecked.length > 0, `the check named no part it could not check: ${JSON.stringify(doc.unchecked)}`);

  for (const part of doc.unchecked) {
    const id = CARD_OF[part.item];
    if (id === undefined) continue;
    const words = await wordsOf(card(page, id));
    ensure(/Could not check[:.]/.test(words), `the ${id} card does not say it could not check: ${words}`);
    if (typeof part.why === "string" && part.why !== "") {
      ensure(words.includes(part.why.replace(/\.$/, "")), `the ${id} card does not give the reason "${part.why}": ${words}`);
    }
  }
  const system = await wordsOf(card(page, "system"));
  ensure(!/Up to date/.test(system), `the System card says it is up to date when it could not check: ${system}`);
  console.log(`the checked line said "${before}" before the check and says "${await wordsOf(checkedLine(page))}" after it`);

  // The change buttons stay: a machine that cannot reach its server may still hold a version or change its settings.
  await t.see(card(page, "settings").getByRole("button", { name: "Save", exact: true }));
  await lookAll(t, page, "updates-offline");

  mustHaveCalled(seen, [routeFor(ROUTES.check), routeFor(ROUTES.last)]);
  await signOut(t, page);
  await page.context().close();
}
