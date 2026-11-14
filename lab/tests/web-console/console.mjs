// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the web console's four browser tests share (sign-in, passkeys,
// unlock and csrf): opening the console, pressing a button through the
// console's limit on sign-in tries, the one-time codes of an authenticator
// app (computed here from the secret the page shows, as an app on a phone
// does), the certificate the console serves, and the check of a page in
// dark and light. The scripts import it from beside themselves: the lab's
// runner starts each script in the test's own folder.

import { createHash, createHmac } from "node:crypto";
import net from "node:net";
import tls from "node:tls";

/** Throws `message` unless `condition` holds. */
export function ensure(condition, message) {
  if (!condition) throw new Error(message);
}

/** Waits `milliseconds`. */
export const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

/** The console's second-step routes, which the account's pages and the tests both name. */
export const SECOND_STEP = "/api/1/account/second-step";

// --- Opening the console ---

/**
 * Opens `path` in a new browser context and gives its page with the set of
 * `METHOD /path` that the context goes on to request, for a test to check
 * against the routes it claims to cover.
 *
 * The context goes without the console's content security policy, which
 * allows no inline script: the lab's accessibility check adds axe-core to
 * the page as one, and the policy would stop it. The policy is the
 * server's, and the sign-in test reads its header.
 */
export async function openConsole(t, path, options = {}) {
  const page = await t.open(path, { bypassCSP: true, ...options });
  const seen = track(page);
  // The page's first calls came before anything listened for them.
  await page.reload();
  return { page, seen };
}

/** The set of `METHOD /path` that the context of `page` requests from now on. */
export function track(page) {
  const seen = new Set();
  page.context().on("request", (request) => {
    seen.add(`${request.method()} ${new URL(request.url()).pathname}`);
  });
  return seen;
}

/** Throws unless every one of `routes`, as `METHOD /path`, is in `seen`. */
export function mustHaveCalled(seen, routes) {
  const missing = routes.filter((route) => !seen.has(route));
  ensure(missing.length === 0, `the test never called ${missing.join(", ")}`);
}

/**
 * Looks at the page as it is in dark and in light: the accessibility
 * check passes and a screenshot is kept for each. `name` is the step, in
 * letters, digits and dashes.
 */
export async function look(t, page, name) {
  for (const colorScheme of ["dark", "light"]) {
    await page.emulateMedia({ colorScheme });
    // The console follows the browser's scheme by a class on its root.
    await page.waitForFunction(
      (scheme) => document.documentElement.classList.contains(scheme),
      colorScheme,
    );
    await t.axe(page);
    await t.screenshot(page, `${name}-${colorScheme}`);
  }
}

// --- Pressing buttons ---

/** The answer of the console to the next POST to `route`, which a press is about to make. */
export function answerTo(page, route) {
  return page.waitForResponse(
    (answer) =>
      answer.request().method() === "POST" && new URL(answer.url()).pathname === route,
  );
}

/**
 * Presses `button` and gives the answer to the POST to `route` that it
 * makes. A console that says this address has made too many sign-in tries
 * is waited for as long as it asks, and the button is pressed again.
 */
export async function press(page, button, route) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const [answer] = await Promise.all([answerTo(page, route), button.click()]);
    if (answer.status() !== 429) return answer;
    const seconds = Number.parseInt(answer.headers()["retry-after"] ?? "10", 10);
    await sleep((seconds + 1) * 1000);
  }
  throw new Error(`the console still said to wait after five tries of ${route}`);
}

/** The error that an answer carries, `{id, title, detail, fix}`, or null when it carries none. */
export async function errorOf(answer) {
  try {
    return (await answer.json()).error ?? null;
  } catch {
    return null;
  }
}

/** Throws unless `answer` is the error `id` with HTTP status `status`; gives the error. */
export async function mustRefuse(answer, status, id) {
  const error = await errorOf(answer);
  ensure(
    answer.status() === status && error?.id === id,
    `${answer.url()} answered ${answer.status()} ${error?.id ?? "with no error"}, not ${status} ${id}`,
  );
  return error;
}

// --- Signing in and out ---

/** The heading of the sign-in page. */
export const signInHeading = (page) => page.getByRole("heading", { name: "Sign in", exact: true });

/** The header's button that signs out, which only a signed-in page has. */
export const signOutButton = (page) => page.getByRole("button", { name: "Sign out" });

/** Fills in the sign-in page and presses its button. Gives the answer of the console. */
export async function signIn(page, name, password) {
  await page.getByLabel("Account", { exact: true }).fill(name);
  await page.getByLabel("Password", { exact: true }).fill(password);
  return press(page, page.getByRole("button", { name: "Sign in", exact: true }), "/api/1/sign-in");
}

/** Signs in with nothing more to give, and waits for the signed-in page. */
export async function signInDone(t, page, name, password) {
  const answer = await signIn(page, name, password);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await t.see(signOutButton(page));
}

/** Presses "Sign out" and waits for the sign-in page. */
export async function signOut(t, page) {
  await press(page, signOutButton(page), "/api/1/sign-out");
  await t.see(signInHeading(page));
}

/** Gives `code` in the field `label` of the page that asks for a second step, and presses "Continue". Gives the answer. */
export async function giveCode(page, label, code) {
  await page.getByLabel(label, { exact: true }).fill(code);
  return press(page, page.getByRole("button", { name: "Continue" }), "/api/1/sign-in/code");
}

/** Signs in with the password and then a recovery code, which works once. */
export async function signInWithRecovery(t, page, name, password, code) {
  const answer = await signIn(page, name, password);
  const body = await answer.json();
  ensure(answer.status() === 200 && body.step !== undefined, `${name} was not asked for a second step: ${answer.status()}`);
  await page.getByRole("button", { name: "Use a recovery code" }).click();
  await t.see(page.getByRole("heading", { name: "Enter a recovery code" }));
  const taken = await giveCode(page, "Recovery code", code);
  ensure(taken.status() === 200, `the recovery code was not taken: the console answered ${taken.status()}`);
  await t.see(signOutButton(page));
}

/** The browser's cookie called `name` for the console, or undefined. */
export async function cookieNamed(page, name) {
  return (await page.context().cookies()).find((cookie) => cookie.name === name);
}

/** What `GET /api/1/session` tells the page's signed-in session. */
export async function sessionOf(page) {
  const answer = await page.request.get(new URL("/api/1/session", page.url()).href);
  ensure(answer.status() === 200, `GET /api/1/session answered ${answer.status()}`);
  return answer.json();
}

/** The page of the signed-in account's second step. */
export async function openSecondStep(t, page) {
  // A page that was on it when the session ended is back on it at once.
  if (!new URL(page.url()).pathname.endsWith("/second-step")) {
    await page.getByRole("link", { name: "Manage the second step" }).click();
  }
  await t.see(page.getByRole("heading", { name: "Second step", level: 1 }));
  await t.see(page.getByRole("heading", { name: "Trusted browsers" }));
}

/** The origin of the console that `page` shows, `https://host:port`. */
export const originOf = (page) => new URL(page.url()).origin;

/**
 * A POST as the console's own page makes it, from the page's origin and
 * with the session's token, so that only the route and the body decide
 * what the console answers. Gives Playwright's answer.
 */
export async function postAsPage(page, route, body) {
  const { csrf } = await sessionOf(page);
  return page.request.post(`${originOf(page)}${route}`, {
    headers: { Origin: originOf(page), "X-Dukovo-CSRF": csrf },
    data: body,
  });
}

// --- The certificate ---

/** The certificate that the console at `url` presents, as Node's `X509Certificate`. */
export function servedCertificate(url) {
  const { hostname, port } = new URL(url);
  return new Promise((resolve, reject) => {
    // A name is sent as the server name; an address may not be.
    const servername = net.isIP(hostname) ? undefined : hostname;
    // Only the certificate is read, whatever it is signed by.
    const socket = tls.connect(
      { host: hostname, port: Number(port) || 443, servername, rejectUnauthorized: false },
      () => {
        const certificate = socket.getPeerX509Certificate();
        socket.end();
        if (certificate) resolve(certificate);
        else reject(new Error(`${hostname} presented no certificate`));
      },
    );
    socket.on("error", reject);
  });
}

/** Whether `hostname`, as a URL gives it, is an IP address, which a passkey's relying party may not be. */
export const isAddress = (hostname) => net.isIP(hostname.replace(/^\[|\]$/g, "")) !== 0;

/** The pin of the certificate's public key, as Chromium's `--ignore-certificate-errors-spki-list` takes it. */
export function pinOf(certificate) {
  const key = certificate.publicKey.export({ type: "spki", format: "der" });
  return createHash("sha256").update(key).digest("base64");
}

// --- An authenticator app ---

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** The bytes of a base32 secret, with or without the spaces the page puts between its groups. */
export function fromBase32(text) {
  const bytes = [];
  let buffer = 0;
  let bits = 0;
  for (const letter of text.replace(/[\s=]/g, "").toUpperCase()) {
    const value = BASE32.indexOf(letter);
    ensure(value >= 0, `"${letter}" is no letter of a base32 secret`);
    buffer = (buffer << 5) | value;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 0xff);
      buffer &= (1 << bits) - 1;
    }
  }
  return Buffer.from(bytes);
}

/** RFC 6238's code of `secret` for the 30-second `step`: HMAC-SHA1, six digits. */
export function appCode(secret, step) {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac("sha1", secret).update(counter).digest();
  const at = mac[mac.length - 1] & 0x0f;
  return String((mac.readUInt32BE(at) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

/**
 * The console's clock in Unix seconds, from the `Date` of one of its
 * answers: an app's codes follow the clock of the machine that checks
 * them, which need not be the lab PC's to the second.
 */
async function consoleSeconds(page) {
  const answer = await page.request.get(new URL("/api/1/health", page.url()).href);
  const date = Date.parse(answer.headers().date ?? "");
  return Math.floor((Number.isNaN(date) ? Date.now() : date) / 1000);
}

/** An authenticator app: its secret, and the last step that a code of it was given for. */
export class App {
  constructor(secret) {
    this.secret = secret;
    this.last = null;
  }

  /**
   * The app's next code that the console has not taken: this step's, or
   * the next one's, which the console takes too for a clock a little off.
   * When even that was taken, this waits for the console's clock to come
   * to the next step, at most 30 seconds.
   */
  async nextCode(page) {
    const giveUp = Date.now() + 90_000;
    while (Date.now() < giveUp) {
      const current = Math.floor((await consoleSeconds(page)) / 30);
      const step = this.last === null ? current : Math.max(current, this.last + 1);
      if (step <= current + 1) {
        this.last = step;
        return appCode(this.secret, step);
      }
      await sleep(1000);
    }
    throw new Error("the console's clock did not come to the app's next step in 90 seconds");
  }

  /** Six digits that none of the app's codes for the steps around now are. */
  async wrongCode(page) {
    const current = Math.floor((await consoleSeconds(page)) / 30);
    const right = [];
    for (let step = current - 2; step <= current + 2; step += 1) right.push(appCode(this.secret, step));
    for (let n = 1; ; n += 1) {
      const code = String((n * 123_457) % 1_000_000).padStart(6, "0");
      if (!right.includes(code)) return code;
    }
  }
}

/**
 * Adds an authenticator app to the signed-in account, on the second-step
 * page, and gives `{app, codes}`: the app, with the secret the page shows,
 * and the recovery codes the console gave, which the page still shows.
 * `password` is the account's, or null when the console is unlocked and
 * asks for none.
 */
export async function addApp(t, page, password) {
  await page.getByRole("button", { name: "Add the app", exact: true }).click();
  const form = page.getByRole("form", { name: "Add the authenticator app" });
  const proof = form.getByLabel("Your password", { exact: true });
  if (password === null) {
    await t.gone(proof);
  } else {
    await proof.fill(password);
  }
  const begun = await press(page, form.getByRole("button", { name: "Continue" }), `${SECOND_STEP}/app/begin`);
  ensure(begun.status() === 200, `the console answered ${begun.status()} to the app's beginning`);
  await t.see(form.locator(".secret"));
  await look(t, page, "add-app");
  const app = new App(fromBase32(await form.locator(".secret").innerText()));
  await form.getByLabel("Code from the app", { exact: true }).fill(await app.nextCode(page));
  const confirmed = await press(
    page,
    form.getByRole("button", { name: "Add the app", exact: true }),
    `${SECOND_STEP}/app/confirm`,
  );
  ensure(confirmed.status() === 200, `the console answered ${confirmed.status()} to the app's code`);
  await t.see(page.getByRole("heading", { name: "Your recovery codes" }));
  return { app, codes: await recoveryCodesShown(page) };
}

/** The recovery codes that the page shows now. */
export async function recoveryCodesShown(page) {
  const codes = await page.locator("ul.codes li").allInnerTexts();
  ensure(codes.length === 10, `the page shows ${codes.length} recovery codes, not 10`);
  for (const code of codes) {
    ensure(/^[A-Z0-9]{4}(-[A-Z0-9]{4}){3}$/.test(code), `"${code}" is not in the form of a recovery code`);
  }
  return codes;
}
