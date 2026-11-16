// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/accounts drives in each engine for what an account is
// signed in with (design 7.7), as a step of its own in accounts.toml: the
// part of the test that waits on admind's sessions route (13.3), so that it
// is finished when the route is there and accounts.mjs does not change.
// `GET /api/1/accounts/{name}/sessions` lists the account's console sessions
// and terminals, to the person for their own and to an unlocked administrator
// for another's; `POST /api/1/accounts/{name}/sessions/{id}/end` ends a
// console session as a sign-out does, with the reason that the session's
// browser is told: "you ended this session from another browser." when the
// person ends their own, and "an administrator ended this session." when an
// administrator does; `POST /api/1/terminals/{id}/end` ends a terminal, and
// is open to an unlocked administrator for another account's.
//
// The account is signed in in two browsers and has a terminal. It ends the
// other browser from its own page: the other browser's sign-in page says so.
// An administrator then ends the terminal and the account's session, and the
// account's browser says so within seconds. The session in use is Sign out's:
// the console does not end it from a page and the route answers ADM-136. A
// session that has ended already is ADM-139, a locked administrator is
// ADM-106, and a person who asks for another account's sessions is ADM-122.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  openConsole,
  signIn,
  signOut,
} from "./console.mjs";
import { PASSWORD, unlockNow } from "./trial.mjs";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them.
const ROUTES = {
  sessions: "GET /api/1/accounts/{name}/sessions",
  endSession: "POST /api/1/accounts/{name}/sessions/{id}/end",
  endTerminal: "POST /api/1/terminals/{id}/end",
};

// The name of the terminal that the person opens, which the page shows.
const TERMINAL = "lab-build";

/** The path of a route as the table names it, for the account `name` and the session or terminal `id`. */
const pathOf = (route, name = "", id = "") =>
  route.slice(route.indexOf(" ") + 1).replace("{name}", encodeURIComponent(name)).replace("{id}", encodeURIComponent(id));

/** A pattern for what the browsers' request log writes of `route`, with any account for {name} and any session or terminal for {id}. */
const patternOf = (route) => {
  const escaped = route.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${escaped.replace("\\{name\\}", "[^/]+").replace("\\{id\\}", "[^/]+")}$`);
};

/** Opens the console and signs in as `name`, which must work, and gives the time zone that the welcome page asks for. */
async function signedIn(t, name) {
  const { page, seen } = await openConsole(t, "/");
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await leaveWelcome(t, page);
  return { page, seen };
}

/** What `GET /api/1/accounts/NAME/sessions` answers `page`. */
async function readSessions(page, name) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.sessions, name));
  ensure(answer.status === 200, `${ROUTES.sessions} for ${name} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });

/** Opens the page of the account `name`, or Your account, and waits until its sessions are drawn. */
async function openSessions(t, page, name, heading = name) {
  await page.goto(new URL(`/accounts/${encodeURIComponent(name)}`, page.url()).href);
  await t.see(page.getByRole("heading", { name: heading, level: 1, exact: true }));
  await t.see(page.locator("#signed-in"));
}

/** Presses End `name` on the page and answers its question: gives the answer to the POST to a route matching `route`. */
async function endIt(t, page, button, title, route) {
  await button.click();
  const dialog = page.getByRole("dialog", { name: title });
  await t.see(dialog);
  const answered = page.waitForResponse((answer) => answer.request().method() === "POST" && route.test(new URL(answer.url()).pathname));
  await buttonOf(dialog, "End it").click();
  return answered;
}

/** Waits until `page`, a browser whose session ended, says why on its sign-in page. */
async function mustBeToldWhy(page, words) {
  await page.getByText(words, { exact: false }).waitFor({ timeout: 40_000 });
}

/** The person opens a terminal, which an administrator ends later; gives its id. */
async function openTerminal(page) {
  const made = await fetchAsPage(page, "POST", "/api/1/terminals", { cols: 80, rows: 24, name: TERMINAL });
  ensure(made.status === 200, `the person's terminal was not opened: ${made.status} ${made.text}`);
  const listed = await fetchAsPage(page, "GET", "/api/1/terminals");
  const terminal = (listed.json?.terminals ?? []).find((one) => one.name === TERMINAL || one.title === TERMINAL) ?? listed.json?.terminals?.[0];
  ensure(terminal !== undefined, `the person has no terminal after opening one: ${listed.text}`);
  return terminal.id;
}

/** The route's lists for the person, as an administrator reads them: two browsers and the terminal, and what each says. */
async function theLists(page, who, terminalId) {
  const shown = await readSessions(page, who.person);
  ensure(Array.isArray(shown.sessions) && shown.sessions.length === 2, `${who.person} has ${shown.sessions?.length} console sessions, not 2: ${JSON.stringify(shown)}`);
  for (const session of shown.sessions) {
    ensure(typeof session.id === "string" && session.id !== "" && session.this === false, `a console session of ${who.person} is not as the route says: ${JSON.stringify(session)}`);
    ensure(Number.isFinite(session.since_ms), `a console session of ${who.person} has no start: ${JSON.stringify(session)}`);
  }
  const [terminal] = shown.terminals;
  ensure(shown.terminals.length === 1 && terminal.id === terminalId && terminal.admin === false, `${who.person}'s terminals are not its one: ${JSON.stringify(shown.terminals)}`);
  return shown;
}

/** An administrator who has not unlocked reads the account's own sessions and not another's. */
async function whileLocked(page, who) {
  const own = await readSessions(page, who.admin);
  ensure(own.sessions.some((session) => session.this === true), `the administrator's own session is not the one in use: ${JSON.stringify(own)}`);
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.sessions, who.person)), 403, "ADM-106", "a locked administrator's read of another account's sessions");
}

/** The person ends their other browser from their own page: it is asked first, and that browser says who ended it. */
async function personEndsOther(t, mine, other, who) {
  await openSessions(t, mine, who.person, "Your account");
  const rows = mine.locator("#signed-in tbody tr");
  await t.see(rows.filter({ hasText: "This browser" }));
  ensure((await rows.filter({ hasText: "This browser" }).getByRole("button").count()) === 0, "the session in use has an End button");
  const ends = mine.locator("#signed-in").getByRole("button", { name: /^End the session in / });
  ensure((await ends.count()) === 1, `Your account has ${await ends.count()} End buttons for browsers, not 1`);

  const route = new RegExp(`^/api/1/accounts/${who.person}/sessions/[^/]+/end$`);
  const answered = await endIt(t, mine, ends, /^End the session in /, route);
  ensure(answered.status() === 200, `${ROUTES.endSession} was answered ${answered.status()}: ${await answered.text()}`);
  await mustBeToldWhy(other, "you ended this session from another browser.");
  await lookAll(t, other, "accounts-by-you");
  ensure((await other.request.get(new URL("/api/1/session", other.url()).href)).status() === 401, "the browser that was ended is still signed in");
  await t.gone(ends);
}

/** An administrator ends the account's terminal and then its session: the browser says so, and neither is there after. */
async function administratorEnds(t, page, mine, who, terminalId) {
  await unlockNow(page);
  await openSessions(t, page, who.person);
  const rows = page.locator("#signed-in tbody tr");
  await t.see(rows.filter({ hasText: `Terminal ${TERMINAL}` }));
  await lookAll(t, page, "accounts-sessions");

  const terminal = await endIt(t, page, buttonOf(page, `End the terminal ${TERMINAL}`), `End the terminal ${TERMINAL}?`, new RegExp(`^/api/1/terminals/${terminalId}/end$`));
  ensure(terminal.status() === 200, `${ROUTES.endTerminal} for another account's terminal was answered ${terminal.status()}: ${await terminal.text()}`);
  const left = await fetchAsPage(mine, "GET", "/api/1/terminals");
  ensure(!(left.json?.terminals ?? []).some((one) => one.id === terminalId), "the terminal that an administrator ended is still the person's");
  await t.gone(buttonOf(page, `End the terminal ${TERMINAL}`));

  const route = new RegExp(`^/api/1/accounts/${who.person}/sessions/[^/]+/end$`);
  const ends = page.locator("#signed-in").getByRole("button", { name: /^End the session in / });
  const session = await endIt(t, page, ends, /^End the session in /, route);
  ensure(session.status() === 200, `${ROUTES.endSession} was answered ${session.status()}: ${await session.text()}`);
  await mustBeToldWhy(mine, "an administrator ended this session.");
  ensure((await mine.request.get(new URL("/api/1/session", mine.url()).href)).status() === 401, "the session that an administrator ended is still signed in");
  await t.see(page.locator("#signed-in").getByText(`${who.person} is not signed in.`));
}

/** The session in use is Sign out's, and the route answers ADM-136 for it; a session that has ended already is ADM-139. */
async function guards(t, page, who, gone) {
  await openSessions(t, page, who.admin, "Your account");
  const mineNow = (await readSessions(page, who.admin)).sessions.find((session) => session.this === true);
  ensure(mineNow !== undefined, "the administrator's own session is not listed as the one in use");
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.endSession, who.admin, mineNow.id), {}), 409, "ADM-136", "ending the session in use");
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.endSession, who.person, gone), {}), 404, "ADM-139", "ending a session that has ended");
}

export default async function check(t) {
  const who = { admin: `wc-acc-${t.engine}`, person: `wc-acc-t-${t.engine}` };
  const admin = await signedIn(t, who.admin);
  await whileLocked(admin.page, who);
  await unlockNow(admin.page);

  const mine = await signedIn(t, who.person);
  const other = await signedIn(t, who.person);
  const terminalId = await openTerminal(mine.page);

  // A person is not given another account's sessions.
  mustBe(await fetchAsPage(mine.page, "GET", pathOf(ROUTES.sessions, who.admin)), 403, "ADM-122", "a person's read of another account's sessions");
  const shown = await theLists(admin.page, who, terminalId);
  const own = await readSessions(mine.page, who.person);
  ensure(own.sessions.filter((session) => session.this === true).length === 1, `the person's own browser is not the one session in use: ${JSON.stringify(own)}`);
  const ended = own.sessions.find((session) => session.this === false);
  ensure(shown.sessions.some((session) => session.id === ended.id), "the sessions that the administrator and the person read are not the same");

  await personEndsOther(t, mine.page, other.page, who);
  await administratorEnds(t, admin.page, mine.page, who, terminalId);
  await guards(t, admin.page, who, ended.id);

  // Every route that the table names was called by one of the browsers.
  const seen = [...admin.seen, ...mine.seen];
  const missing = Object.values(ROUTES).filter((route) => !seen.some((called) => patternOf(route).test(called)));
  ensure(missing.length === 0, `no browser called ${missing.join(", ")}`);
  await signOut(t, admin.page);
  for (const one of [admin, mine, other]) await one.page.context().close();
}
