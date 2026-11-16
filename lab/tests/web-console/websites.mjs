// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/websites drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the list at
// /websites, Add a website at /websites/add and one site's page at
// /websites/{name}, each as the machine has it. The list shows what `website
// list --json` says, which `GET /api/1/websites` runs. Add a website shows what
// happens before anything is done: the form's plan is `website add --dry-run`
// (`GET /api/1/websites/plan/{name}`, read as an administrator, so a locked
// console is asked to unlock), and Preview the files first shows the files the
// command would write. Add the website runs the steps (`POST /api/1/websites/add`,
// a task the page follows). A site's page turns HTTPS on and off
// (`POST /api/1/websites/{name}/https`, a task), removes the site with its folder
// or without it, with its name typed first (`POST /api/1/websites/{name}/remove`,
// a task), and asks nginx to check the sites as they are
// (`GET /api/1/websites/check`).
//
// The script runs three times for each engine, with the machine in the state
// that the word in /tmp/lab-websites-machine says, which websites.toml writes
// with the machine's address and the lab PC's ports for its port 80 and 443:
//
//   a  A machine with no site. A reader is told who adds one. An administrator
//      adds a static site with this machine's own certificate and sees the
//      lab PC reach it over HTTPS; turns its HTTPS off and on; adds a PHP site
//      that asks Let's Encrypt for a certificate, which fails here, so that the
//      site stays live over HTTP and its page has Get the certificate; tries
//      that again, and serves it with the certificate that Pebble made and the
//      machine has, which Certificates then says is used by the site; and adds
//      a plain site that it removes with its folder kept, adds again, and
//      removes with its folder.
//   b  The first site's file has a line added by hand. It is shown as changed,
//      its HTTPS is left alone, nginx's check passes, and the site's file
//      is kept as written when another site is added and removed. A reader
//      sees the same and has no button that works, and a phone at 390 by 844
//      shows the list, a site and Add a website with its files.
//   c  That file has a line that nginx does not know. Check the sites says
//      so, with nginx's own line; a site cannot be removed or added, and the
//      page says why (WEB-103).
//
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll).

import { readFile } from "node:fs/promises";
import http from "node:http";
import https from "node:https";

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-websites";
const READER = "wc-websites-reader";

// What websites.toml writes: the machine, the lab PC's ports for its port 80 and
// 443, and the state the machine is in.
const MACHINE_FILE = "/tmp/lab-websites-machine";

// The sites of the test. The static site is named after this machine, since its
// certificate names no other name; the others are under a domain that Pebble's DNS
// server leads to the machine.
const PHP = "php.acme-lab.example";
const PHP_WWW = `www.${PHP}`;
const PLAIN = "plain.acme-lab.example";
const KEPT = "kept.acme-lab.example";
const PHONE_SITE = "phone.acme-lab.example";
const BROKEN_SITE = "broken.acme-lab.example";
const NOBODY = "nobody.acme-lab.example";

// The certificate that Pebble made for the PHP site, and the machine's own.
const PEBBLE = "site.acme-lab.example";
const OWN = "this-machine";

// What the machine's files hold after the hand edits of websites.sh.
const MARK = "# lab: kept by hand";
const BROKEN = "lab_broken_directive";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with a
// site's name for {name}.
const ROUTES = {
  list: "GET /api/1/websites",
  check: "GET /api/1/websites/check",
  plan: "GET /api/1/websites/plan/{name}",
  add: "POST /api/1/websites/add",
  https: "POST /api/1/websites/{name}/https",
  remove: "POST /api/1/websites/{name}/remove",
};

/** The path of a route as the table names it, for the site `name`. */
const pathOf = (route, name = "") => route.slice(route.indexOf(" ") + 1).replace("{name}", encodeURIComponent(name));

/** The method of a route as the table names it. */
const methodOf = (route) => route.slice(0, route.indexOf(" "));

/** The route as `track` writes it, for `name`. */
const routeFor = (route, name = "") => `${methodOf(route)} ${pathOf(route, name)}`;

/** The address of a site's page. */
const hrefOf = (name) => `/websites/${encodeURIComponent(name)}`;

// The words the pages say where this test looks for them.
const READER_WORDS = "Only an administrator can add, change or remove a website.";
const NONE_YET = "No websites yet.";
const UNLOCK_WORDS = "The checks of the names, nginx and the folder, and the files, are read as an administrator. Unlock to see them.";
const CHECK_PASSED = "nginx's check of the sites passed.";
const CHECK_FAILED = "nginx's check of the sites failed, so nothing is changed until it passes.";
const TERMS = "I agree to Let's Encrypt's terms of service";
const TERMS_FAULT = "Agree to Let's Encrypt's terms of service to get a certificate.";
const NOT_GOT = `${PHP} is live over HTTP. The certificate was not got: the site's page has Get the certificate to try again.`;
const HAND_WORDS = "Changed by hand · kept as you wrote it";
const HAND_LOCKED = "The file was changed by hand, so website does not write it: add the HTTPS server yourself, as website(8) shows.";
const BLOCKED = "Put right what the plan says first.";

// --- Waiting and reading ---

/** Tries `test` until it stops throwing, for up to `seconds`; gives what it gave. */
async function eventually(test, what, seconds = 60) {
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

/** The fingerprint as hex in capitals, which is the same for each way the console and Node write it. */
const hexOf = (text) => String(text ?? "").replace(/[^0-9a-fA-F]/g, "").toUpperCase();

/** What websites.toml wrote: `{host, http, https, phase}`. */
async function labOf() {
  const [host, port80, port443, phase] = (await readFile(MACHINE_FILE, "utf8")).trim().split("\n");
  return { host, http: Number(port80), https: Number(port443), phase };
}

/**
 * What the machine answers the lab PC for the site `name`, over `scheme`, which
 * is how a visitor meets it: nginx chooses the site by the name, so the request
 * says the name wherever it goes. Gives `{status, location, body, fingerprint}`.
 */
function visit(lab, scheme, name) {
  return new Promise((resolve, reject) => {
    const options = { host: lab.host, port: scheme === "https" ? lab.https : lab.http, path: "/", method: "GET", headers: { Host: name }, timeout: 15_000 };
    const request = scheme === "https" ? https.request({ ...options, servername: name, rejectUnauthorized: false }) : http.request(options);
    request.on("response", (reply) => {
      const fingerprint = scheme === "https" ? reply.socket.getPeerCertificate()?.fingerprint256 ?? "" : "";
      const chunks = [];
      reply.on("data", (chunk) => chunks.push(chunk));
      reply.on("end", () => resolve({ status: reply.statusCode, location: reply.headers.location ?? "", body: Buffer.concat(chunks).toString("utf8"), fingerprint: hexOf(fingerprint) }));
    });
    request.on("timeout", () => request.destroy(new Error(`${scheme}://${name}/ gave no answer in 15 seconds`)));
    request.on("error", reject);
    request.end();
  });
}

/** Waits until the visit of `name` over `scheme` gives what `test` accepts, and gives it. */
const visitBecomes = (lab, scheme, name, test, what, seconds = 60) =>
  eventually(async () => {
    const seen = await visit(lab, scheme, name);
    ensure(test(seen), `${scheme}://${name}/ gave ${seen.status} ${seen.location} ${seen.fingerprint}`);
    return seen;
  }, `the machine did not ${what}`, seconds);

/** A visitor of `http://name/` is sent to `https://name/`. */
const sentToHttps = (name) => (seen) => seen.status >= 300 && seen.status < 400 && seen.location.startsWith(`https://${name}`);

/** A site that answers at once, with a page. */
const answered = (seen) => seen.status === 200 && seen.body.length > 0;

/** What a read route answers, which must be 200; gives its JSON. */
async function readOf(page, route, name = "") {
  const answer = await fetchAsPage(page, methodOf(route), pathOf(route, name));
  ensure(answer.status === 200, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** `website list --json`, from the page's own route: `{websites: [...], nginx, php_fpm}`. */
async function readWebsites(page) {
  const read = await readOf(page, ROUTES.list);
  ensure(Array.isArray(read?.websites), `${ROUTES.list} gives no list of websites: ${JSON.stringify(read)}`);
  return read;
}

/** The site `name` of `read`, or undefined. */
const siteIn = (read, name) => read.websites.find((one) => one.name === name);

/** Reads the websites until `test(read)` holds, for up to `seconds`, and gives what was read. */
const websitesBecome = (page, test, what, seconds = 90) =>
  eventually(async () => {
    const read = await readWebsites(page);
    ensure(test(read), `the machine lists ${read.websites.map((one) => one.name).join(", ") || "no website"}`);
    return read;
  }, `the machine did not ${what}`, seconds);

/** `cert list --json` and `cert show`, from the Certificates page's routes. */
const readCertificates = (page) => fetchAsPage(page, "GET", "/api/1/certificates").then((answer) => {
  ensure(answer.status === 200 && Array.isArray(answer.json), `GET /api/1/certificates was answered ${answer.status}: ${answer.text}`);
  return answer.json;
});
const fingerprintOf = async (page, name) => {
  const answer = await fetchAsPage(page, "GET", `/api/1/certificates/${encodeURIComponent(name)}`);
  ensure(answer.status === 200 && answer.json?.fingerprint_sha256, `GET /api/1/certificates/${name} was answered ${answer.status}: ${answer.text}`);
  return hexOf(answer.json.fingerprint_sha256);
};

/** Whether `name` is one that a website can have: a name, not an address, in small letters. */
const isSiteName = (name) => /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*$/.test(name) && !/^\d+(?:\.\d+){3}$/.test(name) && name !== "localhost";

/** The name of the static site: one of the names that this machine's own certificate has, a `.local` name when there is one. */
async function machineSite(page) {
  const own = (await readCertificates(page)).find((one) => one.name === OWN);
  ensure(own !== undefined && Array.isArray(own.names), `cert list has no ${OWN} with its names`);
  const usable = own.names.filter(isSiteName);
  const name = usable.find((one) => one.endsWith(".local")) ?? usable.find((one) => one.includes(".")) ?? usable[0];
  ensure(name !== undefined, `${OWN} names ${JSON.stringify(own.names)}, and none of them is a name a website can have`);
  return name;
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The pages ---

const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
const linkOf = (scope, name) => scope.getByRole("link", { name, exact: true });
const dialogOf = (page, name) => page.getByRole("dialog", { name, exact: true });
const field = (page, label) => page.getByLabel(label, { exact: true });
const planRow = (page, key) => page.locator(`#plan li[data-row="${key}"]`);
const way = (page, title) => page.locator("label.radio-card").filter({ hasText: title });
const rowOf = (page, name) => page.locator(`.web-list tbody tr[data-name="${name}"]`);
const rowFact = (page, key) => page.locator(`.web-site dd[data-row="${key}"]`);

/** Whether the dialog that asks to unlock came within a moment of a press, and answers it when it did. */
async function answerIfAsked(t, page) {
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  return asked;
}

/**
 * Presses `control`, which asks the console to `method` `route`, and gives the
 * answer: 200, or 202 for a task, or any other when `any` is given. A console
 * that is locked asks to unlock first, and the dialog is answered.
 */
async function pressFor(t, page, control, route, method = "POST", { any = false } = {}) {
  const answer = page.waitForResponse((reply) => reply.request().method() === method && new URL(reply.url()).pathname === route, { timeout: 120_000 });
  answer.catch(() => {});
  await control.click();
  await answerIfAsked(t, page);
  const reply = await answer;
  ensure(any || reply.status() === 200 || reply.status() === 202, `${method} ${route} was answered ${reply.status()}: ${await reply.text()}`);
  return reply;
}

/** Waits for `locator` to be visible for up to `seconds`: a read of the machine that the page asks for may take that long. */
const seeSoon = (locator, seconds) => locator.waitFor({ state: "visible", timeout: seconds * 1000 });

/** Opens the list and waits for its rows, or for the words that stand in their place. */
async function openList(t, page) {
  await page.goto(new URL("/websites", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Websites", exact: true }));
  await page.waitForFunction(() => document.querySelectorAll(".web-list table, .web-list .web-empty, .web-list p").length > 0 && !/Reading the websites/.test(document.querySelector(".web-list")?.textContent ?? ""), null, { timeout: 30_000 });
}

/** Opens the page of the site `name` and waits for its facts; the heading is its first name. */
async function openSite(t, page, name) {
  await page.goto(new URL(hrefOf(name), page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name, exact: true }));
  await t.see(rowFact(page, "names"));
}

/** Opens Add a website, with `query` after its address. */
async function openAdd(t, page, query = "") {
  await page.goto(new URL(`/websites/add${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a website", exact: true }));
  await t.see(field(page, "Domain"));
}

/**
 * Does `act`, which changes the form of Add a website, and waits for the plan of
 * the site `name` to be read. The plan is read as an administrator: a console
 * that is locked says so in the card, and is unlocked from there.
 */
async function planAfter(t, page, name, act) {
  const path = pathOf(ROUTES.plan, name);
  const reads = () => page.waitForResponse((reply) => reply.request().method() === "GET" && new URL(reply.url()).pathname === path, { timeout: 90_000 });
  const first = reads();
  first.catch(() => {});
  await act();
  const unlock = page.locator('#plan button[data-action="unlock"]');
  // A card that never asks to unlock must not end the test when its wait runs out: only the plan's own wait does.
  const asks = unlock.waitFor({ state: "visible", timeout: 90_000 }).then(() => "locked", () => new Promise(() => {}));
  const outcome = await Promise.race([first.then(() => "planned"), asks]);
  if (outcome === "locked") {
    ensure((await page.locator("#plan .web-unlock").innerText()).includes(UNLOCK_WORDS), "the card of a locked console does not say why the plan is not read");
    const second = reads();
    second.catch(() => {});
    await unlock.click();
    await answerUnlock(t, page);
    await second;
  }
  // The card draws what it read a moment after the answer.
  await eventually(async () => ensure((await page.locator('#plan .web-unlock:not([hidden])').count()) === 0, "the card still asks to unlock"), "the plan was not drawn", 15);
  return outcome;
}

/** Types `name` as the domain of Add a website and waits for its plan. */
const typeDomain = (t, page, name) => planAfter(t, page, name, () => field(page, "Domain").fill(name));

/** The steps in `scope` as `{title, state}`, when none is waiting or running any more, for up to `seconds`. */
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

/** The states of `steps`, as words: "done, failed, skipped". */
const statesOf = (steps) => steps.map((step) => step.state).join(", ");

/** Presses Add the website, and waits for the steps of adding `name` to end. Gives them. */
async function addIt(t, page, name, seconds = 600) {
  await pressFor(t, page, buttonOf(page, "Add the website"), pathOf(ROUTES.add));
  await t.see(page.locator(".web-run-slot h2").filter({ hasText: `Adding ${name}` }));
  return stepsEnd(page, ".web-run-slot", seconds);
}

/** The words of the banner after the steps of Add a website. */
const afterWords = (page) => page.locator(".web-after .banner").first().innerText().then((text) => text.replace(/\s+/g, " ").trim());

/** The paragraph with the words of nginx's check: the passed or failed banner, and nginx's own line for a failure. */
const checkBanner = (page) => page.locator(".web-check .banner");

/** Presses Check the sites and waits for the words of what nginx found. Gives `{words, line}`. */
async function checkSites(t, page, passes) {
  await pressFor(t, page, buttonOf(page, "Check the sites"), pathOf(ROUTES.check), "GET", { any: !passes });
  const wanted = passes ? CHECK_PASSED : CHECK_FAILED;
  await t.see(checkBanner(page).filter({ hasText: wanted }));
  const words = (await checkBanner(page).first().innerText()).replace(/\s+/g, " ").trim();
  const line = await page.locator(".web-check pre").first().innerText().catch(() => "");
  if (passes) ensure(line === "", `nginx's check passed and the page shows its line ${JSON.stringify(line)}`);
  return { words, line };
}

// --- Add a website ---

/** The fields of the form as they are for a site that is `name`, before anything is changed. */
async function formDefaults(page, name, { owner }) {
  ensure((await field(page, "Folder").inputValue()) === `/shared/www/sites/${name}`, `the folder is ${await field(page, "Folder").inputValue()}, not /shared/www/sites/${name}`);
  ensure((await field(page, "Who may change them").inputValue()) === owner, `the owner is ${await field(page, "Who may change them").inputValue()}, not ${owner}`);
  ensure(await way(page, "Files from a folder").locator("input").isChecked(), "the first kind of site is not chosen");
}

/** Preview the files first: the file that the command writes for the site, new, with its lines numbered. */
async function previewTheFiles(t, page, name) {
  await buttonOf(page, "Preview the files first").click();
  await answerIfAsked(t, page);
  const preview = dialogOf(page, "The files that adding the site writes");
  await t.see(preview);
  const paths = await preview.locator("section.web-file").evaluateAll((all) => all.map((one) => one.getAttribute("data-path")));
  ensure(paths.includes(`/system/settings/websites/${name}.conf`), `the preview shows ${JSON.stringify(paths)}, which has no /system/settings/websites/${name}.conf`);
  const file = preview.locator(`section.web-file[data-path="/system/settings/websites/${name}.conf"]`);
  ensure((await file.locator(".pill").first().innerText()).trim() === "new", "the file of the site is not marked new");
  ensure((await file.locator(".web-no").first().innerText()).trim() === "1", "the first line of the file is not numbered 1");
  const text = await file.locator(".web-lines").innerText();
  ensure(text.includes("server_name") && text.includes(name), `the file the preview shows has no server_name of ${name}: ${text.slice(0, 200)}`);
  return preview;
}

/** The static site with this machine's own certificate: the form, the plan, the files, the steps, and the lab PC reaching it. */
async function addMachineSite(t, page, lab, name) {
  await openAdd(t, page);
  const locked = await typeDomain(t, page, name);
  ensure(locked === "locked", "the console was not locked when the plan was asked for: the card did not ask to unlock");
  await formDefaults(page, name, { owner: ADMIN });
  if (name.includes(".")) ensure(!(await field(page, `Also answer for www.${name}`).isChecked()), "the www. name of this machine's own name is ticked");
  ensure(await way(page, "This machine's own").locator("input").isChecked(), "this machine's own certificate is not the one that is chosen for its own name");
  await t.see(planRow(page, "certificate").filter({ hasText: "Serves it with this machine's own certificate." }));
  await t.see(planRow(page, "site").filter({ hasText: "HTTPS only: http:// sends visitors to https://." }));
  // nginx, and the firewall: a row either says that nothing is to be changed or says what closes a port.
  await t.see(page.locator('#plan li[data-row="firewall"], #plan li[data-row^="port-"]').first());
  await lookAll(t, page, "websites-add");

  const preview = await previewTheFiles(t, page, name);
  await lookAll(t, page, "websites-preview");
  await buttonOf(preview, "Close").click();
  await t.gone(preview);

  const steps = await addIt(t, page, name);
  ensure(steps.length === 1 && steps[0].state === "done" && steps[0].title === `Add ${name}`, `the steps of adding ${name} are ${JSON.stringify(steps)}`);
  const words = await afterWords(page);
  ensure(words.includes(`${name} is live: https://${name}/`), `the page says ${JSON.stringify(words)} when ${name} is added`);
  await t.see(page.locator(".web-after").getByText("Browsers warn until they trust this machine's certificate; Certificates says how.", { exact: true }));
  await lookAll(t, page, "websites-added");

  // The lab PC reaches the site the way a visitor does: over HTTPS with this machine's certificate, and from http:// sent there.
  const own = await fingerprintOf(page, OWN);
  await visitBecomes(lab, "https", name, (seen) => answered(seen) && seen.fingerprint === own, `serve ${name} with ${OWN}`);
  await visitBecomes(lab, "http", name, sentToHttps(name), `send visitors of http://${name}/ to https://`);
  ensure(await linkOf(page, "Open its page").getAttribute("href") === hrefOf(name), "Open its page leads to another page than the site's");
}

/** The static site's page: its facts, its file and the words of what its HTTPS may do. */
async function machineSitePage(t, page, name) {
  await linkOf(page, "Open its page").click();
  await page.waitForURL((url) => url.pathname === hrefOf(name));
  await t.see(page.getByRole("heading", { level: 1, name, exact: true }));
  await t.see(rowFact(page, "names"));
  const read = await readWebsites(page);
  const site = siteIn(read, name);
  ensure(site !== undefined && site.https === true && site.certificate === OWN, `${ROUTES.list} says ${JSON.stringify(site)} of ${name}, which was added with ${OWN}`);
  ensure((await rowFact(page, "names").locator("a").first().getAttribute("href")) === `https://${name}/`, "the first name is not a link to the site over HTTPS");
  const serves = (await rowFact(page, "serves").innerText()).replace(/\s+/g, " ");
  ensure(serves.startsWith(`Files from /shared/www/sites/${name}, owned by ${ADMIN}`), `the page says ${JSON.stringify(serves)} of what the site serves`);
  const https = rowFact(page, "https");
  ensure((await https.innerText()).includes("This machine's certificate"), `the page says ${JSON.stringify(await https.innerText())} of its HTTPS`);
  ensure((await linkOf(https, OWN).getAttribute("href")) === `/certificates/${OWN}`, "the certificate is not a link to its page");
  // Its file is shown as website wrote it, and its text is there when it is asked for.
  const file = rowFact(page, "file");
  ensure((await file.innerText()).includes(`/system/settings/websites/${name}.conf`) && (await file.innerText()).includes("As website wrote it"), `the page says ${JSON.stringify(await file.innerText())} of the file`);
  await buttonOf(file, "Show the file").click();
  const shown = file.locator("pre");
  await t.see(shown);
  ensure((await shown.innerText()).includes("server_name"), "the text of the file has no server_name");
  await buttonOf(file, "Hide the file").click();
  await t.gone(shown);
  await lookAll(t, page, "websites-site");
}

/** Turn HTTPS off, and this machine's certificate on again: each a step with its words, and nginx's restart said first. */
async function httpsOffAndOn(t, page, lab, name) {
  await buttonOf(page, "Turn HTTPS off…").click();
  await answerIfAsked(t, page);
  const off = dialogOf(page, `Turn HTTPS off for ${name}?`);
  await t.see(off);
  const said = (await off.innerText()).replace(/\s+/g, " ");
  ensure(said.includes("is served over HTTP only afterwards") && said.includes("The certificate stays in certd.") && said.includes("nginx restarts once, since no other site uses this certificate"), `the dialog that turns HTTPS off says ${JSON.stringify(said)}`);
  await lookAll(t, page, "websites-off");
  await pressFor(t, page, buttonOf(off, "Serve it over HTTP only"), pathOf(ROUTES.https, name));
  await t.see(page.locator(".web-flow h2").filter({ hasText: `Turning HTTPS off for ${name}` }));
  const steps = await stepsEnd(page, ".web-flow", 300);
  ensure(statesOf(steps) === "done", `the steps of turning HTTPS off are ${JSON.stringify(steps)}`);
  await t.see(rowFact(page, "https").getByText("HTTP only", { exact: false }));
  ensure((await buttonOf(page, "Turn HTTPS off…").count()) === 0, "Turn HTTPS off… is still there on a site that is served over HTTP only");
  // The certificate stays in certd, and the site answers over HTTP, as a page, and no longer sends visitors on.
  ensure((await readCertificates(page)).some((one) => one.name === OWN), `${OWN} is gone from certd after HTTPS was turned off`);
  await visitBecomes(lab, "http", name, answered, `answer ${name} over HTTP after HTTPS was turned off`);

  await buttonOf(page, "Use this machine's certificate").click();
  await answerIfAsked(t, page);
  const use = dialogOf(page, `Serve ${name} over HTTPS`);
  await t.see(use);
  ensure((await field(use, "The certificate").inputValue()) === OWN, "the certificate that is chosen is not this machine's own");
  await pressFor(t, page, buttonOf(use, "Serve it over HTTPS"), pathOf(ROUTES.https, name));
  await t.see(page.locator(".web-flow h2").filter({ hasText: `Serving ${name} over HTTPS` }));
  const again = await stepsEnd(page, ".web-flow", 300);
  ensure(statesOf(again) === "done", `the steps of turning HTTPS on are ${JSON.stringify(again)}`);
  await t.see(rowFact(page, "https").getByText("This machine's certificate", { exact: false }));
  const own = await fingerprintOf(page, OWN);
  await visitBecomes(lab, "https", name, (seen) => answered(seen) && seen.fingerprint === own, `serve ${name} with ${OWN} again`);
}

/** A PHP site that asks Let's Encrypt for a certificate: the step fails, the site is live over HTTP, and nothing but its certificate is missing. */
async function addPhpSite(t, page, lab) {
  await openAdd(t, page);
  // The names are checked as a certificate's are, a moment after they are typed.
  const checked = page.waitForRequest((request) => new URL(request.url()).pathname.startsWith("/api/1/certificates/check/"), { timeout: 90_000 });
  checked.catch(() => {});
  await typeDomain(t, page, PHP);
  ensure(await field(page, `Also answer for ${PHP_WWW}`).isChecked(), "the www. name of a name that is not this machine's is not ticked");
  await way(page, "PHP pages").click();
  ensure(await way(page, "PHP pages").locator("input").isChecked(), "PHP pages is not chosen");
  ensure(await way(page, "From Let's Encrypt").locator("input").isChecked(), "Let's Encrypt is not the certificate that is chosen for a name that is not this machine's");
  await t.see(planRow(page, "certificate").filter({ hasText: `Gets a certificate for ${PHP} and ${PHP_WWW} from Let's Encrypt; certd renews it by itself.` }));
  await t.see(planRow(page, "restart").filter({ hasText: "Restarts nginx once" }));

  await checked;

  // The terms are agreed to before anything is sent.
  const sent = [];
  const watch = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.add)) sent.push(request.url());
  };
  page.on("request", watch);
  await buttonOf(page, "Add the website").click();
  await t.see(page.getByText(TERMS_FAULT, { exact: true }));
  await sleep(1000);
  page.off("request", watch);
  ensure(sent.length === 0, `Add the website sent ${sent.join(", ")} before the terms were agreed to`);
  await field(page, TERMS).check();
  await field(page, "Contact email (optional)").fill("lab@acme-lab.example");

  // The site is added, and the certificate cannot be got: the steps say so, and what stands.
  const steps = await addIt(t, page, PHP, 900);
  ensure(statesOf(steps) === "done, failed, skipped", `the steps of adding ${PHP} are ${JSON.stringify(steps)}, not the site added and the certificate not got`);
  ensure(steps[1].title === `Get the certificate for ${PHP}` && steps[2].title === `Serve ${PHP} over HTTPS`, `the steps are called ${JSON.stringify(steps.map((step) => step.title))}`);
  ensure((await page.locator(".web-run-slot .flow-stands:not([hidden])").first().innerText()).trim() === NOT_GOT, "the failed step does not say what stands");
  ensure((await page.locator('.web-run-slot .flow-step[data-state="failed"] .flow-problem').innerText()).trim() !== "", "the failed step says nothing of why");
  ensure((await afterWords(page)).includes(NOT_GOT), `the page says ${JSON.stringify(await afterWords(page))} after the certificate failed`);
  ensure((await page.locator('.web-after button[data-action="try-again"]').count()) === 0, "a certificate that cannot be got offers Try again, where the site's page has Get the certificate");
  ensure((await linkOf(page, "Open its page").getAttribute("href")) === hrefOf(PHP), "Open its page leads to another page than the PHP site's");
  await lookAll(t, page, "websites-certificate-failed");

  // The site is live over HTTP, which the lab PC sees, with PHP-FPM running its page.
  await visitBecomes(lab, "http", PHP, answered, `answer ${PHP} over HTTP after its certificate failed`);
  const read = await readWebsites(page);
  const site = siteIn(read, PHP);
  ensure(site !== undefined && site.https === false && site.kind === "php", `${ROUTES.list} says ${JSON.stringify(site)} of ${PHP}`);
  ensure(JSON.stringify(site.names) === JSON.stringify([PHP, PHP_WWW]), `the names of ${PHP} are ${JSON.stringify(site.names)}`);
}

/** The PHP site's page: Get the certificate again, which fails the same way, and the certificate that is here. */
async function getAndUse(t, page, lab) {
  await linkOf(page, "Open its page").click();
  await page.waitForURL((url) => url.pathname === hrefOf(PHP));
  await t.see(rowFact(page, "https"));
  ensure((await rowFact(page, "https").innerText()).includes("HTTP only"), "the HTTPS of a site whose certificate failed is not HTTP only");
  await t.see(buttonOf(page, "Get a certificate"));
  await t.see(buttonOf(page, "Use one that is here…"));
  ensure((await buttonOf(page, "Use this machine's certificate").count()) === 0, "this machine's certificate is offered for a name that is not its own");

  // Get a certificate: the terms first, then the same failure, and the site stays as it is.
  await buttonOf(page, "Get a certificate").click();
  const get = dialogOf(page, `Get a certificate for ${PHP}`);
  await seeSoon(get, 90);
  await buttonOf(get, "Get the certificate").click();
  await t.see(get.getByText(TERMS_FAULT, { exact: true }));
  await field(get, TERMS).check();
  await pressFor(t, page, buttonOf(get, "Get the certificate"), "/api/1/certificates/" + encodeURIComponent(PHP) + "/get");
  await t.see(page.locator(".web-flow h2").filter({ hasText: `Getting a certificate for ${PHP}` }));
  const steps = await stepsEnd(page, ".web-flow", 900);
  ensure(statesOf(steps) === "failed, skipped", `the steps of getting a certificate for ${PHP} are ${JSON.stringify(steps)}`);
  await t.see(page.locator(".web-flow .banner").filter({ hasText: NOT_GOT }));
  ensure((await page.locator('.web-flow button[data-action="try-again"]').count()) === 0, "Try again is offered for a certificate that cannot be got");
  await visitBecomes(lab, "http", PHP, answered, `answer ${PHP} over HTTP after the second try`);

  // One that is here: Pebble's certificate for the site's names.
  await buttonOf(page, "Use one that is here…").click();
  const use = dialogOf(page, `Serve ${PHP} over HTTPS`);
  await t.see(use);
  const options = await use.locator("select option").evaluateAll((all) => all.map((one) => one.value));
  ensure(options.includes(PEBBLE) && !options.includes(OWN), `the certificates the dialog offers are ${JSON.stringify(options)}: ${PEBBLE} names every name of the site, and ${OWN} none`);
  await field(use, "The certificate").selectOption(PEBBLE);
  await pressFor(t, page, buttonOf(use, "Serve it over HTTPS"), pathOf(ROUTES.https, PHP));
  await t.see(page.locator(".web-flow h2").filter({ hasText: `Serving ${PHP} over HTTPS` }));
  const done = await stepsEnd(page, ".web-flow", 300);
  ensure(statesOf(done) === "done", `the steps of serving ${PHP} over HTTPS are ${JSON.stringify(done)}`);
  const https = rowFact(page, "https");
  await t.see(linkOf(https, PEBBLE));
  ensure((await linkOf(https, PEBBLE).getAttribute("href")) === `/certificates/${PEBBLE}`, "the certificate is not a link to its page");
  const pebble = await fingerprintOf(page, PEBBLE);
  await visitBecomes(lab, "https", PHP, (seen) => answered(seen) && seen.fingerprint === pebble, `serve ${PHP} with ${PEBBLE}`);
  await visitBecomes(lab, "http", PHP, sentToHttps(PHP), `send visitors of http://${PHP}/ to https://`);
}

/** Certificates says which website uses a certificate, with its link. */
async function whereItIsUsed(t, page) {
  await page.goto(new URL("/certificates", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Certificates", exact: true }));
  const names = page.locator("#get-names");
  await t.see(names);
  await names.fill(`${PEBBLE} ${PHP} ${PHP_WWW}`);
  await names.press("Tab");
  const line = page.locator("#get p.small").filter({ hasText: "that uses it" });
  await seeSoon(line, 120);
  const words = (await line.innerText()).replace(/\s+/g, " ");
  ensure(words.includes(`${PEBBLE} is used by`) && words.includes(`The website that uses it: ${PHP}.`), `Certificates says ${JSON.stringify(words)} of where ${PEBBLE} is used`);
  ensure((await linkOf(line, PHP).getAttribute("href")) === hrefOf(PHP), "the website is not a link to its page");
}

/** A plain site, added and removed twice: with its folder kept, and with the folder taken too. */
async function addPlainSite(t, page, lab, name) {
  await openAdd(t, page);
  await typeDomain(t, page, name);
  await way(page, "None, HTTP only").click();
  await t.see(planRow(page, "certificate").filter({ hasText: "Serves it over HTTP only: pages and passwords travel unencrypted." }));
  const steps = await addIt(t, page, name);
  ensure(statesOf(steps) === "done", `the steps of adding ${name} are ${JSON.stringify(steps)}`);
  ensure((await afterWords(page)).includes(`${name} is live: http://${name}/`), `the page says ${JSON.stringify(await afterWords(page))} when ${name} is added`);
  await visitBecomes(lab, "http", name, answered, `answer ${name} over HTTP`);
}

/** Removes the site `name` from its page: its dialog says what stays, the name is typed first, and with `files` the folder goes too. */
async function removeSite(t, page, name, { files }) {
  await openSite(t, page, name);
  await buttonOf(page, "Remove…").click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Remove ${name}?`);
  await t.see(dialog);
  const said = (await dialog.innerText()).replace(/\s+/g, " ");
  ensure(said.includes(`Removes nginx's site for ${name}.`) && said.includes("Its folder is kept unless you tick the box below.") && said.includes("nginx reads its settings again, and no visitor is turned away."), `the dialog that removes ${name} says ${JSON.stringify(said)}`);
  const box = dialog.getByLabel(`Also remove its folder /shared/www/sites/${name} and everything in it`);
  ensure(!(await box.isChecked()), "the box that also removes the folder is ticked at the start");
  if (files) await box.check();
  const remove = buttonOf(dialog, "Remove the website");
  ensure(await remove.isDisabled(), "Remove the website works before the name is typed");
  const typed = dialog.getByLabel(`Type ${name} to go on`);
  await typed.fill(name.slice(0, -1));
  ensure(await remove.isDisabled(), "Remove the website works with a name that is not the site's");
  if (!files) await lookAll(t, page, "websites-remove");
  await typed.fill(name);
  ensure(!(await remove.isDisabled()), "Remove the website does not work with the site's name typed");
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.remove, name));
  sent.catch(() => {});
  await pressFor(t, page, remove, pathOf(ROUTES.remove, name));
  const body = (await sent).postData() ?? "";
  ensure(files === /"files"\s*:\s*true/.test(body), `${ROUTES.remove} was sent ${JSON.stringify(body)}, ${files ? "without" : "with"} the folder`);
  const steps = await stepsEnd(page, ".web-flow", 300);
  ensure(statesOf(steps) === "done", `the steps of removing ${name} are ${JSON.stringify(steps)}`);
  await t.see(page.getByText(`${name} is removed. ${files ? "Its folder is removed too." : "Its folder is kept."}`, { exact: true }));
  await websitesBecome(page, (read) => siteIn(read, name) === undefined, `remove ${name}`);
}

/** The plan of a site `name` that Add a website is opened for says what is in its folder. */
async function folderIs(t, page, name, { there }) {
  await planAfter(t, page, name, () => openAdd(t, page, `?name=${encodeURIComponent(name)}`));
  const row = planRow(page, "folder");
  if (there) {
    await eventually(async () => ensure(/^Keeps \S+ as it is/.test((await row.innerText()).trim()), `the folder's row says ${JSON.stringify(await row.innerText())}`), "the plan did not find the folder", 30);
    await t.see(page.locator(".hint").filter({ hasText: /^The folder is there(?: with \d+ files?)?: they are kept, and served as they are\.$/ }));
  } else {
    await eventually(async () => {
      const text = (await row.innerText()).trim();
      ensure(text.startsWith("Makes ") && !text.includes("if it is not there"), `the folder's row says ${JSON.stringify(text)}`);
    }, "the plan did not find the folder gone", 30);
    ensure((await page.locator(".hint").filter({ hasText: "The folder is there" }).count()) === 0, "the form says the folder is there");
  }
}

/** The list is the machine's: a link to each site, with what it serves, its HTTPS and its state. */
async function listAsAnswered(t, page, read) {
  const links = await page.locator('.web-list td[data-label="Site"] a').evaluateAll((all) => all.map((link) => link.getAttribute("href")));
  const wanted = read.websites.map((one) => hrefOf(one.name));
  ensure(JSON.stringify([...links].sort()) === JSON.stringify([...wanted].sort()), `the page links ${JSON.stringify(links)} and ${ROUTES.list} gives ${JSON.stringify(wanted)}`);
  for (const one of read.websites) {
    const row = rowOf(page, one.name);
    const https = (await row.locator('td[data-label="HTTPS"]').innerText()).trim();
    ensure(one.https === !https.startsWith("HTTP only"), `the row of ${one.name} says ${JSON.stringify(https)} of its HTTPS, and the machine says ${one.https}`);
    ensure((await row.locator('td[data-label="Serves"]').innerText()).startsWith(one.kind === "php" ? "PHP pages" : "Files"), `the row of ${one.name} does not say what it serves`);
  }
}

// --- The people and the phone ---

/** A reader is told who adds a site and has none of the buttons that change one, and is refused when a route is asked. */
async function reader(t, read) {
  const { page } = await signedIn(t, READER);
  await openList(t, page);
  await t.see(page.getByText(READER_WORDS, { exact: true }));
  ensure((await linkOf(page, "Add a website").count()) === 0 && (await buttonOf(page, "Check the sites").count()) === 0, "a reader's list has Add a website or Check the sites");
  if (read === null) {
    await t.see(page.locator(".web-empty").getByText(NONE_YET, { exact: true }));
    await t.see(page.locator(".web-empty").getByText("elevate website add NAME", { exact: false }));
  } else {
    const now = await readWebsites(page);
    await listAsAnswered(t, page, now);
  }
  await lookAll(t, page, read === null ? "websites-reader-empty" : "websites-reader");
  if (read !== null) {
    await openSite(t, page, read);
    await t.see(page.getByText(READER_WORDS, { exact: true }));
    for (const name of ["Remove…", "Check the sites", "Turn HTTPS off…", "Get a certificate", "Use one that is here…", "Use this machine's certificate"]) {
      ensure((await buttonOf(page, name).count()) === 0, `a reader's page of a site has ${name}`);
    }
    await t.see(buttonOf(rowFact(page, "file"), "Show the file"));
    await lookAll(t, page, "websites-reader-site");
  }
  await page.goto(new URL("/websites/add", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add a website", exact: true }));
  await t.see(page.getByText(READER_WORDS, { exact: true }));
  await t.see(page.getByText("elevate website add NAME", { exact: false }));
  ensure((await buttonOf(page, "Add the website").count()) === 0, "a reader's Add a website has Add the website");
  await lookAll(t, page, "websites-reader-add");
  const got = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(got.status === 200, `a reader's ${ROUTES.list} was answered ${got.status}: ${got.text}`);
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.check)), 403, "ADM-122", `a reader's ${ROUTES.check}`);
  mustBe(await fetchAsPage(page, "GET", `${pathOf(ROUTES.plan, NOBODY)}?kind=static&owner=${READER}`), 403, "ADM-122", `a reader's ${ROUTES.plan}`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.add), { name: NOBODY, kind: "static", owner: READER }), 403, "ADM-122", `a reader's ${ROUTES.add}`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.https, NOBODY), { off: true }), 403, "ADM-122", `a reader's ${ROUTES.https}`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.remove, NOBODY), {}), 403, "ADM-122", `a reader's ${ROUTES.remove}`);
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list as a card for each site, a site, and Add a website with its files, with controls a thumb reaches and no sideways scroll. */
async function phone(t, name) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const controls = 'main .web-page button, main .web-page a.btn, main .web-page .input, main .web-page .select, main .web-list td[data-label="Site"] a, main .web-names a, main .web-page .rows a, main .web-page label.check';
  const fits = async (what) => {
    ensure((await sidewaysScroll(page)) === 0, `the phone's ${what} scrolls ${await sidewaysScroll(page)} px sideways`);
    const small = await smallControls(page, controls);
    ensure(small.length === 0, `these controls of the phone's ${what} are under 44 px tall: ${small.join(", ")}`);
  };

  await openList(t, page);
  await t.see(page.locator(".web-list tbody tr").first());
  await fits("list");
  await lookAll(t, page, "websites-phone-list");

  await openSite(t, page, name);
  await t.see(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Websites", exact: true }));
  await fits("site");
  await lookAll(t, page, "websites-phone-site");

  await openAdd(t, page);
  await typeDomain(t, page, PHONE_SITE);
  await fits("Add a website");
  await lookAll(t, page, "websites-phone-add");
  // The files fill the screen, and the buttons in them are as tall.
  await buttonOf(page, "Preview the files first").click();
  const preview = dialogOf(page, "The files that adding the site writes");
  await t.see(preview);
  const box = await preview.evaluate((dialog) => {
    const { width, height } = dialog.getBoundingClientRect();
    return { width: Math.round(width), height: Math.round(height) };
  });
  ensure(box.width >= PHONE.width - 1 && box.height >= PHONE.height - 1, `the files fill ${box.width} by ${box.height} of the phone's ${PHONE.width} by ${PHONE.height}`);
  const smallInside = await smallControls(page, "dialog.web-preview button");
  ensure(smallInside.length === 0, `these controls of the phone's files are under 44 px tall: ${smallInside.join(", ")}`);
  await lookAll(t, page, "websites-phone-preview");
  await buttonOf(preview, "Close").click();
  await signOut(t, page);
  await page.context().close();
}

// --- The three states of the machine ---

/** A machine with no site: the sites are added, changed and removed. */
async function phaseA(t, lab) {
  await reader(t, null);
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const own = await machineSite(page);

  // The list of a machine with no site, nginx and PHP-FPM on.
  await openList(t, page);
  await t.see(page.locator(".web-empty").getByText(NONE_YET, { exact: true }));
  await t.see(linkOf(page.locator(".web-empty"), "Add a website"));
  ensure((await page.locator(".web-list .banner[data-line]").count()) === 0, "the list says that nginx or PHP-FPM is not on");
  await lookAll(t, page, "websites-empty");

  // The console is locked until the plan of the first site asks to unlock it.
  await addMachineSite(t, page, lab, own);
  await machineSitePage(t, page, own);
  await checkSites(t, page, true);
  await httpsOffAndOn(t, page, lab, own);
  await addPhpSite(t, page, lab);
  await getAndUse(t, page, lab);
  await whereItIsUsed(t, page);

  // A plain site: removed with its folder kept, which the next plan finds, added again, and removed with its folder.
  await addPlainSite(t, page, lab, PLAIN);
  await removeSite(t, page, PLAIN, { files: false });
  await folderIs(t, page, PLAIN, { there: true });
  await addPlainSite(t, page, lab, PLAIN);
  await removeSite(t, page, PLAIN, { files: true });
  await folderIs(t, page, PLAIN, { there: false });

  // What the list holds is the machine's: the static site and the PHP site, both over HTTPS.
  await openList(t, page);
  const read = await readWebsites(page);
  ensure(read.websites.length === 2 && siteIn(read, own)?.https === true && siteIn(read, PHP)?.https === true, `${ROUTES.list} lists ${JSON.stringify(read.websites.map((one) => [one.name, one.https]))}, not ${own} and ${PHP} over HTTPS`);
  await listAsAnswered(t, page, read);
  await lookAll(t, page, "websites-list");

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.list),
    routeFor(ROUTES.check),
    routeFor(ROUTES.plan, own),
    routeFor(ROUTES.plan, PHP),
    routeFor(ROUTES.add),
    routeFor(ROUTES.https, own),
    routeFor(ROUTES.https, PHP),
    routeFor(ROUTES.remove, PLAIN),
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** The first site's file has a line by hand: shown as changed, kept, and its HTTPS left alone. */
async function phaseB(t, lab) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const own = await machineSite(page);

  // The list marks it, and the site's page says so in words, with the text.
  await openList(t, page);
  await t.see(rowOf(page, own).locator(".pill").filter({ hasText: "Changed by hand" }));
  ensure((await rowOf(page, PHP).locator(".pill").count()) === 0, `${PHP} is marked as changed by hand`);
  await openSite(t, page, own);
  const file = rowFact(page, "file");
  ensure((await file.innerText()).includes(HAND_WORDS), `the page says ${JSON.stringify(await file.innerText())} of a file that was changed by hand`);
  await t.see(linkOf(file, "What changed"));
  await buttonOf(file, "Show the file").click();
  ensure((await file.locator("pre").innerText()).includes(MARK), `the text of the file does not show the line that was added by hand: ${await file.locator("pre").innerText()}`);
  const https = rowFact(page, "https");
  ensure((await https.innerText()).includes(HAND_LOCKED), `the page says ${JSON.stringify(await https.innerText())} where HTTPS cannot be changed`);
  for (const name of ["Turn HTTPS off…", "Get a certificate", "Use one that is here…", "Use this machine's certificate"]) {
    ensure((await buttonOf(page, name).count()) === 0, `a site whose file was changed by hand has ${name}`);
  }
  await lookAll(t, page, "websites-site-by-hand");
  const checked = await checkSites(t, page, true);
  ensure(checked.words === CHECK_PASSED, `the check of the sites says ${JSON.stringify(checked.words)}`);
  const ownFingerprint = await fingerprintOf(page, OWN);
  await visitBecomes(lab, "https", own, (seen) => answered(seen) && seen.fingerprint === ownFingerprint, `serve ${own} as its file says`);

  // Kept: another site is added and removed, and the file is as it was written.
  await addPlainSite(t, page, lab, KEPT);
  await openSite(t, page, own);
  await buttonOf(rowFact(page, "file"), "Show the file").click();
  ensure((await rowFact(page, "file").locator("pre").innerText()).includes(MARK), "the line that was added by hand is gone after another site was added");
  await removeSite(t, page, KEPT, { files: true });
  await openSite(t, page, own);
  ensure((await rowFact(page, "file").innerText()).includes(HAND_WORDS), "the file is not said to be changed by hand after another site was removed");
  mustHaveCalled(a.seen, [routeFor(ROUTES.list), routeFor(ROUTES.check), routeFor(ROUTES.plan, KEPT), routeFor(ROUTES.add), routeFor(ROUTES.remove, KEPT)]);
  await signOut(t, page);
  await page.context().close();

  await reader(t, own);
  await phone(t, own);
}

/** The first site's file has a line that nginx does not know: the check says so, and nothing is changed until it passes. */
async function phaseC(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const own = await machineSite(page);

  await openList(t, page);
  const checked = await checkSites(t, page, false);
  ensure(checked.line.includes(BROKEN), `nginx's own line is ${JSON.stringify(checked.line)}, which does not name ${BROKEN}`);
  ensure(checked.words.startsWith(CHECK_FAILED), `the check of the sites says ${JSON.stringify(checked.words)}`);
  await lookAll(t, page, "websites-check-failed");

  // Removing the PHP site is refused by the command, which checks nginx's settings before it changes anything; the site is still there.
  await openSite(t, page, PHP);
  await buttonOf(page, "Remove…").click();
  await answerIfAsked(t, page);
  const dialog = dialogOf(page, `Remove ${PHP}?`);
  await t.see(dialog);
  await dialog.getByLabel(`Type ${PHP} to go on`).fill(PHP);
  await pressFor(t, page, buttonOf(dialog, "Remove the website"), pathOf(ROUTES.remove, PHP), "POST", { any: true });
  const steps = await stepsEnd(page, ".web-flow", 300);
  ensure(statesOf(steps) === "failed", `the steps of removing ${PHP} are ${JSON.stringify(steps)}, with nginx's settings broken`);
  await t.see(page.locator(".web-flow .flow-stands").filter({ hasText: `${PHP} is still there: nothing was changed.` }));
  ensure((await page.locator(".web-flow .flow-problem").innerText()).includes("WEB-103"), `the failed step does not give the error WEB-103: ${await page.locator(".web-flow .flow-problem").innerText()}`);
  await t.see(page.locator('.web-flow button[data-action="try-again"]'));
  await lookAll(t, page, "websites-remove-refused");
  ensure(siteIn(await readWebsites(page), PHP) !== undefined, `${PHP} is gone though its removal was refused`);

  // Adding a site is stopped before it is asked for: the plan has the check, and the button says why.
  await openAdd(t, page);
  await typeDomain(t, page, BROKEN_SITE);
  await t.see(page.locator('#plan [data-check="WEB-103"]'));
  ensure((await buttonOf(page, "Add the website").getAttribute("aria-disabled")) === "true", "Add the website is on with nginx's settings broken");
  await t.see(page.getByText(BLOCKED, { exact: true }));
  await lookAll(t, page, "websites-add-blocked");
  ensure(siteIn(await readWebsites(page), own) !== undefined, `${own} is gone`);

  mustHaveCalled(a.seen, [routeFor(ROUTES.check), routeFor(ROUTES.remove, PHP), routeFor(ROUTES.plan, BROKEN_SITE)]);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  const lab = await labOf();
  if (lab.phase === "a") await phaseA(t, lab);
  else if (lab.phase === "b") await phaseB(t, lab);
  else if (lab.phase === "c") await phaseC(t, lab);
  else ensure(false, `${MACHINE_FILE} says the machine is in the state ${JSON.stringify(lab.phase)}, which is not a, b or c`);
}
