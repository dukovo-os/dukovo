// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/networking drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Networking
// pages (design 9): the cards at /networking, and the editor of one card at
// /networking/{card} (with the card's name for {card}). The page reads what
// `network show --json` says, which `GET /api/1/network` runs, and changes a
// card with `network set`, tried for a window that Spark undoes by itself
// unless the change is kept (`network keep`) or taken back at once (`network
// undo`), the way `POST /api/1/network/cards/{card}`, `POST
// /api/1/network/trial/keep` and `POST /api/1/network/trial/undo` run them.
//
// An administrator signs in and finds the cards held to what the machine says,
// the same page at the same time: each card's link, addresses and the card the
// console is reached through; the traffic with its range in the address; and
// the browser's own connection among the connections, marked "This browser".
// Change... asks to unlock first, and a cancel goes nowhere. The editor says
// what is wrong under its field, as the command's checks do, and sends nothing
// until every field is right.
//
// The changes that are tried are one a person keeps, one that Undo now takes
// back, one that Spark undoes at its time, and one that takes away the address
// the browser reaches the console by. The first has a second administrator's
// browser watch the banner count down and say that the change was kept, and
// the machine holds the second address when the window is over. The second
// ends at once. The third is begun by a call of the page's own route with a
// short window, and the three browsers (the one that began it, another
// administrator's and a reader's) all see it count down and be undone. The
// fourth warns before it is made (design 8.5), offers 5 and 10 minutes and no
// Apply without trying, and the console cannot be reached over it: the page
// says so, Keep is off, and the change is undone by itself within the window,
// with the page back at the console and the card as it was. The console asks
// whether the browser still reaches it with GET /api/1/console/fresh/{trial},
// with `network` for {trial}.
//
// A reader sees the cards and a card's settings, no Change..., and the banner
// of a change that an administrator tries without a button; a phone at 390 by
// 844 shows the cards and the editor with every control 44 px tall. Each look
// is the console's own: dark, light, high contrast, forced colours and less
// motion, each checked by axe (console.mjs's lookAll). The card the browser
// comes through is the one the script changes; the settings it had are put
// back by the test's last step, which also leaves the file as the image has it.
// The accounts are the three that networking.toml made, so no script needs the
// lab's password.

import {
  ensure,
  fetchAsPage,
  isAddress,
  leaveWelcome,
  look,
  lookAll,
  mustHaveCalled,
  openConsole,
  sessionOf,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import {
  PASSWORD,
  PHONE,
  WORDS,
  answerUnlock,
  bannerBecomes,
  bannerEnds,
  keepOf,
  pressAndAnswer,
  pressAndMaybeAnswer,
  reaches,
  reachesSoon,
  readBanner,
  readDocument,
  secondsOfClock,
  sendsNothing,
  smallControls,
  trialIn,
  undoneIn,
  undoneSince,
  undoOf,
  undoRowAfter,
  unlockDialog,
  unlockNow,
} from "./trial.mjs";

const ADMIN = "wc-networking";
const OTHER = "wc-networking-other";
const READER = "wc-networking-reader";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with the
// card's name for {card} and `network` for {trial}.
const ROUTES = {
  show: "GET /api/1/network", // network.show
  connections: "GET /api/1/network/connections",
  set: "POST /api/1/network/cards/{card}",
  keep: "POST /api/1/network/trial/keep",
  undo: "POST /api/1/network/trial/undo",
  fresh: "GET /api/1/console/fresh/{trial}",
};

const routeFor = (template, name) => template.replace("{card}", encodeURIComponent(name)).replace("{trial}", "network");

/** The path of a route as the table names it: `GET /api/1/network` is `/api/1/network`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

/** The path that changes the card `name`. */
const setPath = (name) => pathOf(routeFor(ROUTES.set, name));

// The words the page says where this test looks for them.
const BANNER_WORDS = (card) => `The network settings of ${card} are being tried.`;
const READER_LINE = "Administrators change the network settings.";
const BUSY_LINE = "Keep or undo the change being tried first.";

// --- Reading the machine ---

/** The document that `GET /api/1/network` answers: `network show --json`, `{cards, trial}`. */
async function readNetwork(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.show));
  ensure(answer.status === 200 && Array.isArray(answer.json?.cards), `${ROUTES.show} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Reads the card `name` until `test(card, show)` holds, for up to `seconds`, and gives it. */
async function cardBecomes(page, name, test, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const show = await readNetwork(page).catch(() => null);
    const card = show?.cards.find((each) => each.name === name);
    if (card !== undefined && test(card, show)) return card;
    ensure(Date.now() < until, `${name} did not ${what} within ${seconds} seconds: ${JSON.stringify(card?.settings)}, ${JSON.stringify(card?.addresses)}`);
    await sleep(1000);
  }
}

/** The addresses of a card as `ADDRESS/PREFIX`, in the order the machine says them. */
const heldOf = (card) => card.addresses.map((held) => `${held.address}/${held.prefix}`);

/** An address in the network of `address` that no machine of the lab's network is likely to have: its last number 99, or 98 when that is its own. */
function spareOf(address) {
  const parts = address.split(".");
  ensure(parts.length === 4, `the console is reached at ${address}, which is not an IPv4 address: the test needs one`);
  parts[3] = parts[3] === "99" ? "98" : "99";
  return parts.join(".");
}

/** The card the browser reaches the console through, with its IPv4 address and prefix, from the session and the machine. */
async function theCard(page) {
  const session = await sessionOf(page);
  ensure(typeof session.via?.card === "string" && typeof session.via?.address === "string", `GET /api/1/session does not say how the console is reached: ${JSON.stringify(session.via)}`);
  const show = await readNetwork(page);
  const card = show.cards.find((each) => each.name === session.via.card);
  ensure(card !== undefined, `the machine has no card ${session.via.card}, which the session says the console is reached through`);
  const held = card.addresses.find((each) => each.address === session.via.address);
  ensure(held !== undefined, `${card.name} has ${heldOf(card).join(", ")}, not ${session.via.address}, which the session says the console is reached at`);
  const router = card.routers.find((one) => !one.includes(":")) ?? null;
  const dns = card.dns.filter((one) => !one.includes(":")).slice(0, 3);
  return { session, card, address: held.address, prefix: held.prefix, spare: spareOf(held.address), router, dns };
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The cards ---

/** Opens the list of cards and waits for the first card to be drawn. */
async function openCards(t, page, address = "/networking") {
  await page.goto(new URL(address, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Networking", level: 1, exact: true }));
  await t.see(page.locator(".net-card").first());
}

/** The cards as drawn, in order: name, state, whether the console comes through it, its link, its addresses and its other facts. */
const cardsShown = (page) =>
  page.locator(".net-card").evaluateAll((sections) =>
    sections.map((section) => {
      const facts = Object.fromEntries([...section.querySelectorAll("dl.net-rows dt")].map((dt) => [dt.textContent.trim(), dt.nextElementSibling]));
      return {
        name: section.dataset.card,
        state: section.querySelector("h2 .st")?.textContent.trim() ?? "",
        through: section.querySelector(".net-through") !== null,
        href: section.querySelector("h2 a")?.getAttribute("href") ?? null,
        addresses: [...(facts.Addresses?.querySelectorAll(".mono") ?? [])].map((one) => one.textContent.trim()),
        change: section.querySelector("a.btn")?.getAttribute("href") ?? null,
      };
    }),
  );

/** The cards say what the machine says: the same cards in the same order, each with its state, addresses and link, and the one the console comes through marked. */
async function cardsAsAnswered(t, page, session) {
  const before = await readNetwork(page);
  const shown = await cardsShown(page);
  const after = await readNetwork(page);
  const names = (show) => JSON.stringify(show.cards.map((card) => card.name));
  ensure(JSON.stringify(shown.map((card) => card.name)) === names(before) || JSON.stringify(shown.map((card) => card.name)) === names(after), `the page shows ${shown.map((card) => card.name).join(", ")} and ${ROUTES.show} gives ${JSON.parse(names(before)).join(", ")}`);
  for (const card of shown) {
    const entries = [before, after].map((show) => show.cards.find((each) => each.name === card.name)).filter((each) => each !== undefined);
    ensure(entries.length > 0, `the page shows ${card.name}, which the machine does not list`);
    ensure(entries.some((entry) => JSON.stringify(heldOf(entry)) === JSON.stringify(card.addresses)), `${card.name} shows ${card.addresses.join(", ")} and the machine has ${entries.map((entry) => heldOf(entry).join(", ")).join(" and then ")}`);
    ensure(entries.some((entry) => card.state.startsWith(entry.up ? "Connected" : "No cable")), `${card.name} shows "${card.state}" and the machine says up is ${entries.map((entry) => entry.up).join(", then ")}`);
    ensure(card.href === `/networking/${encodeURIComponent(card.name)}`, `the link of ${card.name} is ${card.href}`);
    ensure(card.through === (card.name === session.via.card), `${card.name} ${card.through ? "says" : "does not say"} that the console comes through it, and the session says it comes through ${session.via.card}`);
  }
  return shown;
}

/** The traffic card: its heading, the range choice that puts itself in the address, and a chart or the words that there is no history. */
async function trafficShown(t, page) {
  await t.see(page.getByRole("heading", { level: 2, name: /^Traffic/ }));
  const radios = page.getByRole("radiogroup", { name: "Range", exact: true }).getByRole("radio");
  ensure((await radios.count()) >= 2, `the traffic's range has ${await radios.count()} choices`);
  const other = radios.nth(1);
  await other.click();
  ensure((await other.getAttribute("aria-checked")) === "true", "the traffic's range does not mark the choice that was pressed");
  await page.waitForFunction(() => new URLSearchParams(location.search).has("range"), null, { timeout: 15_000 });
  await page.waitForFunction(
    () => document.querySelector(".net-traffic .net-chart") !== null || (document.querySelector(".net-traffic [role=status]")?.textContent ?? "").trim() !== "",
    null,
    { timeout: 30_000 },
  );
}

/** The browser's own connection is among the connections, as the machine's `ss` has it. */
async function connectionsShown(t, page, session) {
  const row = page.locator("table.net-conn tbody tr", { hasText: "This browser" }).first();
  await t.see(row);
  const local = (await row.locator("th").textContent()).trim();
  const remote = (await row.locator("td").first().textContent()).trim();
  ensure(local.endsWith(`:${session.via.port}`), `the browser's connection is on ${local}, and the console is on port ${session.via.port}`);
  ensure(remote.includes(session.from), `the browser's connection is from ${remote}, and the session says ${session.from}`);
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.connections));
  ensure(answer.status === 200 && Array.isArray(answer.json), `${ROUTES.connections} was answered ${answer.status}: ${answer.text}`);
  ensure(
    answer.json.some((one) => one.local?.port === session.via.port && one.remote?.address === session.from),
    `${ROUTES.connections} has no connection from ${session.from} to port ${session.via.port}`,
  );
}

// --- The editor ---

const dhcpRadio = (page) => page.getByRole("radio", { name: /^From the router \(DHCP\)/ });
const fixedRadio = (page) => page.getByRole("radio", { name: /^Fixed/ });
const addressFields = (page) => page.locator(".net-address input");
const tryButton = (page) => page.getByRole("button", { name: "Try it", exact: true });
const applyButton = (page) => page.getByRole("button", { name: "Apply without trying", exact: true });
const windowSelect = (page) => page.getByLabel("Undo by itself unless kept within", { exact: true });
const cutWarning = (page) => page.locator(".net-cut .banner");

/** Opens the editor of `card` and waits for its form. */
async function openEditor(t, page, card) {
  await page.goto(new URL(`/networking/${encodeURIComponent(card)}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: card, exact: true }));
  await t.see(page.getByRole("heading", { level: 2, name: "Change the settings", exact: true }));
}

/** Opens the editor of `card` as a reader sees it: the settings and the line that says who may change them. */
async function openReaderEditor(t, page, card) {
  await page.goto(new URL(`/networking/${encodeURIComponent(card)}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: card, exact: true }));
  await t.see(page.getByText(READER_LINE, { exact: true }));
}

/** Chooses Fixed and has exactly the addresses `wanted` in the fields. */
async function setAddresses(page, wanted) {
  await fixedRadio(page).check();
  const fields = addressFields(page);
  while ((await fields.count()) > 1) await page.getByRole("button", { name: /^Take away address/ }).last().click();
  await fields.first().fill(wanted[0]);
  for (const text of wanted.slice(1)) {
    await page.getByRole("button", { name: "Add another", exact: true }).click();
    await fields.last().fill(text);
  }
}

/** The sentence under the address field number `at` (from 0), or "". */
const sentenceOf = (page, at) => page.locator(".net-address").nth(at).locator(".msg").textContent().then((text) => text.trim());

/** The editor's checks are the command's: a wrong value is said under its field, nothing is sent, and a right one clears the sentence. */
async function editorChecks(t, page, where) {
  const route = setPath(where.card.name);
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === route) sent += 1;
  };
  page.on("request", count);
  const cases = [
    [`${where.address}/33`, `${where.address}/33: an IPv4 prefix is 1 to 32`],
    [where.address, `${where.address} has no prefix: write it as ADDRESS/PREFIX, as 192.168.1.10/24`],
    ["300.1.1.1/24", "300.1.1.1 is not an IP address"],
  ];
  for (const [value, sentence] of cases) {
    await setAddresses(page, [value]);
    await tryButton(page).click();
    await t.see(page.locator(".net-address").first().locator(".msg", { hasText: sentence }));
    ensure((await sentenceOf(page, 0)) === sentence, `the sentence under the address ${value} is "${await sentenceOf(page, 0)}", not "${sentence}"`);
    ensure((await addressFields(page).first().getAttribute("aria-invalid")) === "true", `the address ${value} is not marked as wrong`);
    ensure(await addressFields(page).first().evaluate((input) => input === document.activeElement), `focus is not on the address ${value}`);
  }
  // A right value takes the sentence away.
  await addressFields(page).first().fill(`${where.address}/${where.prefix}`);
  ensure((await sentenceOf(page, 0)) === "", "the sentence stays under an address that is right");
  // The router that is on no network of the card's.
  await page.getByLabel("Router (optional)", { exact: true }).fill("192.168.9.1");
  await tryButton(page).click();
  await t.see(page.locator("#net-router-error", { hasText: "the router 192.168.9.1 is on none of the networks of the card's addresses" }));
  await page.getByLabel("Router (optional)", { exact: true }).fill("");
  await sleep(500);
  page.off("request", count);
  ensure(sent === 0, `the editor sent ${sent} changes while a field was wrong`);
  // The page is left as it was found.
  await page.reload();
  await t.see(page.getByRole("heading", { level: 2, name: "Change the settings", exact: true }));
}

/** The command that `POST /api/1/show` answers for the change names `network set` and the card. */
async function commandOfChange(page, where) {
  const args = { card: where.card.name, mode: "fixed", addresses: `${where.address}/${where.prefix}`, ...(where.router === null ? {} : { router: where.router }) };
  const shown = await fetchAsPage(page, "POST", "/api/1/show", { action: "network.set", args });
  ensure(shown.status === 200 && typeof shown.json?.command === "string", `POST /api/1/show for network.set was answered ${shown.status}: ${shown.text}`);
  ensure(shown.json.command.includes(`network set ${where.card.name} fixed`), `the command of network.set is "${shown.json.command}"`);
}

// --- The tried changes ---

/**
 * Fills the editor with Fixed, the card's own address and `extra`, and has
 * nothing more to do than press Try it.
 */
async function fillSecondAddress(t, page, where) {
  await setAddresses(page, [`${where.address}/${where.prefix}`, `${where.spare}/${where.prefix}`]);
  await t.gone(cutWarning(page));
  await t.see(applyButton(page));
  ensure((await windowSelect(page).inputValue()) === "60", `the window starts at ${await windowSelect(page).inputValue()} seconds, not 60`);
}

/** The banner of a trial that this page began: its words and clock, "Applied", the reach line and Keep on once reached. */
async function beganHere(t, page, where, seconds) {
  const shown = await bannerBecomes(page, (banner) => banner.running && banner.heading === BANNER_WORDS(where.card.name), "show the change being tried");
  ensure(shown.applied, `the banner of a change made here does not say ${WORDS.applied}: ${JSON.stringify(shown)}`);
  const left = secondsOfClock(shown.clock);
  ensure(left !== null && left <= seconds && left > seconds - 20, `the clock says ${shown.clock} for a window of ${seconds} seconds`);
  ensure(/^The change is undone at \d{1,2}:\d\d [AP]M unless it is kept\./.test(shown.words), `the banner says "${shown.words}" about when the change is undone`);
  ensure(shown.polite !== "", "the banner's polite region says nothing when the change begins");
  ensure(await page.locator(".net-form fieldset.net-sheet").evaluate((sheet) => sheet.disabled), "the form is still on while a change is tried");
  await t.see(page.getByText(BUSY_LINE, { exact: true }));
  return shown;
}

/** One administrator tries a second address and another watches: the first reaches the console, keeps the change, and the second sees it kept. */
async function keptChange(t, a, b, where) {
  const { card, spare } = where;
  await unlockNow(a.page);
  await openEditor(t, a.page, card.name);
  await openCards(t, b.page);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "network") === null, "a trial runs before the test's own begins");
  const before = undoneIn(document, "network");

  await fillSecondAddress(t, a.page, where);
  await pressAndAnswer(t, a.page, tryButton(a.page), setPath(card.name));
  const beganAt = Date.now();
  await beganHere(t, a.page, where, 60);
  // The page asks whether it reaches the console over the new settings, and Keep is on once it has.
  const reached = await bannerBecomes(a.page, (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 40);
  ensure(reached.applied, "the banner lost its Applied step");
  await look(t, a.page, "networking-trial");

  // Another administrator's page shows the banner, who began it and the clock, and offers Keep and Undo now without having asked about the reach.
  const other = await bannerBecomes(b.page, (banner) => banner.running && banner.heading === BANNER_WORDS(card.name), "show the change another administrator is trying", 30);
  ensure(other.words.includes(`Begun by ${ADMIN} in this console.`), `the banner of another administrator's page says "${other.words}", not that ${ADMIN} began it`);
  ensure(secondsOfClock(other.clock) !== null, `the other page's clock says "${other.clock}"`);
  ensure(!other.applied && other.reach === "", `a page that did not begin the change shows ${JSON.stringify(other)}`);
  ensure(other.buttons.includes("Keep this change") && other.buttons.includes("Undo now"), `the other administrator's banner has ${other.buttons.join(", ")}`);
  ensure(trialIn(await readDocument(a.page), "network") !== null, "Spark says no change is tried while the page shows one");
  const held = await cardBecomes(a.page, card.name, (each) => heldOf(each).includes(`${spare}/${where.prefix}`), "have the second address");
  ensure(held.settings.mode === "fixed", `the card is ${held.settings.mode} while its second address is tried`);

  await pressAndAnswer(t, a.page, keepOf(a.page), pathOf(ROUTES.keep));
  await bannerEnds(a.page, "kept", 20);
  await bannerEnds(b.page, "kept", 30);
  await lookAll(t, a.page, "networking-kept");
  ensure(!(await a.page.locator(".net-form fieldset.net-sheet").evaluate((sheet) => sheet.disabled)), "the form stays off after the change is kept");

  // Kept: the window's end changes nothing.
  const kept = await readDocument(a.page);
  ensure(trialIn(kept, "network") === null && undoneSince(kept, "network", before) === 0, `Spark still tries the change or undid one after it was kept: ${JSON.stringify(kept.trials)}`);
  await sleep(Math.max(0, beganAt + 60_000 + 5000 - Date.now()));
  const still = await cardBecomes(a.page, card.name, () => true, "be read");
  ensure(heldOf(still).includes(`${spare}/${where.prefix}`) && still.settings.mode === "fixed", `${card.name} lost its second address after the window: ${JSON.stringify(heldOf(still))}`);
  ensure(undoneSince(await readDocument(a.page), "network", before) === 0, "Spark undid a change that was kept");
  await openCards(t, b.page);
  const listed = (await cardsShown(b.page)).find((one) => one.name === card.name);
  ensure(listed !== undefined && listed.addresses.includes(`${spare}/${where.prefix}`), `the cards do not show ${spare}/${where.prefix} on ${card.name}: ${JSON.stringify(listed)}`);
  await putBack(a.page, where);
}

/** Puts the card back to asking DHCP, as the test found it, and waits for it to have its address again. */
async function putBack(page, where) {
  await unlockNow(page);
  const answer = await fetchAsPage(page, "POST", setPath(where.card.name), { mode: "dhcp" });
  ensure(answer.status === 200, `putting ${where.card.name} back to DHCP was answered ${answer.status}: ${answer.text}`);
  await cardBecomes(
    page,
    where.card.name,
    (card) => card.settings.mode === "dhcp" && card.addresses.some((held) => held.address === where.address) && !heldOf(card).includes(`${where.spare}/${where.prefix}`),
    "be back with DHCP and its address",
    90,
  );
}

/** Undo now takes a change back at once, for the page that began it and for another's. */
async function undoneAtOnce(t, a, b, where) {
  const { card } = where;
  await unlockNow(a.page);
  await openEditor(t, a.page, card.name);
  await openCards(t, b.page);
  const before = undoneIn(await readDocument(a.page), "network");
  await fillSecondAddress(t, a.page, where);
  await pressAndAnswer(t, a.page, tryButton(a.page), setPath(card.name));
  await beganHere(t, a.page, where, 60);
  // The other page has seen the change before it is taken back, so that it can say it was undone.
  await bannerBecomes(b.page, (banner) => banner.running, "show the change another administrator is trying", 30);
  await pressAndAnswer(t, a.page, undoOf(a.page), pathOf(ROUTES.undo));
  await bannerEnds(a.page, "undone", 20);
  await bannerEnds(b.page, "undone", 30);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "network") === null, "a change is still tried after Undo now");
  ensure(undoneSince(document, "network", before) > 0, `Spark's document does not list the change Undo now took back: ${JSON.stringify(document.undone)}`);
  await cardBecomes(a.page, card.name, (each) => each.settings.mode === "dhcp" && !heldOf(each).includes(`${where.spare}/${where.prefix}`), "have its settings back", 60);
  // The form is on again, with the card's own values.
  ensure(!(await a.page.locator(".net-form fieldset.net-sheet").evaluate((sheet) => sheet.disabled)), "the form stays off after Undo now");
  await t.see(dhcpRadio(a.page));
  ensure(await dhcpRadio(a.page).isChecked(), "the form does not show the card's own settings after Undo now");
}

/**
 * A change that is begun by a call of the page's own route with a short
 * window, so that three browsers that did not press Try it see it count
 * down and Spark undo it.
 */
async function undoneByTimer(t, a, b, c, where) {
  const { card } = where;
  await unlockNow(a.page);
  await openCards(t, a.page);
  await openCards(t, b.page);
  await openReaderEditor(t, c.page, card.name);
  const before = undoneIn(await readDocument(a.page), "network");
  const args = {
    mode: "fixed",
    addresses: `${where.address}/${where.prefix} ${where.spare}/${where.prefix}`,
    ...(where.router === null ? {} : { router: where.router }),
    ...(where.dns.length === 0 ? {} : { dns: where.dns.join(" ") }),
    try: 20,
  };
  const begun = await fetchAsPage(a.page, "POST", setPath(card.name), args);
  ensure(begun.status === 200, `${ROUTES.set} for ${card.name} with a window of ${args.try} seconds was answered ${begun.status}: ${begun.text}`);
  for (const [who, page] of [["the administrator who began it", a.page], ["another administrator", b.page], ["a reader", c.page]]) {
    const banner = await bannerBecomes(page, (shown) => shown.running, `show ${who} the change that is tried`, 30);
    ensure(banner.heading.startsWith("The network settings"), `the banner of ${who} says "${banner.heading}"`);
    ensure(secondsOfClock(banner.clock) !== null, `the banner of ${who} has the clock "${banner.clock}"`);
  }
  const reader = await readBanner(c.page);
  ensure(reader.note === WORDS.reader && !reader.buttons.includes("Keep this change") && !reader.buttons.includes("Undo now"), `a reader's banner is ${JSON.stringify(reader)}`);
  await look(t, c.page, "networking-trial-reader");
  for (const page of [a.page, b.page, c.page]) await bannerEnds(page, "undone", 60);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "network") === null, "a change is still tried after its window");
  ensure(undoneSince(document, "network", before) > 0, `Spark's document does not list the change it undid: ${JSON.stringify(document.undone)}`);
  await cardBecomes(a.page, card.name, (each) => each.settings.mode === "dhcp" && !heldOf(each).includes(`${where.spare}/${where.prefix}`), "have its settings back", 60);
}

/**
 * A change that takes away the address the browser comes by (design 8.5): the
 * page warns before and offers the long windows and no Apply without trying;
 * after Try it the console is out of reach, the page says so and Keep is off,
 * whether or not the answer to Try came back before the address went; Spark
 * undoes the change within the window and the page is back, and so is the
 * other session's, which came by the same address.
 */
async function cutOff(t, a, b, where) {
  const { card, session } = where;
  const here = new URL(a.page.url());
  const url = `${here.origin}/networking`;
  await unlockNow(a.page);
  await openEditor(t, a.page, card.name);
  await openCards(t, b.page);
  const before = undoneIn(await readDocument(a.page), "network");
  const lastRow = await lastChange(a.page);
  const settings = JSON.stringify((await readNetwork(a.page)).cards.find((each) => each.name === card.name).settings);

  await setAddresses(a.page, [`${where.spare}/${where.prefix}`]);
  const warning = cutWarning(a.page);
  await t.see(warning);
  const words = (await warning.textContent()).replace(/\s+/g, " ");
  // A browser that came by an address is told to open the new one; one that came by a name, that the name finds it.
  const literal = isAddress(here.hostname);
  const newUrl = `${here.protocol}//${where.spare}${here.port === "" ? "" : `:${here.port}`}/networking`;
  if (literal) {
    ensure(words.includes(`${where.address}, which this change takes away`), `the warning does not say that ${where.address} is taken away: "${words}"`);
    ensure(words.includes(newUrl), `the warning does not give the new address ${newUrl}: "${words}"`);
  } else {
    ensure(words.includes(`${here.hostname}, which has ${where.address} now`), `the warning does not say that ${here.hostname} has ${where.address} now: "${words}"`);
  }
  await t.gone(applyButton(a.page));
  const labels = await windowSelect(a.page).locator("option").allTextContents();
  ensure(labels.includes("5 minutes") && labels.includes("10 minutes") && !labels.includes("60 seconds"), `the windows offered are ${labels.join(", ")}`);
  ensure((await windowSelect(a.page).inputValue()) === "300", `the window starts at ${await windowSelect(a.page).inputValue()} seconds, not 300`);
  await lookAll(t, a.page, "networking-cut-warning");

  await pressAndMaybeAnswer(t, a.page, tryButton(a.page), setPath(card.name));
  const began = await bannerBecomes(a.page, (banner) => banner.running && banner.heading === BANNER_WORDS(card.name), "show the change being tried");
  ensure(began.link === newUrl, `the banner links to ${began.link}, not to ${newUrl}`);
  // The console is not reached over the new settings: the page says so, and Keep is off and says why.
  const cannot = await bannerBecomes(a.page, (banner) => banner.reach.startsWith(WORDS.cannot) && banner.keepOff, "say that the console cannot be reached", 60);
  ensure(cannot.reason.startsWith(WORDS.cannot), `Keep's reason is "${cannot.reason}"`);
  ensure(await sendsNothing(a.page, keepOf(a.page), pathOf(ROUTES.keep)), "Keep sent a request while it said the console cannot be reached");
  ensure(!(await reaches(url)), "the lab PC reaches the console at its old address while the change is tried");
  await lookAll(t, a.page, "networking-cut");

  // Nothing is kept: Spark undoes it at its time, and the page finds the console again.
  await bannerEnds(a.page, "undone", 420);
  ensure(await reachesSoon(url, 60), "the lab PC does not reach the console again after the change was undone");
  const back = await cardBecomes(a.page, card.name, (each) => JSON.stringify(each.settings) === settings && each.addresses.some((held) => held.address === where.address), "be as it was", 90);
  ensure(JSON.stringify(back.settings) === settings, `${card.name}'s settings are ${JSON.stringify(back.settings)} after the undo, and were ${settings}`);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "network") === null && undoneSince(document, "network", before) > 0, `Spark's document after the undo: ${JSON.stringify(document.undone)}`);
  // The other session came by the same address, so it lost the console when
  // the change was made and never saw the trial: once the console is back, its
  // page shows the card as it was and no trial.
  await cardBecomes(b.page, card.name, (each) => JSON.stringify(each.settings) === settings, "be as it was for the other session", 90);
  ensure(!(await readBanner(b.page)).running, "the other session's page shows a trial after the undo");
  ensure((await undoRowAfter(a.page, lastRow)) !== null, "the journal has no row of the change that was undone");
  await lookAll(t, a.page, "networking-undone");
}

/** The number of the newest row of the journal, or 0. */
async function lastChange(page) {
  const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
  ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
  return list.json.changes.reduce((most, row) => Math.max(most, row.id), 0);
}

// --- The administrator, the reader and the phone ---

/** Change... asks to unlock first, a cancel goes nowhere, and the password opens the editor. */
async function changeAsksToUnlock(t, page, where) {
  const change = page.locator(`.net-card[data-card="${where.card.name}"]`).getByRole("link", { name: "Change…", exact: true });
  await t.see(change);
  ensure((await change.getAttribute("href")) === `/networking/${encodeURIComponent(where.card.name)}`, `Change... leads to ${await change.getAttribute("href")}`);
  await change.click();
  const dialog = unlockDialog(page);
  await t.see(dialog);
  await t.see(dialog.getByText(/change the network settings/));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  ensure(new URL(page.url()).pathname === "/networking", `a cancelled unlock went to ${page.url()}`);
  await change.click();
  await answerUnlock(t, page);
  await page.waitForURL((address) => address.pathname === `/networking/${encodeURIComponent(where.card.name)}`, { timeout: 15_000 });
  await t.see(page.getByRole("heading", { level: 2, name: "Change the settings", exact: true }));
}

async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const b = await signedIn(t, OTHER);
  const where = await theCard(a.page);
  const { session } = where;

  await openCards(t, a.page);
  await cardsAsAnswered(t, a.page, session);
  await trafficShown(t, a.page);
  await connectionsShown(t, a.page, session);
  await lookAll(t, a.page, "networking-cards");

  // The console is locked after signing in: the way to the editor asks to unlock.
  await changeAsksToUnlock(t, a.page, where);
  ensure((await dhcpRadio(a.page).isChecked()) === (where.card.settings.mode === "dhcp"), "the editor does not start from the card's own mode");
  await lookAll(t, a.page, "networking-editor");
  await commandOfChange(a.page, where);
  await editorChecks(t, a.page, where);

  await undoneAtOnce(t, a, b, where);
  await keptChange(t, a, b, where);
  const c = await signedIn(t, READER);
  await undoneByTimer(t, a, b, c, where);
  await c.page.context().close();
  await cutOff(t, a, b, where);

  mustHaveCalled(a.seen, [
    ROUTES.show,
    ROUTES.connections,
    routeFor(ROUTES.set, where.card.name),
    ROUTES.keep,
    ROUTES.undo,
    routeFor(ROUTES.fresh, where.card.name),
  ]);
  await signOut(t, a.page);
  await b.page.context().close();
  await a.page.context().close();
}

/** A reader sees the cards and a card's settings, and no way to change them. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  const { session, card } = await theCard(page);
  await openCards(t, page);
  await cardsAsAnswered(t, page, session);
  ensure((await page.getByRole("link", { name: "Change…", exact: true }).count()) === 0, "a reader's cards have a Change... link");
  await connectionsShown(t, page, session);
  await openReaderEditor(t, page, card.name);
  ensure((await tryButton(page).count()) === 0 && (await page.locator("form.net-form").count()) === 0, "a reader's editor has the form");
  await lookAll(t, page, "networking-reader");
  await signOut(t, page);
  await page.context().close();
}

/** A card that is not there says so and leads back to the cards. */
async function unknownCard(t, page) {
  await page.goto(new URL("/networking/no-such-card", page.url()).href);
  await t.see(page.getByRole("heading", { level: 2, name: "There is no such network card", exact: true }));
  await t.see(page.getByRole("link", { name: "Back to the cards", exact: true }));
}

/** At 390 wide: the cards, the editor and its warning, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  const where = await theCard(page);

  await openCards(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's cards scroll ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .net button, main .net a.btn, main .net .seg button");
  ensure(small.length === 0, `these controls of the phone's cards are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "networking-phone-cards");

  await openEditor(t, page, where.card.name);
  await setAddresses(page, [`${where.spare}/${where.prefix}`]);
  await t.see(cutWarning(page));
  ensure((await sidewaysScroll(page)) === 0, `the phone's editor scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallEditor = await smallControls(page, "main .net button, main .net a.btn");
  ensure(smallEditor.length === 0, `these controls of the phone's editor are under 44 px tall: ${smallEditor.join(", ")}`);
  const widths = await tryButton(page).evaluate((button) => ({ button: button.getBoundingClientRect().width, content: button.closest(".net-form").getBoundingClientRect().width }));
  ensure(widths.button >= widths.content - 4 || widths.button >= 300, `Try it is ${Math.round(widths.button)} px wide in a form ${Math.round(widths.content)} px wide`);
  await lookAll(t, page, "networking-phone-editor");
  await unknownCard(t, page);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
