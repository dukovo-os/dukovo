// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the journeys' browser scripts share: the facts a journey's host steps
// leave for them on the lab PC, a sign-in as one of the journey's accounts,
// and a wait for something the machine does a moment after the page asks.
// The console's own helpers (web-console/console.mjs and trial.mjs) are used
// as they are: a journey is told in the console's words and held to the
// machine as the single-feature tests hold it. The scripts import this file
// from beside themselves.

import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

import { ensure, fetchAsPage, openConsole, signInDone, sleep } from "../web-console/console.mjs";

/**
 * The folder where a journey's host steps keep what the scripts read, on the
 * lab PC: the test's own, `{work}`, which the browser runner runs in as the
 * host steps do. It is the machine's own for the run, so a journey can run
 * on two machines at once.
 */
export const journeyFolder = () => process.cwd();

/**
 * The facts the journey's first host step wrote to `info` in its folder, a
 * line each as `name=value`: the lab PC's ports and addresses, which only a
 * host step knows, since the script has no placeholders.
 */
export async function journeyInfo() {
  const text = await readFile(`${journeyFolder()}/info`, "utf8");
  const facts = {};
  for (const line of text.split("\n")) {
    const at = line.indexOf("=");
    if (at > 0) facts[line.slice(0, at)] = line.slice(at + 1);
  }
  return facts;
}

/** Opens the console in a context of its own and signs in as `name`, which the machine already has. */
export async function signedIn(t, name, password, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, password);
  return { page, seen };
}

/** Tries `test` until it stops throwing, for up to `seconds`; throws `what` with the last failure when it never does. */
export async function eventually(test, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    try {
      return await test();
    } catch (error) {
      if (Date.now() >= until) throw new Error(`${what}: ${error.message}`);
      await sleep(1500);
    }
  }
}

/** The text of `locator` with its white space made single. */
export const wordsOf = async (locator) => (await locator.innerText()).replace(/\s+/g, " ").trim();

/** Throws unless the machine's answer to `route`, which the page's own fetch got, is 200; gives its JSON. */
export async function readJson(answer, route) {
  ensure(answer.status === 200 && answer.json !== null, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/**
 * Unlocks the console for the session of `page` with the account's own
 * password, as the unlock dialog does: the console's helpers unlock with the
 * password the single-feature tests give their accounts, which a journey's
 * people do not have.
 */
export async function unlock(page, password) {
  const answer = await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password });
  ensure(answer.status === 200, `the unlock was answered ${answer.status}: ${answer.text}`);
  // The page follows the unlock by the console's stream: it knows within a moment.
  await sleep(1500);
}

/**
 * A second engine for the same story, opened next to the runner's own: the
 * browser kit's engine that is not `t.engine`, so two people are in two
 * engines at once, which the runner's one engine a run cannot give. The kit is
 * found by the folder of engines the runner names for Playwright. Gives
 * `{engine, open(pathname, options), close()}`, or null when the kit has no
 * other engine, in which case the caller uses `t.open` for a second browser of
 * the same engine.
 */
export async function otherEngine(t) {
  const engines = process.env.PLAYWRIGHT_BROWSERS_PATH;
  const wanted = t.engine === "chromium" ? "firefox" : "chromium";
  if (!engines || !existsSync(path.join(path.dirname(engines), "package.json"))) return null;
  const playwright = createRequire(path.join(path.dirname(engines), "package.json"))("playwright");
  const kind = playwright[wanted];
  if (!existsSync(kind.executablePath())) return null;
  const browser = await kind.launch().catch(() => null);
  if (browser === null) return null;
  return {
    engine: wanted,
    async open(pathname, options = {}) {
      const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1280, height: 800 }, locale: "en-US", ...options });
      const page = await context.newPage();
      await page.goto(new URL(pathname, t.url).href, { waitUntil: "load" });
      return page;
    },
    close: () => browser.close(),
  };
}
