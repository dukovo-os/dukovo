// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/add-app drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the page that
// adds an app at /apps/add and the sheet of one app at /apps/add/{id}, with an
// app's ID for {id} and each dot of it written %2E. The search shows what
// `bundle search --json` says, which `GET /api/1/apps/catalog/search` runs:
// the apps the catalogs list, sorted by name, each a link to its sheet, with
// the version installed when there is one, and a banner for each catalog that
// could not be read. The setups are the page's four cards, each with what it
// uses and what it installs first. A sheet is from what `bundle sheet ID
// --json` says, which `GET /api/1/apps/catalog/{id}/sheet` runs: who made it,
// its licence, its size, its flaws, its services, what it may use, the commands
// that run with a person's access, its data and what it works with, with
// Install for everyone (`bundle install ID --for-everyone --yes --sha256 H`,
// `POST /api/1/apps/catalog/{id}/install`, a task), Show command and Cancel.
//
// The apps are nginx, which has a service and a data folder, and Python, which
// has neither, from the catalog `labtest` that add-app.toml adds, nothing
// installed to begin with. Install for everyone installs nginx as a task,
// pinned by the sheet's SHA-256, and what stands after it is held to the
// machine: the apps installed, the service Spark lists, its data folder and
// the next step's link, Add a website. Turn it on enables nginx's service
// (`POST /api/1/services/{name}/enable`), which add-app.toml sees running. The
// install of Python has its pin changed on the way, which is what the machine
// sees when the catalog offers another file than the sheet that was read: it
// refuses (APP-102), the page says so and offers Read it again, which reads the
// sheet anew, and the install that follows is the right one. An app that no
// catalog lists says why and offers Try again. A reader sees the search and the
// sheets, is told who installs apps, has no Install, and is refused when the
// route is asked. A phone at 390 by 844 shows the list, and a sheet alone with
// controls that are 44 px tall and no sideways scroll. Each look is the
// console's own: dark, light, high contrast, forced colours and less motion,
// each checked by axe (console.mjs's lookAll).
//
// The script runs once for each engine, add-app.toml having taken both apps
// away before it. The accounts are the two that add-app.toml made, so no script
// needs the lab's password.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
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

const ADMIN = "wc-add";
const READER = "wc-add-reader";

// The apps of the test catalog that this test installs, the apps the setups
// use, and an ID that no catalog has.
const NGINX = "org.nginx.nginx";
const PYTHON = "org.python.python";
const PHP = "net.php.php";
const POSTGRESQL = "org.postgresql.postgresql";
const NOBODY = "org.example.nothing";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with an
// app's ID for {id} and a service's name for {name}.
const ROUTES = {
  list: "GET /api/1/apps",
  show: "GET /api/1/apps/{id}",
  search: "GET /api/1/apps/catalog/search",
  sheet: "GET /api/1/apps/catalog/{id}/sheet",
  install: "POST /api/1/apps/catalog/{id}/install",
  services: "GET /api/1/services",
  enable: "POST /api/1/services/{name}/enable",
  backups: "GET /api/1/backups",
};

/** The path of a route as the table names it, for the app `id` or the service `name`: `GET /api/1/apps/catalog/{id}/sheet` is `/api/1/apps/catalog/org.nginx.nginx/sheet`. */
const pathOf = (route, name = "") => route.slice(route.indexOf(" ") + 1).replace("{id}", encodeURIComponent(name)).replace("{name}", encodeURIComponent(name));

/** The route as `track` writes it, for `name`. */
const routeFor = (route, name = "") => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, name)}`;

/** The address of an app's page, of its sheet and of a service's page: a dot is written %2E. */
const written = (id) => encodeURIComponent(id).replaceAll(".", "%2E");
const hrefOf = (id) => `/apps/${written(id)}`;
const sheetHrefOf = (id) => `/apps/add/${written(id)}`;
const serviceHrefOf = (name) => `/services/${written(name)}`;

// The words the page says where this test looks for them.
const READER_WORDS = "An administrator installs apps for everyone.";
const STALE_WORDS = "The catalog has a newer version than the sheet you read. Read it again.";
const EVERYONE_WORDS = "A bundle that runs services is installed for everyone.";
const YOUR_ACCESS_WORDS = "These run with the access of whoever runs them; the first time each asks, with a warning, in that person's terminal.";
const SANDBOX_WORDS = "Its commands run in its sandbox.";
const NO_FLAWS = "None known.";
const CATALOG_SILENT = "The catalog says nothing of this version's flaws.";
const COVERED = "Snapshots and backups copy /shared.";
const NOT_COVERED = "No backup place copies /shared yet.";

// The sections of a sheet, in the order the page draws them.
const SECTIONS = ["Who made it", "Its size", "Flaws", "Runs as services", "May use", "Commands that run with your access", "Its data", "Works with"];

// The setups' cards: the title, the apps it uses (null for what is no app) and where it leads.
const SETUPS = [
  { title: "Static website", uses: [[NGINX, "nginx"]], href: "/websites/add?kind=static" },
  { title: "PHP website with a database", uses: [[NGINX, "nginx"], [PHP, "PHP-FPM"], [POSTGRESQL, "PostgreSQL"]], href: "/websites/add?kind=php&database=1" },
  { title: "A database", uses: [[POSTGRESQL, "PostgreSQL"]], href: "/databases/add" },
  { title: "Private access for your devices", uses: [[null, "WireGuard"]], href: "/security#private" },
];

/** Whether `value` is text with something in it, as the page counts it. */
const isText = (value) => typeof value === "string" && value.trim() !== "";

/** `value`'s first letter in capitals, as a catalog's name is written on a sheet. */
const capital = (value) => value.charAt(0).toUpperCase() + value.slice(1);

/** "nginx", "nginx and PostgreSQL", "nginx, PHP-FPM and PostgreSQL". */
const listWords = (names) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`);

// --- Reading the machine ---

/** What the machine says to `route` for `name`: `{status, json, text}`. */
const ask = (page, route, name = "", query = "") => fetchAsPage(page, route.slice(0, route.indexOf(" ")), `${pathOf(route, name)}${query}`);

/** What `GET /api/1/apps` answers: `bundle list --json`; gives the bundles. */
async function readApps(page) {
  const answer = await ask(page, ROUTES.list);
  ensure(answer.status === 200 && Array.isArray(answer.json?.bundles), `${ROUTES.list} was answered ${answer.status}: ${answer.text}`);
  return answer.json.bundles;
}

/** What `GET /api/1/apps/{id}` answers: `bundle info ID --json` of an app that is installed. */
async function readApp(page, id) {
  const answer = await ask(page, ROUTES.show, id);
  ensure(answer.status === 200 && answer.json?.id === id, `${ROUTES.show} for ${id} was answered ${answer.status}: ${answer.text}`);
  ensure(Array.isArray(answer.json.service_list), `${ROUTES.show} for ${id} gives no list service_list: ${answer.text}`);
  return answer.json;
}

/** The words as the search sends them: lowercase, split at spaces. */
const wordsOf = (text) => text.toLowerCase().split(/\s+/).filter(Boolean);

/** What `GET /api/1/apps/catalog/search` answers for `words`: `bundle search --json`, `{bundles, unread}`. */
async function readSearch(page, words = []) {
  const answer = await ask(page, ROUTES.search, "", words.length > 0 ? `?words=${encodeURIComponent(words.join(" "))}` : "");
  ensure(answer.status === 200 && Array.isArray(answer.json?.bundles) && Array.isArray(answer.json?.unread), `${ROUTES.search} for ${JSON.stringify(words)} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** What `GET /api/1/apps/catalog/{id}/sheet` answers: `bundle sheet ID --json` of an app a catalog lists. */
async function readSheet(page, id) {
  const answer = await ask(page, ROUTES.sheet, id);
  ensure(answer.status === 200 && answer.json?.id === id, `${ROUTES.sheet} for ${id} was answered ${answer.status}: ${answer.text}`);
  const sheet = answer.json;
  ensure(/^[0-9a-f]{64}$/.test(sheet.sha256 ?? ""), `${ROUTES.sheet} for ${id} gives no sha256: ${answer.text}`);
  for (const member of ["permissions", "commands_with_your_access", "service_list"]) {
    ensure(Array.isArray(sheet[member]), `${ROUTES.sheet} for ${id} gives no list ${member}: ${answer.text}`);
  }
  return sheet;
}

/** Spark's services as `GET /api/1/services` lists them. */
async function readServices(page) {
  const answer = await ask(page, ROUTES.services);
  ensure(answer.status === 200 && Array.isArray(answer.json?.services), `${ROUTES.services} was answered ${answer.status}: ${answer.text}`);
  return answer.json.services;
}

/** Reads the apps until `test(bundles)` holds, for up to `seconds`, and gives them. */
async function appsBecome(page, test, what, seconds = 180) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const bundles = await readApps(page).catch(() => null);
    if (bundles !== null && test(bundles)) return bundles;
    ensure(Date.now() < until, `the machine did not ${what} within ${seconds} seconds`);
    await sleep(1000);
  }
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The page ---

const panel = (page) => page.locator("section.add-panel");
const searchField = (page) => page.getByLabel("Search apps and setups", { exact: true });
const installButton = (page) => panel(page).getByRole("button", { name: "Install for everyone", exact: true });
const rowLink = (page, id) => page.locator(`.add-list a.add-who[href^="${sheetHrefOf(id)}"]`);

/** Waits for `locator` to show, for up to `seconds`: the sheet is read by downloading the app, and an install takes the time of its download. */
const soon = (locator, seconds = 120) => locator.waitFor({ state: "visible", timeout: seconds * 1000 });

/** Waits until the table of the catalogs is drawn from an answer, not from the wait for one. */
const settled = (page, seconds = 120) =>
  page.waitForFunction(
    () => {
      const box = document.querySelector('.add-list section[aria-labelledby="add-cat-h"]');
      return box !== null && box.getAttribute("aria-busy") !== "true" && !box.textContent.includes("Reading the catalogs");
    },
    null,
    { timeout: seconds * 1000 },
  );

/** Opens the page that adds an app, with `query` in its address, and waits for its table. */
async function openAdd(t, page, query = "") {
  await page.goto(new URL(`/apps/add${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Add an app", exact: true }));
  await settled(page);
}

/** What the table says when it has no row, for the answer and the words typed. */
function emptyWords(answer, words) {
  if (answer.bundles.length > 0) return null;
  const unread = answer.unread.length > 0;
  if (words.length > 0) return `No app in the catalogs ${unread ? "that could be read " : ""}matches “${words.join(" ")}”.`;
  return unread ? "No app is listed: no catalog could be read." : "No catalog is turned on.";
}

/** Whether `a` comes before `b`: by name, as people read it, then by ID. */
const byName = (a, b) => {
  const [one, other] = [(isText(a.name) ? a.name : a.id).toLowerCase(), (isText(b.name) ? b.name : b.id).toLowerCase()];
  if (one !== other) return one < other ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
};

/** The words of the Flaws cell of a bundle's `flaws`, as the page says them. */
function flawsCell(flaws) {
  if (flaws?.state === "known" && flaws.count > 0) return /^\d+ known flaws?(, worst \w+)?$/;
  if (flaws?.state === "none" || flaws?.state === "known") return /^No known flaws$/;
  return /^Flaws not listed$/;
}

/** The head: the page's name, the two views of the apps, and how many apps are installed. */
async function headAsAnswered(t, page, apps) {
  const add = page.locator('.seg a[href="/apps/add"]');
  await t.see(add);
  ensure((await add.getAttribute("aria-current")) === "page", "Add an app is not the view that is current");
  const installed = page.locator('.seg a[href="/apps"]');
  await t.see(installed);
  await page.waitForFunction((words) => document.querySelector('.seg a[href="/apps"]')?.textContent.trim() === words, `Installed (${apps.length})`, { timeout: 15_000 }).catch(async () => {
    ensure(false, `the head says ${JSON.stringify(await installed.innerText())} and ${ROUTES.list} gives ${apps.length} apps`);
  });
}

/** The setups' cards are the page's four, those that the words typed match, each with what it uses and what it installs first. */
async function setupsAsAnswered(t, page, apps, typed) {
  const have = new Set(apps.map((app) => app.id));
  const words = wordsOf(typed);
  const wanted = SETUPS.filter((setup) => {
    const hay = [setup.title, ...setup.uses.map(([, name]) => name)].join(" ").toLowerCase();
    return words.every((word) => hay.includes(word));
  });
  const shown = await page.locator(".setups li").evaluateAll((items) =>
    items.map((item) => ({
      title: item.querySelector("b").textContent,
      href: item.querySelector("a").getAttribute("href"),
      uses: [...item.querySelectorAll(".chip")].map((chip) => chip.textContent),
      says: item.querySelector(".warn")?.textContent ?? null,
    })),
  );
  ensure(JSON.stringify(shown.map((card) => card.title)) === JSON.stringify(wanted.map((setup) => setup.title)), `the setups are ${JSON.stringify(shown.map((card) => card.title))}, not ${JSON.stringify(wanted.map((setup) => setup.title))} for ${JSON.stringify(typed)}`);
  wanted.forEach((setup, at) => {
    ensure(shown[at].href === setup.href, `${setup.title} leads to ${shown[at].href}, not ${setup.href}`);
    ensure(JSON.stringify(shown[at].uses) === JSON.stringify(setup.uses.map(([, name]) => name)), `${setup.title} says it uses ${JSON.stringify(shown[at].uses)}`);
    const missing = setup.uses.filter(([id]) => id !== null && !have.has(id)).map(([, name]) => name);
    const says = missing.length > 0 ? `Installs ${listWords(missing)} first` : null;
    ensure(shown[at].says === says, `${setup.title} says ${JSON.stringify(shown[at].says)}, and the machine's apps make it ${JSON.stringify(says)}`);
  });
  if (wanted.length > 0) await t.see(page.getByRole("heading", { level: 2, name: "Ready-made setups", exact: true }));
}

/** The page's rows are what the search answers: each app a link to its sheet, with its name, ID, version, catalog, flaws and whether it is installed; the banners are the catalogs that were not read. */
async function tableAsAnswered(t, page, answer, apps, typed = "") {
  const here = new Map(apps.map((app) => [app.id, app.version]));
  const wanted = [...answer.bundles].filter((one) => isText(one?.id)).sort(byName);
  const rows = await page.locator(".add-list tbody tr").evaluateAll((trs) =>
    trs.map((tr) => ({
      href: tr.querySelector("a.add-who")?.getAttribute("href") ?? null,
      name: tr.querySelector("a.add-who b")?.textContent ?? null,
      id: tr.querySelector("a.add-who small")?.textContent ?? null,
      version: tr.querySelector('td[data-label="Version"]')?.textContent ?? null,
      catalog: tr.querySelector('td[data-label="Catalog"]')?.textContent ?? null,
      flaws: tr.querySelector('td[data-label="Flaws"]')?.textContent ?? null,
      installed: tr.querySelector('td[data-label="Installed"]')?.textContent ?? null,
    })),
  );
  ensure(JSON.stringify(rows.map((row) => row.id)) === JSON.stringify(wanted.map((one) => one.id)), `the page lists ${JSON.stringify(rows.map((row) => row.id))} and ${ROUTES.search} gives ${JSON.stringify(wanted.map((one) => one.id))} for ${JSON.stringify(typed)}`);
  wanted.forEach((one, at) => {
    const row = rows[at];
    const kept = typed === "" ? "" : `?q=${encodeURIComponent(typed)}`;
    ensure(row.href === `${sheetHrefOf(one.id)}${kept}`, `the row of ${one.id} leads to ${row.href}`);
    ensure(row.name === (isText(one.name) ? one.name : one.id), `the row of ${one.id} says ${row.name}`);
    ensure(row.version === String(one.version), `the row of ${one.id} says ${row.version}, not ${one.version}`);
    ensure(row.catalog === (one.catalog ?? ""), `the row of ${one.id} says the catalog ${row.catalog}, not ${one.catalog}`);
    ensure(flawsCell(one.flaws).test(row.flaws), `the row of ${one.id} says ${JSON.stringify(row.flaws)} of its flaws, and ${ROUTES.search} says ${JSON.stringify(one.flaws)}`);
    const version = isText(one.installed) ? one.installed : here.get(one.id) ?? null;
    if (version === null) ensure(row.installed === "Not installed", `the row of ${one.id} says ${JSON.stringify(row.installed)} of whether it is installed, and the machine has no ${one.id}`);
    else ensure(row.installed === `InstalledVersion ${version}`, `the row of ${one.id} says ${JSON.stringify(row.installed)}, and the machine has version ${version}`);
  });
  const banners = await page.locator(".add-list .banner.warn").evaluateAll((boxes) => boxes.map((box) => box.querySelector("p").textContent));
  ensure(banners.length === answer.unread.length, `the page has ${banners.length} banners and ${ROUTES.search} gives ${answer.unread.length} catalogs that were not read`);
  answer.unread.forEach((one, at) => {
    ensure(banners[at].startsWith(`The catalog ${one.catalog} could not be read: `), `the banner ${JSON.stringify(banners[at])} does not say that ${one.catalog} could not be read`);
    ensure(banners[at].endsWith("The list shows the others.") === (wanted.length > 0), `the banner ${JSON.stringify(banners[at])} is wrong about the others`);
  });
  const empty = emptyWords(answer, wordsOf(typed));
  if (empty === null) {
    ensure((await page.locator(".add-list .add-empty").count()) === 0, "the page says that no app is listed and lists some");
    ensure(/^\d+ apps? in the catalogs$/.test(await page.locator(".add-list table caption").textContent()), "the table has no caption that counts the apps");
  } else {
    ensure((await page.locator(".add-list .add-empty p").innerText()) === empty, `the page says ${JSON.stringify(await page.locator(".add-list .add-empty p").innerText())} of an empty table, not ${JSON.stringify(empty)}`);
  }
  await setupsAsAnswered(t, page, apps, typed);
}

/** Types `text` in the search and waits for the search that follows, 300 ms after the typing stops. Gives what the machine answers for the same words. */
async function typeSearch(t, page, text) {
  const words = wordsOf(text);
  const asked = page.waitForRequest(
    (request) => request.method() === "GET" && new URL(request.url()).pathname === pathOf(ROUTES.search) && new URL(request.url()).searchParams.get("words") === words.join(" "),
    { timeout: 20_000 },
  );
  await searchField(page).fill(text);
  await asked;
  await settled(page);
  return readSearch(page, words);
}

// --- A sheet ---

/** The section of the sheet that is headed `name`. */
const sectionOf = (page, name) => panel(page).locator("section.add-sec").filter({ has: page.getByRole("heading", { level: 3, name, exact: true }) });

/** The lines of a section, one for each of its list's items. */
const linesOf = (page, name) => sectionOf(page, name).locator("ul.add-lines > li").allInnerTexts();

/** Opens the sheet of `id` from its row, and waits for its heading. Gives the sheet as the machine says it. */
async function openSheet(t, page, id) {
  const sheet = await readSheet(page, id);
  await rowLink(page, id).click();
  await page.waitForURL((url) => url.pathname === sheetHrefOf(id));
  await soon(panel(page).getByRole("heading", { level: 2, name: `${sheet.name} ${sheet.version}`, exact: true }));
  await t.see(page.getByRole("heading", { level: 1, name: "Add an app", exact: true }));
  return sheet;
}

/** The sheet the page draws is the one the machine reads: its line, its sections and their lines. */
async function sheetAsAnswered(t, page, sheet) {
  const where = `the sheet of ${sheet.id}`;
  const title = panel(page).locator(".add-title");
  const line = await title.locator("span").last().innerText();
  if (isText(sheet.license)) ensure(line.includes(sheet.license), `${where} does not say the licence ${sheet.license}: ${line}`);
  ensure(line.includes("signed"), `${where} does not say that it is signed: ${line}`);
  if (isText(sheet.catalog) && sheet.trust?.kind === "catalog") ensure(line.startsWith(`${capital(sheet.catalog)} catalog`), `${where} does not say the catalog ${sheet.catalog}: ${line}`);
  if (isText(sheet.summary)) await t.see(panel(page).getByText(sheet.summary, { exact: true }));

  // The sections, in the order of the plain sheet.
  const shown = await panel(page).locator("section.add-sec > h3").allInnerTexts();
  ensure(JSON.stringify(shown) === JSON.stringify(SECTIONS.filter((name) => shown.includes(name))), `${where} has the sections ${JSON.stringify(shown)}, which are not in the order of ${JSON.stringify(SECTIONS)}`);
  for (const name of ["May use", "Commands that run with your access"]) ensure(shown.includes(name), `${where} has no section ${name}`);
  const wants = [
    ["Who made it", isText(sheet.publisher) || isText(sheet.support)],
    ["Its size", Number.isFinite(sheet.bytes) || Number.isFinite(sheet.download_bytes)],
    ["Flaws", sheet.flaws !== null && typeof sheet.flaws === "object"],
    ["Runs as services", sheet.service_list.length > 0 || sheet.services_need_everyone === true],
  ];
  if (Array.isArray(sheet.data)) wants.push(["Its data", sheet.data.filter(isText).length > 0]);
  for (const [name, there] of wants) ensure(shown.includes(name) === there, `${where} ${shown.includes(name) ? "has" : "has no"} section ${name}, and ${ROUTES.sheet} ${there ? "gives what it says" : "gives nothing for it"}`);

  // Flaws: none known, or the catalog's silence, or a line for each.
  if (shown.includes("Flaws")) {
    const known = sheet.flaws.known ?? [];
    const text = await sectionOf(page, "Flaws").innerText();
    if (known.length > 0) for (const flaw of known) ensure(text.includes(flaw.id), `${where} does not list the flaw ${flaw.id}`);
    else ensure(text.includes(sheet.flaws.state === "unknown" ? CATALOG_SILENT : NO_FLAWS), `${where} says ${JSON.stringify(text)} of its flaws, and ${ROUTES.sheet} says ${JSON.stringify(sheet.flaws)}`);
  }

  // Services: a line for each, and that a bundle that runs services is installed for everyone.
  if (sheet.service_list.length > 0) {
    const lines = await linesOf(page, "Runs as services");
    ensure(lines.length === sheet.service_list.length, `${where} has ${lines.length} service lines for ${sheet.service_list.length} services`);
    sheet.service_list.forEach((service, at) => {
      const account = isText(service.account) ? service.account : service.name;
      ensure(lines[at] === `${service.name} · as its own account, ${account} · ${service.start === "on" ? "starts at boot" : "off until you turn it on"}`, `${where} says ${JSON.stringify(lines[at])} of its service ${service.name}`);
    });
  }
  if (sheet.services_need_everyone === true) await t.see(sectionOf(page, "Runs as services").getByText(EVERYONE_WORDS, { exact: true }));

  // May use: a line for each permission, a warning one with the word first, then what is left out.
  const wordsOfPermission = (permission) => (isText(permission?.lets) ? permission.lets : isText(permission?.name) ? permission.name : null);
  const may = sheet.permissions.filter((permission) => wordsOfPermission(permission) !== null);
  const lines = (await linesOf(page, "May use")).map((each) => each.trim());
  ensure(lines.length === may.length + 1 && /^(and )?nothing else$/.test(lines.at(-1)), `${where} lists what it may use as ${JSON.stringify(lines)}`);
  may.forEach((permission, at) => {
    const warns = /warning/i.test(String(permission.tier ?? ""));
    ensure(lines[at].startsWith(warns ? `Warning: ${wordsOfPermission(permission)}` : wordsOfPermission(permission)), `${where} says ${JSON.stringify(lines[at])} of ${wordsOfPermission(permission)}${warns ? ", which asks with a warning" : ""}`);
  });

  // The commands that run with a person's access, or that they run in the sandbox.
  const commands = sheet.commands_with_your_access.filter(isText);
  const commandText = await sectionOf(page, "Commands that run with your access").innerText();
  ensure(commandText.includes(commands.length > 0 ? YOUR_ACCESS_WORDS : SANDBOX_WORDS), `${where} says ${JSON.stringify(commandText)} of its commands`);
  for (const command of commands) ensure(commandText.includes(command), `${where} does not name the command ${command}`);

  // Its data: the folders the sheet gives, and whether a backup place copies /shared, as the backups say.
  if (shown.includes("Its data")) {
    const folders = await sectionOf(page, "Its data").locator("code").allInnerTexts();
    ensure(JSON.stringify(folders) === JSON.stringify(sheet.data.filter(isText)), `${where} says the folders ${JSON.stringify(folders)} of its data, and ${ROUTES.sheet} gives ${JSON.stringify(sheet.data)}`);
    const backups = await ask(page, ROUTES.backups);
    if (backups.status === 200) {
      const covered = (backups.json?.places ?? []).some((place) => (place?.places ?? []).includes("/shared"));
      const text = await sectionOf(page, "Its data").innerText();
      ensure(text.includes(covered ? COVERED : NOT_COVERED), `${where} says ${JSON.stringify(text)} of backups, and ${ROUTES.backups} says ${covered ? "/shared is" : "/shared is not"} copied`);
    }
  }
}

/** The buttons of a sheet for whoever looks, and what Show command shows. */
async function buttonsAsAnswered(t, page, sheet, { admin }) {
  const installable = admin && sheet.refusal === null && sheet.installed === null;
  ensure((await installButton(page).count()) === (installable ? 1 : 0), `${sheet.id}'s sheet has ${await installButton(page).count()} Install for everyone, for ${admin ? "an administrator" : "a reader"} and a refusal of ${JSON.stringify(sheet.refusal)}`);
  if (!admin) await t.see(panel(page).getByText(READER_WORDS, { exact: true }));
  else ensure((await panel(page).getByText(READER_WORDS).count()) === 0, "an administrator is told who installs apps");
  const cancel = panel(page).getByRole("link", { name: "Cancel", exact: true });
  await t.see(cancel);
  ensure((await cancel.getAttribute("href")) === "/apps/add" || (await cancel.getAttribute("href")).startsWith("/apps/add?q="), "Cancel does not lead back to the catalogs");

  // Show command: the command that does the same at a terminal, hidden until it is asked for.
  const command = `elevate bundle install ${sheet.id} --for-everyone`;
  const show = panel(page).getByRole("button", { name: "Show command", exact: true });
  ensure((await show.getAttribute("aria-expanded")) === "false", "Show command says it is open before it is pressed");
  ensure(!(await panel(page).locator(".add-command").isVisible()), "the command is shown before Show command is pressed");
  await show.click();
  await t.see(panel(page).locator(".add-command"));
  ensure((await panel(page).locator(".add-command pre").innerText()) === command, `Show command says ${JSON.stringify(await panel(page).locator(".add-command pre").innerText())}, not ${command}`);
  ensure((await show.getAttribute("aria-expanded")) === "true", "Show command does not say it is open");
  await show.click();
  await t.gone(panel(page).locator(".add-command"));
}

// --- Install ---

/** Presses Install for everyone, answers the unlock if the console asks for it, and gives the POST and its answer. */
async function pressInstall(t, page, id) {
  const path = pathOf(ROUTES.install, id);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === path, { timeout: 60_000 });
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path, { timeout: 120_000 });
  await installButton(page).click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  return { request: await sent, answer: await answered };
}

/** The install ended well: the machine has the app at the sheet's version, and the page says what happened and what comes next. */
async function installIsDone(t, page, sheet, typed = "") {
  const done = page.locator(`ul.add-done[data-installed="${sheet.id}"]`);
  await soon(done, 600);
  const info = await readApp(page, sheet.id);
  ensure(info.version === sheet.version, `${sheet.id} is installed at ${info.version}, and the sheet was of ${sheet.version}`);
  const apps = await readApps(page);
  ensure(apps.some((app) => app.id === sheet.id && app.for_everyone === true), `${ROUTES.list} does not list ${sheet.id} for everyone`);
  await t.see(panel(page).getByRole("heading", { level: 2, name: `${info.name} ${info.version}`, exact: true }));
  ensure((await page.evaluate(() => document.activeElement?.id)) === "add-h", "the keyboard is not on the heading after the install");
  const lines = await done.locator(":scope > li").allInnerTexts();
  ensure(lines[0].trim() === `Installed ${info.name} ${info.version} for everyone.`, `the page says ${JSON.stringify(lines[0])} of what was installed`);
  const services = info.service_list;
  ensure(lines.length === 1 + services.length + (services.length > 0 ? 1 : 0), `the page has ${lines.length} lines of what happened, for ${services.length} services`);
  if (services.length > 0) {
    const data = lines.at(-1).trim();
    ensure(data.startsWith(`Its data is in /shared/services/${sheet.id}/`) && data.endsWith(" once it has started."), `the page says ${JSON.stringify(data)} of the data`);
  }
  const listed = services.length > 0 ? await readServices(page) : [];
  for (const [at, service] of services.entries()) {
    const entry = listed.find((one) => one.name === service.name);
    const on = entry ? entry.enabled === true : service.start === "on";
    const text = lines[1 + at].trim();
    ensure(text.startsWith(on ? `Its service ${service.name} is on` : `Its service ${service.name} is off.`), `the page says ${JSON.stringify(text)} of the service ${service.name}, which Spark lists as ${JSON.stringify(entry)}`);
    ensure((await done.getByRole("button", { name: "Turn it on", exact: true }).count()) === (on ? 0 : 1), `the line of ${service.name} has the wrong number of Turn it on`);
    const link = done.locator(`a[href="${serviceHrefOf(service.name)}"]`);
    await t.see(link);
    ensure((await link.innerText()) === "Open its service", `the link to ${service.name} says ${await link.innerText()}`);
  }
  const next = panel(page).locator('a[data-action="next"]');
  const [words, href] = sheet.id === NGINX ? ["Add a website", "/websites/add"] : ["Open its page", hrefOf(sheet.id)];
  ensure((await next.innerText()) === words && (await next.getAttribute("href")) === href, `the next step says ${JSON.stringify(await next.innerText())} and leads to ${await next.getAttribute("href")}, not ${words} and ${href}`);
  const back = panel(page).locator('a[data-action="close"]');
  ensure((await back.innerText()) === "Back to the catalogs" && (await back.getAttribute("href")) === `/apps/add${typed === "" ? "" : `?q=${encodeURIComponent(typed)}`}`, "Back to the catalogs does not lead back, with the search kept");
  await headAsAnswered(t, page, apps);
  return info;
}

/** Turn it on enables the service, which starts it: the page says so, and Spark lists it enabled. */
async function turnItOn(t, page, name) {
  const button = page.locator(`button[data-action="turn-on-${name}"]`);
  await t.see(button);
  const path = pathOf(ROUTES.enable, name);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path, { timeout: 120_000 });
  await button.click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  const reply = await answered;
  ensure(reply.status() === 200 || reply.status() === 202, `${ROUTES.enable} for ${name} was answered ${reply.status()}: ${await reply.text()}`);
  await soon(page.getByText(new RegExp(`^Its service ${name} is on`)).first(), 120);
  await t.gone(page.locator(`button[data-action="turn-on-${name}"]`));
  const until = Date.now() + 60_000;
  for (;;) {
    const entry = (await readServices(page)).find((one) => one.name === name);
    if (entry?.enabled === true) break;
    ensure(Date.now() < until, `Spark does not list ${name} as enabled a minute after Turn it on`);
    await sleep(1000);
  }
}

/** An install whose catalog moved on: the pin is changed on the way, the machine refuses (APP-102), the page says so and offers Read it again, which reads the sheet anew. */
async function catalogMovedOn(t, page, sheet) {
  const pattern = `**${pathOf(ROUTES.install, sheet.id)}`;
  const other = `${sheet.sha256[0] === "0" ? "1" : "0"}${sheet.sha256.slice(1)}`;
  const changePin = async (route) => {
    const body = route.request().postDataJSON() ?? {};
    await route.continue({ postData: JSON.stringify({ ...body, sha256: other }) });
  };
  await page.route(pattern, changePin);
  await pressInstall(t, page, sheet.id);
  await soon(panel(page).getByText(STALE_WORDS, { exact: true }), 240);
  await page.unroute(pattern, changePin);
  await t.see(panel(page).getByText("Nothing was installed.", { exact: true }));
  await t.see(panel(page).getByRole("button", { name: "Try again", exact: true }));
  const reread = panel(page).getByRole("button", { name: "Read it again", exact: true });
  await t.see(reread);
  await appsNotThere(page, sheet.id);
  await look(t, page, "add-app-stale");

  // Read it again: the sheet is read anew, and its Install is the right one.
  const asked = page.waitForRequest((request) => request.method() === "GET" && new URL(request.url()).pathname === pathOf(ROUTES.sheet, sheet.id));
  await reread.click();
  await asked;
  await soon(installButton(page));
  await t.gone(panel(page).getByText(STALE_WORDS, { exact: true }));
}

/** The machine has no app `id` installed: `bundle info` refuses, as the Apps page's own test finds. */
async function appsNotThere(page, id) {
  mustBe(await ask(page, ROUTES.show, id), 422, "ADM-123", `${ROUTES.show} for ${id}`);
}

/** An app that no catalog lists: the page says why, that nothing was kept, and offers Try again, which asks again. */
async function noCatalogLists(t, page) {
  const refused = page.waitForResponse((reply) => reply.request().method() === "GET" && new URL(reply.url()).pathname === pathOf(ROUTES.sheet, NOBODY));
  await page.goto(new URL(sheetHrefOf(NOBODY), page.url()).href);
  ensure((await refused).status() === 422, `${ROUTES.sheet} for ${NOBODY} was answered ${(await refused).status()}`);
  await soon(panel(page).getByText(/^The app could not be fetched: /));
  await t.see(panel(page).getByText("Nothing was installed or kept.", { exact: true }));
  const help = panel(page).getByRole("link", { name: "What APP-101 means", exact: true });
  await t.see(help);
  ensure((await help.getAttribute("href")).includes("APP-101"), "the link to what APP-101 means does not lead to its page");
  ensure((await installButton(page).count()) === 0, "a sheet that could not be read has Install for everyone");
  mustBe(await ask(page, ROUTES.sheet, NOBODY), 422, "APP-101", `${ROUTES.sheet} for ${NOBODY}`);
  await look(t, page, "add-app-missing");
  const again = page.waitForRequest((request) => request.method() === "GET" && new URL(request.url()).pathname === pathOf(ROUTES.sheet, NOBODY));
  await panel(page).getByRole("button", { name: "Try again", exact: true }).click();
  await again;
  await soon(panel(page).getByText("Nothing was installed or kept.", { exact: true }));
  const cancel = panel(page).getByRole("link", { name: "Cancel", exact: true });
  ensure((await cancel.getAttribute("href")) === "/apps/add", "Cancel does not lead back to the catalogs");
}

/** An address that cannot be an app's ID has no sheet to read, and says so. */
async function notAnId(t, page) {
  await page.goto(new URL("/apps/add/a%20b", page.url()).href);
  await t.see(page.getByText(/^No app can have the ID /));
  const back = page.getByRole("link", { name: "Back to the catalogs", exact: true });
  await t.see(back);
  ensure((await back.getAttribute("href")) === "/apps/add", "Back to the catalogs does not lead to /apps/add");
}

// --- The people and the phone ---

/** An administrator: the search as the machine answers it, typed words kept in the address, a sheet, nginx installed with Turn it on and the next step, the catalog moved on, an app no catalog lists. */
async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await openAdd(t, page);
  const everything = await readSearch(page);
  ensure(everything.bundles.some((one) => one.id === NGINX) && everything.bundles.some((one) => one.id === PYTHON), `${ROUTES.search} does not list ${NGINX} and ${PYTHON}, which the catalog labtest lists`);
  let apps = await readApps(page);
  ensure(!apps.some((app) => app.id === NGINX || app.id === PYTHON), `${ROUTES.list} lists ${NGINX} or ${PYTHON}, which add-app.toml took away`);
  await tableAsAnswered(t, page, everything, apps);
  await headAsAnswered(t, page, apps);
  await lookAll(t, page, "add-app-list");

  // Typing: the setups follow at once, the search is sent when the typing stops, and the words are kept in the address, in a reload and in the row's link.
  const nginxOnly = await typeSearch(t, page, "nginx");
  ensure(nginxOnly.bundles.some((one) => one.id === NGINX), `${ROUTES.search} for nginx does not list ${NGINX}`);
  await tableAsAnswered(t, page, nginxOnly, apps, "nginx");
  ensure(new URL(page.url()).searchParams.get("q") === "nginx", `the address is ${page.url()} and does not keep what was typed`);
  await page.reload();
  await settled(page);
  ensure((await searchField(page).inputValue()) === "nginx", "a reload does not keep what was typed");
  await tableAsAnswered(t, page, await readSearch(page, ["nginx"]), apps, "nginx");
  const database = await typeSearch(t, page, "database");
  await tableAsAnswered(t, page, database, apps, "database");
  const nothing = await typeSearch(t, page, "nosuchappanywhere");
  await tableAsAnswered(t, page, nothing, apps, "nosuchappanywhere");
  if (nothing.bundles.length === 0) {
    const clear = page.getByRole("button", { name: "Clear the search", exact: true });
    await t.see(clear);
    await clear.click();
    ensure((await searchField(page).inputValue()) === "", "Clear the search leaves words in the field");
    await settled(page);
  }
  await typeSearch(t, page, "nginx");
  await searchField(page).fill("a b c d e f");
  await t.see(page.getByText("Search for at most 5 words at a time.", { exact: true }));
  await searchField(page).fill("a_b");
  await t.see(page.getByText("Words of a search are letters, digits, dots, @ and dashes.", { exact: true }));
  await typeSearch(t, page, "nginx");

  // The sheet of nginx opens beside the list, with the search kept in its address.
  const nginx = await openSheet(t, page, NGINX);
  ensure(new URL(page.url()).searchParams.get("q") === "nginx", `the sheet's address ${page.url()} does not keep the search`);
  await t.see(page.locator(`.add-list a.add-who[href^="${sheetHrefOf(NGINX)}"][aria-current="page"]`));
  await sheetAsAnswered(t, page, nginx);
  await buttonsAsAnswered(t, page, nginx, { admin: true });
  await lookAll(t, page, "add-app-sheet");

  // Install for everyone: the unlock is asked first, the install is a task pinned by the sheet's SHA-256, and the machine has the app when it ends.
  const { request, answer } = await pressInstall(t, page, NGINX);
  ensure(new RegExp(`"sha256"\\s*:\\s*"${nginx.sha256}"`).test(request.postData() ?? ""), `${ROUTES.install} was sent ${JSON.stringify(request.postData())}, not the sheet's SHA-256`);
  ensure(answer.status() === 200 || answer.status() === 202, `${ROUTES.install} was answered ${answer.status()}: ${await answer.text()}`);
  const info = await installIsDone(t, page, nginx, "nginx");
  ensure(info.service_list.some((service) => service.name === "nginx"), `${NGINX} has no service nginx: ${JSON.stringify(info.service_list)}`);
  await lookAll(t, page, "add-app-installed");
  await turnItOn(t, page, "nginx");
  await look(t, page, "add-app-on");

  // Back to the catalogs: nginx says it is installed, and the setups say what is left to install.
  await panel(page).locator('a[data-action="close"]').click();
  await page.waitForURL((url) => url.pathname === "/apps/add");
  await settled(page);
  apps = await readApps(page);
  ensure(apps.some((app) => app.id === NGINX), `${ROUTES.list} does not list ${NGINX} after its install`);
  await tableAsAnswered(t, page, await readSearch(page, ["nginx"]), apps, "nginx");

  // Python: the catalog moves on between the sheet and the install, and the page offers to read the sheet again.
  await typeSearch(t, page, "python");
  const python = await openSheet(t, page, PYTHON);
  await sheetAsAnswered(t, page, python);
  await catalogMovedOn(t, page, python);
  await pressInstall(t, page, PYTHON);
  await installIsDone(t, page, python, "python");
  await appsBecome(page, (bundles) => bundles.some((bundle) => bundle.id === PYTHON), `list ${PYTHON}`);

  // The sheet of an app that is installed says so, with a link to its page and no Install.
  await panel(page).locator('a[data-action="close"]').click();
  await settled(page);
  await rowLink(page, PYTHON).click();
  await soon(panel(page).getByRole("heading", { level: 2, name: `${python.name} ${python.version}`, exact: true }));
  await t.see(panel(page).getByText(`Version ${python.version} is installed. Updating is on its page.`));
  ensure((await panel(page).getByRole("link", { name: "Open its page", exact: true }).getAttribute("href")) === hrefOf(PYTHON), "Open its page does not lead to the app's page");
  ensure((await installButton(page).count()) === 0, "the sheet of an app that is installed has Install for everyone");

  await noCatalogLists(t, page);
  await notAnId(t, page);

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.list),
    routeFor(ROUTES.show, NGINX),
    routeFor(ROUTES.show, PYTHON),
    routeFor(ROUTES.search),
    routeFor(ROUTES.sheet, NGINX),
    routeFor(ROUTES.sheet, PYTHON),
    routeFor(ROUTES.sheet, NOBODY),
    routeFor(ROUTES.install, NGINX),
    routeFor(ROUTES.install, PYTHON),
    routeFor(ROUTES.services),
    routeFor(ROUTES.enable, "nginx"),
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the search and a sheet, is told who installs apps, has no Install, and is refused when the route is asked. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openAdd(t, page);
  const apps = await readApps(page);
  await tableAsAnswered(t, page, await readSearch(page), apps);
  const nginx = await openSheet(t, page, NGINX);
  await sheetAsAnswered(t, page, nginx);
  await buttonsAsAnswered(t, page, nginx, { admin: false });
  const controls = await panel(page).locator('button[data-action="install"], button[data-action^="turn-on"]').count();
  ensure(controls === 0, `a reader's sheet of ${NGINX} has ${controls} buttons that change the machine`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.install, NGINX), { sha256: nginx.sha256 }), 403, "ADM-122", "a reader's install of nginx");
  await lookAll(t, page, "add-app-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list is a card for each app, a sheet is alone with a way back, and the controls are a thumb's size, with no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const controls = "main .add button, main .add a.btn, main .add a.add-who, main .add a.radio-card, main .seg a";
  await openAdd(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, controls);
  ensure(small.length === 0, `these controls of the phone's list are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "add-app-phone-list");

  const nginx = await openSheet(t, page, NGINX);
  ensure(await page.locator(".add-list").isHidden(), "the phone shows the list beside a sheet");
  await t.see(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Apps", exact: true }));
  ensure((await sidewaysScroll(page)) === 0, `the phone's sheet scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallSheet = await smallControls(page, controls);
  ensure(smallSheet.length === 0, `these controls of the phone's sheet are under 44 px tall: ${smallSheet.join(", ")}`);
  const install = await installButton(page).boundingBox();
  const box = await panel(page).boundingBox();
  ensure(install !== null && box !== null && install.width >= box.width * 0.8, `Install for everyone is ${install?.width} px wide in a sheet ${box?.width} px wide`);
  ensure((await panel(page).getByText(nginx.name).count()) > 0, "the phone's sheet does not name the app");
  await lookAll(t, page, "add-app-phone-sheet");

  // Cancel leads back to the list, which is shown again.
  await panel(page).getByRole("link", { name: "Cancel", exact: true }).click();
  await page.waitForURL((url) => url.pathname === "/apps/add");
  await settled(page);
  ensure(await page.locator(".add-list").isVisible(), "the phone does not show the list after Cancel");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The reader and the phone come first: the administrator installs both apps.
  await reader(t);
  await phone(t);
  await administrator(t);
}
