// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/shell drives in one engine while a trial of the
// firewall's rules runs that a step of shell.toml began at the machine's own
// console, with no browser involved: the chip in the header of every page
// but the firewall's (C5 design 3.4). Three tabs of one browser context, on
// the Overview, Services and Logs, each show the chip within 35 seconds (a
// tab that opens is told at once, and one that was open would be told at the
// next of admind's 30-second rounds); it says what is tried, counts the time
// left down by the browser's clock, links to /firewall and says nothing
// aloud; on /firewall, whose banner says the same, it is hidden; and when
// Spark undoes the trial at the end of its window, which nobody kept, the
// chip goes from the tabs that are still open, within one more round.
//
// The accounts are shell.toml's, an administrator for each engine. The
// trial's window is longer than this script needs to look at the chip, so
// that the end is the one thing it waits for.

import { ensure, lookAll, mustHaveCalled, openConsole, signInDone, signOut, sleep } from "./console.mjs";
import { PASSWORD, bannerOf } from "./trial.mjs";

/** The pages the tabs are on, none of them the firewall's. */
const PATHS = ["/", "/services", "/logs"];

/** How long a chip may take to show once a trial runs, in milliseconds: admind's round is 30 seconds. */
const SHOWN_WITHIN = 35_000;

/** The chip's accessible name, which has its words and when it is undone. */
const CHIP_NAME = /^Trying: the firewall's rules, undone at .+ unless kept$/;

/** The chip of the trial, a link in the header. */
const chipOf = (page) => page.getByRole("banner").getByRole("link", { name: CHIP_NAME });

/** What the chip's clock says, in seconds. */
async function secondsLeft(page) {
  const text = (await chipOf(page).locator(".clock").textContent()).trim();
  const match = /^(\d+):(\d\d)$/.exec(text);
  ensure(match !== null, `the chip's clock says "${text}", not minutes and seconds`);
  return Number(match[1]) * 60 + Number(match[2]);
}

/** Waits for the chip to show on `page`, which is on `path`. */
async function seeChip(page, path) {
  try {
    await chipOf(page).waitFor({ state: "visible", timeout: SHOWN_WITHIN });
  } catch {
    throw new Error(`the tab of ${path} shows no chip ${SHOWN_WITHIN / 1000} seconds after the trial began`);
  }
}

/** The chip says what is tried, links to the firewall's page and counts down; it is not a live region. */
async function words(page, path) {
  const chip = chipOf(page);
  const said = (await chip.innerText()).replace(/\s+/g, " ").trim();
  ensure(said.startsWith("Trying: the firewall's rules"), `the chip on ${path} says "${said}"`);
  ensure((await chip.getAttribute("href")) === "/firewall", `the chip on ${path} links to ${await chip.getAttribute("href")}`);
  const before = await secondsLeft(page);
  ensure(before > 0 && before <= 150, `the chip's clock says ${before} seconds on ${path}, of a window of 150`);
  await sleep(3000);
  const after = await secondsLeft(page);
  ensure(after < before && before - after <= 6, `the chip's clock went from ${before} to ${after} seconds in three`);
  const loud = await chip.evaluate((link) => link.closest("[aria-live], [role=status], [role=alert]") !== null);
  ensure(!loud, `the chip on ${path} is in a live region, which would read it out`);
  return after;
}

export default async function check(t) {
  const name = `wc-shell-${t.engine}`;
  const { page: first, seen } = await openConsole(t, "/");
  await signInDone(t, first, name, PASSWORD);

  const context = first.context();
  const tabs = [first, await context.newPage(), await context.newPage()];
  await Promise.all(tabs.map((tab, at) => tab.goto(new URL(PATHS[at], first.url()).href)));
  await Promise.all(tabs.map((tab, at) => seeChip(tab, PATHS[at])));

  // The same chip on every tab, with the time left that the same trial has.
  const left = [];
  for (const [at, tab] of tabs.entries()) left.push(await words(tab, PATHS[at]));
  ensure(Math.max(...left) - Math.min(...left) <= 12, `the tabs' clocks say ${left.join(", ")} seconds of one trial`);
  await lookAll(t, tabs[1], "trial-chip");

  // The chip is a link to the page that has the trial's banner, and is not
  // there: the banner says the same.
  await chipOf(first).click();
  await t.see(first.getByRole("heading", { name: "Firewall", level: 1 }));
  ensure(new URL(first.url()).pathname === "/firewall", `the chip led to ${first.url()}`);
  await t.see(bannerOf(first));
  await t.gone(chipOf(first));
  ensure((await chipOf(tabs[1]).isVisible()) && (await chipOf(tabs[2]).isVisible()), "the other tabs lost their chips when one went to the firewall");

  // Nobody keeps the trial, so Spark undoes it at the end of its window and
  // the console tells the tabs within a round after that.
  const untilEnd = left[1] * 1000;
  for (const [at, tab] of [[1, tabs[1]], [2, tabs[2]]]) {
    try {
      await chipOf(tab).waitFor({ state: "hidden", timeout: untilEnd + SHOWN_WITHIN + 5000 });
    } catch {
      throw new Error(`the chip of the tab of ${PATHS[at]} did not go within ${SHOWN_WITHIN / 1000} seconds of the trial's end`);
    }
  }
  await first.getByRole("link", { name: "Dukovo Admin, Overview" }).click();
  await t.see(first.getByRole("heading", { name: "Overview", level: 1 }));
  await sleep(2000);
  ensure(!(await chipOf(first).isVisible()), "the chip came back on the Overview after the trial ended");

  mustHaveCalled(seen, ["GET /api/1/events", "GET /api/1/firewall"]);
  await signOut(t, first);
  await context.close();
}
