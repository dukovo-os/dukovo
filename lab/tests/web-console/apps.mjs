// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/apps drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Apps page at /apps
// and one app's page at /apps/{id}, with an app's ID for {id} and each dot of
// it written %2E. The list shows what `bundle list --json` says, which
// `GET /api/1/apps` runs: the apps installed for everyone in a table of their
// own, the apps a person installed for themselves apart. An app's page is the
// panel beside the list, from what `bundle info ID --json` says, which
// `GET /api/1/apps/{id}` runs: where the app came from and its licence, its
// services, what it may use, what it provides, uses and what uses it, its data,
// its size, its flaws and its versions, with Hold (`bundle hold`,
// `POST /api/1/apps/{id}/hold`), Release the hold (`bundle unhold`,
// `POST /api/1/apps/{id}/unhold`) and Remove… (`bundle remove`,
// `POST /api/1/apps/{id}/remove`, a task).
//
// The apps are nginx, which has a service and a data folder, and Python, which
// has neither, both installed for everyone by apps.toml before each engine and
// with nginx running. The page's rows are held to what the route answers, and
// its panel's facts to what the route says of the app. A locked administrator
// who presses Remove… is asked to unlock first, and the dialog says what the
// app is removed for, keeps its data as the answer that is chosen, and asks for
// the app's name to be typed before it removes the data too: for Python a name
// that is not typed, and one that is typed wrong, send nothing. Hold and its
// release change the row and the panel, and the machine says so. Remove… with
// Keep its data takes nginx away as a task, the page leaves its panel for the
// list when the task is done, and apps.toml finds the data folder where it was.
// An app that is not installed says so and leads back to the list. A reader
// sees every fact and no button, is told who may change an app, and is refused
// when the route is asked. A phone at 390 by 844 shows the list, and an app
// alone, with no sideways scroll and every control 44 px tall. Each look is the
// console's own: dark, light, high contrast, forced colours and less motion,
// each checked by axe (console.mjs's lookAll). Going back and updating are
// tested with the Updates page, which has the apps that update.
//
// The script runs once for each engine, apps.toml having put the apps back
// before it, so that the removal of nginx is the same in each. The accounts
// are the two that apps.toml made, so no script needs the lab's password.

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
import { PASSWORD, PHONE, answerUnlock, pressAndAnswer, smallControls, unlockDialog, unlockNow } from "./trial.mjs";

const ADMIN = "wc-apps";
const READER = "wc-apps-reader";

// The apps that apps.toml installs, and an ID that no machine has.
const NGINX = "org.nginx.nginx";
const PYTHON = "org.python.python";
const NOBODY = "org.example.nothing";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with an
// app's ID for {id}.
const ROUTES = {
  list: "GET /api/1/apps",
  show: "GET /api/1/apps/{id}",
  hold: "POST /api/1/apps/{id}/hold",
  unhold: "POST /api/1/apps/{id}/unhold",
  remove: "POST /api/1/apps/{id}/remove",
};

/** The path of a route as the table names it, for the app `id`: `GET /api/1/apps/{id}` is `/api/1/apps/org.nginx.nginx`. */
const pathOf = (route, id = "") => route.slice(route.indexOf(" ") + 1).replace("{id}", encodeURIComponent(id));

/** The route as `track` writes it, for `id`. */
const routeFor = (route, id = "") => `${route.slice(0, route.indexOf(" "))} ${pathOf(route, id)}`;

/** The address of an app's page, and of a service's: a dot is written %2E. */
const hrefOf = (id) => `/apps/${encodeURIComponent(id).replaceAll(".", "%2E")}`;
const serviceHrefOf = (name) => `/services/${encodeURIComponent(name).replaceAll(".", "%2E")}`;

// The words the page says where this test looks for them.
const ADD_LEAD = "Apps come from a catalog: bundle search WORD, then elevate bundle install ID --for-everyone.";
const NO_PREVIOUS = "No earlier version is kept yet: an update keeps the one it replaces.";
const NOTHING_USES = "Nothing installed uses it.";
const NO_FLAWS = "None known.";
const CATALOG_SILENT = "The catalog says nothing of this version's flaws.";
const READER_LINE = "An administrator can update, hold, go back and remove apps.";
const RULES = { auto: "Update by themselves", notify: "Tell me" };

/** Whether `value` is text with something in it, as the page counts it. */
const isText = (value) => typeof value === "string" && value.trim() !== "";

// --- Reading the machine ---

/** What `GET /api/1/apps` answers: `bundle list --json`, `{format, bundles}`; gives the bundles. */
async function readApps(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(answer.status === 200 && Array.isArray(answer.json?.bundles), `${ROUTES.list} was answered ${answer.status}: ${answer.text}`);
  return answer.json.bundles;
}

/** What `GET /api/1/apps/{id}` answers: `bundle info ID --json`, with the members the page needs. */
async function readApp(page, id) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.show, id));
  ensure(answer.status === 200 && answer.json?.id === id, `${ROUTES.show} for ${id} was answered ${answer.status}: ${answer.text}`);
  const info = answer.json;
  for (const member of ["service_list", "provides", "uses", "used_by"]) {
    ensure(Array.isArray(info[member]), `${ROUTES.show} for ${id} gives no list ${member}: ${answer.text}`);
  }
  return info;
}

/** Reads the apps until `test(bundles)` holds, for up to `seconds`, and gives them. */
async function appsBecome(page, test, what, seconds = 90) {
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

const panel = (page) => page.locator("section.apps-panel");
const button = (page, name) => panel(page).getByRole("button", { name, exact: true });
const rowLink = (page, id) => page.locator(`.apps-list a.apps-who[href="${hrefOf(id)}"]`);
const rowOf = (page, id) => page.locator(".apps-list tbody tr").filter({ has: page.locator(`a.apps-who[href="${hrefOf(id)}"]`) });

/** What the key in hand does, by its `data-action`: the panel's buttons say what they do. */
const focusedAction = (page) => page.evaluate(() => document.activeElement?.getAttribute("data-action") ?? "");

/** The panel's rows of facts, by their names: `{Service: "nginx · Running, starts at boot", ...}`; a fact that is not there is not a member. */
const factsOf = (page) =>
  panel(page).locator("dl.rows").evaluate((dl) => {
    const facts = {};
    for (const term of dl.querySelectorAll("dt")) facts[term.textContent.trim()] = term.nextElementSibling.innerText.trim();
    return facts;
  });

/** Opens the list and waits for its rows to be drawn. Gives the bundles that the machine says are installed. */
async function openList(t, page) {
  await page.goto(new URL("/apps", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Apps", exact: true }));
  await page.waitForFunction(() => document.querySelectorAll(".apps-list a.apps-who, .apps-list .apps-empty").length > 0, null, { timeout: 30_000 });
  return readApps(page);
}

/** Opens the page of the app `id` and waits for its panel. On a phone the panel is alone. Gives what `bundle info` says of it. */
async function openApp(t, page, id, { phone = false } = {}) {
  const info = await readApp(page, id);
  await page.goto(new URL(hrefOf(id), page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: info.name, exact: true }));
  await t.see(panel(page).getByRole("heading", { level: 2, name: `${info.name} ${info.version}`, exact: true }));
  if (phone) {
    ensure(await page.locator(".apps-list").isHidden(), "the phone shows the list beside an app's page");
  } else {
    await t.see(page.locator(`.apps-list a.apps-who[href="${hrefOf(id)}"][aria-current="page"]`));
  }
  return info;
}

/** The page's rows are the machine's: each app of the list a link to its own page with its name, its ID, its version and what is known of it. */
async function listAsAnswered(t, page, bundles) {
  const everyone = bundles.filter((bundle) => bundle.for_everyone === true);
  const yours = bundles.filter((bundle) => bundle.for_everyone !== true);
  const shown = await page.locator(".apps-list a.apps-who").evaluateAll((links) => links.map((link) => link.getAttribute("href")));
  const wanted = everyone.map((bundle) => hrefOf(bundle.id));
  ensure(JSON.stringify([...shown].sort()) === JSON.stringify([...wanted].sort()), `the page links ${JSON.stringify(shown)} and ${ROUTES.list} gives ${JSON.stringify(wanted)} for everyone`);
  for (const bundle of everyone) {
    const row = rowOf(page, bundle.id);
    const name = isText(bundle.name) ? bundle.name : bundle.id;
    ensure((await row.locator("b").innerText()) === name, `the row of ${bundle.id} does not say ${name}`);
    ensure((await row.locator("small.mono").innerText()) === bundle.id, `the row of ${bundle.id} does not say its ID`);
    ensure((await row.locator('td[data-label="Version"]').innerText()) === String(bundle.version), `the row of ${bundle.id} does not say ${bundle.version}`);
    const update = await row.locator('td[data-label="Update"]').innerText();
    const flawed = bundle.flaws?.state === "known" && bundle.flaws.count > 0;
    ensure(flawed === /known flaws?/.test(update), `the row of ${bundle.id} says ${JSON.stringify(update)} of flaws and the machine knows ${bundle.flaws?.count ?? 0}`);
    if (bundle.held === true) ensure(update.includes(`Held at ${bundle.version}`), `the row of ${bundle.id} is held and says ${JSON.stringify(update)}`);
  }
  ensure((await page.locator(".apps-yours tbody tr").count()) === yours.length, `the page's Yours has ${await page.locator(".apps-yours tbody tr").count()} rows and ${ROUTES.list} gives ${yours.length} apps that are not for everyone`);
  const line = await page.locator(".apps-tools p").first().innerText();
  ensure(/^(Checked .+\.|Not checked yet\.|The last check could not be read\.)/.test(line), `the line over the table says ${JSON.stringify(line)}`);
  // Update all is there for an administrator when more than one app has an update.
  const waiting = await page.locator('td[data-updates="true"]').count();
  const updateAll = await page.getByRole("button", { name: /^Update all/ }).count();
  return { waiting, updateAll };
}

/** Where apps come from: a page that adds one, or, while there is none, the words that say how it is done at a terminal. */
async function leadShown(page) {
  const add = await page.getByRole("link", { name: "Add an app", exact: true }).count();
  const lead = await page.getByText(ADD_LEAD, { exact: true }).count();
  ensure(add + lead > 0, "the list neither has Add an app nor says where apps come from");
}

/** The page's facts of an app are what `bundle info` says of it. */
async function factsAsAnswered(t, page, info) {
  const facts = await factsOf(page);
  const where = `the page of ${info.id}`;

  // The line under the name has the licence.
  if (info.id === NGINX) ensure(isText(info.license), `${ROUTES.show} gives no licence for ${info.id}`);
  if (isText(info.license)) {
    const line = await panel(page).locator(".apps-title .muted").innerText();
    ensure(line.includes(info.license), `${where} does not say the licence ${info.license}: ${line}`);
  }

  // Services: a link to each one's page, the account it runs as, how it starts, and its data folder.
  const services = info.service_list;
  if (services.length === 0) {
    ensure(facts.Service === undefined && facts["Runs as"] === undefined, `${where} has a Service row for an app that has none: ${JSON.stringify(facts)}`);
    const data = isText(info.data) ? info.data : undefined;
    ensure(facts.Data === data, `${where} says ${JSON.stringify(facts.Data)} of its data, and ${ROUTES.show} says ${JSON.stringify(data)}`);
  } else {
    for (const service of services) {
      const link = panel(page).locator(`dl.rows a[href="${serviceHrefOf(service.name)}"]`);
      await t.see(link);
      ensure((await link.innerText()) === service.name, `${where} links ${service.name} by another name`);
      ensure(facts.Data.includes(`/shared/services/${info.id}/${isText(service.account) ? service.account : service.name}`), `${where} does not say the data folder of ${service.name}: ${facts.Data}`);
    }
    ensure(/ · .*(starts at boot|off at boot|on connection|by its timer)$/im.test(facts.Service), `${where} does not say how its service starts: ${facts.Service}`);
    const accounts = [...new Set(services.map((service) => (isText(service.account) ? service.account : service.name)))];
    const runsAs = `${accounts.length === 1 ? "its own account" : "its own accounts"}, ${accounts.join(", ")}`;
    ensure(facts["Runs as"] === runsAs, `${where} says ${JSON.stringify(facts["Runs as"])} of the account, not ${runsAs}`);
  }

  // May use: a line for each permission, then what is left out.
  const wordsOf = (permission) => (isText(permission?.lets) ? permission.lets : isText(permission?.name) ? permission.name : null);
  const may = (info.permissions ?? []).filter((permission) => wordsOf(permission) !== null);
  const lines = (facts["May use"] ?? "").split("\n").map((line) => line.trim()).filter(Boolean);
  ensure(lines.length === may.length + 1 && lines.at(-1) === (may.length > 0 ? "nothing else" : "nothing"), `${where} lists what it may use as ${JSON.stringify(lines)}`);
  may.forEach((permission, at) => ensure(lines[at].startsWith(wordsOf(permission)), `${where} does not say ${wordsOf(permission)} first`));

  // Provides and uses: a row when there is something, and none when there is not.
  for (const [name, key] of [["Provides", "provides"], ["Uses", "uses"]]) {
    const links = info[key].map((link) => [isText(link?.interface) ? link.interface : null, isText(link?.socket) ? `at ${link.socket}` : null].filter(Boolean).join(" ")).filter(Boolean);
    if (links.length === 0) ensure(facts[name] === undefined, `${where} has a ${name} row and ${ROUTES.show} gives none`);
    else for (const link of links) ensure(facts[name]?.includes(link), `${where} does not say ${name.toLowerCase()} ${link}`);
  }

  // Used by: the apps that use it, each a link, or that none does.
  if (info.used_by.length === 0) {
    ensure(facts["Used by"] === NOTHING_USES, `${where} says ${JSON.stringify(facts["Used by"])} of what uses it`);
  } else {
    for (const user of info.used_by) {
      const link = panel(page).locator(`dl.rows a[href="${hrefOf(user.id)}"]`);
      await t.see(link);
      ensure((await link.innerText()) === (isText(user.name) ? user.name : user.id), `${where} links ${user.id} by another name`);
    }
  }

  // Size, flaws and the rule the apps update by.
  if (Number.isFinite(info.bytes)) ensure(/^\d/.test(facts.Size ?? ""), `${where} says ${JSON.stringify(facts.Size)} of its size`);
  else ensure(facts.Size === undefined, `${where} has a size and ${ROUTES.show} gives none`);
  const known = info.flaws?.known ?? [];
  if (known.length > 0) {
    for (const flaw of known) await t.see(panel(page).locator(".banner").getByText(flaw.id).first());
  } else if (info.flaws) {
    const flaws = info.flaws.state === "unknown" ? CATALOG_SILENT : NO_FLAWS;
    ensure(facts.Flaws === flaws, `${where} says ${JSON.stringify(facts.Flaws)} of its flaws and ${ROUTES.show} says ${JSON.stringify(info.flaws)}`);
  }
  const settings = await fetchAsPage(page, "GET", "/api/1/updates/settings");
  if (settings.status === 200 && RULES[settings.json?.apps]) ensure(facts.Updates?.startsWith(RULES[settings.json.apps]), `${where} says ${JSON.stringify(facts.Updates)} of how the apps update`);

  // Versions: the installed one, the one kept for going back or the words that say none is.
  const versions = await panel(page).locator(".apps-versions tbody tr").evaluateAll((rows) => rows.map((row) => row.innerText));
  ensure(versions.some((row) => row.includes(info.version) && /installed|held/.test(row)), `${where} has no row for the installed ${info.version}: ${JSON.stringify(versions)}`);
  if (isText(info.previous)) ensure(versions.some((row) => row.includes(info.previous) && row.includes("Kept for going back")), `${where} has no row for the kept ${info.previous}`);
  else await t.see(panel(page).getByText(NO_PREVIOUS));
}

// --- Pressing the buttons ---

/** Presses Remove… and answers the unlock if the console asks for it. Gives the dialog and whether it asked. */
async function openRemove(t, page, info) {
  await button(page, "Remove…").click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  const dialog = page.getByRole("dialog", { name: `Remove ${info.name}?`, exact: true });
  await t.see(dialog);
  return { dialog, asked };
}

const cancelOf = (dialog) => dialog.getByRole("button", { name: "Cancel", exact: true });
const removeOf = (dialog) => dialog.getByRole("button", { name: "Remove the app", exact: true });
const choiceOf = (dialog, words) => dialog.locator("label.radio-card", { hasText: words });

/** What the dialog says of the app it removes: for whom, what becomes of its data, and what uses it. */
async function removeAsked(t, page, dialog, info) {
  await t.see(dialog.getByText(`${info.name} ${info.version} is removed for everyone, and its services stop and go.`));
  ensure(await choiceOf(dialog, "Keep its data").locator("input").isChecked(), "Keep its data is not the answer that is chosen at first");
  const typed = dialog.getByLabel(`Type ${info.name} to go on`);
  ensure(!(await typed.isVisible()), "the dialog asks for the app's name before the data is to go");
  await choiceOf(dialog, "Remove its data too").click();
  await t.see(typed);
  await choiceOf(dialog, "Keep its data").click();
  await t.gone(typed);
  for (const user of info.used_by) {
    const name = isText(user.name) ? user.name : user.id;
    await t.see(dialog.getByText(isText(user.what) ? `${name} uses it: removing it stops ${name}'s ${user.what}.` : `${name} uses it, and may stop working without it.`));
  }
  if (info.used_by.length === 0) ensure((await dialog.locator(".apps-users").count()) === 0, "the dialog says that apps use an app that nothing uses");
}

/** Remove… of a locked console asks to unlock first; what the dialog says is read, and Cancel closes it with the keyboard where it was. */
async function removeIsAsked(t, page, info) {
  const { dialog, asked } = await openRemove(t, page, info);
  ensure(asked, "a locked console did not ask to unlock before Remove… opened its dialog");
  await removeAsked(t, page, dialog, info);
  await lookAll(t, page, "apps-remove");
  await cancelOf(dialog).click();
  await t.gone(dialog);
  ensure((await focusedAction(page)) === "remove", `the keyboard is on ${JSON.stringify(await focusedAction(page))} after Cancel, not on Remove…`);
}

/** The data goes only when the app's name is typed: with none and with a wrong one, nothing is sent. */
async function purgeNeedsTheName(t, page, info) {
  const sent = [];
  const watch = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.remove, info.id)) sent.push(request.url());
  };
  page.on("request", watch);
  const { dialog } = await openRemove(t, page, info);
  await removeAsked(t, page, dialog, info);
  await choiceOf(dialog, "Remove its data too").click();
  const typed = dialog.getByLabel(`Type ${info.name} to go on`);
  await t.see(typed);
  const words = dialog.getByText(`Type ${info.name} exactly as it is written`);
  await removeOf(dialog).click();
  await t.see(words);
  await typed.fill(info.name.slice(0, -1));
  await t.gone(words);
  await removeOf(dialog).click();
  await t.see(words);
  await sleep(1000);
  page.off("request", watch);
  ensure(sent.length === 0, `Remove the app sent ${sent.join(", ")} without the app's name`);
  await cancelOf(dialog).click();
  await t.gone(dialog);
}

/** Hold at VERSION holds the app and Release the hold lets it go: the panel, the row and the machine say so each time. */
async function holdAndRelease(t, page, info) {
  const held = `Held at ${info.version}: no update is taken, security fixes included. Release the hold to take updates again.`;
  await pressAndAnswer(t, page, button(page, `Hold at ${info.version}`), pathOf(ROUTES.hold, info.id));
  await t.see(button(page, "Release the hold"));
  await t.see(panel(page).getByText(held, { exact: true }));
  await t.see(rowOf(page, info.id).locator('td[data-label="Update"]').getByText(`Held at ${info.version}`));
  ensure((await readApp(page, info.id)).held === true, `${info.id} is not held on the machine`);
  ensure((await focusedAction(page)) === "hold", `the keyboard is on ${JSON.stringify(await focusedAction(page))} after Hold, not on the button that now releases`);
  await lookAll(t, page, "apps-held");

  await pressAndAnswer(t, page, button(page, "Release the hold"), pathOf(ROUTES.unhold, info.id));
  await t.see(button(page, `Hold at ${info.version}`));
  await t.gone(panel(page).getByText(held, { exact: true }));
  ensure((await readApp(page, info.id)).held !== true, `${info.id} is still held on the machine`);
  ensure(!(await rowOf(page, info.id).locator('td[data-label="Update"]').innerText()).includes("Held at"), "the row still says held");
}

/** Remove… with Keep its data takes the app away as a task, and the page leaves its panel for the list. */
async function removeKeepingData(t, page, info) {
  const { dialog } = await openRemove(t, page, info);
  const path = pathOf(ROUTES.remove, info.id);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === path);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === path);
  await removeOf(dialog).click();
  const body = (await sent).postData() ?? "";
  ensure(/"data"\s*:\s*"keep"/.test(body), `${ROUTES.remove} was sent ${JSON.stringify(body)}, not the data kept`);
  const status = (await answered).status();
  ensure(status === 200 || status === 202, `${ROUTES.remove} was answered ${status}`);
  await page.waitForURL((url) => url.pathname === "/apps", { timeout: 180_000 });
  await t.see(page.getByRole("heading", { level: 1, name: "Apps", exact: true }));
  await appsBecome(page, (bundles) => !bundles.some((bundle) => bundle.id === info.id), `take ${info.id} away`);
  await t.gone(rowLink(page, info.id));
}

/** An app that is not installed says so, and leads back to the list. */
async function notInstalled(t, page) {
  await page.goto(new URL(hrefOf(NOBODY), page.url()).href);
  await t.see(page.getByText(`No app called ${NOBODY} is installed here.`, { exact: true }));
  const link = page.getByRole("link", { name: "All the apps", exact: true });
  await t.see(link);
  ensure((await link.getAttribute("href")) === "/apps", "All the apps does not lead to /apps");
  await t.gone(panel(page));
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.show, NOBODY)), 422, "ADM-123", `${ROUTES.show} for ${NOBODY}`);
}

// --- The people and the phone ---

/** An administrator: the list, the facts of two apps, Remove… asked of a locked console, Hold and Release, the name typed for the data, and nginx removed with its data kept. */
async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const bundles = await openList(t, page);
  const { waiting, updateAll } = await listAsAnswered(t, page, bundles);
  ensure(updateAll === (waiting > 1 ? 1 : 0), `${waiting} apps have an update and the list has ${updateAll} Update all`);
  await leadShown(page);
  ensure(bundles.some((bundle) => bundle.id === NGINX) && bundles.some((bundle) => bundle.id === PYTHON), `${ROUTES.list} does not list ${NGINX} and ${PYTHON}, which apps.toml installed`);
  await lookAll(t, page, "apps-list");

  // The row is a link: pressing it opens the app's page beside the list.
  await rowLink(page, NGINX).click();
  await page.waitForURL((url) => url.pathname === hrefOf(NGINX));
  const nginx = await openApp(t, page, NGINX);
  await factsAsAnswered(t, page, nginx);
  const settings = panel(page).getByRole("link", { name: "Settings…", exact: true });
  await t.see(settings);
  ensure((await settings.getAttribute("href")) === serviceHrefOf(nginx.service_list[0].name), "Settings… does not lead to the service's page");
  await lookAll(t, page, "apps-app");

  await removeIsAsked(t, page, nginx);
  await holdAndRelease(t, page, nginx);

  const python = await openApp(t, page, PYTHON);
  await factsAsAnswered(t, page, python);
  const settingsOfPython = await panel(page).getByRole("link", { name: "Settings…", exact: true }).count();
  ensure(settingsOfPython === (python.service_list.length > 0 ? 1 : 0), `${PYTHON} has ${python.service_list.length} services and ${settingsOfPython} Settings…`);
  await lookAll(t, page, "apps-plain");
  await purgeNeedsTheName(t, page, python);

  await openApp(t, page, NGINX);
  await removeKeepingData(t, page, nginx);
  await lookAll(t, page, "apps-list-after");

  await notInstalled(t, page);
  await lookAll(t, page, "apps-missing");

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.list),
    routeFor(ROUTES.show, NGINX),
    routeFor(ROUTES.show, PYTHON),
    routeFor(ROUTES.hold, NGINX),
    routeFor(ROUTES.unhold, NGINX),
    routeFor(ROUTES.remove, NGINX),
  ]);
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees the list and every fact of an app and no button, is told who may change it, and is refused when the route is asked. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  const bundles = await openList(t, page);
  const { updateAll } = await listAsAnswered(t, page, bundles);
  ensure(updateAll === 0, "a reader's list has Update all");
  const info = await openApp(t, page, NGINX);
  await factsAsAnswered(t, page, info);
  await t.see(panel(page).getByText(READER_LINE, { exact: true }));
  const controls = await panel(page).locator("button, a.btn").count();
  ensure(controls === 0, `a reader's page of ${NGINX} has ${controls} buttons`);
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.hold, NGINX), {}), 403, "ADM-122", "a reader's hold of nginx");
  mustBe(await fetchAsPage(page, "POST", pathOf(ROUTES.remove, NGINX), { data: "keep" }), 403, "ADM-122", "a reader's removal of nginx");
  await lookAll(t, page, "apps-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the list, and an app alone with a way back to the list, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const bundles = await openList(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's list scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .apps button, main .apps a.btn, main .apps a.apps-who");
  ensure(small.length === 0, `these controls of the phone's list are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "apps-phone-list");

  const first = bundles.find((bundle) => bundle.for_everyone === true);
  ensure(first !== undefined, `${ROUTES.list} lists no app for everyone`);
  await openApp(t, page, first.id, { phone: true });
  await t.see(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Apps", exact: true }));
  ensure((await sidewaysScroll(page)) === 0, `the phone's app scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallApp = await smallControls(page, "main .apps button, main .apps a.btn, main .apps a.apps-who");
  ensure(smallApp.length === 0, `these controls of the phone's app are under 44 px tall: ${smallApp.join(", ")}`);
  await lookAll(t, page, "apps-phone-app");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The reader and the list come first: the administrator takes nginx away.
  await reader(t);
  await administrator(t);
  await phone(t);
}
