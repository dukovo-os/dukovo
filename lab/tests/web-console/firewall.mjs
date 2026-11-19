// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/firewall drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Firewall
// page (design 10) at /firewall. It shows what `firewall list --json` says,
// which `GET /api/1/firewall` runs: the rules in the order they are checked,
// with Change and Remove for an administrator and the form that adds, replaces
// or removes a rule, tried for a window that Spark undoes by itself unless the
// change is kept (`firewall keep`) or taken back (`firewall undo`). Under the
// rules are the test of a connection (`firewall test`, `GET
// /api/1/firewall/test`), what was blocked (`firewall dropped`, `GET
// /api/1/firewall/dropped`) and what listens (`GET /api/1/firewall/status`),
// the last two for an administrator who has unlocked the console.
//
// An administrator signs in with the console locked: the cards of what was
// blocked and what listens say to unlock, and the Unlock beside them opens
// the dialog and then fills them with what the machine says. The form says
// under its field what is wrong with a port and sends nothing. Then a rule is
// added (allow tcp 8080 from the browser's own address), tried, reached over
// and kept, while a second administrator's browser watches the banner count
// down and say that it was kept; the test of a connection says what the rule
// decides, as `firewall test` says it; the rule is replaced and tried and kept,
// and removed: once taken back with Undo now, and once kept. A rule that is
// begun by a call of the page's own route with a short window is seen by the
// three browsers (the administrator's, another's and a reader's) and undone by
// Spark.
//
// The cut-off is the last: `deny tcp 9090 from` the browser's own address, the
// port the console answers on. The form says before it is made that it would
// cut off the browser's own connection and names the rule, still offers Try
// for 60 seconds and offers no Apply without trying. After the try the console
// is out of reach (a connection from the lab PC to it is not made, as the
// page's test of a connection says it would not be), the page says so and Keep
// is off; nothing is kept, and Spark takes the rule away by itself within the
// window, with the console reached again, the rules as they were and the
// journal's row of the undo. The browser's own tries at the console in that
// time are what Blocked then lists, and Allow this fills the form with the
// rule that would let them in.
//
// A reader sees the rules, the test of a connection and the words that
// say that administrators see the other two cards, and no button that changes
// anything; a phone at 390 by 844 shows the rules, the form and its warning with
// every control 44 px tall. Each look is the console's own: dark, light, high
// contrast, forced colours and less motion, each checked by axe (console.mjs's
// lookAll). The accounts are the three that firewall.toml made, so no script
// needs the lab's password. The rules are the ones the test adds and takes
// away; the file is put back by the test's last steps.

import {
  ensure,
  fetchAsPage,
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
  unlockNow,
} from "./trial.mjs";

const ADMIN = "wc-firewall";
const OTHER = "wc-firewall-other";
const READER = "wc-firewall-reader";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end, with
// `firewall` for {trial}.
const ROUTES = {
  list: "GET /api/1/firewall",
  test: "GET /api/1/firewall/test",
  status: "GET /api/1/firewall/status",
  dropped: "GET /api/1/firewall/dropped",
  add: "POST /api/1/firewall/add",
  replace: "POST /api/1/firewall/replace",
  remove: "POST /api/1/firewall/remove",
  keep: "POST /api/1/firewall/trial/keep",
  undo: "POST /api/1/firewall/trial/undo",
  fresh: "GET /api/1/console/fresh/{trial}",
};

/** The path of a route as the table names it: `GET /api/1/firewall` is `/api/1/firewall`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

// The words the page says where this test looks for them.
const BANNER_WORDS = "The firewall rules are being tried.";
const BLOCKED_LOCKED = "Unlock to see what the firewall blocked.";
const BLOCKED_READER = "Administrators see what the firewall blocked.";
const LISTENING_LOCKED = "Unlock to see what listens on this machine.";
const LISTENING_READER = "Administrators see what listens on this machine.";
const TRIAL_LINE = "Keep or undo the change being tried first.";

// --- Reading the machine ---

/** What `GET /api/1/firewall` answers: `firewall list --json`, `{rules, problems, trial}`. */
async function readRules(page) {
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.list));
  ensure(answer.status === 200 && Array.isArray(answer.json?.rules), `${ROUTES.list} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** Reads the rules until `test(rules)` holds, for up to `seconds`, and gives them. */
async function rulesBecome(page, test, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const listed = await readRules(page).catch(() => null);
    if (listed !== null && test(listed.rules)) return listed.rules;
    ensure(Date.now() < until, `the rules did not ${what} within ${seconds} seconds: ${JSON.stringify(listed?.rules?.map((rule) => rule.text))}`);
    await sleep(1000);
  }
}

/** The texts of the rules, as `firewall list` writes them. */
const textsOf = (rules) => rules.map((rule) => rule.text);

/** What the console knows of how it is reached: the browser's address, the port it answers on and the card. */
async function theConnection(page) {
  const session = await sessionOf(page);
  ensure(typeof session.from === "string" && typeof session.via?.port === "number", `GET /api/1/session does not say how the console is reached: ${JSON.stringify({ from: session.from, via: session.via })}`);
  return session;
}

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

// --- The page ---

const rulesCard = (page) => page.locator(".fw-rules");
const addButton = (page) => page.getByRole("button", { name: "Add a rule", exact: true });
const form = (page) => page.locator("form#add");
const tryButton = (page) => form(page).getByRole("button", { name: /^Try for / });
const applyButton = (page) => form(page).getByRole("button", { name: "Apply without trying", exact: true });
const cancelButton = (page) => form(page).getByRole("button", { name: "Cancel", exact: true });
const portsField = (page) => page.locator("#fw-ports");
const fromText = (page) => page.locator("#fw-from-text");
const blockedCard = (page) => page.locator("#fw-blocked");
const listeningCard = (page) => page.locator("#fw-listening");
const testForm = (page) => page.locator("form.fw-test");

/** Opens the page and waits for the rules to be read: the table is not busy, or the words that there are none are there. */
async function openFirewall(t, page, address = "/firewall") {
  await page.goto(new URL(address, page.url()).href);
  await t.see(page.getByRole("heading", { name: "Firewall", level: 1, exact: true }));
  await t.see(rulesCard(page).getByRole("heading", { level: 2, name: "Rules", exact: true }));
  await page.waitForFunction(
    () => {
      const table = document.querySelector(".fw-rules table");
      return table !== null && !table.hasAttribute("aria-busy");
    },
    null,
    { timeout: 30_000 },
  );
}

/** The rows of the table of rules as drawn: their number and words. */
const rowsShown = (page) =>
  page.locator(".fw-rules tbody tr[data-number]").evaluateAll((rows) =>
    rows.map((row) => ({ number: Number(row.dataset.number), words: row.querySelector(".fw-words")?.textContent.trim() ?? "", buttons: [...row.querySelectorAll("button")].map((button) => button.dataset.action) })),
  );

/** The page's rules are the machine's: the same numbers in the same order, and the words that there are none when there are none. */
async function rulesAsAnswered(t, page, { admin }) {
  const before = await readRules(page);
  const rows = await rowsShown(page);
  const after = await readRules(page);
  const numbers = (listed) => JSON.stringify(listed.rules.map((rule) => rule.number));
  const shown = JSON.stringify(rows.map((row) => row.number));
  ensure(shown === numbers(before) || shown === numbers(after), `the page shows rules ${shown} and ${ROUTES.list} gives ${numbers(before)}, then ${numbers(after)}`);
  if (rows.length === 0) await t.see(page.getByText("There are no rules.", { exact: true }));
  await t.see(page.locator(".fw-default"));
  for (const row of rows) {
    ensure(row.words !== "", `the rule ${row.number} has no words`);
    ensure(admin === row.buttons.includes("change") || row.words.includes("program"), `rule ${row.number} has the buttons ${row.buttons.join(", ")}`);
  }
  return rows;
}

/** The card of what listens is what `firewall status` says: each of its sockets, and the console's own, with who reaches it. */
async function listeningAsAnswered(t, page, session) {
  const card = listeningCard(page);
  await t.see(card.locator("tbody tr").first());
  const answer = await fetchAsPage(page, "GET", pathOf(ROUTES.status));
  ensure(answer.status === 200 && Array.isArray(answer.json?.listening), `${ROUTES.status} was answered ${answer.status}: ${answer.text}`);
  const shown = await card.locator("tbody tr th").allTextContents();
  const named = (one) => `${one.port}/${one.tcp ? "TCP" : "UDP"}`;
  for (const one of answer.json.listening) ensure(shown.includes(named(one)), `What listens does not show ${named(one)}: it shows ${shown.join(", ")}`);
  ensure(shown.includes(`${session.via.port}/TCP`), `What listens does not show the console's port ${session.via.port}/TCP`);
}

// --- The test of a connection ---

/** Asks the test of a connection about `protocol`, `port` and the address in its field, and gives what the page says and what the machine answered to it. */
async function askTest(t, page, protocol, port) {
  const card = testForm(page);
  await card.locator("#fw-test-protocol").selectOption(protocol);
  await card.locator("#fw-test-port").fill(String(port));
  const answered = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.test));
  await card.getByRole("button", { name: "Test", exact: true }).click();
  const answer = await answered;
  ensure(answer.status() === 200, `${ROUTES.test} was answered ${answer.status()}: ${await answer.text()}`);
  const machine = await answer.json();
  const verdict = card.locator(".fw-verdict");
  await t.see(verdict);
  return { said: (await verdict.textContent()).trim(), machine };
}

/** What the page says of a test, from what the machine answered: the rule that decides, or that none does. */
function agreesWith(said, machine, port) {
  if (machine.rule === null || machine.rule === undefined) return said.startsWith(`No rule decides: it reaches port ${port}`);
  return said.startsWith(`Rule ${machine.rule} decides: ${machine.allowed ? "allowed" : "denied"}.`);
}

// --- The form ---

/** Opens the form with "Add a rule" and waits for it. */
async function openForm(t, page) {
  await addButton(page).click();
  await t.see(form(page));
  await t.see(page.getByRole("heading", { level: 2, name: "Add a rule", exact: true }));
}

/** Fills the add form with `protocol`, `ports` and the address `from` as the address the rule is for. */
async function fillRule(page, { action = "allow", protocol = "tcp", ports, from }) {
  await form(page).getByRole("radio", { name: action === "allow" ? "Allow" : "Deny", exact: true }).check();
  await form(page).locator("#fw-protocol").selectOption(protocol);
  await portsField(page).fill(ports);
  await form(page).getByRole("radio", { name: "An address or network…", exact: true }).check();
  await fromText(page).fill(from);
}

/** A port that is no port is said under its field, nothing is sent, and the form closes with Escape and gives focus back. */
async function formChecks(t, page, session) {
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.add)) sent += 1;
  };
  page.on("request", count);
  await openForm(t, page);
  await fillRule(page, { ports: "70000", from: session.from });
  await tryButton(page).click();
  const sentence = "`70000` is not a port, 1 to 65535, or a range of them";
  await t.see(page.locator("#fw-ports-error", { hasText: sentence }));
  ensure((await portsField(page).getAttribute("aria-invalid")) === "true", "the port 70000 is not marked as wrong");
  ensure(await portsField(page).evaluate((input) => input === document.activeElement), "focus is not on the wrong port");
  await portsField(page).fill("8080");
  ensure(((await page.locator("#fw-ports-error").textContent()) ?? "").trim() === "", "the sentence stays under a port that is right");
  await fromText(page).fill("300.1.1.1");
  await tryButton(page).click();
  await t.see(page.locator("#fw-from-error", { hasText: "`300.1.1.1` is not an address or network" }));
  await sleep(500);
  page.off("request", count);
  ensure(sent === 0, `the form sent ${sent} rules while a field was wrong`);
  await page.keyboard.press("Escape");
  await t.gone(form(page));
  ensure(await addButton(page).evaluate((button) => button === document.activeElement), "Escape did not give focus back to Add a rule");
}

// --- The tried changes ---

/** The banner of a trial that this page began, with its words and clock and the reach line. */
async function beganHere(t, page, seconds) {
  const shown = await bannerBecomes(page, (banner) => banner.running && banner.heading === BANNER_WORDS, "show the rules being tried");
  ensure(shown.applied, `the banner of a change made here does not say ${WORDS.applied}: ${JSON.stringify(shown)}`);
  const left = secondsOfClock(shown.clock);
  ensure(left !== null && left <= seconds && left > seconds - 20, `the clock says ${shown.clock} for a window of ${seconds} seconds`);
  ensure(/^The change is undone at \d{1,2}:\d\d [AP]M unless it is kept\./.test(shown.words), `the banner says "${shown.words}" about when the change is undone`);
  ensure(shown.polite !== "", "the banner's polite region says nothing when the change begins");
  await t.see(page.getByText(TRIAL_LINE, { exact: true }));
  return shown;
}

/**
 * Presses Try for the form as it is, and has the page that pressed it reach
 * the console over the new rules. The second administrator's page is shown
 * the banner. Gives the time the try began, for a test of the window.
 */
async function tried(t, a, b, route) {
  await pressAndAnswer(t, a.page, tryButton(a.page), pathOf(route));
  const beganAt = Date.now();
  await beganHere(t, a.page, 60);
  // The rules are drawn as they were until the trial ends: the table's buttons are off, and say so.
  ensure((await rulesCard(a.page).locator("button[aria-disabled=true]").count()) > 0 || (await rowsShown(a.page)).length === 0, "the buttons of the rules are on while a change is tried");
  const reached = await bannerBecomes(a.page, (banner) => banner.reach === WORDS.reached && !banner.keepOff, "say that this browser reached the console", 40);
  ensure(reached.applied, "the banner lost its Applied step");
  const other = await bannerBecomes(b.page, (banner) => banner.running && banner.heading === BANNER_WORDS, "show the change another administrator is trying", 30);
  ensure(other.words.includes(`Begun by ${ADMIN} in this console.`), `the banner of another administrator's page says "${other.words}", not that ${ADMIN} began it`);
  ensure(!other.applied && other.reach === "" && other.buttons.includes("Keep this change") && other.buttons.includes("Undo now"), `a page that did not begin the change shows ${JSON.stringify(other)}`);
  return beganAt;
}

/** Keeps the change that is tried, and has both pages say so. */
async function kept(t, a, b) {
  await pressAndAnswer(t, a.page, keepOf(a.page), pathOf(ROUTES.keep));
  await bannerEnds(a.page, "kept", 20);
  await bannerEnds(b.page, "kept", 30);
  ensure(trialIn(await readDocument(a.page), "firewall") === null, "Spark still tries the rules after they were kept");
}

/** Takes the change that is tried back with Undo now, and has both pages say so. */
async function undoneNow(t, a, b) {
  await pressAndAnswer(t, a.page, undoOf(a.page), pathOf(ROUTES.undo));
  await bannerEnds(a.page, "undone", 20);
  await bannerEnds(b.page, "undone", 30);
  ensure(trialIn(await readDocument(a.page), "firewall") === null, "Spark still tries the rules after Undo now");
}

/** The rules as the page draws them are the machine's, once it has read them again. */
async function seeRules(t, page, texts) {
  await page.waitForFunction(
    (count) => document.querySelectorAll(".fw-rules tbody tr[data-number]").length === count,
    texts.length,
    { timeout: 30_000 },
  );
  // The buttons are off until the page has read the rules again after a trial.
  await page.waitForFunction(() => document.querySelectorAll(".fw-rules button[aria-disabled=true]").length === 0, null, { timeout: 30_000 });
  const rows = await rowsShown(page);
  ensure(rows.length === texts.length, `the page shows ${rows.length} rules and the machine has ${texts.length}`);
}

/** A rule is added, tried, reached over and kept; the test of a connection says what it decides; it is replaced, and removed with Undo now and then for good. */
async function rulesChanged(t, a, b, session) {
  const first = a.page;
  await unlockNow(first);
  await openFirewall(t, first);
  await openFirewall(t, b.page);
  const document = await readDocument(first);
  ensure(trialIn(document, "firewall") === null, "a trial runs before the test's own begins");
  const rulesBefore = textsOf((await readRules(first)).rules);
  const added = `allow tcp 8080 from ${session.from}`;

  // Add.
  await openForm(t, first);
  await fillRule(first, { ports: "8080", from: session.from });
  await t.gone(form(first).locator(".fw-warning .banner"));
  await t.see(applyButton(first));
  await t.see(tryButton(first));
  ensure((await tryButton(first).textContent()).trim() === "Try for 60 seconds", `Try says "${await tryButton(first).textContent()}"`);
  const beganAt = await tried(t, a, b, ROUTES.add);
  await look(t, first, "firewall-trial");
  await kept(t, a, b);
  const afterAdd = await rulesBecome(first, (rules) => rules.length === rulesBefore.length + 1, "have the rule");
  ensure(afterAdd.at(-1).text === added, `the rule is "${afterAdd.at(-1).text}", not "${added}"`);
  await seeRules(t, first, afterAdd);
  const number = afterAdd.length;
  const row = (await rowsShown(first)).find((one) => one.number === number);
  ensure(row.words.includes("8080") && row.words.includes(session.from), `the table says "${row.words}" of the rule`);
  await lookAll(t, first, "firewall-rules");

  // The test of a connection says what the rule decides, as the machine does.
  const asked = await askTest(t, first, "tcp", 8080);
  ensure(agreesWith(asked.said, asked.machine, 8080) && asked.machine.rule === number && asked.machine.allowed === true, `the test of tcp 8080 says "${asked.said}" and the machine ${JSON.stringify(asked.machine)}`);
  const other = await askTest(t, first, "tcp", 8081);
  ensure(agreesWith(other.said, other.machine, 8081) && other.machine.rule === null, `the test of tcp 8081 says "${other.said}" and the machine ${JSON.stringify(other.machine)}`);

  // The window's end changes nothing of what was kept.
  await sleep(Math.max(0, beganAt + 60_000 + 5000 - Date.now()));
  ensure(JSON.stringify(textsOf((await readRules(first)).rules)) === JSON.stringify(afterAdd.map((rule) => rule.text)), "the rules changed after the window of a change that was kept");

  // Replace: the form starts from the rule, and the new one is kept.
  await rulesCard(first).locator(`tr[data-number="${number}"] [data-action="change"]`).click();
  await t.see(headingOf(first, `Replace rule ${number}`));
  ensure((await portsField(first).inputValue()) === "8080", `Change starts with the port ${await portsField(first).inputValue()}`);
  await portsField(first).fill("8081");
  await tried(t, a, b, ROUTES.replace);
  await kept(t, a, b);
  const replaced = await rulesBecome(first, (rules) => rules.some((rule) => rule.text.includes("8081")), "have the new port");
  ensure(!textsOf(replaced).some((text) => text.includes("8080")), `the old rule is still there: ${textsOf(replaced).join("; ")}`);

  // Remove, taken back at once: the rule is still there.
  await seeRules(t, first, replaced);
  await rulesCard(first).locator(`tr[data-number="${number}"] [data-action="remove"]`).click();
  await t.see(headingOf(first, `Remove rule ${number}`));
  await tried(t, a, b, ROUTES.remove);
  await undoneNow(t, a, b);
  ensure(textsOf((await readRules(first)).rules).some((text) => text.includes("8081")), "Undo now did not leave the rule there");
  await seeRules(t, first, (await readRules(first)).rules);

  // Remove for good.
  await rulesCard(first).locator(`tr[data-number="${number}"] [data-action="remove"]`).click();
  await t.see(headingOf(first, `Remove rule ${number}`));
  await tried(t, a, b, ROUTES.remove);
  await kept(t, a, b);
  const gone = await rulesBecome(first, (rules) => rules.length === rulesBefore.length, "be as they were");
  ensure(JSON.stringify(textsOf(gone)) === JSON.stringify(rulesBefore), `the rules are ${textsOf(gone).join("; ")}, and were ${rulesBefore.join("; ")}`);
  await seeRules(t, first, gone);
}

/** The heading of the form, at level 2, with these words. */
const headingOf = (page, words) => page.getByRole("heading", { level: 2, name: words, exact: true });

/** A rule that is begun by a call of the page's own route with a short window is seen by three browsers and undone by Spark. */
async function undoneByTimer(t, a, b, c) {
  await unlockNow(a.page);
  await openFirewall(t, a.page);
  await openFirewall(t, b.page);
  await openFirewall(t, c.page);
  const before = undoneIn(await readDocument(a.page), "firewall");
  const rulesBefore = textsOf((await readRules(a.page)).rules);
  const begun = await fetchAsPage(a.page, "POST", pathOf(ROUTES.add), { action: "deny", protocol: "tcp", ports: "7", try: 20 });
  ensure(begun.status === 200, `${ROUTES.add} with a window of 20 seconds was answered ${begun.status}: ${begun.text}`);
  for (const [who, page] of [["the administrator who began it", a.page], ["another administrator", b.page], ["a reader", c.page]]) {
    const banner = await bannerBecomes(page, (shown) => shown.running, `show ${who} the rules that are tried`, 30);
    ensure(banner.heading === BANNER_WORDS, `the banner of ${who} says "${banner.heading}"`);
    ensure(secondsOfClock(banner.clock) !== null, `the banner of ${who} has the clock "${banner.clock}"`);
  }
  const reader = await readBanner(c.page);
  ensure(reader.note === WORDS.reader && !reader.buttons.includes("Keep this change") && !reader.buttons.includes("Undo now"), `a reader's banner is ${JSON.stringify(reader)}`);
  await look(t, c.page, "firewall-trial-reader");
  for (const page of [a.page, b.page, c.page]) await bannerEnds(page, "undone", 60);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "firewall") === null, "the rules are still tried after their window");
  ensure(undoneSince(document, "firewall", before) > 0, `Spark's document does not list the change it undid: ${JSON.stringify(document.undone)}`);
  const back = await rulesBecome(a.page, (rules) => rules.length === rulesBefore.length, "be as they were");
  ensure(JSON.stringify(textsOf(back)) === JSON.stringify(rulesBefore), `the rules are ${textsOf(back).join("; ")} after the undo, and were ${rulesBefore.join("; ")}`);
}

/**
 * The rule that would cut off the browser's own connection (design 10.4): the
 * form says so and offers Try and no Apply without trying; after Try the
 * console is out of reach, the page says so and Keep is off; Spark takes the
 * rule away within the window and the page is back.
 */
async function cutOff(t, a, b, session) {
  const url = `${new URL(a.page.url()).origin}/firewall`;
  await unlockNow(a.page);
  await openFirewall(t, a.page);
  await openFirewall(t, b.page);
  const before = undoneIn(await readDocument(a.page), "firewall");
  const rulesBefore = textsOf((await readRules(a.page)).rules);
  const lastRow = await lastChange(a.page);

  // Before the rule, nothing decides the console's port, and the lab PC reaches it.
  const open = await askTest(t, a.page, "tcp", session.via.port);
  ensure(agreesWith(open.said, open.machine, session.via.port) && open.machine.rule === null, `the test of the console's port says "${open.said}" and the machine ${JSON.stringify(open.machine)}`);
  ensure(await reaches(url), "the lab PC does not reach the console before the rule");

  await openForm(t, a.page);
  await fillRule(a.page, { action: "deny", ports: String(session.via.port), from: session.from });
  const warning = form(a.page).locator(".fw-warning .banner");
  await t.see(warning);
  const words = (await warning.textContent()).replace(/\s+/g, " ");
  ensure(words.includes("This would cut off your own connection."), `the warning says "${words}"`);
  ensure(words.includes(`The console reaches you from ${session.from} through ${session.via.card} on port ${session.via.port}, and this rule would deny it.`), `the warning does not say how the console is reached: "${words}"`);
  await t.gone(applyButton(a.page));
  await t.see(tryButton(a.page));
  ensure((await tryButton(a.page).getAttribute("aria-describedby")) === "fw-cut", "Try is not described by the warning");
  await lookAll(t, a.page, "firewall-cut-warning");

  await pressAndAnswer(t, a.page, tryButton(a.page), pathOf(ROUTES.add));
  await bannerBecomes(a.page, (banner) => banner.running && banner.heading === BANNER_WORDS, "show the rules being tried");
  // The console is not reached over the new rules: the page says so, and Keep is off and says why.
  const cannot = await bannerBecomes(a.page, (banner) => banner.reach.startsWith(WORDS.cannot) && banner.keepOff, "say that the console cannot be reached", 60);
  ensure(cannot.reason.startsWith(WORDS.cannot), `Keep's reason is "${cannot.reason}"`);
  ensure(await sendsNothing(a.page, keepOf(a.page), pathOf(ROUTES.keep)), "Keep sent a request while it said the console cannot be reached");
  ensure(!(await reaches(url)), "the lab PC reaches the console while the rule that denies it is tried");
  await lookAll(t, a.page, "firewall-cut");

  // Nothing is kept: Spark takes the rule away, and the page finds the console again.
  await bannerEnds(a.page, "undone", 150);
  ensure(await reachesSoon(url, 60), "the lab PC does not reach the console again after the rule was undone");
  const back = await rulesBecome(a.page, (rules) => rules.length === rulesBefore.length, "be as they were");
  ensure(JSON.stringify(textsOf(back)) === JSON.stringify(rulesBefore), `the rules are ${textsOf(back).join("; ")} after the undo, and were ${rulesBefore.join("; ")}`);
  const document = await readDocument(a.page);
  ensure(trialIn(document, "firewall") === null && undoneSince(document, "firewall", before) > 0, `Spark's document after the undo: ${JSON.stringify(document.undone)}`);
  await bannerEnds(b.page, "undone", 60);
  ensure((await undoRowAfter(a.page, lastRow)) !== null, "the journal has no row of the rule that was undone");
  const again = await askTest(t, a.page, "tcp", session.via.port);
  ensure(agreesWith(again.said, again.machine, session.via.port) && again.machine.rule === null, `the test of the console's port after the undo says "${again.said}"`);
  await lookAll(t, a.page, "firewall-undone");
}

/** The number of the newest row of the journal, or 0. */
async function lastChange(page) {
  const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
  ensure(list.status === 200 && Array.isArray(list.json?.changes), `GET /api/1/changes was answered ${list.status}: ${list.text}`);
  return list.json.changes.reduce((most, row) => Math.max(most, row.id), 0);
}

// --- What was blocked ---

/** The console's own port is among what was blocked, as the machine says it, and Allow this fills the form with the rule that would let it in. */
async function blockedAsAnswered(t, page, session) {
  await openFirewall(t, page);
  const card = blockedCard(page);
  // The log is written a moment after the kernel drops: ask again until it has the tries.
  const until = Date.now() + 60_000;
  let group;
  for (;;) {
    const answer = await fetchAsPage(page, "GET", `${pathOf(ROUTES.dropped)}?since=24h`);
    ensure(answer.status === 200 && Array.isArray(answer.json?.groups), `${ROUTES.dropped} was answered ${answer.status}: ${answer.text}`);
    group = answer.json.groups.find((one) => one.port === session.via.port && one.from === session.from && one.protocol === "tcp");
    if (group !== undefined) break;
    ensure(Date.now() < until, `the machine's list of what was blocked has no try at port ${session.via.port} from ${session.from}: ${JSON.stringify(answer.json.groups)}`);
    await sleep(2000);
  }
  await openFirewall(t, page);
  const item = card.locator("li.fw-drop", { hasText: `port ${session.via.port}` }).first();
  await t.see(item);
  const text = (await item.textContent()).replace(/\s+/g, " ");
  ensure(text.includes(session.from) && text.includes("at least"), `the entry of what was blocked says "${text}"`);
  await item.getByRole("button", { name: /^Allow this/ }).click();
  await t.see(form(page));
  ensure((await portsField(page).inputValue()) === String(session.via.port), `Allow this fills the port with ${await portsField(page).inputValue()}`);
  ensure((await fromText(page).inputValue()) === session.from, `Allow this fills the address with ${await fromText(page).inputValue()}`);
  ensure(await form(page).locator("#fw-protocol").inputValue() === "tcp", "Allow this does not fill the protocol with TCP");
  ensure(await form(page).getByRole("radio", { name: "Allow", exact: true }).isChecked(), "Allow this does not choose Allow");
  await look(t, page, "firewall-allow-this");
  await cancelButton(page).click();
  await t.gone(form(page));
}

// --- The administrator, the reader and the phone ---

async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const b = await signedIn(t, OTHER);
  const session = await theConnection(a.page);

  // The console is locked after signing in: what was blocked and what listens ask to unlock.
  await openFirewall(t, a.page);
  await rulesAsAnswered(t, a.page, { admin: true });
  await t.see(blockedCard(a.page).getByText(BLOCKED_LOCKED, { exact: true }));
  await t.see(listeningCard(a.page).getByText(LISTENING_LOCKED, { exact: true }));
  await look(t, a.page, "firewall-locked");
  await blockedCard(a.page).getByRole("button", { name: "Unlock", exact: true }).click();
  await answerUnlock(t, a.page);
  await t.gone(blockedCard(a.page).getByText(BLOCKED_LOCKED, { exact: true }));
  await t.gone(listeningCard(a.page).getByText(LISTENING_LOCKED, { exact: true }));
  await listeningAsAnswered(t, a.page, session);
  await lookAll(t, a.page, "firewall-page");

  await formChecks(t, a.page, session);
  await rulesChanged(t, a, b, session);
  const c = await signedIn(t, READER);
  await undoneByTimer(t, a, b, c);
  await c.page.context().close();
  await cutOff(t, a, b, session);
  await blockedAsAnswered(t, a.page, session);

  mustHaveCalled(a.seen, [
    ROUTES.list,
    ROUTES.test,
    ROUTES.status,
    ROUTES.dropped,
    ROUTES.add,
    ROUTES.replace,
    ROUTES.remove,
    ROUTES.keep,
    ROUTES.undo,
    ROUTES.fresh.replace("{trial}", "firewall"),
  ]);
  await signOut(t, a.page);
  await b.page.context().close();
  await a.page.context().close();
}

/** A reader sees the rules, can test a connection and is told who sees the rest, and has no button that changes anything. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  const session = await theConnection(page);
  await openFirewall(t, page);
  await rulesAsAnswered(t, page, { admin: false });
  ensure((await addButton(page).count()) === 0, "a reader's page has Add a rule");
  ensure((await rulesCard(page).locator("button:visible").count()) === 0, "a reader's rules have a button");
  await t.see(blockedCard(page).getByText(BLOCKED_READER, { exact: true }));
  await t.see(listeningCard(page).getByText(LISTENING_READER, { exact: true }));
  await t.gone(blockedCard(page).getByRole("button", { name: "Unlock", exact: true }));
  const asked = await askTest(t, page, "tcp", 22);
  ensure(agreesWith(asked.said, asked.machine, 22), `a reader's test of tcp 22 says "${asked.said}" and the machine ${JSON.stringify(asked.machine)}`);
  ensure((await testForm(page).locator("#fw-test-from").inputValue()) === session.from, "the test of a connection does not start from the browser's own address");
  await lookAll(t, page, "firewall-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the rules, the form and its warning, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  const session = await theConnection(page);

  await openFirewall(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .fw button, main .fw a.btn");
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "firewall-phone");

  await openForm(t, page);
  await fillRule(page, { action: "deny", ports: String(session.via.port), from: session.from });
  await t.see(form(page).locator(".fw-warning .banner"));
  ensure((await sidewaysScroll(page)) === 0, `the phone's form scrolls ${await sidewaysScroll(page)} px sideways`);
  const smallForm = await smallControls(page, "main .fw-add button, main .fw-add a.btn");
  ensure(smallForm.length === 0, `these controls of the phone's form are under 44 px tall: ${smallForm.join(", ")}`);
  const widths = await tryButton(page).evaluate((button) => ({ button: button.getBoundingClientRect().width, content: button.closest("form").getBoundingClientRect().width }));
  ensure(widths.button >= widths.content - 4 || widths.button >= 300, `Try is ${Math.round(widths.button)} px wide in a form ${Math.round(widths.content)} px wide`);
  await lookAll(t, page, "firewall-phone-form");
  await cancelButton(page).click();
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
