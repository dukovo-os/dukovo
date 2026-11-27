// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The third part of journeys/new-server, in the browser: the administrator
// adds a website that uses the certificate Pebble made for it (new-server.toml
// got it with `cert get`, as services/acme does), and sets alerts by email
// from the Alerts page. new-server.toml then fetches the site from the lab PC
// over HTTPS with Pebble's authority and has the machine mail an alert to the
// lab PC's Mailpit.

import { ensure, fetchAsPage, lookAll } from "../web-console/console.mjs";
import { pressAndAnswer } from "../web-console/trial.mjs";
import { eventually, journeyInfo, readJson, signedIn, unlock } from "./journey.mjs";

const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });

/** The steps of the run in `scope` as `{title, state}`, once none waits or runs any more. */
async function stepsEnd(page, scope, seconds) {
  const items = `${scope} li.flow-step`;
  await page.waitForFunction(
    (css) => {
      const all = [...document.querySelectorAll(css)];
      return all.length > 0 && all.every((li) => !["waiting", "running"].includes(li.getAttribute("data-state")));
    },
    items,
    { timeout: seconds * 1000 },
  );
  return page.locator(items).evaluateAll((all) => all.map((li) => ({ title: li.querySelector(".flow-head b")?.textContent.trim() ?? "", state: li.getAttribute("data-state") })));
}

/** Add a website for `name`, sent to use the certificate `name` that is here; gives the site as `website list` says it. */
async function addWebsite(t, page, name, owner) {
  await page.goto(new URL(`/websites/add?name=${encodeURIComponent(name)}&certificate=${encodeURIComponent(name)}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a website", exact: true }));
  const field = (label) => page.getByLabel(label, { exact: true });
  await t.see(field("Domain"));
  ensure((await field("Domain").inputValue()) === name, `the domain is ${await field("Domain").inputValue()}, not ${name}`);
  ensure((await field("Folder").inputValue()) === `/shared/www/sites/${name}`, `the folder is ${await field("Folder").inputValue()}`);
  ensure((await field("Who may change them").inputValue()) === owner, `the owner is ${await field("Who may change them").inputValue()}, not ${owner}`);
  const here = page.locator("label.radio-card").filter({ hasText: "One that is here" });
  await t.see(here);
  ensure(await here.locator("input").isChecked(), "One that is here is not the certificate chosen for a site sent here with a certificate");
  ensure((await field("The certificate").inputValue()) === name, `the certificate offered is ${await field("The certificate").inputValue()}, not ${name}`);
  // The plan is read as an administrator; it says what the add will do before anything is done.
  await t.see(page.locator('#plan li[data-row="certificate"]'));
  await lookAll(t, page, "journey-website-add");

  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === "/api/1/websites/add", { timeout: 120_000 });
  await buttonOf(page, "Add the website").click();
  const reply = await answered;
  ensure(reply.status() === 200 || reply.status() === 202, `Add the website was answered ${reply.status()}: ${await reply.text()}`);
  await t.see(page.locator(".web-run-slot h2").filter({ hasText: `Adding ${name}` }));
  const steps = await stepsEnd(page, ".web-run-slot", 600);
  ensure(steps.every((step) => step.state === "done"), `the steps of adding ${name} are ${JSON.stringify(steps)}`);
  await t.see(page.locator(".web-after .banner").filter({ hasText: `${name} is live: https://${name}/` }));

  const site = await eventually(async () => {
    const listed = await readJson(await fetchAsPage(page, "GET", "/api/1/websites"), "GET /api/1/websites");
    const found = listed.websites.find((one) => one.name === name);
    ensure(found !== undefined && found.https === true, `website list says ${JSON.stringify(found)} of ${name}`);
    return found;
  }, `${name} is not listed with HTTPS`, 60);
  ensure(site.certificate === name, `${name} uses the certificate ${site.certificate}, not ${name}`);
  return site;
}

/** The Email card: the address, the server and the port, and Save; the page then says that email is set up. */
async function setEmail(t, page, info) {
  await page.goto(new URL("/alerts", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Alerts", exact: true }));
  const email = page.locator("main section.card#email");
  await t.see(email.getByText("Send alerts to an email address through your mail provider.", { exact: true }));
  const field = (label) => email.getByLabel(label, { exact: true });
  await field("Send to").fill(info.mail_to);
  await field("Mail server").fill(info.mail_server);
  await field("Port").selectOption("587");
  await pressAndAnswer(t, page, buttonOf(email, "Save"), "/api/1/alerts/email");
  await t.see(email.getByText(/^Email is set up\./));
  const channels = await eventually(async () => {
    const listed = await readJson(await fetchAsPage(page, "GET", "/api/1/alerts/channels"), "GET /api/1/alerts/channels");
    ensure(listed.some((one) => one.kind === "email"), "alerts status lists no email channel after Save");
    return listed;
  }, "the machine did not list email after Save", 30);
  ensure(String(channels.find((one) => one.kind === "email").channel).includes(info.mail_to), `the email channel is not for ${info.mail_to}`);
  await lookAll(t, page, "journey-alerts-email");
}

export default async function check(t) {
  const info = await journeyInfo();
  const { page } = await signedIn(t, info.admin, info.admin_password);
  await unlock(page, info.admin_password);
  await addWebsite(t, page, info.site, info.admin);
  await setEmail(t, page, info);
}
