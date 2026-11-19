// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The console's part of journeys/data-safety, after the cord was pulled in
// the middle of a backup. The administrator opens Snapshots and backups and
// the page says what the machine knows: both places, the last good backup to
// the folder on the disk is the one before the cut, nothing is running, and
// the stick that was taken away is not plugged in. The reader then tries to
// restore the administrator's files, which the machine refuses, and the
// administrator's folder is as it was (data-safety.toml looks at it after).

import { ensure, fetchAsPage, lookAll, mustBe, signOut } from "../web-console/console.mjs";
import { infoOf, readJson, signedIn, unlock, wordsOf } from "./journey.mjs";

/** What the machine says of its backup places, as `backup list --json` says it, through the console. */
async function placesNow(page) {
  const listed = await readJson(await fetchAsPage(page, "GET", "/api/1/backups"), "GET /api/1/backups");
  ensure(Array.isArray(listed.places), `GET /api/1/backups gives no places: ${JSON.stringify(listed)}`);
  return listed.places;
}

async function administrator(t, info) {
  const { page } = await signedIn(t, info.alice, info.alice_password);
  await unlock(page, info.alice_password);
  await page.goto(new URL("/snapshots", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Snapshots and backups", level: 1, exact: true }));

  const places = await placesNow(page);
  const shelf = places.find((place) => place.name === "shelf");
  const stick = places.find((place) => place.name === "stick");
  ensure(shelf !== undefined && stick !== undefined, `the machine has the places ${places.map((place) => place.name).join(", ")}, not shelf and stick`);
  ensure(shelf.running === false, "the machine says a backup to shelf is running after the cord was pulled");
  ensure(shelf.state.done_ms !== null, "the machine has no good backup to shelf, and the earlier one was whole");
  ensure(!shelf.state.failed, `the machine says the last backup to shelf failed: ${shelf.state.failed}`);
  ensure(shelf.state.tried_ms >= shelf.state.done_ms, "the machine's last try at shelf is older than its last good backup");
  ensure(stick.plugged_in === false, `the machine says the stick is plugged in after it was taken away: ${JSON.stringify(stick.plugged_in)}`);

  // The shelf lists only backups that were whole, none later than its last good round: the one that was cut off is not among them.
  const shown = await readJson(await fetchAsPage(page, "GET", "/api/1/backups/shelf"), "GET /api/1/backups/shelf");
  const times = shown.places.flatMap((one) => one.backups.map((backup) => backup.at_ms));
  ensure(times.length > 0 && times.every((at) => at <= shelf.state.done_ms), `the shelf lists backups at ${JSON.stringify(times)}, and its last good round ended at ${shelf.state.done_ms}`);

  // The page draws both places, and says nothing is being backed up.
  await page.waitForFunction(() => document.body.innerText.includes("shelf") && document.body.innerText.includes("stick"), null, { timeout: 30_000 });
  const words = await wordsOf(page.locator("main"));
  console.log(`Snapshots and backups says: ${words.slice(0, 1500)}`);
  ensure(!/Backing up|Running now/i.test(words), `the page says a backup runs: ${words}`);
  await lookAll(t, page, "journey-backups-after-cut");
  await signOut(t, page);
  await page.context().close();
}

async function reader(t, info) {
  const { page } = await signedIn(t, info.bob, info.bob_password);
  const asked = await fetchAsPage(page, "POST", "/api/1/snapshots/restore", {
    snapshot: "journey-before",
    label: "taken",
    paths: `/users/${info.alice}/docs/notes.txt`,
  });
  ensure(asked.status >= 400, `the machine let a reader restore ${info.alice}'s file: ${asked.status} ${asked.text}`);
  mustBe(await fetchAsPage(page, "POST", "/api/1/snapshots/restore/all", { snapshot: "journey-before", label: "taken", paths: `/users/${info.alice}/docs/notes.txt` }), 403, "ADM-122", "a reader's restore of every file");
  mustBe(await fetchAsPage(page, "GET", "/api/1/snapshots/browse/all?place=%2Fusers&snapshot=journey-before"), 403, "ADM-122", "a reader's look at every file of a snapshot");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const info = await infoOf("data-safety");
  await administrator(t, info);
  await reader(t, info);
}
