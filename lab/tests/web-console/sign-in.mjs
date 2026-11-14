// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/sign-in drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the
// console's sign-in page and everything between a name and a signed-in
// session, in dark and in light. The account, an administrator that
// sign-in.toml made for this engine alone, adds an authenticator app and
// is then asked for its codes: a wrong one, one used already, one that
// trusts the browser for 30 days, and the recovery codes. Each engine has
// an account of its own so that an engine that fails halfway does not
// leave the next one an app whose secret it cannot know.

import {
  SECOND_STEP,
  addApp,
  answerTo,
  cookieNamed,
  ensure,
  giveCode,
  look,
  mustHaveCalled,
  mustRefuse,
  openConsole,
  press,
  recoveryCodesShown,
  servedCertificate,
  sessionOf,
  signIn,
  signInDone,
  signInHeading,
  signOut,
  signOutButton,
  sleep,
} from "./console.mjs";

// The password that sign-in.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// An account that is neither an administrator nor in group console, and
// so may not use the console.
const PLAIN = "wc-signin-plain";

// The calls that lab/coverage.toml says this test covers, which the
// browser must have made by the end. GET /api/1/health is read below with
// the page's own request, as it is no call of the page's.
const ROUTES = [
  "GET /api/1/fingerprint",
  "POST /api/1/sign-in",
  "POST /api/1/sign-in/code",
  "GET /api/1/session",
  "POST /api/1/sign-out",
  "GET /api/1/account/second-step",
  "POST /api/1/account/second-step/app/begin",
  "POST /api/1/account/second-step/app/confirm",
  "POST /api/1/account/second-step/app/remove",
  "POST /api/1/account/second-step/codes",
  "POST /api/1/account/second-step/trusted/forget",
];

const APP_ASKED = "Enter the code from your app";
const RECOVERY_ASKED = "Enter a recovery code";
const APP_CODE = "Six-digit code";
const RECOVERY_CODE = "Recovery code";
const TRUSTED = "__Host-dukovo-trust";

/** The alert on the page that holds `text`. */
const alertWith = (page, text) => page.getByRole("alert").filter({ hasText: text });

/** The "Trust this browser" box of the step's page. */
const trustBox = (page) => page.getByRole("checkbox", { name: "Trust this browser for 30 days" });

/** The page of the signed-in account's second step. */
async function openSecondStep(t, page) {
  // A page that was on it when the session ended is back on it at once.
  if (!new URL(page.url()).pathname.endsWith("/second-step")) {
    await page.getByRole("link", { name: "Manage the second step" }).click();
  }
  await t.see(page.getByRole("heading", { name: "Second step", level: 1 }));
  await t.see(page.getByRole("heading", { name: "Trusted browsers" }));
}

/** Signs in and waits for the page that asks for the second step. */
async function signInToStep(t, page, name) {
  const answer = await signIn(page, name, PASSWORD);
  const body = await answer.json();
  ensure(
    answer.status() === 200 && body.step?.kinds.includes("app"),
    `the console did not ask ${name} for the app's code: ${answer.status()} ${JSON.stringify(body)}`,
  );
  await t.see(page.getByRole("heading", { name: APP_ASKED }));
}

/** Gives a code and waits for the signed-in page. */
async function giveRightCode(t, page, label, code) {
  const answer = await giveCode(page, label, code);
  ensure(answer.status() === 200, `the code ${code} was not taken: the console answered ${answer.status()}`);
  await t.see(signOutButton(page));
}

/** The sign-in page, its fingerprint note, and the headers every answer has. */
async function theSignInPage(t, page) {
  await t.see(signInHeading(page));
  const note = page.locator(".fingerprint");
  await t.see(note);
  await look(t, page, "sign-in");

  // What the page says to compare with the browser's warning is the
  // certificate the console really serves.
  const served = (await servedCertificate(t.url)).fingerprint256.toUpperCase();
  const shown = (await note.innerText()).replace(/\s/g, "").toUpperCase();
  ensure(shown === served, `the page shows the fingerprint ${shown}, and the certificate has ${served}`);
  const told = await page.request.get(`${t.url}/api/1/fingerprint`);
  ensure(
    (await told.json()).fingerprint.toUpperCase() === served,
    "GET /api/1/fingerprint tells a fingerprint that is not the certificate's",
  );

  const health = await page.request.get(`${t.url}/api/1/health`);
  ensure(
    health.status() === 200 && (await health.json()).ok === true,
    `GET /api/1/health did not say ok: ${health.status()}`,
  );
  // The page runs here without its policy, so the policy is read from the
  // server's own answer.
  const policy = health.headers()["content-security-policy"] ?? "";
  ensure(
    policy.includes("script-src 'self'") && !policy.includes("unsafe-inline"),
    `the console's content security policy lets inline script run: ${policy}`,
  );
}

/** A wrong password, and an account that may not use the console. */
async function refusals(t, page, name) {
  const wrong = await signIn(page, name, "not the password");
  await mustRefuse(wrong, 401, "ADM-101");
  await t.see(alertWith(page, "ADM-101"));
  await look(t, page, "wrong-password");

  const plain = await signIn(page, PLAIN, PASSWORD);
  await mustRefuse(plain, 403, "ADM-104");
  await t.see(alertWith(page, "ADM-104"));
}

/** The signed-in session, as the server and the browser each keep it. */
async function signedIn(t, page, name) {
  await signInDone(t, page, name, PASSWORD);
  const session = await sessionOf(page);
  ensure(
    session.name === name && session.admin === true && session.unlock_seconds === null,
    `GET /api/1/session names ${session.name}, admin ${session.admin}, unlocked for ${session.unlock_seconds}`,
  );
  ensure(session.needs_step === false && session.csrf.length === 43, "the session's fields are not as a first sign-in's");

  const cookie = await cookieNamed(page, "__Host-dukovo");
  ensure(
    cookie?.httpOnly === true && cookie.secure === true && cookie.sameSite === "Strict" && cookie.path === "/",
    `the session's cookie is ${JSON.stringify(cookie)}`,
  );
  ensure(!(await page.evaluate(() => document.cookie)).includes("dukovo"), "a script can read the session's cookie");
  await look(t, page, "signed-in");
}

/** Adds the app on the second-step page and gives it with the recovery codes. */
async function addTheApp(t, page) {
  await openSecondStep(t, page);
  await t.see(page.getByText("Not added", { exact: true }));
  await look(t, page, "second-step");

  const { app, codes } = await addApp(t, page, PASSWORD);
  await look(t, page, "recovery-codes");
  await page.getByRole("button", { name: "I have saved them" }).click();
  await t.gone(page.getByRole("heading", { name: "Your recovery codes" }));
  await t.see(page.getByText("Added", { exact: true }));
  await t.see(page.getByText("10 left", { exact: true }));
  return { app, codes };
}

/**
 * The app's codes at sign-in: the page asks for one, a wrong one and one
 * used already are refused, and the browser trusted with the right one.
 */
async function codesAtSignIn(t, page, name, app) {
  await signOut(t, page);
  await signInToStep(t, page, name);
  await look(t, page, "step-asked");
  ensure(!(await trustBox(page).isChecked()), "the box that trusts this browser is ticked before anyone ticked it");

  await mustRefuse(await giveCode(page, APP_CODE, await app.wrongCode(page)), 401, "ADM-102");
  await t.see(alertWith(page, "ADM-102"));
  await look(t, page, "step-wrong-code");

  const first = await app.nextCode(page);
  await giveRightCode(t, page, APP_CODE, first);
  ensure(!(await cookieNamed(page, TRUSTED)), "a browser that was not trusted has the cookie of one that was");

  // The same code again, within its 90 seconds, is a replay.
  await signOut(t, page);
  await signInToStep(t, page, name);
  const replay = await mustRefuse(await giveCode(page, APP_CODE, first), 401, "ADM-102");
  ensure(replay.detail.includes("used already"), `a code used twice was refused as: ${replay.detail}`);

  // The next step's code, once the console's clock comes to it, with the
  // box ticked.
  await trustBox(page).ensure();
  await giveRightCode(t, page, APP_CODE, await app.nextCode(page));
  ensure(await cookieNamed(page, TRUSTED), "the browser was not trusted although the box was ticked");
}

/** A trusted browser skips the step, until its trust is forgotten. */
async function trustedBrowser(t, page, name) {
  await signOut(t, page);
  const answer = await signIn(page, name, PASSWORD);
  ensure(
    answer.status() === 200 && (await answer.json()).done === true,
    "a trusted browser was asked for the second step",
  );
  await t.see(signOutButton(page));

  await openSecondStep(t, page);
  await t.see(page.getByText("1 trusted", { exact: true }));
  await look(t, page, "trusted");
  const forget = page.getByRole("button", { name: "Forget trusted browsers" });
  const forgotten = await press(page, forget, `${SECOND_STEP}/trusted/forget`);
  ensure(
    forgotten.status() === 200 && (await forgotten.json()).forgotten === 1,
    "forgetting the trusted browsers did not forget one",
  );
  await t.see(page.getByText("0 trusted", { exact: true }));

  // The browser still holds its cookie, which no longer works: the step is
  // asked, and the cookie is dropped.
  await signOut(t, page);
  await signInToStep(t, page, name);
  ensure(!(await cookieNamed(page, TRUSTED)), "the console left a browser its cookie of a trust it forgot");
}

/** Signs in with a recovery code, which works once. */
async function recoveryCodesAtSignIn(t, page, name, codes) {
  await page.getByRole("button", { name: "Use a recovery code" }).click();
  await t.see(page.getByRole("heading", { name: RECOVERY_ASKED }));
  await look(t, page, "recovery-asked");
  await giveRightCode(t, page, RECOVERY_CODE, codes[0]);

  // The same one again, and one of the others in lower case.
  await signOut(t, page);
  await signInToStep(t, page, name);
  await page.getByRole("button", { name: "Use a recovery code" }).click();
  await mustRefuse(await giveCode(page, RECOVERY_CODE, codes[0]), 401, "ADM-102");
  await giveRightCode(t, page, RECOVERY_CODE, codes[1].toLowerCase());
}

/** New recovery codes replace the old, which stop working. */
async function newRecoveryCodes(t, page, name, old) {
  await openSecondStep(t, page);
  await t.see(page.getByText("8 left", { exact: true }));
  await page.getByRole("button", { name: "Make new codes…" }).click();
  const form = page.getByRole("form", { name: "Make new recovery codes" });
  await form.getByLabel("Your password", { exact: true }).fill(PASSWORD);
  const made = await press(page, form.getByRole("button", { name: "Make new codes", exact: true }), `${SECOND_STEP}/codes`);
  ensure(made.status() === 200, `the console answered ${made.status()} to new recovery codes`);
  await t.see(page.getByRole("heading", { name: "Your recovery codes" }));
  const fresh = await recoveryCodesShown(page);
  ensure(fresh.every((code) => !old.includes(code)), "a new recovery code is one of the old ones");
  await page.getByRole("button", { name: "I have saved them" }).click();
  await t.see(page.getByText("10 left", { exact: true }));

  await signOut(t, page);
  await signInToStep(t, page, name);
  await page.getByRole("button", { name: "Use a recovery code" }).click();
  await mustRefuse(await giveCode(page, RECOVERY_CODE, old[2]), 401, "ADM-102");
  await giveRightCode(t, page, RECOVERY_CODE, fresh[0]);
}

/** Takes the app away: the account signs in with its password alone again. */
async function removeTheApp(t, page, name) {
  await openSecondStep(t, page);
  await page.getByRole("button", { name: "Remove the app" }).click();
  const form = page.getByRole("form", { name: "Remove the authenticator app" });
  await form.getByLabel("Your password", { exact: true }).fill(PASSWORD);
  const removed = await press(page, form.getByRole("button", { name: "Remove the app", exact: true }), `${SECOND_STEP}/app/remove`);
  ensure(removed.status() === 200, `the console answered ${removed.status()} to the app's removal`);
  await t.see(page.getByText("Not added", { exact: true }));
  await t.see(page.getByText("None", { exact: true }));
  await look(t, page, "app-removed");

  await signOut(t, page);
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200 && (await answer.json()).done === true, `${name} was asked for a step that is gone`);
  await t.see(signOutButton(page));
  await signOut(t, page);
}

/**
 * Ten sign-in requests a minute is all one address may make: the next is
 * told to wait, and the page says so in words. The requests are ones with
 * no password, which ask authd nothing and so cost no account anything.
 */
async function waitMessage(t, page, name) {
  const headers = { Origin: new URL(t.url).origin, "Content-Type": "application/json" };
  let told = null;
  for (let tries = 0; tries < 12 && told === null; tries += 1) {
    const answer = await page.request.post(`${t.url}/api/1/sign-in`, { headers, data: { name: "", password: "" } });
    if (answer.status() === 429) told = answer;
  }
  ensure(told !== null, "the console never told this address to wait, after twelve sign-in requests in a row");
  await mustRefuse(told, 429, "ADM-105");
  const seconds = Number.parseInt(told.headers()["retry-after"], 10);
  ensure(seconds >= 1 && seconds <= 60, `the console said to wait ${told.headers()["retry-after"]} seconds`);

  await page.getByLabel("Account", { exact: true }).fill(name);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const [answer] = await Promise.all([
    answerTo(page, "/api/1/sign-in"),
    page.getByRole("button", { name: "Sign in", exact: true }).click(),
  ]);
  await mustRefuse(answer, 429, "ADM-105");
  await t.see(alertWith(page, "Try again in"));
  await look(t, page, "wait");
  // The next test, or engine, starts with its tries back.
  await sleep((seconds + 1) * 1000);
}

export default async function check(t) {
  const name = `wc-signin-${t.engine}`;
  const { page, seen } = await openConsole(t, "/");

  await theSignInPage(t, page);
  await refusals(t, page, name);
  await signedIn(t, page, name);
  const { app, codes } = await addTheApp(t, page);
  await codesAtSignIn(t, page, name, app);
  await trustedBrowser(t, page, name);
  await recoveryCodesAtSignIn(t, page, name, codes);
  await newRecoveryCodes(t, page, name, codes);
  await removeTheApp(t, page, name);
  await waitMessage(t, page, name);

  mustHaveCalled(seen, ROUTES);
}
