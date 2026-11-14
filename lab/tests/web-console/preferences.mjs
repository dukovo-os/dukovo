// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/preferences drives in each engine, with the lab's
// browser runner (lab repository, docs/test-files.md, "Browser tests"): a
// person's preferences of the console, which are keys of their own settings
// file and are read and changed by GET and POST /api/1/account/preferences
// (ADR 0516). An administrator that is new signs in, keeps the time zone
// the welcome page offers, the browser's, and reads them: the defaults,
// `dark`, 100 percent, `full`, commands hidden and the terminal's own,
// with that zone. The session stays locked throughout, since the
// preferences ask for no unlock. One value of each kind the route takes is
// changed, a theme, a size of the text, a way of moving, a time zone of the
// machine's, whether commands are shown and one of the terminal's, and the
// answer and the next read have it. A value that the console does not know
// is refused, as ADM-125 with the name of its key, and changes nothing.
// The defaults, with no zone, are put back at the end.

import { isDeepStrictEqual } from "node:util";

import { ensure, fetchAsPage, mustBe, mustHaveCalled, openConsole, signInDone, signOut } from "./console.mjs";

// The password that preferences.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

const ROUTE = "/api/1/account/preferences";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end.
const ROUTES = {
  read: "GET /api/1/account/preferences",
  change: "POST /api/1/account/preferences",
};

// What a person prefers who has not chosen: the console reads a key that
// is not in their file as its default.
const DEFAULTS = {
  theme: "dark",
  text: 100,
  motion: "full",
  zone: "",
  commands: "hidden",
  terminal: { size: 14, scrollback: 10000, "copy-on-select": false, "keep-days": 7, announce: false, shortcuts: true },
};

// The zone that signInDone keeps for a new account on the welcome page.
const WELCOME_ZONE = "UTC";

// One value of each kind the route takes, each one other than the default.
// A change of the terminal changes only the keys it names.
const CHANGES = [
  { theme: "high-contrast" },
  { text: 125 },
  { motion: "less" },
  { zone: "Europe/Berlin" },
  { commands: "shown" },
  { terminal: { size: 16, "copy-on-select": true } },
];

// Values that the console does not know, each with the key it refuses.
const REFUSED = [
  [{ theme: "purple" }, "theme"],
  [{ text: 130 }, "text"],
  [{ motion: "none" }, "motion"],
  [{ zone: "Mars/Base" }, "zone"],
  [{ commands: "loud" }, "commands"],
  [{ terminal: { size: 30 } }, "terminal.size"],
];

/** Throws unless `actual` holds exactly the preferences `expected`, in any order; `what` names them. */
function mustHold(actual, expected, what) {
  ensure(isDeepStrictEqual(actual, expected), `${what}: ${JSON.stringify(actual)}, not ${JSON.stringify(expected)}`);
}

/** The preferences `now` once `body` is changed: a change of the terminal keeps the keys it does not name. */
function changed(now, body) {
  const terminal = body.terminal === undefined ? now.terminal : { ...now.terminal, ...body.terminal };
  return { ...now, ...body, terminal };
}

/** The preferences that GET gives the account of `page`. */
async function read(page) {
  const answer = await fetchAsPage(page, "GET", ROUTE);
  ensure(answer.status === 200, `GET ${ROUTE} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The preferences, all of them, that POST gives the account of `page` once it has changed what `body` holds. */
async function change(page, body) {
  const answer = await fetchAsPage(page, "POST", ROUTE, body);
  ensure(
    answer.status === 200,
    `POST ${ROUTE} with ${JSON.stringify(body)} was answered ${answer.status}: ${answer.text}`,
  );
  return answer.json;
}

/** The test, run once for each engine of the step that names it. */
export default async function check(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, `wc-preferences-${t.engine}`, PASSWORD);
  let now = { ...DEFAULTS, zone: WELCOME_ZONE };
  mustHold(await read(page), now, "the preferences of a new account");

  for (const body of CHANGES) {
    now = changed(now, body);
    mustHold(await change(page, body), now, `the answer to ${JSON.stringify(body)}`);
    mustHold(await read(page), now, `the read after ${JSON.stringify(body)}`);
  }

  for (const [body, key] of REFUSED) {
    const refused = await fetchAsPage(page, "POST", ROUTE, body);
    mustBe(refused, 400, "ADM-125", `the value ${JSON.stringify(body)}`);
    ensure(
      refused.json.error.field === key,
      `${JSON.stringify(body)} was refused for ${refused.json.error.field}, not for ${key}`,
    );
  }
  mustHold(await read(page), now, "the read after the refusals");

  mustHold(await change(page, DEFAULTS), DEFAULTS, "the answer to the defaults");
  mustHold(await read(page), DEFAULTS, "the read after the defaults");

  mustHaveCalled(seen, [ROUTES.read, ROUTES.change]);
  await signOut(t, page);
}
