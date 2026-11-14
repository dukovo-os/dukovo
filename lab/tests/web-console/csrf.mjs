// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/csrf drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): every
// route of the console that changes anything, asked for as another site
// would ask, and refused. An administrator signs in, and the test posts to
// each of the 21 routes with what a forger can send: no Origin, the Origin
// of another site, the console's own Origin said to come from another
// site, no token, a token that is not the session's. Every one is refused
// with 403, and the session still stands afterwards, locked or unlocked as
// it was. The sign-in routes have no token, so they are given the first
// four forgeries alone. A page of another origin, which the browser opens,
// then posts to the same routes, and the browser itself withholds the
// session's cookie from it.
//
// The requests of the first part are the browser context's own API calls,
// which carry its cookies and nothing a browser adds, so every header that
// decides the answer is the test's. Their control is the page's own
// request, which the console takes.

import { randomBytes } from "node:crypto";

import {
  cookieNamed,
  ensure,
  look,
  mustHaveCalled,
  mustRefuse,
  openConsole,
  originOf,
  postAsPage,
  sessionOf,
  signInDone,
  signInHeading,
  signOut,
  signOutButton,
} from "./console.mjs";

// The password that csrf.toml gives the account it makes.
const PASSWORD = "lab-console-password";
const NAME = "wc-csrf";

// The routes that sign a browser in. They have no session yet, so they
// have no token: the Origin and the Sec-Fetch-Site of the request are all
// that keep another site from signing a browser in as someone else. Each
// is given a body that would do harm if the console took it.
const SIGN_IN_ROUTES = [
  ["POST /api/1/sign-in", { name: NAME, password: PASSWORD }],
  ["POST /api/1/sign-in/code", { ticket: "00", code: "000000" }],
  ["POST /api/1/sign-in/passkey/options", {}],
  ["POST /api/1/sign-in/passkey", { credential: {} }],
];

// The routes of a signed-in session, which ask for its token as well.
const SESSION_ROUTES = [
  ["POST /api/1/sign-out", {}],
  ["POST /api/1/unlock", { kind: "password", password: PASSWORD }],
  ["POST /api/1/unlock/passkey/options", {}],
  ["POST /api/1/unlock/extend", {}],
  ["POST /api/1/lock", {}],
  ["POST /api/1/account/second-step/app/begin", { proof: PASSWORD }],
  ["POST /api/1/account/second-step/app/confirm", { code: "000000" }],
  ["POST /api/1/account/second-step/app/remove", { proof: PASSWORD }],
  ["POST /api/1/account/second-step/passkey/begin", { proof: PASSWORD }],
  ["POST /api/1/account/second-step/passkey/add", { name: "forged", credential: {} }],
  ["POST /api/1/account/second-step/passkey/remove", { proof: PASSWORD, id: "00" }],
  ["POST /api/1/account/second-step/passkey/rename", { id: "00", name: "forged" }],
  ["POST /api/1/account/second-step/codes", { proof: PASSWORD }],
  ["POST /api/1/account/second-step/trusted/forget", {}],
  ["POST /api/1/account/tokens/add", { proof: PASSWORD, name: "forged" }],
  ["POST /api/1/account/tokens/remove", { id: "00" }],
  ["POST /api/1/account/preferences", { theme: "light" }],
];

/** The path of a route written as `METHOD /path`. */
const pathOf = (route) => route.split(" ")[1];

/**
 * The headers of the forgeries to try on a route, by what they get wrong.
 * `origin` is the console's own, and `token` the session's. A sign-in route
 * has no token to get wrong.
 */
function forgeries(origin, token, withToken) {
  const wrongToken = randomBytes(32).toString("base64url");
  const ours = { Origin: origin, "X-Dukovo-CSRF": token };
  const all = [
    ["no Origin", { "X-Dukovo-CSRF": token }],
    ["an Origin of another site", { ...ours, Origin: "https://evil.example" }],
    ["its own Origin, said to come from the same site", { ...ours, "Sec-Fetch-Site": "same-site" }],
    ["its own Origin, said to come from another site", { ...ours, "Sec-Fetch-Site": "cross-site" }],
    ["no token", { Origin: origin }],
    ["a token that is not the session's", { Origin: origin, "X-Dukovo-CSRF": wrongToken }],
  ];
  return withToken ? all : all.slice(0, 4);
}

/** Posts to each of `routes` as each forgery does, and sees every one refused as foreign. */
async function forge(page, routes, token, withToken) {
  const origin = originOf(page);
  for (const [route, body] of routes) {
    for (const [what, headers] of forgeries(origin, token, withToken)) {
      const answer = await page.request.post(`${origin}${pathOf(route)}`, { headers, data: body });
      await mustRefuse(answer, 403, "ADM-109").catch((error) => {
        throw new Error(`${route} with ${what}: ${error.message}`);
      });
      ensure(!answer.headers()["set-cookie"], `${route} with ${what} set a cookie`);
    }
  }
}

/** The requests of the page itself are taken, so that the refusals are of the forgery alone. */
async function controls(page, token) {
  const origin = originOf(page);
  const own = { Origin: origin, "Sec-Fetch-Site": "same-origin" };
  // A sign-in with nothing to prove passes the Origin check and is
  // refused for what it holds.
  const empty = await page.request.post(`${origin}/api/1/sign-in`, { headers: own, data: { name: "", password: "" } });
  await mustRefuse(empty, 401, "ADM-101");
  const lock = await page.request.post(`${origin}/api/1/lock`, {
    headers: { ...own, "X-Dukovo-CSRF": token },
    data: {},
  });
  ensure(lock.status() === 200, `the page's own POST /api/1/lock was refused: ${lock.status()}`);
}

/** The session is as it was: signed in, and locked or unlocked as before. */
async function stillStanding(t, page, before) {
  const session = await sessionOf(page);
  ensure(session.name === NAME, `the session names ${session.name}, not ${NAME}`);
  if (before === null) {
    ensure(session.unlock_seconds === null, `the console was locked, and is unlocked for ${session.unlock_seconds} seconds`);
  } else {
    // An extension would have made it 15 minutes again, and a lock null.
    ensure(
      session.unlock_seconds > 0 && session.unlock_seconds <= before,
      `the console was unlocked for ${before} seconds, and is for ${session.unlock_seconds}`,
    );
  }
  // What the page shows follows what the server says.
  await page.reload();
  await t.see(signOutButton(page));
  await t.see(page.getByRole("banner").getByText(before === null ? "Limited access" : "Unlocked ·"));
}

/**
 * A page of another origin, open in the same browser, posts a form to each
 * route, as a site that tries a request on someone's behalf does. The
 * browser sends its own Origin and withholds the session's cookie, which
 * the cookie's SameSite attribute asks of it, so a route of the signed in
 * finds no session, and a route that signs in finds a foreign Origin.
 * `localhost` is another origin than the console's address, and one that
 * the console knows by name. A form is a navigation, so the browser shows
 * the answer whatever headers it carries, where a script's request of
 * another origin would have its answer withheld.
 */
async function fromAnotherOrigin(page) {
  const { hostname, port } = new URL(page.url());
  if (hostname !== "127.0.0.1") {
    console.log(`${hostname} has no other name that this lab PC reaches it by: the page of another origin is not tried`);
    return false;
  }
  const attacker = await page.context().newPage();
  const attackerOrigin = `https://localhost:${port}`;
  // The other site's page, which the browser is handed without asking the
  // machine: a page the console served would carry the console's content
  // security policy, which sends forms to its own origin alone.
  const attackerPage = `${attackerOrigin}/lab-another-site.html`;
  await attacker.route(attackerPage, (route) =>
    route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>Another site</title>" }),
  );
  // A browser that asks before a page reaches the machine's own addresses
  // (local network access, which a handed-in page counts as public) is
  // told yes for this page: that check is the browser's own, and what is
  // tested here is the console's refusal. An engine that has no such
  // check has no such permission either, and refuses to be given it.
  await page
    .context()
    .grantPermissions(["local-network-access"], { origin: attackerOrigin })
    .catch(() => undefined);
  for (const [route, body] of [...SIGN_IN_ROUTES, ...SESSION_ROUTES]) {
    const url = `${originOf(page)}${pathOf(route)}`;
    await attacker.goto(attackerPage);
    // A form of plain text, which needs no preflight, holding the body.
    const [answer] = await Promise.all([
      attacker.waitForResponse((reply) => reply.url() === url && reply.request().method() === "POST"),
      attacker.evaluate(
        ({ url: target, text }) => {
          const form = document.createElement("form");
          form.method = "POST";
          form.action = target;
          form.enctype = "text/plain";
          const field = document.createElement("input");
          field.type = "hidden";
          field.name = text;
          form.append(field);
          document.body.append(form);
          form.submit();
        },
        { url, text: JSON.stringify(body) },
      ),
    ]);
    // The answer is a page now: it is let finish before the next form.
    await attacker.waitForURL(url);
    // A route that signs in has no session to look for and refuses the
    // Origin. Any other finds no session: 401 says that the browser sent no
    // cookie, as 403 would say that it did, and the console then refused
    // the request for what it came from.
    const signsIn = SIGN_IN_ROUTES.some(([each]) => each === route);
    ensure(
      answer.status() === (signsIn ? 403 : 401),
      `${route} from another origin was answered ${answer.status()}, not ${signsIn ? 403 : 401}`,
    );
  }
  await attacker.close();
  return true;
}

export default async function check(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, NAME, PASSWORD);
  await look(t, page, "signed-in");
  const { csrf } = await sessionOf(page);
  await controls(page, csrf);

  // Locked.
  await forge(page, SESSION_ROUTES, csrf, true);
  await forge(page, SIGN_IN_ROUTES, csrf, false);
  await stillStanding(t, page, null);
  const another = await fromAnotherOrigin(page);
  await stillStanding(t, page, null);

  // Unlocked: now a forged lock, extension or sign-out would show.
  const unlocked = await postAsPage(page, "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(unlocked.status() === 200, `the page's own unlock was refused: ${unlocked.status()}`);
  const { unlock_seconds: seconds } = await unlocked.json();
  await forge(page, SESSION_ROUTES, csrf, true);
  await stillStanding(t, page, seconds);
  await fromAnotherOrigin(page);
  await stillStanding(t, page, seconds);

  // Signed out: a forged sign-in with the right name and password signs
  // no one in.
  await signOut(t, page);
  await forge(page, SIGN_IN_ROUTES, csrf, false);
  ensure(!(await cookieNamed(page, "__Host-dukovo")), "a forged request signed the browser in");
  await t.see(signInHeading(page));
  await look(t, page, "signed-out");

  // The browser posted to every route, where it had a page to do it from.
  if (another) mustHaveCalled(seen, [...SIGN_IN_ROUTES, ...SESSION_ROUTES].map(([route]) => route));
}
