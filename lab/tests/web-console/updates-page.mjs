// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the web console's Updates scripts share (updates.mjs, updates-offline.mjs,
// updates-restart.mjs, updates-back.mjs and updates-bisect.mjs): the routes the
// page calls, its cards as locators, opening the page and waiting until its
// first read is drawn, the machine's own answers read through the console, and
// the words that a card says for each state of an offer. The scripts import it
// from beside themselves.

import { ensure, fetchAsPage, openConsole, signInDone, sleep } from "./console.mjs";
import { PASSWORD } from "./trial.mjs";

/** The routes that lab/actions.toml names for web-console/updates, as the console's table writes them. */
export const ROUTES = {
  last: "GET /api/1/updates",
  check: "GET /api/1/updates/check",
  slots: "GET /api/1/updates/slots",
  history: "GET /api/1/updates/history",
  advisories: "GET /api/1/updates/advisories",
  settings: "GET /api/1/updates/settings",
  set: "POST /api/1/updates/settings",
  channel: "POST /api/1/updates/channel",
  checks: "GET /api/1/updates/checks",
  checkAdd: "POST /api/1/updates/checks",
  checkRemove: "POST /api/1/updates/checks/{name}/remove",
  essential: "POST /api/1/updates/essential",
  apply: "POST /api/1/updates/{item}/apply",
  unstage: "POST /api/1/updates/unstage",
  hold: "POST /api/1/updates/{item}/hold",
  unhold: "POST /api/1/updates/{item}/unhold",
  undo: "POST /api/1/updates/{item}/undo",
  later: "POST /api/1/updates/{item}/later",
  failed: "GET /api/1/updates/failed",
  bisectStatus: "GET /api/1/updates/bisect",
  bisect: "POST /api/1/updates/bisect",
  power: "GET /api/1/power",
  restart: "POST /api/1/power/restart",
  cancel: "POST /api/1/power/cancel",
};

/** The path of `route`, with `{item}` and `{name}` filled in from `fill`. */
export const pathOf = (route, fill = {}) =>
  route
    .slice(route.indexOf(" ") + 1)
    .replace("{item}", encodeURIComponent(fill.item ?? ""))
    .replace("{name}", encodeURIComponent(fill.name ?? ""));

/** The route as `track` writes it, `METHOD /path`. */
export const routeFor = (route, fill = {}) => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, fill)}`;

// The accounts and the words that the scripts look for.
export const READER_WORDS = "An administrator can stage updates, hold versions and change these settings.";
export const LOCKED_SLOTS = "Unlock to see the version kept for going back.";

// --- The page ---

export const card = (page, id) => page.locator(`main section#${id}`);
export const checkedLine = (page) => page.locator("main .upd-checked");
export const bannerOf = (page) => page.locator("main .upd > .banner.info");
export const pendingLine = (page) => card(page, "system").locator(".banner.info");

/** The text of a card with its white space made single, for a test to match. */
export const wordsOf = async (locator) => (await locator.innerText()).replace(/\s+/g, " ").trim();

/** Opens a console in a context of its own and signs in as `name`. */
export async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

/**
 * Opens /updates, optionally at `hash`, and waits for the first read to be
 * drawn: the cards are there and the checked line no longer says it is
 * reading.
 */
export async function openUpdates(t, page, hash = "") {
  await page.goto(new URL(`/updates${hash}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Updates", level: 1, exact: true }));
  await page.waitForFunction(
    () => document.querySelector("main section#system") !== null && !/^Reading/.test(document.querySelector("main .upd-checked")?.textContent ?? "Reading"),
    null,
    { timeout: 60_000 },
  );
}

// --- The machine, read through the console ---

/** What a read route answers, `{status, json}`, as the page's own fetch gets it. */
export async function read(page, route, fill = {}) {
  const answer = await fetchAsPage(page, route.slice(0, route.indexOf(" ")), pathOf(route, fill));
  return answer;
}

/** `update check --json`, asked of the update server through `GET /api/1/updates/check`. */
export async function checkNow(page) {
  const answer = await read(page, ROUTES.check);
  ensure(answer.status === 200 && Array.isArray(answer.json?.offers), `${ROUTES.check} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** `update settings --json`. */
export async function settingsNow(page) {
  const answer = await read(page, ROUTES.settings);
  ensure(answer.status === 200 && answer.json !== null, `${ROUTES.settings} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The offer for `item` in a check document, or null. */
export const offerOf = (doc, item) => doc.offers.find((offer) => offer.item === item) ?? null;

/** The offers of apps in a check document. */
export const appOffers = (doc) => doc.offers.filter((offer) => offer.item !== "system" && offer.item !== "firmware");

/** The sentence the System card's title says for `offer`, as a pattern. */
export function systemWords(offer) {
  if (offer === null) return /Nothing is offered\.|Not checked yet\./;
  const to = offer.to ? offer.to.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : "The update";
  switch (offer.state) {
    case "current":
      return /Up to date/;
    case "update":
      return new RegExp(`${to} is ready`);
    case "held":
      return new RegExp(`${to} is held`);
    case "waiting":
      return new RegExp(`${to} reaches this machine on day`);
    case "withdrawn":
      return new RegExp(`${to} was withdrawn`);
    default:
      return /Nothing is offered\./;
  }
}

/** The sentence the Firmware card's title says for `offer`, as a pattern. */
export function firmwareWords(offer) {
  if (offer === null || offer.state === "current") return /up to date|Firmware set|Not checked yet/;
  const to = offer.to.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return offer.state === "held" ? new RegExp(`${to} is held`) : new RegExp(`${to} is offered`);
}

/** Waits until the machine's console answers again after a restart, for up to `seconds`. Gives how long it took. */
export async function consoleReturns(page, seconds) {
  const began = Date.now();
  const until = began + seconds * 1000;
  for (;;) {
    const answer = await page.evaluate(() => fetch("/api/1/health", { cache: "no-store" }).then((reply) => reply.status, () => 0));
    if (answer === 200) return Math.round((Date.now() - began) / 1000);
    ensure(Date.now() < until, `the console did not answer again within ${seconds} seconds`);
    await sleep(2000);
  }
}
