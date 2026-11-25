// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The last part of journeys/new-server, in the browser, after the machine was
// restarted: the administrator signs in again, in an engine of its own, and
// the console shows what the owner set before: the accounts and the key, the
// firewall rules, the website with its certificate and the email for alerts.
// The reader then signs in and can change none of it: each page offers it no
// button and each call it makes is refused, and the machine's rules are the
// same afterwards.

import { readFile } from "node:fs/promises";

import { ensure, fetchAsPage, mustBe, signOut } from "../web-console/console.mjs";
import { journeyFolder, journeyInfo, readJson, signedIn, unlock, wordsOf } from "./journey.mjs";

/** The administrator's view of each page, held to what the machine answers. */
async function administrator(t, info) {
  const { page } = await signedIn(t, info.admin, info.admin_password);
  await unlock(page, info.admin_password);

  await page.goto(new URL("/accounts", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Accounts", level: 1, exact: true }));
  for (const name of [info.owner, info.admin, info.reader]) await t.see(page.getByRole("link", { name, exact: true }).first());
  const shown = await readJson(await fetchAsPage(page, "GET", `/api/1/accounts/${info.admin}`), `GET /api/1/accounts/${info.admin}`);
  ensure(shown.keys.length === 1 && shown.keys[0].comment === "journey-key", `${info.admin} has the keys ${JSON.stringify(shown.keys)} after the restart`);

  await page.goto(new URL("/firewall", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Firewall", level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelectorAll(".fw-rules tbody tr[data-number]").length === 2, null, { timeout: 30_000 });
  const rules = (await readJson(await fetchAsPage(page, "GET", "/api/1/firewall"), "GET /api/1/firewall")).rules.map((rule) => rule.text);
  ensure(rules.length === 2 && rules[0].startsWith("allow tcp 8081 from ") && rules[1].startsWith("allow tcp 8084-8085 from "), `the firewall has ${JSON.stringify(rules)} after the restart`);

  await page.goto(new URL(`/websites/${encodeURIComponent(info.site)}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: info.site, exact: true }));
  const listed = await readJson(await fetchAsPage(page, "GET", "/api/1/websites"), "GET /api/1/websites");
  const site = listed.websites.find((one) => one.name === info.site);
  ensure(site?.https === true && site.certificate === info.site, `website list says ${JSON.stringify(site)} after the restart`);
  ensure((await wordsOf(page.locator(".web-site dd[data-row=\"https\"]"))).includes(info.site), "the site's page does not name its certificate");

  await page.goto(new URL(`/certificates/${encodeURIComponent(info.site)}`, page.url()).href);
  const certificate = page.getByRole("dialog", { name: info.site, exact: true });
  await t.see(certificate);
  await t.see(certificate.getByText(/^Pebble /));
  await t.see(certificate.getByRole("link", { name: "nginx", exact: true }));

  await page.goto(new URL("/alerts", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Alerts", exact: true }));
  await t.see(page.locator("main section.card#email").getByText(/^Email is set up\./));
  const channels = await readJson(await fetchAsPage(page, "GET", "/api/1/alerts/channels"), "GET /api/1/alerts/channels");
  ensure(channels.some((one) => one.kind === "email"), "alerts status lists no email after the restart");
  await signOut(t, page);
  await page.context().close();
}

/** The reader sees the pages and changes nothing: no button, and every call refused. */
async function reader(t, info) {
  const { page } = await signedIn(t, info.reader, info.reader_password);
  const stranger = (await readFile(`${journeyFolder()}/stranger.pub`, "utf8")).trim();
  const refused = [
    ["/api/1/accounts/add", { name: "journey-intruder", admin: true, password: "lab-journey-password-9" }],
    ["/api/1/firewall/add", { action: "allow", protocol: "tcp", ports: "8090" }],
    ["/api/1/websites/add", { name: "intruder.acme-lab.example", kind: "static", owner: info.reader }],
    ["/api/1/alerts/email", { to: "intruder@example.org", server: "mail.example.org", port: "587" }],
    [`/api/1/accounts/${info.admin}/keys/add`, { line: stranger }],
  ];
  for (const [route, body] of refused) {
    const answer = await fetchAsPage(page, "POST", route, body);
    mustBe(answer, 403, "ADM-122", `a reader's POST ${route}`);
  }

  await page.goto(new URL("/firewall", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Firewall", level: 1, exact: true }));
  await page.waitForFunction(() => document.querySelectorAll(".fw-rules tbody tr[data-number]").length === 2, null, { timeout: 30_000 });
  ensure((await page.getByRole("button", { name: "Add a rule", exact: true }).count()) === 0, "a reader's Firewall page has Add a rule");
  await page.goto(new URL("/accounts", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Accounts", level: 1, exact: true }));
  ensure((await page.getByRole("button", { name: "Add an account…", exact: true }).count()) === 0, "a reader's Accounts page has Add an account…");
  await page.goto(new URL(`/websites/${encodeURIComponent(info.site)}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: info.site, exact: true }));
  await t.see(page.getByText("Only an administrator can add, change or remove a website."));
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const info = await journeyInfo();
  await administrator(t, info);
  await reader(t, info);
}
