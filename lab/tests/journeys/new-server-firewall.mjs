// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The second part of journeys/new-server, in the browser: the administrator
// that the first part added signs in to the console and sets two firewall
// rules from the Firewall page, each tried and kept. new-server.toml then
// looks at the rules over SSH and from the lab PC, which reaches only the
// ports they allow.

import { ensure, fetchAsPage, lookAll, sessionOf, sleep } from "../web-console/console.mjs";
import { WORDS, bannerBecomes, bannerEnds, keepOf, pressAndAnswer } from "../web-console/trial.mjs";
import { infoOf, readJson, signedIn, unlock } from "./journey.mjs";

const TRIED = "The firewall rules are being tried.";

const form = (page) => page.locator("form#add");
const rulesCard = (page) => page.locator(".fw-rules");

/** The rules as the machine lists them: `firewall list --json`'s `rules`, each with its number and text. */
async function rulesNow(page) {
  const listed = await readJson(await fetchAsPage(page, "GET", "/api/1/firewall"), "GET /api/1/firewall");
  ensure(Array.isArray(listed.rules), `GET /api/1/firewall gives no rules: ${JSON.stringify(listed)}`);
  return listed.rules.map((rule) => rule.text);
}

/** Reads the rules until they are `wanted`, for up to `seconds`: the page and the machine are a moment apart. */
async function rulesBecome(page, wanted, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const now = await rulesNow(page).catch(() => null);
    if (JSON.stringify(now) === JSON.stringify(wanted)) return;
    ensure(Date.now() < until, `the rules did not become ${JSON.stringify(wanted)} within ${seconds} seconds: ${JSON.stringify(now)}`);
    await sleep(1000);
  }
}

/** Fills the form of Add a rule with an allowing rule for `ports` from `from`, tries it and keeps it. */
async function allowAndKeep(t, page, ports, from) {
  await page.getByRole("button", { name: "Add a rule", exact: true }).click();
  await t.see(form(page));
  await form(page).getByRole("radio", { name: "Allow", exact: true }).check();
  await form(page).locator("#fw-protocol").selectOption("tcp");
  await page.locator("#fw-ports").fill(ports);
  await form(page).getByRole("radio", { name: "An address or network…", exact: true }).check();
  await page.locator("#fw-from-text").fill(from);
  const tryButton = form(page).getByRole("button", { name: /^Try for / });
  await pressAndAnswer(t, page, tryButton, "/api/1/firewall/add");
  await bannerBecomes(page, (banner) => banner.running && banner.heading === TRIED, "show the rules being tried");
  // The browser reaching the console over the new rules is what Keep waits for.
  await bannerBecomes(page, (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 40);
  await pressAndAnswer(t, page, keepOf(page), "/api/1/firewall/trial/keep");
  await bannerEnds(page, "kept", 20);
}

export default async function check(t) {
  const info = await infoOf("new-server");
  const { page } = await signedIn(t, info.admin, info.admin_password);
  const session = await sessionOf(page);
  ensure(session.admin === true, `${info.admin} signs in to the console as no administrator`);
  await unlock(page, info.admin_password);
  await page.goto(new URL("/firewall", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Firewall", level: 1, exact: true }));
  await t.see(rulesCard(page).getByRole("heading", { level: 2, name: "Rules", exact: true }));
  ensure((await rulesNow(page)).length === 0, "a new server has firewall rules before its owner sets any");

  await allowAndKeep(t, page, "8081", session.from);
  await allowAndKeep(t, page, "8084-8085", session.from);
  const wanted = [`allow tcp 8081 from ${session.from}`, `allow tcp 8084-8085 from ${session.from}`];
  await rulesBecome(page, wanted);

  // The page draws the machine's rules, and its test of a connection says which one decides.
  await page.waitForFunction((count) => document.querySelectorAll(".fw-rules tbody tr[data-number]").length === count, wanted.length, { timeout: 30_000 });
  const card = page.locator("form.fw-test");
  await card.locator("#fw-test-protocol").selectOption("tcp");
  await card.locator("#fw-test-port").fill("8085");
  const answered = page.waitForResponse((reply) => reply.request().method() === "GET" && new URL(reply.url()).pathname === "/api/1/firewall/test");
  await card.getByRole("button", { name: "Test", exact: true }).click();
  const machine = await (await answered).json();
  ensure(machine.rule === 2 && machine.allowed === true, `the test of tcp 8085 gives ${JSON.stringify(machine)}, and rule 2 allows it`);
  await t.see(card.locator(".fw-verdict"));
  await lookAll(t, page, "journey-firewall");
}
