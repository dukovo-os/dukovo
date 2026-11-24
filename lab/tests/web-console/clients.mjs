// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/clients drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the AI
// clients page at /clients (ADR 0690), and the console's MCP server behind
// it, with the lab PC as the client.
//
// A second administrator lets in a client of their own first, lab-b, by the
// page's route. The administrator then opens the page with the console
// locked: nothing of theirs is listed, and the page says that these are
// their own clients. Add a client... refuses no name, a name that is none
// and, later, a name in use, and sends nothing for them; with a name, it
// asks to unlock, adds lab-ssh as the dialog opens, reading everything,
// and shows the settings that run `admin mcp lab-ssh` over SSH and the
// check to try at a terminal. With Show commands on, the dialog says the
// `admin mcp add` that does the same. lab-https is added over HTTPS with
// the services whole: its token is shown once, in the settings a client
// takes, and is nowhere in the page once the dialog is closed.
//
// The lab PC then sends what a client on another machine sends, the token
// and a message to POST /mcp, with nothing of the browser's: it is told
// what it reaches, reads the services, and asks to restart diskhealth. The
// request waits on the page, with the client's reason, the command a yes
// runs, which is POST /api/1/show's, whether it changes anything and the
// files it writes, and the navigation counts it. Approve runs it: the
// client is told done, by the administrator, the journal's row has the
// administrator as who and `client lab-https of NAME` as how, and What they
// did has its line. With the console locked again, the list is the
// administrator's own. A second request, to stop the service, has its
// Approve ask to unlock, and a cancel sends nothing; Refuse needs no
// unlock, and the client is told refused. No row of the journal stops the
// service.
//
// Unlocked from the list, the page shows every administrator's clients, as
// GET /api/1/console/clients answers, which is the action console.clients,
// with its command under the table. lab-b is removed with its Remove, which
// shows the command of console.client-remove and runs it. The
// administrator's own are removed by their own route, and the token is
// then refused at /mcp (ADM-143).
//
// A reader has no link to the page, is told there that AI clients are for
// administrators, and is refused each of its routes (ADM-122), but for the
// list of their own, which is empty. A phone at 390 by 844 shows a request
// that waits without scrolling sideways, with every control 44 px tall, and
// refuses it. Each look is the console's own: dark, light, high contrast,
// forced colours and less motion, each checked by axe (console.mjs's
// lookAll). The dialog that shows the token is checked and not kept as a
// picture. The accounts are those that clients.toml made, so no script
// needs the lab's password.

import https from "node:https";
import { isDeepStrictEqual } from "node:util";

import {
  answerTo,
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  originOf,
  press,
  routeOf,
  sidewaysScroll,
  signIn,
  signInDone,
  signOut,
  sleep,
} from "./console.mjs";

// The password that clients.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";
const READER = "wc-clients-reader";

/** The administrator of `engine`, and the second one, whose client the first removes. */
const adminOf = (engine) => `wc-clients-${engine}`;
const otherOf = (engine) => `wc-clients-b-${engine}`;

const PHONE = { width: 390, height: 844 };

// The clients: the two the administrator adds through the dialog, the
// second administrator's, and the one of the phone's part.
const OVER_SSH = "lab-ssh";
const OVER_HTTPS = "lab-https";
const OTHERS = "lab-b";
const ON_PHONE = "lab-phone";

// The service the client asks about: every image runs it and nothing needs it.
const SERVICE = "diskhealth";
const REASON = "The disk checks have not run since the last update.";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them. The browser must have called each of the page's by the
// end, with a request's number for {id} and a client for {owner} and
// {name}; POST /mcp is the lab PC's own call.
const ROUTES = {
  areas: "GET /api/1/clients/areas",
  requests: "GET /api/1/clients/requests",
  history: "GET /api/1/clients/history",
  own: "GET /api/1/account/clients",
  add: "POST /api/1/account/clients/add",
  remove: "POST /api/1/account/clients/remove",
  all: "GET /api/1/console/clients",
  removeOf: "POST /api/1/console/clients/{owner}/{name}/remove",
  approve: "POST /api/1/clients/requests/{id}/approve",
  refuse: "POST /api/1/clients/requests/{id}/refuse",
  mcp: "POST /mcp",
};

/** The path of a route written as `METHOD /path`. */
const pathOf = (route) => route.split(" ")[1];

/** The route that removes the client `name` of `owner`, from its template. */
const ofClient = (owner, name) => ROUTES.removeOf.replace("{owner}", owner).replace("{name}", name);

// What the page says, word for word.
const OWN_ONLY = "These are your own clients. Unlock to see every administrator's.";
const NO_NAME = "Give the client a name.";
const BAD_NAME = "A name is 1 to 32 lowercase letters, digits and hyphens.";
const FOR_ADMINISTRATORS = "AI clients are for administrators.";
const TABLE_OWN = "Your AI clients";
const TABLE_ALL = "Every administrator's AI clients";

/** How long a client's ask is waited for: the console answers it after 45 seconds when nobody has. */
const ASK_SECONDS = 70;

// --- The page ---

/** The page's card `id`. */
const card = (page, id) => page.locator(`#${id}`);

/** The notice that the shell shows when a change is done. */
const noticeOf = (page, words) => page.locator(".notice-words", { hasText: words }).first();

/** The dialog that asks to unlock. */
const unlockDialog = (page) => page.getByRole("dialog", { name: /^Unlock/ });

/** The header's button called `name`. */
const headerButton = (page, name) => page.getByRole("banner").getByRole("button", { name, exact: true });

/**
 * How many pixels taller the document is than its window. The page's own
 * box is what scrolls, so anything here is a part of the page that left
 * that box, and the window then scrolls into nothing.
 */
const beyondWindow = (page) =>
  page.evaluate(() => Math.max(0, document.documentElement.scrollHeight - document.documentElement.clientHeight));

/** Throws if the page scrolls sideways or the document is taller than its window; `where` says which page. */
async function fitsWindow(page, where) {
  const [sideways, beyond] = [await sidewaysScroll(page), await beyondWindow(page)];
  ensure(sideways === 0, `${where} scrolls ${sideways} px sideways`);
  ensure(beyond === 0, `the document of ${where} is ${beyond} px taller than its window`);
}

/** Opens /clients and waits for its cards to hold what the page read. */
async function openPage(t, page) {
  await page.goto(new URL("/clients", page.url()).href);
  await t.see(page.getByRole("heading", { name: "AI clients", level: 1, exact: true }));
  // What they did is the last card the page fills.
  await t.see(card(page, "cl-did").locator(".cl-body > *").first());
}

/** What `route`, a GET, answers the session of `page`, which must be 200. */
async function read(page, route) {
  const answer = await fetchAsPage(page, "GET", pathOf(route));
  ensure(answer.status === 200 && answer.json !== null, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Waits, for up to 20 seconds, until what `get` gives passes `test`, and gives it; `what` says what was waited for. */
async function becomes(get, test, what) {
  const until = Date.now() + 20_000;
  for (;;) {
    const value = await get();
    if (test(value)) return value;
    ensure(Date.now() < until, `${what}: it is ${JSON.stringify(value)}`);
    await sleep(250);
  }
}

/** The clients the table shows, each as `OWNER/NAME`, in order. */
const rowsShown = (page) =>
  card(page, "cl-clients").locator("tbody tr").evaluateAll((rows) =>
    rows.map((row) => `${row.querySelector("td small")?.textContent.trim()}/${row.querySelector("td b")?.textContent.trim()}`));

/** The clients of `answer`, a list of the console's, each as `OWNER/NAME`, sorted. */
const namesOf = (answer) => answer.clients.map((client) => `${client.owner}/${client.name}`).sort();

/** Waits for the table to show the clients `wanted`, each `OWNER/NAME`, and no other. */
const tableShows = (page, wanted, what) =>
  becomes(async () => (await rowsShown(page)).sort(), (shown) => isDeepStrictEqual(shown, [...wanted].sort()), what);

/** Counts the requests of `page` for `route`; gives `{count(), stop()}`. */
function watchRoute(page, route) {
  let counted = 0;
  const note = (request) => {
    if (`${request.method()} ${new URL(request.url()).pathname}` === route) counted += 1;
  };
  page.on("request", note);
  return { count: () => counted, stop: () => page.off("request", note) };
}

/** Answers the dialog that asks to unlock, which says `words` of what is about to be done, with the account's password. */
async function unlockHere(t, page, words) {
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText(words, { exact: true }));
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const answered = answerTo(page, "/api/1/unlock");
  await dialog.getByRole("button", { name: "Unlock", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `the unlock was answered ${answer.status()}`);
  await t.gone(dialog);
}

/** Unlocks the console for the session of `page` without the page, as a script does. */
async function unlock(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(answer.status === 200, `the unlock was answered ${answer.status}: ${answer.text}`);
}

/** Lets in a client of the session's account by the page's route, from an unlocked session; gives its token, null over SSH. */
async function letIn(page, name, way, scopes) {
  const added = await fetchAsPage(page, "POST", pathOf(ROUTES.add), { proof: "", name, way, scopes, days: 30 });
  ensure(added.status === 200, `adding ${name} was answered ${added.status}: ${added.text}`);
  return added.json?.token ?? null;
}

/** Removes the client `name` of the session's account by its own route. */
async function takeAway(page, name) {
  const removed = await fetchAsPage(page, "POST", pathOf(ROUTES.remove), { name });
  ensure(removed.status === 200 && removed.json?.done === true, `removing ${name} was answered ${removed.status}: ${removed.text}`);
}

// --- The client ---

/**
 * A message to the console's MCP server from the client with `token`, as a
 * program on another machine sends it: from the lab PC, with the token and
 * nothing of the browser's, no cookie and no Origin. The console's
 * certificate is the machine's own, which no authority signed, and is
 * taken as it is, as the lab's browsers take it. Gives `{status, text,
 * json}`, `json` being null when the answer is not JSON.
 */
function send(origin, token, message, seconds = 20) {
  const body = JSON.stringify(message);
  return new Promise((resolve, reject) => {
    const request = https.request(
      new URL(pathOf(ROUTES.mcp), origin),
      {
        method: "POST",
        rejectUnauthorized: false,
        timeout: seconds * 1000,
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
          "Content-Length": Buffer.byteLength(body),
        },
      },
      (answer) => {
        const chunks = [];
        answer.on("data", (chunk) => chunks.push(chunk));
        answer.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let json = null;
          try {
            json = JSON.parse(text);
          } catch {
            // Not JSON: the caller sees the text.
          }
          resolve({ status: answer.statusCode, text, json });
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error(`${ROUTES.mcp} was not answered in ${seconds} seconds`)));
    request.on("error", reject);
    request.end(body);
  });
}

let lastId = 0;

/** What the tool `tool` answers the client with `token` for `args`: its data, which must be no failure. */
async function told(origin, token, tool, args, seconds = 20) {
  lastId += 1;
  const call = { jsonrpc: "2.0", id: lastId, method: "tools/call", params: { name: tool, arguments: args } };
  const answer = await send(origin, token, call, seconds);
  const result = answer.json?.result;
  ensure(
    answer.status === 200 && answer.json?.id === call.id && result?.isError === false && typeof result.structuredContent === "object",
    `the tool ${tool} with ${JSON.stringify(args)} was answered ${answer.status}: ${answer.text}`,
  );
  return result.structuredContent;
}

/**
 * Has the client ask for `action` of the service, and gives the promise of
 * what the tool ask answers, which comes when an administrator has
 * answered the request or after 45 seconds.
 */
function asks(origin, token, action, reason) {
  const pending = told(origin, token, "ask", { action, arguments: { name: SERVICE }, reason }, ASK_SECONDS);
  // It is awaited later: until then a failure is nobody's to catch.
  pending.catch(() => {});
  return pending;
}

/**
 * What the client is told of request `id`: the tool request says `state`,
 * by `admin`, and the ask that waited was told the same, or, if its 45
 * seconds ran out first, where the request stood then.
 */
async function toldAs(origin, token, pending, id, state, admin) {
  const stands = await told(origin, token, "request", { id });
  ensure(stands.state === state && stands.by === admin && stands.request === id, `request ${id} is told as ${JSON.stringify(stands)}, not ${state} by ${admin}`);
  const first = await pending;
  ensure(
    first.request === id && ["waiting", "approved", state].includes(first.state),
    `the ask of request ${id} was told ${JSON.stringify(first)}`,
  );
}

/** The card of the request that the client `client` of `owner` has waiting. */
const waitingCard = (page, client, owner) =>
  card(page, "cl-waiting").locator(".ask").filter({ hasText: `Asked by ${client}, ${owner}'s client` });

/** The request of `client` of `owner` that waits, as the console lists it, once it does. */
async function waitingRequest(page, client, owner) {
  const mine = (list) => list.requests.filter((request) => request.client === client && request.owner === owner);
  const found = await becomes(async () => mine(await read(page, ROUTES.requests)), (list) => list.length === 1, `the request of ${client} to wait`);
  return found[0];
}

/** The newest rows of the journal that `how` made, as `GET /api/1/changes` lists them. */
async function rowsBy(page, how) {
  const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=100");
  ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
  return list.json.changes.filter((row) => row.how === how);
}

// --- The dialog that adds a client ---

/** Opens Add a client... and gives its dialog. */
async function openAdd(t, page) {
  await page.getByRole("button", { name: "Add a client…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add an AI client", exact: true });
  await t.see(dialog);
  return dialog;
}

/** The command the dialog says does the same at a terminal, on one line. */
async function commandOf(dialog) {
  const text = await dialog.getByRole("group", { name: "The command that adds this client", exact: true }).locator("pre").textContent();
  return text.replace(/ \\\n\s+/g, " ");
}

/** Presses Add the client, which the dialog must refuse with `words` and send nothing for. */
async function refusedName(t, page, dialog, name, words) {
  const sent = watchRoute(page, ROUTES.add);
  await dialog.getByLabel("Name", { exact: true }).fill(name);
  await dialog.getByRole("button", { name: "Add the client", exact: true }).click();
  await t.see(dialog.getByRole("alert").filter({ hasText: words }));
  ensure(sent.count() === 0, `the dialog sent ${ROUTES.add} for the name "${name}"`);
  sent.stop();
}

/** Adds lab-ssh as the dialog opens: over SSH, reading everything. The console is locked, so it asks to unlock first. */
async function addOverSsh(t, page, admin) {
  const dialog = await openAdd(t, page);
  await refusedName(t, page, dialog, "", NO_NAME);
  await refusedName(t, page, dialog, "Lab Client", BAD_NAME);
  await dialog.getByLabel("Name", { exact: true }).fill(OVER_SSH);
  const command = await commandOf(dialog);
  ensure(command === `admin mcp add ${OVER_SSH} --scope '*:read' --days 90`, `the dialog's command for ${OVER_SSH} is "${command}"`);
  await look(t, page, "clients-add");

  const answered = answerTo(page, pathOf(ROUTES.add));
  await dialog.getByRole("button", { name: "Add the client", exact: true }).click();
  await unlockHere(t, page, `add the AI client ${OVER_SSH}`);
  const answer = await answered;
  ensure(answer.status() === 200, `adding ${OVER_SSH} was answered ${answer.status()}: ${await answer.text()}`);
  ensure((await answer.json()).token === null, `a client that connects over SSH was given a token`);

  // What to put in the client's settings: nothing in it is secret.
  const added = page.getByRole("dialog", { name: `${OVER_SSH} is added`, exact: true });
  await t.see(added);
  const host = new URL(page.url()).hostname.replace(/^\[(.*)\]$/, "$1");
  const settings = JSON.parse(await added.getByRole("group", { name: "MCP settings for the AI client", exact: true }).locator("pre").textContent());
  const servers = Object.values(settings.mcpServers ?? {});
  ensure(
    servers.length === 1 && servers[0].command === "ssh" && isDeepStrictEqual(servers[0].args, [`${admin}@${host}`, "admin", "mcp", OVER_SSH]),
    `the settings of ${OVER_SSH} are ${JSON.stringify(settings)}`,
  );
  const atTerminal = await added.getByRole("group", { name: "A check from a terminal", exact: true }).locator("pre").textContent();
  ensure(atTerminal === `ssh ${admin}@${host} admin mcp ${OVER_SSH} --check`, `the check of ${OVER_SSH} is "${atTerminal}"`);
  await look(t, page, "clients-added");
  await added.getByRole("button", { name: "Done", exact: true }).click();
  await t.gone(added);
}

/** Adds lab-https: over HTTPS, reading everything and with the services whole. Gives its token, which the page shows once. */
async function addOverHttps(t, page, admin, services) {
  const dialog = await openAdd(t, page);
  await refusedName(t, page, dialog, OVER_SSH, `You already have a client named ${OVER_SSH}. Remove it first, or choose another name.`);
  await dialog.getByLabel("Name", { exact: true }).fill(OVER_HTTPS);
  await dialog.locator("label.radio-card").filter({ hasText: "Over HTTPS, with a token" }).click();
  ensure(await dialog.getByRole("radio", { name: /^Over HTTPS, with a token/ }).isChecked(), "Over HTTPS, with a token is not chosen after its press");
  // The most the services have, which is the last of their levels; the start is then Choose.
  const levels = dialog.getByRole("group", { name: services.title, exact: true }).getByRole("button");
  await levels.last().click();
  ensure((await levels.last().getAttribute("aria-pressed")) === "true", `the last level of ${services.title} is not pressed`);
  const choose = dialog.getByRole("group", { name: "Start from", exact: true }).getByRole("button", { name: "Choose", exact: true });
  ensure((await choose.getAttribute("aria-pressed")) === "true", "Choose is not pressed after a level was");
  const command = await commandOf(dialog);
  ensure(
    command === `admin mcp add ${OVER_HTTPS} --https --scope '*:read' --scope services --days 90`,
    `the dialog's command for ${OVER_HTTPS} is "${command}"`,
  );

  // The console is unlocked from the first client, so it asks nothing.
  const answered = answerTo(page, pathOf(ROUTES.add));
  await dialog.getByRole("button", { name: "Add the client", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `adding ${OVER_HTTPS} was answered ${answer.status()}: ${await answer.text()}`);

  const added = page.getByRole("dialog", { name: `${OVER_HTTPS} is added`, exact: true });
  await t.see(added);
  await t.see(added.getByText("The token is shown once.", { exact: true }));
  const settings = JSON.parse(await added.locator("pre.cmdtext").textContent());
  const servers = Object.values(settings.mcpServers ?? {});
  const token = servers[0]?.headers?.Authorization?.replace(/^Bearer /, "") ?? "";
  ensure(
    servers.length === 1 && servers[0].type === "http" && servers[0].url === `${originOf(page)}${pathOf(ROUTES.mcp)}` && token.startsWith(`dkc_${admin}_`),
    `the settings of ${OVER_HTTPS} name ${servers[0]?.url} and a token that is not ${admin}'s client's`,
  );
  // Checked, and not kept as a picture: it shows the token.
  await t.axe(page);
  await added.getByRole("button", { name: "I have copied the token", exact: true }).click();
  await t.gone(added);
  ensure(!(await page.content()).includes(token), "the token is still in the page after its dialog was closed");
  return token;
}

// --- What an administrator does ---

/** The second administrator lets in a client of their own, by the page's route, and leaves. */
async function anothersClient(t, other) {
  const { page } = await openConsole(t, "/");
  await signInDone(t, page, other, PASSWORD);
  await unlock(page);
  // A run before this one may have left it.
  if ((await read(page, ROUTES.own)).clients.some((client) => client.name === OTHERS)) await takeAway(page, OTHERS);
  await letIn(page, OTHERS, "ssh", ["*:read"]);
  await signOut(t, page);
  await page.context().close();
}

/** The page of an administrator who has let in no client, with the console locked. */
async function nothingYet(t, page, admin) {
  await openPage(t, page);
  for (const [id, heading] of [["cl-waiting", "Waiting for a yes"], ["cl-clients", "Clients"], ["cl-did", "What they did"], ["cl-never", "What no client can do"]]) {
    await t.see(card(page, id).getByRole("heading", { name: heading, level: 2 }));
  }
  ensure((await card(page, "cl-never").locator("li").count()) === 4, "What no client can do has not its four rules");
  await t.see(card(page, "cl-clients").getByText(OWN_ONLY));
  ensure((await rowsShown(page)).length === 0, `${admin} has let in no client, and the page lists ${await rowsShown(page)}`);
  ensure(isDeepStrictEqual(await read(page, ROUTES.own), { clients: [] }), `${ROUTES.own} lists clients of ${admin}, who has let in none`);
  mustBe(await fetchAsPage(page, "GET", pathOf(ROUTES.all)), 403, "ADM-106", "every administrator's clients, locked");
  await fitsWindow(page, "the page with no client");
  await lookAll(t, page, "clients-none");
}

/** The request the client makes to restart the service: its card, held to what the console lists and shows, and the count beside the page's name. */
async function requestShown(t, page, admin) {
  const request = await waitingRequest(page, OVER_HTTPS, admin);
  const box = waitingCard(page, OVER_HTTPS, admin);
  await t.see(box);
  ensure(
    request.action === "service.restart" && request.area === "services" && isDeepStrictEqual(request.args, { name: SERVICE }) && request.reason === REASON,
    `the request is listed as ${JSON.stringify(request)}`,
  );
  const label = await box.getAttribute("aria-label");
  ensure(label === `Request ${request.request}: ${request.title}`, `the card is named "${label}", and the request is ${request.request}, ${request.title}`);
  await t.see(box.locator(".said").getByText(REASON, { exact: true }));

  const shown = await fetchAsPage(page, "POST", "/api/1/show", { action: request.action, args: request.args });
  ensure(shown.status === 200 && typeof shown.json?.command === "string", `POST /api/1/show for ${request.action} was answered ${shown.status}: ${shown.text}`);
  const command = await box.getByRole("group", { name: "The command this runs", exact: true }).locator("pre").textContent();
  ensure(
    command === request.command && command === shown.json.command && command.includes(SERVICE),
    `the card's command is "${command}", the request's "${request.command}" and POST /api/1/show's "${shown.json.command}"`,
  );
  // A restart always changes something, which the page asks POST /api/1/check.
  const facts = box.locator("dl.rows dd");
  await becomes(() => facts.nth(0).textContent(), (words) => words === "Yes.", "what the card says a restart changes");
  const files = shown.json.files.length === 0 ? "None" : shown.json.files.join(", ");
  ensure((await facts.nth(1).textContent()) === files, `the card says the restart writes "${await facts.nth(1).textContent()}", not "${files}"`);

  const waiting = (await read(page, ROUTES.requests)).requests.length;
  await becomes(() => card(page, "cl-waiting").locator("h2 .pill").textContent(), (count) => count === String(waiting), "the count beside Waiting for a yes");
  await becomes(
    () => page.locator('a.nav[href="/clients"]').first().getAttribute("aria-label"),
    (name) => name === `AI clients, ${waiting} waiting for a yes`,
    "the name of the navigation's link while a request waits",
  );
  return request;
}

/** Approve runs the restart as the administrator: the notice, the journal, the page's history and what the client is told. */
async function approved(t, page, admin, request, pending, { origin, token }) {
  const how = `client ${OVER_HTTPS} of ${admin}`;
  // A run before this one on the same machine left rows of its own.
  const before = new Set((await rowsBy(page, how)).map((row) => row.id));
  const box = waitingCard(page, OVER_HTTPS, admin);
  const answered = answerTo(page, pathOf(routeOf(ROUTES.approve, request.request)));
  await box.getByRole("button", { name: "Approve", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200 && (await answer.json()).done === true, `the yes was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(noticeOf(page, `${request.title}: done.`));
  await t.gone(box);
  await toldAs(origin, token, pending, request.request, "done", admin);

  const rows = await becomes(
    async () => (await rowsBy(page, how)).filter((row) => !before.has(row.id)),
    (found) => found.length > 0,
    `a row of the journal made by the ${how}`,
  );
  ensure(
    rows.length === 1 && rows[0].action === "service.restart" && rows[0].who === admin && rows[0].status === 0,
    `the journal's rows of the ${how} are ${JSON.stringify(rows)}`,
  );
  const line = card(page, "cl-did").locator(".tl-item").filter({ hasText: `${OVER_HTTPS} asked to ${request.title[0].toLowerCase()}${request.title.slice(1)}` }).first();
  await t.see(line);
  await t.see(line.getByText(`Approved by ${admin}`, { exact: false }));
}

/** With the console locked: the list is the administrator's own, Approve asks to unlock and a cancel sends nothing, and Refuse needs no unlock. */
async function refusedLocked(t, page, admin, { origin, token }) {
  const locked = await press(page, headerButton(page, "Lock"), "/api/1/lock");
  ensure(locked.status() === 200, `the lock was answered ${locked.status()}`);
  await t.see(page.getByRole("table", { name: TABLE_OWN, exact: true }));
  await tableShows(page, [`${admin}/${OVER_SSH}`, `${admin}/${OVER_HTTPS}`], `the locked page to list ${admin}'s own clients alone`);
  await t.see(card(page, "cl-clients").getByText(OWN_ONLY));

  const pending = asks(origin, token, "service.stop", "To see what a no does.");
  const request = await waitingRequest(page, OVER_HTTPS, admin);
  const box = waitingCard(page, OVER_HTTPS, admin);
  await t.see(box);
  const approve = pathOf(routeOf(ROUTES.approve, request.request));
  const sent = watchRoute(page, `POST ${approve}`);
  await box.getByRole("button", { name: "Approve", exact: true }).click();
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText(`approve request ${request.request}`, { exact: true }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  ensure(sent.count() === 0, "a cancelled unlock still sent the yes");
  sent.stop();
  mustBe(await fetchAsPage(page, "POST", approve, {}), 403, "ADM-106", "a yes from a locked console");

  const answered = answerTo(page, pathOf(routeOf(ROUTES.refuse, request.request)));
  await box.getByRole("button", { name: "Refuse", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `the no was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(noticeOf(page, `Request ${request.request} is refused. Nothing was changed.`));
  await t.gone(box);
  await toldAs(origin, token, pending, request.request, "refused", admin);
  const rows = await rowsBy(page, `client ${OVER_HTTPS} of ${admin}`);
  ensure(rows.every((row) => row.action !== "service.stop"), `the journal has a stop that nobody approved: ${JSON.stringify(rows)}`);
  // A request that does not wait has no yes and no no.
  mustBe(await fetchAsPage(page, "POST", pathOf(routeOf(ROUTES.refuse, request.request)), {}), 404, "ADM-145", "a second no");
}

/** Unlocked from the list: every administrator's clients, as console.clients answers, with its command; and the second administrator's removed by console.client-remove. */
async function everyClient(t, page, admin, other) {
  await card(page, "cl-clients").locator(".cl-own").getByRole("button", { name: "Unlock", exact: true }).click();
  await unlockHere(t, page, "see every administrator's clients");
  await t.see(page.getByRole("table", { name: TABLE_ALL, exact: true }));
  const all = namesOf(await read(page, ROUTES.all));
  for (const client of [`${admin}/${OVER_SSH}`, `${admin}/${OVER_HTTPS}`, `${other}/${OTHERS}`]) {
    ensure(all.includes(client), `${ROUTES.all} does not list ${client}: ${all}`);
  }
  await tableShows(page, all, `the unlocked page to list what ${ROUTES.all} answers, ${all}`);
  const listed = await fetchAsPage(page, "POST", "/api/1/show", { action: "console.clients", args: {} });
  ensure(listed.status === 200 && listed.json?.command.includes("admin mcp list --all"), `POST /api/1/show for console.clients was answered ${listed.status}: ${listed.text}`);
  await t.see(card(page, "cl-clients").getByRole("button", { name: `Show the command: ${listed.json.command}`, exact: true }));
  await fitsWindow(page, "the page with every administrator's clients");
  await lookAll(t, page, "clients-all");

  // Another's client: Remove names whose it is, shows the command and runs the action.
  const named = `${other}'s ${OTHERS}`;
  await page.getByRole("button", { name: `Remove the client ${named}`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Remove ${named}?`, exact: true });
  await t.see(dialog);
  const shown = await fetchAsPage(page, "POST", "/api/1/show", { action: "console.client-remove", args: { owner: other, name: OTHERS } });
  ensure(shown.status === 200 && shown.json?.command.includes(`admin mcp remove ${OTHERS}`), `POST /api/1/show for console.client-remove was answered ${shown.status}: ${shown.text}`);
  await t.see(dialog.getByRole("heading", { name: "This runs", exact: true }));
  await t.see(dialog.locator("pre.cmdtext").filter({ hasText: shown.json.command }).first());
  await look(t, page, "clients-remove");
  const answered = answerTo(page, pathOf(ofClient(other, OTHERS)));
  await dialog.getByRole("button", { name: "Remove", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `console.client-remove was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(noticeOf(page, `${OTHERS} is removed.`));
  await t.gone(dialog);
  await tableShows(page, all.filter((client) => client !== `${other}/${OTHERS}`), `the page to list no ${named}`);
  ensure(!namesOf(await read(page, ROUTES.all)).includes(`${other}/${OTHERS}`), `${ROUTES.all} still lists ${named}`);
}

/** The administrator's own are removed by their own route, one with its Remove; the token then lets nothing in. */
async function ownRemoved(t, page, admin, { origin, token }) {
  const ping = { jsonrpc: "2.0", id: 1, method: "ping" };
  const before = await send(origin, token, ping);
  ensure(before.status === 200, `a ping of ${OVER_HTTPS} before it is removed was answered ${before.status}: ${before.text}`);

  await page.getByRole("button", { name: `Remove the client ${OVER_HTTPS}`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Remove ${OVER_HTTPS}?`, exact: true });
  await t.see(dialog);
  const answered = answerTo(page, pathOf(ROUTES.remove));
  await dialog.getByRole("button", { name: "Remove", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `removing ${OVER_HTTPS} was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(noticeOf(page, `${OVER_HTTPS} is removed.`));
  await t.gone(dialog);

  const after = await send(origin, token, ping);
  ensure(after.status === 401 && after.json?.error?.id === "ADM-143", `the token of a removed client was answered ${after.status}: ${after.text}`);
  await takeAway(page, OVER_SSH);
  ensure(isDeepStrictEqual(await read(page, ROUTES.own), { clients: [] }), `${admin} still has clients after removing both`);
}

/** An administrator: from no client to two, a request approved and one refused, every administrator's clients, and none again. */
async function administrator(t) {
  const [admin, other] = [adminOf(t.engine), otherOf(t.engine)];
  await anothersClient(t, other);

  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, admin, PASSWORD);
  const shown = await fetchAsPage(page, "POST", "/api/1/account/preferences", { commands: "shown" });
  ensure(shown.status === 200, `Show commands was not kept: the console answered ${shown.status}: ${shown.text}`);
  // A run before this one may have left clients: the account starts with none.
  await unlock(page);
  for (const client of (await read(page, ROUTES.own)).clients) await takeAway(page, client.name);
  const locked = await fetchAsPage(page, "POST", "/api/1/lock", {});
  ensure(locked.status === 200, `the lock was answered ${locked.status}: ${locked.text}`);

  await nothingYet(t, page, admin);
  await addOverSsh(t, page, admin);
  const services = (await read(page, ROUTES.areas)).areas.find((area) => area.area === "services");
  ensure(services !== undefined && services.asks > 0, `the areas a client can be given have no services with changes to ask for`);
  const client = { origin: originOf(page), token: await addOverHttps(t, page, admin, services) };
  await tableShows(page, namesOf(await read(page, ROUTES.all)), "the page to list the clients that were added");
  const own = (await read(page, ROUTES.own)).clients.find((one) => one.name === OVER_HTTPS);
  ensure(
    own?.way === "https" && isDeepStrictEqual([...own.scopes].sort(), ["*:read", "services"]) && own.used === 0,
    `${OVER_HTTPS} is listed as ${JSON.stringify(own)}`,
  );
  ensure(!JSON.stringify(own).includes(client.token.split("_").pop()), `the list of ${admin}'s clients holds the secret of a token`);

  // The lab PC is the client: what it reaches, a read, and a change that waits.
  const reached = await told(client.origin, client.token, "areas", {});
  const reach = reached.areas.find((area) => area.area === "services")?.reach;
  ensure(reach === "read all and ask", `${OVER_HTTPS} is told it reaches the services as "${reach}": ${JSON.stringify(reached)}`);
  const listed = await told(client.origin, client.token, "read", { action: "service.list" });
  ensure(
    listed.answer?.services?.some((service) => service.name === SERVICE),
    `the services the client read have no ${SERVICE}: ${JSON.stringify(listed).slice(0, 400)}`,
  );
  const pending = asks(client.origin, client.token, "service.restart", REASON);
  const request = await requestShown(t, page, admin);
  await lookAll(t, page, "clients-waiting");
  await approved(t, page, admin, request, pending, client);

  await refusedLocked(t, page, admin, client);
  await everyClient(t, page, admin, other);
  await ownRemoved(t, page, admin, client);

  const hidden = await fetchAsPage(page, "POST", "/api/1/account/preferences", { commands: "hidden" });
  ensure(hidden.status === 200, `Show commands was not put back: the console answered ${hidden.status}: ${hidden.text}`);
  mustHaveCalled(seen, [
    ROUTES.areas,
    ROUTES.requests,
    ROUTES.history,
    ROUTES.own,
    ROUTES.add,
    ROUTES.remove,
    ROUTES.all,
    ofClient(other, OTHERS),
    routeOf(ROUTES.approve, request.request),
    "POST /api/1/check",
    "POST /api/1/show",
  ]);
  await signOut(t, page);
  await page.context().close();
}

// --- A reader, and the phone ---

/** A reader has no link to the page, is told whose it is, and is refused its routes but the list of their own. */
async function reader(t) {
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, READER, PASSWORD);
  ensure((await page.locator('a.nav[href="/clients"]').count()) === 0, "a reader's navigation has a link to AI clients");
  await page.goto(new URL("/clients", page.url()).href);
  await t.see(page.getByRole("heading", { name: "AI clients", level: 1, exact: true }));
  await t.see(page.getByText(FOR_ADMINISTRATORS, { exact: true }));
  await t.see(page.getByRole("link", { name: "Who are the administrators?", exact: true }));
  ensure((await page.locator("main button").count()) === 0, "a reader's AI clients page has a button");
  for (const route of [ROUTES.areas, ROUTES.requests, ROUTES.history, ROUTES.all]) {
    mustBe(await fetchAsPage(page, "GET", pathOf(route)), 403, "ADM-122", `a reader's ${route}`);
  }
  for (const route of [routeOf(ROUTES.approve, 1), routeOf(ROUTES.refuse, 1)]) {
    mustBe(await fetchAsPage(page, "POST", pathOf(route), {}), 403, "ADM-122", `a reader's ${route}`);
  }
  // With the right password, and still no administrator.
  const added = await fetchAsPage(page, "POST", pathOf(ROUTES.add), { proof: PASSWORD, name: "lab-mine", way: "https", scopes: ["services:read"], days: 30 });
  ensure(added.status !== 200 && added.json?.error?.id !== undefined, `a reader's own client was answered ${added.status}: ${added.text}`);
  ensure(isDeepStrictEqual(await read(page, ROUTES.own), { clients: [] }), "a reader has a client");
  await look(t, page, "clients-reader");
  mustHaveCalled(seen, [ROUTES.own]);
  await signOut(t, page);
  await page.context().close();
}

/** The controls of the page that are under 44 px tall, which a thumb needs. */
const smallControls = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("main button, main a.btn")]
      .filter((element) => element.offsetParent !== null)
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.height < 43.5)
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );

/** At 390 wide: a client's card, a request that waits, which Refuse answers, and nothing that scrolls sideways or is too small for a thumb. */
async function phone(t) {
  const admin = adminOf(t.engine);
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  // The account menu is in the More sheet here, so the page is signed in when the tab bar is there.
  const answer = await signIn(page, admin, PASSWORD);
  ensure(answer.status() === 200, `${admin} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);

  // A client and a request of its own, made as a script makes them; the page is then opened locked.
  await unlock(page);
  const client = { origin: originOf(page), token: await letIn(page, ON_PHONE, "https", ["services"]) };
  ensure(typeof client.token === "string", `${ON_PHONE}, which connects over HTTPS, was given no token`);
  const locked = await fetchAsPage(page, "POST", "/api/1/lock", {});
  ensure(locked.status === 200, `the lock was answered ${locked.status}: ${locked.text}`);
  const pending = asks(client.origin, client.token, "service.stop", "To see a request on a phone.");
  const request = await waitingRequest(page, ON_PHONE, admin);

  await openPage(t, page);
  const box = waitingCard(page, ON_PHONE, admin);
  await t.see(box);
  await tableShows(page, [`${admin}/${ON_PHONE}`], "the phone to list the one client");
  await fitsWindow(page, "the phone's page");
  const small = await smallControls(page);
  ensure(small.length === 0, `these controls of the AI clients page are under 44 px tall on a phone: ${small.join(", ")}`);
  for (const name of ["Approve", "Refuse"]) {
    const { control, within } = await box.getByRole("button", { name, exact: true }).evaluate((element) => ({
      control: element.getBoundingClientRect().width,
      within: element.closest(".ask").getBoundingClientRect().width,
    }));
    ensure(control >= within - 40, `${name} is ${Math.round(control)} px wide in a card ${Math.round(within)} px wide`);
  }
  await lookAll(t, page, "clients-phone");

  const answered = answerTo(page, pathOf(routeOf(ROUTES.refuse, request.request)));
  await box.getByRole("button", { name: "Refuse", exact: true }).click();
  const refused = await answered;
  ensure(refused.status() === 200, `the phone's no was answered ${refused.status()}: ${await refused.text()}`);
  await t.gone(box);
  await toldAs(client.origin, client.token, pending, request.request, "refused", admin);
  await takeAway(page, ON_PHONE);

  mustHaveCalled(seen, [ROUTES.areas, ROUTES.requests, ROUTES.history, ROUTES.own, routeOf(ROUTES.refuse, request.request)]);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
