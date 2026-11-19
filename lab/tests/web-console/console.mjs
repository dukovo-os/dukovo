// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the web console's browser tests share: opening the console,
// pressing a button through the console's limit on sign-in tries, the
// one-time codes of an authenticator app (computed here from the secret the
// page shows, as an app on a phone does), the certificate the console
// serves, a call made from the page with fetch() and the error it gets, and
// the check of a page in each way the console can look. The scripts import
// it from beside themselves: the lab's runner starts each script in the
// test's own folder.

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
 * The page keeps the console's content security policy, and `look` fails
 * when the policy stopped anything (`watchPolicy`): a page the policy
 * breaks fails here, as it would for a person.
 */
export async function openConsole(t, path, options = {}) {
  const page = await t.open(path, options);
  const seen = track(page);
  await watchPolicy(page);
  // The page's first calls came before anything listened for them.
  await page.reload();
  return { page, seen };
}

/**
 * Has every page of `page`'s context note what its content security policy
 * stops, from its next load on, for `look` to read (`policyStopped`), and
 * keep the timing of every file it asks for.
 */
export async function watchPolicy(page) {
  await page.context().addInitScript(() => {
    // A browser keeps the timing of 250 files by default and drops the rest;
    // a page that has called the API that often has none for its icons.
    performance.setResourceTimingBufferSize(100_000);
    window.labPolicyStopped = [];
    window.labShooting = 0;
    document.addEventListener(
      "securitypolicyviolation",
      (event) => {
        // Playwright hides the caret for a screenshot with a style of its
        // own, which WebKit puts in the page, where the policy stops it.
        const inline = event.blockedURI === "" || event.blockedURI === "inline";
        if (window.labShooting > 0 && inline && event.effectiveDirective.startsWith("style-src")) return;
        window.labPolicyStopped.push(`${event.effectiveDirective} ${event.blockedURI || "inline"}`);
      },
      { capture: true },
    );
  });
}

/**
 * `t.screenshot(page, name)`, with the page told that a screenshot is being
 * taken until the browser has said what its policy stopped meanwhile, so
 * that the style Playwright adds for it is not taken for the console's.
 */
export async function screenshot(t, page, name) {
  await page.evaluate(() => { window.labShooting = (window.labShooting ?? 0) + 1; });
  try {
    await t.screenshot(page, name);
  } finally {
    // The browser tells of a stopped style in a task of its own, after the
    // screenshot has returned.
    await page.evaluate(() => new Promise((done) => setTimeout(() => {
      window.labShooting -= 1;
      done();
    }, 100))).catch(() => {});
  }
}

/** What the content security policy stopped on `page` since it loaded, if it is watched. */
export const policyStopped = (page) => page.evaluate(() => window.labPolicyStopped ?? []);

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

/** The root's classes that say how the page looks: the colours, the text size and the motion. */
const DISPLAY_CLASSES = ["dark", "light", "hc", "t112", "t125", "t150", "calm"];

/**
 * Has the page show itself with `display`, `{theme, text, motion}`, the way
 * its own code does for a person's choice (theme.js's `dukovoDisplay`), and
 * waits for the root to carry the colours' class. A choice that is left out
 * is the default. Gives a function that puts the page's classes back as they
 * were.
 */
export async function showAs(page, display, colours) {
  const before = await page.evaluate(() => document.documentElement.className);
  await page.evaluate((wanted) => window.dukovoDisplay.apply(wanted), display);
  await page.waitForFunction((name) => document.documentElement.classList.contains(name), colours);
  return () => page.evaluate((classes) => { document.documentElement.className = classes; }, before);
}

/** The animations of the page that are running now, by the name of their element's class. */
export const runningAnimations = (page) =>
  page.evaluate(() =>
    document.getAnimations()
      .filter((animation) => animation.playState === "running")
      .map((animation) => animation.effect?.target?.className || animation.effect?.target?.localName || "unknown"));

/**
 * Whether the page scrolls sideways, which only a box made to keep two
 * dimensions may (the terminal's screen, a wide table in a box of its own).
 * Gives how many pixels wider the page is than its window.
 */
export const sidewaysScroll = (page) =>
  page.evaluate(() => Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth));

/**
 * Looks at the page as it is in dark and in light: its content security
 * policy has stopped nothing, the accessibility check passes, and a
 * screenshot is kept for each. `name` is the step, in letters, digits and
 * dashes. The colours are the console's own choice, which theme.js applies,
 * and not the browser's colour scheme: the console shows dark until a
 * person chooses "Same as this device".
 */
export async function look(t, page, name) {
  const stopped = await policyStopped(page);
  ensure(stopped.length === 0, `${name}: the content security policy stopped ${stopped.join(", ")}`);
  for (const theme of ["dark", "light"]) {
    const restore = await showAs(page, { theme }, theme);
    await t.axe(page);
    await screenshot(t, page, `${name}-${theme}`);
    await restore();
  }
}

/**
 * Looks at the page as `look` does, and then in the three other ways the
 * console can look (design 14): high contrast, with the browser's forced
 * colours, and with less motion. Each is checked by the accessibility check
 * and kept as a screenshot. With high contrast the root has `hc`; with
 * forced colours the browser sets the colours and the page's own are not
 * used, so the check there is every rule of axe but the contrast of the
 * page's own colours, and that no control has lost its border; with less
 * motion nothing on the page animates.
 * An engine that cannot emulate forced colours (WebKit's does not) skips
 * that look and says so in the log.
 */
export async function lookAll(t, page, name) {
  await look(t, page, name);

  let restore = await showAs(page, { theme: "high-contrast" }, "hc");
  await t.axe(page);
  await screenshot(t, page, `${name}-high-contrast`);
  await restore();

  let forced = false;
  try {
    await page.emulateMedia({ forcedColors: "active" });
    forced = await page.evaluate(() => matchMedia("(forced-colors: active)").matches);
  } catch {
    // An engine that cannot emulate forced colours says so by refusing.
  }
  if (forced) {
    // The browser draws the page in the system's colours, which axe cannot
    // read: it takes the page's own text colour against a white page and
    // calls it unreadable. Every other rule holds.
    const found = (await t.violations(page)).filter(({ id }) => id !== "color-contrast");
    ensure(found.length === 0, `${name}: with forced colours the accessibility check found ${found.map(({ id }) => id).join(", ")}`);
    await screenshot(t, page, `${name}-forced-colours`);
    const bare = await page.evaluate(() =>
      [...document.querySelectorAll("main button, main .btn, header button")]
        .filter((button) => button.offsetParent !== null && getComputedStyle(button).borderTopWidth === "0px")
        .map((button) => button.textContent.trim() || button.getAttribute("aria-label") || button.className),
    );
    ensure(bare.length === 0, `${name}: with forced colours these buttons have no border: ${bare.join(", ")}`);
  } else {
    console.log(`${name}: ${t.engine} cannot emulate forced colours, so that look is skipped`);
  }
  await page.emulateMedia({ forcedColors: "none" }).catch(() => {});

  restore = await showAs(page, { motion: "less" }, "dark");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await t.axe(page);
  await screenshot(t, page, `${name}-less-motion`);
  const moving = await runningAnimations(page);
  ensure(moving.length === 0, `${name}: with less motion these still move: ${moving.join(", ")}`);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await restore();
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

/**
 * The header's account menu, which only a signed-in page has: its button is
 * the person's name and holds "Your account", the display choices and
 * "Sign out". Its name is "NAME account menu".
 */
export const signOutButton = (page) => page.getByRole("banner").getByRole("button", { name: / account menu$/ });

/**
 * Opens the account menu and gives it. A phone's header has no such menu: the
 * "More" sheet holds the same items, and gives the dialog that holds them.
 */
export async function openAccountMenu(t, page) {
  const button = signOutButton(page);
  if (await button.isVisible()) {
    await button.click();
    const menu = page.getByRole("menu", { name: /^Account of / });
    await t.see(menu);
    return menu;
  }
  await page.getByRole("button", { name: "More", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "More" });
  await t.see(sheet);
  return sheet;
}

/** The item `name` of the menu or the sheet that `openAccountMenu` gave. */
export const accountItem = (menu, name) => menu.getByRole("menuitem", { name, exact: true }).or(menu.getByRole("link", { name, exact: true })).or(menu.getByRole("button", { name, exact: true }));

/** Fills in the sign-in page and presses its button. Gives the answer of the console. */
export async function signIn(page, name, password) {
  await page.getByLabel("Account", { exact: true }).fill(name);
  await page.getByLabel("Password", { exact: true }).fill(password);
  return press(page, page.getByRole("button", { name: "Sign in", exact: true }), "/api/1/sign-in");
}

/**
 * Gives the account a time zone if it has none, and shows the page again. A
 * new account has none, and the console asks for it on /welcome before any
 * other page: a test that is not about that page gives it one, the way a
 * person who chose "Skip all" on it would have.
 */
export async function leaveWelcome(t, page, zone = "UTC") {
  const read = await fetchAsPage(page, "GET", "/api/1/account/preferences");
  ensure(read.status === 200, `GET /api/1/account/preferences was answered ${read.status}: ${read.text}`);
  if (read.json.zone !== "") return;
  const saved = await fetchAsPage(page, "POST", "/api/1/account/preferences", { zone });
  ensure(saved.status === 200, `the time zone was not kept: the console answered ${saved.status}: ${saved.text}`);
  await page.goto(new URL("/", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
}

/** Signs in with nothing more to give, and waits for the signed-in page. */
export async function signInDone(t, page, name, password) {
  const answer = await signIn(page, name, password);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await t.see(signOutButton(page));
  await leaveWelcome(t, page);
}

/**
 * Chooses "Sign out" in the account menu and waits for the sign-in page.
 * With terminals running, the console asks first, and they end with the
 * sign-out.
 */
export async function signOut(t, page) {
  const menu = await openAccountMenu(t, page);
  const answer = answerTo(page, "/api/1/sign-out");
  await accountItem(menu, "Sign out").click();
  const question = page.getByRole("dialog", { name: "Sign out?" });
  const asked = await Promise.race([
    answer.then(() => false),
    question.waitFor({ state: "visible" }).then(() => true, () => false),
  ]);
  if (asked) await question.getByRole("button", { name: "End them and sign out", exact: true }).click();
  await answer;
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
  await leaveWelcome(t, page);
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

/**
 * The page of the signed-in account's second step: from the account menu to
 * "Your account", and there the link "Manage the second step".
 */
export async function openSecondStep(t, page) {
  // A page that was on it when the session ended is back on it at once.
  if (!new URL(page.url()).pathname.endsWith("/second-step")) {
    await leaveWelcome(t, page);
    const menu = await openAccountMenu(t, page);
    await accountItem(menu, "Your account").click();
    await page.getByRole("link", { name: "Manage the second step" }).click();
  }
  await t.see(page.getByRole("heading", { name: "Second step", level: 1 }));
  await t.see(page.getByRole("heading", { name: "Trusted browsers" }));
}

/** The origin of the console that `page` shows, `https://host:port`. */
export const originOf = (page) => new URL(page.url()).origin;

/**
 * The file on the lab PC in which a test's host step leaves what its browser
 * script reads, for the machine whose console `page` shows. The lab runs a
 * test on each of its machines at once, so each machine has its own:
 * `/tmp/NAME-{machine}-{machine-port:9090}` in the test's steps.
 */
export const labNote = (name, page) => {
  const { hostname, port } = new URL(page.url());
  return `/tmp/${name}-${hostname}-${port || "443"}`;
};

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

/**
 * A request as the console's own page makes it with fetch(): run in the
 * page, so that its origin, its cookie and its content security policy
 * (`connect-src 'self'`) decide what is sent and whether it may be, and
 * carrying the session's token. `body` is sent as JSON when it is not
 * null. Gives `{status, text, json}`, `json` being null when the answer is
 * not JSON.
 */
export async function fetchAsPage(page, method, route, body = null) {
  const { csrf } = await sessionOf(page);
  return page.evaluate(
    async ({ method, route, body, csrf }) => {
      const headers = { "X-Dukovo-CSRF": csrf };
      const init = { method, headers, credentials: "same-origin" };
      if (body !== null) {
        headers["Content-Type"] = "application/json";
        init.body = JSON.stringify(body);
      }
      const answer = await fetch(route, init);
      const text = await answer.text();
      let json = null;
      try {
        json = JSON.parse(text);
      } catch {
        // Not JSON: the caller sees the text.
      }
      return { status: answer.status, text, json };
    },
    { method, route, body, csrf },
  );
}

/** The route `template`, whose `{id}` part is `id`. */
export const routeOf = (template, id) => template.replace("{id}", String(id));

/** Throws unless `answer`, as `fetchAsPage` gives it, is the console's error `id` with HTTP status `status`; `what` says what was asked. */
export function mustBe(answer, status, id, what) {
  const error = answer.json?.error;
  ensure(
    answer.status === status && error?.id === id,
    `${what} was answered ${answer.status} ${error?.id ?? answer.text}, not ${status} ${id}`,
  );
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
