// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in one engine, on the machine that updates.toml
// leaves after the first search: Valkey mended and a required check of the
// boot option that fails the start (the second search of startup/bisect.toml),
// restarted into that failed start. This script stops a search between its
// tries, which only a search that waits on its first restart can be.
//
// The Reliability page's card of a failed start (`GET /api/1/starts/failed`,
// reliability.failed) has this start, with the link to it and the command
// that begins the search. What happened offers Start the search, which sets
// the restart for the first try a minute away; Stop the search
// (`POST /api/1/updates/bisect/stop`, updates.bisect-stop) ends it before
// that, and the search says in the machine's words that it was stopped. The
// restart that the search asked for stays set, as `changes bisect --stop`
// leaves it, and Cancel takes it back, so that the machine goes on as it is.
// A reader and a locked administrator are refused the route of the stop and of
// Make sure (`POST /api/1/updates/bisect/confirm`, updates.bisect-confirm), and
// what the machine holds after the stop is its own: updates.toml reads it with
// `changes bisect --status`.

import { ensure, fetchAsPage, lookAll, mustBe, mustHaveCalled, signOut } from "./console.mjs";
import { pressAndAnswer, unlockNow } from "./trial.mjs";
import { ROUTES, card, openUpdates, pathOf, pendingLine, read, routeFor, signedIn, wordsOf } from "./updates-page.mjs";

const ADMIN = "wc-updates";
const READER = "wc-updates-reader";

/** The search as `changes bisect --status --json` says it, through the console. */
async function searchNow(page) {
  const answer = await read(page, ROUTES.bisectStatus);
  ensure(answer.status === 200 && answer.json !== null, `${ROUTES.bisectStatus} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The restart or shut down that waits, as `GET /api/1/power` says it, or null. */
async function restartSet(page) {
  const answer = await read(page, ROUTES.power);
  ensure(answer.status === 200, `${ROUTES.power} was answered ${answer.status}: ${answer.text}`);
  return answer.json?.shutdown ?? null;
}

/** The routes that stop the search and make sure of its answer change the machine, so a reader and a locked administrator are refused. */
async function refused(t, page, who, status, id) {
  for (const route of [ROUTES.bisectStop, ROUTES.bisectConfirm]) {
    mustBe(await fetchAsPage(page, "POST", pathOf(route), {}), status, id, `${who}'s ${route}`);
  }
}

/** The Reliability page has the card of the failed start, with its link and the command that begins the search. */
async function reliabilityHasIt(t, page) {
  const failed = await read(page, "GET /api/1/starts/failed");
  ensure(failed.status === 200 && failed.json?.failed?.start !== undefined, `GET /api/1/starts/failed was answered ${failed.status}: ${failed.text}`);
  await page.goto(new URL("/reliability", page.url()).href);
  const failedCard = page.locator("#ov-failed");
  await t.see(failedCard);
  const link = failedCard.getByRole("link", { name: "Open the start", exact: true });
  ensure((await link.getAttribute("href")) === `/reliability/starts/${failed.json.failed.start}`, `Open the start leads to ${await link.getAttribute("href")}, and the failed start is ${failed.json.failed.start}`);
  ensure((await failedCard.innerText()).includes("(BOOT-103)"), "the card of the failed start does not say BOOT-103, the error of a required check that failed");
  await t.see(failedCard.locator("pre.cmdtext", { hasText: "elevate changes bisect" }));
}

export default async function check(t) {
  const reader = await signedIn(t, READER);
  await refused(t, reader.page, "a reader", 403, "ADM-122");
  await signOut(t, reader.page);
  await reader.page.context().close();

  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await refused(t, page, "a locked administrator", 403, "ADM-106");
  await unlockNow(page);
  await reliabilityHasIt(t, page);

  await openUpdates(t, page, "#happened");
  const box = card(page, "happened");
  await t.see(box.getByRole("heading", { level: 2, name: /^The start .* failed$/ }));
  ensure((await wordsOf(box)).includes("bisect option"), "What happened does not name the check that failed");
  const before = await searchNow(page);
  ensure(before.running === false && before.result == null && before.levers.length >= 2, `the machine has ${JSON.stringify(before)} before any search`);

  await pressAndAnswer(t, page, box.getByRole("button", { name: "Start the search", exact: true }), pathOf(ROUTES.bisect));
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
  const stop = box.getByRole("button", { name: "Stop the search", exact: true });
  await t.see(stop);
  await pressAndAnswer(t, page, stop, pathOf(ROUTES.bisectStop));

  await t.see(box.getByText("The search was stopped.", { exact: false }));
  const stopped = await searchNow(page);
  ensure(stopped.running === false && stopped.result?.end === "stopped", `${ROUTES.bisectStatus} says ${JSON.stringify(stopped.result)} after Stop the search`);
  for (const name of ["Stop the search", "Start the search", "Make sure"]) {
    ensure((await box.getByRole("button", { name, exact: true }).count()) === 0, `the page offers ${name} after the search was stopped`);
  }

  // The restart that the search asked for is still set, and Cancel takes it back.
  const left = await restartSet(page);
  ensure(left?.end === "reboot", `${ROUTES.power} says ${JSON.stringify(left)} after the search was stopped, and its restart is left set`);
  await t.see(pendingLine(page).getByText(/restarts in \d+ seconds?/));
  await pressAndAnswer(t, page, pendingLine(page).getByRole("button", { name: "Cancel", exact: true }), pathOf(ROUTES.cancel));
  await t.gone(pendingLine(page));
  ensure((await restartSet(page)) === null, "the machine still has a restart set after Cancel");
  await lookAll(t, page, "updates-stopped");

  mustHaveCalled(a.seen, [routeFor(ROUTES.bisectStatus), routeFor(ROUTES.bisect), routeFor(ROUTES.bisectStop), routeFor(ROUTES.cancel), "GET /api/1/starts/failed"]);
  await signOut(t, page);
  await page.context().close();
}
