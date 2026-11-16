// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/hardware drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the Hardware
// page at /hardware, its first half. This machine is what `hardware show`
// says, which `GET /api/1/hardware` runs, with the memory's slots and modules
// from `hardware inventory` (`GET /api/1/hardware/inventory`), which only an
// unlocked administrator reads: a locked console shows the memory alone and a
// button, Show the modules, that asks to unlock. The Last check card holds
// what `hardware last` says (`GET /api/1/hardware/check`), a failure first,
// and the checks before it come from `hardware history`
// (`GET /api/1/hardware/history`). Check the hardware
// (`POST /api/1/hardware/check`, a task) opens a dialog with the six areas,
// all on, and the full check, which is only read here: its words come with its
// box and it is never started, since it takes hours. With no area chosen
// nothing is sent. The check runs to its end, the page says the step it is on
// while it does, and the card, the line under the head and the memory's last
// run are the machine's own once it is done. Test part of the memory is the
// memory area of a check, started from its card. Restart… and the memory
// card's Restart now… ask first (`POST /api/1/power/restart`) and are cancelled
// here; a restart at a time of day is set and taken back with Cancel
// (`POST /api/1/power/cancel`), and Shut down… asks for the machine's name
// before it sets a shut down (`POST /api/1/power/shutdown`), which is taken
// back within its minute. What is waiting is the machine's own
// (`GET /api/1/power`). A reader sees every fact and no button, is told who
// may change the machine, and is refused when a route is asked. A phone at 390
// by 844 shows the page with no sideways scroll and controls 44 px tall. Each
// look is the console's own: dark, light, high contrast, forced colours and
// less motion, each checked by axe (console.mjs's lookAll).
//
// This script is the page's first half: the devices and the temperatures are
// not read here. hardware.toml puts the machine back before each engine: no
// check is kept, and nothing is waiting to restart. The accounts are the two
// that hardware.toml made, so no script needs the lab's password.

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

const ADMIN = "wc-hw";
const READER = "wc-hw-reader";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each by the end.
const ROUTES = {
  show: "GET /api/1/hardware",
  inventory: "GET /api/1/hardware/inventory",
  last: "GET /api/1/hardware/check",
  history: "GET /api/1/hardware/history",
  check: "POST /api/1/hardware/check",
  power: "GET /api/1/power",
  restart: "POST /api/1/power/restart",
  shutdown: "POST /api/1/power/shutdown",
  cancel: "POST /api/1/power/cancel",
};

/** The path of a route as the table names it: `GET /api/1/hardware` is `/api/1/hardware`. */
const pathOf = (route) => route.slice(route.indexOf(" ") + 1);

/** The method of a route as the table names it. */
const methodOf = (route) => route.slice(0, route.indexOf(" "));

// The words the page says where this test looks for them.
const LEAD_READER = "An administrator can check the hardware and restart or shut down this machine.";
const NOT_CHECKED = "Not checked yet.";
const READER_CHECK = "An administrator can check the hardware.";
const READER_TEST = "An administrator can test the memory.";
const READER_POWER = "An administrator can restart or shut down this machine.";
const NO_AREA = "Choose at least one thing to check.";
const WHY_OFF = "A check is running. Its step is under the heading.";
const MEMORY_RESTART_WHY = "To test all of the memory from the start menu.";
const BAD_TIME = "A time is written HH:MM, from 00:00 to 23:59, such as 03:00. Nothing was set.";

// The areas of the check dialog, in the words of their boxes. A check of one
// engine is the whole quick check and the others' is shorter, so that three
// engines fit in the test's time: the boxes work the same in each.
const AREAS = ["Memory", "Disks", "Processors", "Sensors", "Battery", "Network cards"];
const SOME = ["Memory", "Processors"];

/** What a result is called on the card, and the order the card lists them in. */
const WORDS = { failed: "Failed", warning: "Warning", passed: "Passed", skipped: "Skipped" };
const ORDER = Object.keys(WORDS);

const GIB = 2 ** 30;
const MIB = 2 ** 20;

/** Whether `value` is text with something in it, as the page counts it. */
const isText = (value) => typeof value === "string" && value.trim() !== "";

/** "15.5 GiB" or "512 MiB", as the page says an amount of memory. */
const amountOf = (bytes) => (bytes >= GIB ? `${Number((bytes / GIB).toFixed(1))} GiB` : `${Math.round(bytes / MIB)} MiB`);

const counted = (count, noun) => `${count} ${noun}${count === 1 ? "" : "s"}`;

// --- Reading the machine ---

/** What `route` answers to the page's own request, whole: `{status, text, json}`. */
const askFor = (page, route, body = null) => fetchAsPage(page, methodOf(route), pathOf(route), body);

/** What a read route answers, which must be 200; gives its JSON. */
async function readOf(page, route) {
  const answer = await askFor(page, route);
  ensure(answer.status === 200, `${route} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** `hardware last`: the last check, or null while none is kept. */
async function readLast(page) {
  const last = await readOf(page, ROUTES.last);
  ensure(last === null || (typeof last === "object" && Array.isArray(last.checks)), `${ROUTES.last} gives neither no check nor a check: ${JSON.stringify(last)}`);
  return last;
}

/** `hardware history`: the lines, oldest first. */
async function readHistory(page) {
  const history = await readOf(page, ROUTES.history);
  ensure(Array.isArray(history), `${ROUTES.history} does not give a list: ${JSON.stringify(history)}`);
  return history;
}

/** The restart or shut down that waits, as `GET /api/1/power` says it, or null. */
async function readPower(page) {
  const power = await readOf(page, ROUTES.power);
  return power?.shutdown ?? null;
}

/** The machine's name, which the dialogs say and the shut down asks to have typed. */
async function hostOf(page) {
  const about = await readOf(page, "GET /api/1/console/about");
  ensure(isText(about?.host), `GET /api/1/console/about gives no name for the machine: ${JSON.stringify(about)}`);
  return about.host;
}

/** Tries `test` until it stops throwing, for up to `seconds`: what the page draws follows what the machine says by a moment. */
async function eventually(test, seconds = 30) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    try {
      return await test();
    } catch (error) {
      if (Date.now() >= until) throw error;
      await sleep(1000);
    }
  }
}

/** The task `id` once it has ended, as `GET /api/1/tasks/{id}` gives it, waiting up to `seconds`. */
async function taskEnd(page, id, seconds) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const answer = await fetchAsPage(page, "GET", `/api/1/tasks/${id}`);
    ensure(answer.status === 200, `GET /api/1/tasks/${id} was answered ${answer.status}: ${answer.text}`);
    if (answer.json.state !== "waiting" && answer.json.state !== "running") return answer.json;
    ensure(Date.now() < until, `the check, task ${id}, did not end within ${seconds} seconds: ${answer.text}`);
    await sleep(3000);
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

const card = (page, id) => page.locator(`main section.card#${id}`);
const lead = (page, words) => page.locator("p#hw-lead", { hasText: words });
const checkNow = (page) => page.getByRole("button", { name: "Check the hardware", exact: true });
const buttonIn = (scope, name) => scope.getByRole("button", { name, exact: true });
const pendingLine = (page) => card(page, "power").locator(".hw-pending .banner");

/** Opens /hardware, with `hash` after it, and waits for the cards, which come when the page has read what it shows. */
async function openHardware(t, page, hash = "") {
  await page.goto(new URL(`/hardware${hash}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Hardware", exact: true }));
  await t.see(card(page, "power"));
  await page.waitForFunction(() => !(document.querySelector("#hw-lead")?.textContent ?? "").includes("Reading the hardware"), null, { timeout: 30_000 });
}

/** `text` as a regular expression's own letters. */
const escaped = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** What the keyboard is on, by the id of the element. */
const focusedId = (page) => page.evaluate(() => document.activeElement?.id ?? "");

/**
 * Presses `control`, which asks the console to `method` `route`, and gives
 * the answer. A console that is locked asks to unlock first: the dialog is
 * answered when it comes. A change that is a task is answered 202, and any
 * other change 200.
 */
async function pressFor(t, page, control, route, method = "POST") {
  const answered = page.waitForResponse((answer) => answer.request().method() === method && new URL(answer.url()).pathname === route, { timeout: 120_000 });
  answered.catch(() => {});
  await control.click();
  const asked = unlockDialog(page).waitFor({ state: "visible", timeout: 60_000 }).then(() => "unlock", () => "never");
  const first = await Promise.race([answered.then(() => "answer"), asked]);
  if (first === "unlock") await answerUnlock(t, page);
  const answer = await answered;
  ensure(answer.status() === 200 || answer.status() === 202, `${method} ${route} was answered ${answer.status()}: ${await answer.text()}`);
  return answer;
}

/** Presses `control`, which asks a question before it changes anything, and waits for `dialog`; a locked console asks to unlock first. */
async function opensDialog(t, page, control, dialog) {
  await control.click();
  const first = await Promise.race([
    dialog.waitFor({ state: "visible", timeout: 30_000 }).then(() => "dialog", () => "never"),
    unlockDialog(page).waitFor({ state: "visible", timeout: 30_000 }).then(() => "unlock", () => "never"),
  ]);
  if (first === "unlock") await answerUnlock(t, page);
  await t.see(dialog);
}

/** Whether `control`, pressed, sends no request that is a POST to `route`. */
async function sendsNoPost(page, control, route) {
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === route) sent += 1;
  };
  page.on("request", count);
  // Pressed as a person may: Playwright would wait for it to come on.
  await control.click({ force: true });
  await sleep(1200);
  page.off("request", count);
  return sent === 0;
}

// --- This machine ---

/** The rows of the machine's card, `{Model: "...", Memory: "...", ...}`; a fact that is not there is not a member. */
const factsOf = (page) =>
  card(page, "machine").locator("dl.rows").evaluate((dl) => {
    const facts = {};
    for (const term of dl.querySelectorAll("dt")) facts[term.textContent.trim()] = term.nextElementSibling.innerText.trim();
    return facts;
  });

/**
 * The card's memory row is what the machine says. `access` is `locked` for a
 * console that has not read the slots, `ok` for one that has and `reader` for
 * anyone who may not; `inventory` is what `hardware inventory` says, or null.
 */
function memoryAsAnswered(facts, show, inventory, access) {
  const memory = inventory?.memory ?? show.memory;
  const words = facts.Memory;
  ensure(words !== undefined && words.startsWith(amountOf(memory.bytes)), `the memory row says ${JSON.stringify(words)}, and the machine has ${amountOf(memory.bytes)}`);
  if (access === "reader") ensure(words.includes("An administrator sees the slots"), `a reader's memory row says ${JSON.stringify(words)} and not who sees the slots`);
  if (access === "locked") ensure(words.includes("Show the modules"), `a locked console's memory row has no Show the modules: ${JSON.stringify(words)}`);
  if (access !== "ok") return;
  if (!Array.isArray(memory.modules)) {
    ensure(words.includes("The firmware does not list the slots"), `the memory row says ${JSON.stringify(words)} of slots the firmware does not list`);
    return;
  }
  if (Number.isInteger(memory.slots)) ensure(words.includes(`in ${memory.modules.length} of ${memory.slots} slots`), `the memory row does not say ${memory.modules.length} of ${memory.slots} slots: ${JSON.stringify(words)}`);
  if (typeof memory.ecc === "boolean") ensure(words.includes(memory.ecc ? "error correction" : "no error correction"), `the memory row says ${JSON.stringify(words)} of error correction`);
}

/** The modules that the memory row lists are those of `hardware inventory`, one line each. */
async function modulesAsAnswered(t, page, inventory) {
  const modules = inventory?.memory?.modules;
  const list = card(page, "machine").locator("details.hw-modules");
  if (!Array.isArray(modules) || modules.length === 0) {
    ensure((await list.count()) === 0, "the card lists modules and the machine reports none");
    return;
  }
  await t.see(list.locator("summary", { hasText: `Show the modules (${modules.length})` }));
  await list.locator("summary").click();
  const lines = await list.locator("li").evaluateAll((items) => items.map((item) => item.textContent.trim()));
  ensure(lines.length === modules.length, `the card lists ${lines.length} modules and the machine has ${modules.length}`);
  modules.forEach((module, at) => {
    if (isText(module.slot)) ensure(lines[at].includes(module.slot.trim()), `module ${at + 1} is ${JSON.stringify(lines[at])}, not ${module.slot}`);
  });
}

/** The card says what `hardware show` says (and `hardware inventory`, when the slots were read): the model, the processor, the memory, the firmware and how long the machine has run. */
async function machineAsAnswered(t, page, access, inventory = null) {
  const show = await readOf(page, ROUTES.show);
  const about = await readOf(page, "GET /api/1/console/about");
  const facts = await factsOf(page);
  const where = "the This machine card";

  const named = [show.maker, show.model].filter(isText).map((text) => text.trim());
  if (named.length === 0) ensure(facts.Model === undefined, `${where} has a Model row, and ${ROUTES.show} names no maker or model`);
  else for (const word of named) ensure(facts.Model?.includes(word), `${where} says ${JSON.stringify(facts.Model)}, not ${word}`);
  if (named.length > 0 && isText(show.chassis)) ensure(facts.Model.includes(show.chassis.trim().toLowerCase()), `${where} does not say the chassis ${show.chassis}: ${facts.Model}`);

  const processor = show.processor ?? {};
  if (isText(processor.model)) ensure(facts.Processor?.includes(processor.model.trim()), `${where} says ${JSON.stringify(facts.Processor)} of the processor, not ${processor.model}`);
  if (processor.cores > 0) ensure(facts.Processor?.includes(counted(processor.cores, "core")), `${where} does not say ${counted(processor.cores, "core")}: ${facts.Processor}`);

  memoryAsAnswered(facts, show, inventory, access);

  if (isText(show.firmware?.version)) {
    ensure(facts.Firmware?.includes(`BIOS ${show.firmware.version.trim()}`), `${where} says ${JSON.stringify(facts.Firmware)} of the firmware, not ${show.firmware.version}`);
    const link = card(page, "machine").locator('dl.rows a[href="/updates#firmware"]');
    await t.see(link);
    ensure((await link.innerText()) === "Firmware sets on the Updates page", `${where} links the firmware sets by another name`);
  } else {
    ensure(facts.Firmware === undefined, `${where} has a Firmware row, and ${ROUTES.show} gives no version`);
  }

  if (Number.isFinite(about.up)) ensure(/^(less than a minute|\d+ (day|hour|minute)s?)/.test(facts["Up for"] ?? ""), `${where} says ${JSON.stringify(facts["Up for"])} of how long the machine has run`);
  else ensure(facts["Up for"] === undefined, `${where} has an Up for row, and the console says no time`);
  if (access === "ok") await modulesAsAnswered(t, page, inventory);
}

// --- The last check and the checks before it ---

/** The checks of `last` as the card lists them: a failure first, then a warning, a pass and what was skipped, each in the order the machine gave it. */
const listedOf = (last) => ORDER.flatMap((result) => last.checks.filter((check) => check.result === result));

/** The sentence under the head: "Last check Nov 9 2:10 AM (a quick check): 8 passed, 1 warning (HW-115)." */
function leadAsAnswered(text, last) {
  const found = /^Last check .+ \((a quick check|a full check)\): (.+?)\./.exec(text);
  ensure(found !== null, `the line under the head says ${JSON.stringify(text)}, and the machine has a check`);
  ensure(found[1] === (last.full ? "a full check" : "a quick check"), `the line under the head says ${found[1]} of a check that is ${last.full ? "full" : "quick"}`);
  const counts = Object.fromEntries(ORDER.map((result) => [result, last.checks.filter((check) => check.result === result).length]));
  const wanted = [
    [counts.passed, `${counts.passed} passed`],
    [counts.warning, counted(counts.warning, "warning")],
    [counts.failed, `${counts.failed} failed`],
    [counts.skipped, `${counts.skipped} skipped`],
  ];
  for (const [count, words] of wanted) ensure((count > 0) === found[2].includes(words), `the line under the head says ${JSON.stringify(found[2])}, and the machine has ${count} of ${words.replace(/^\d+ /, "")}`);
}

/** The Last check card lists every check of `hardware last`, with its result in words first, and its ID linked. */
async function lastAsAnswered(t, page, last) {
  const where = "the Last check card";
  const worst = ORDER.find((result) => last.checks.some((check) => check.result === result)) ?? "skipped";
  ensure((await card(page, "check").locator("h2 .pill").innerText()) === WORDS[worst], `${where} says ${JSON.stringify(await card(page, "check").locator("h2 .pill").innerText())} of the worst result, not ${WORDS[worst]}`);
  leadAsAnswered(await page.locator("p#hw-lead").innerText(), last);

  const shown = await card(page, "check").locator("li.hw-check").evaluateAll((items) =>
    items.map((item) => {
      const first = item.querySelector("p");
      const id = item.querySelector("a.mono");
      return {
        word: first.querySelector(".st")?.textContent.trim() ?? "",
        name: first.querySelector("b")?.textContent.trim() ?? "",
        area: (first.querySelector(".muted")?.textContent ?? "").replace(/^\s*·\s*/, "").trim(),
        text: item.innerText,
        id: id?.textContent.trim() ?? null,
        href: id?.getAttribute("href") ?? null,
      };
    }),
  );
  const wanted = listedOf(last);
  ensure(shown.length === wanted.length, `${where} lists ${shown.length} checks and ${ROUTES.last} has ${wanted.length}`);
  wanted.forEach((check, at) => {
    const row = shown[at];
    const one = `check ${at + 1}, ${check.area}/${check.check}`;
    ensure(row.name === check.check.trim() && row.area === check.area.trim(), `${where}'s ${one} is ${row.area}/${row.name}`);
    ensure(row.word === WORDS[check.result], `${where}'s ${one} says ${JSON.stringify(row.word)}, not ${WORDS[check.result]}`);
    if (isText(check.detail)) ensure(row.text.includes(check.detail.trim()), `${where}'s ${one} does not say ${JSON.stringify(check.detail)}`);
    if (isText(check.id)) {
      ensure(row.id === check.id.trim() && row.href === `/help/errors/${encodeURIComponent(check.id.trim())}`, `${where}'s ${one} has the ID ${row.id} at ${row.href}, not ${check.id}`);
      if (isText(check.advice)) ensure(row.text.includes(check.advice.trim()), `${where}'s ${one} does not say what to do: ${JSON.stringify(check.advice)}`);
    } else {
      ensure(row.id === null, `${where}'s ${one} has the ID ${row.id}, which the machine does not give`);
    }
  });
}

/** The checks before the last, from `hardware history`: at most five, newest first, and not the last check's own line. */
function earlierOf(history, last) {
  const lines = history.filter((line) => Number.isFinite(line?.at_ms)).reverse();
  const before = last !== null && lines[0] && Math.floor(lines[0].at_ms / 1000) === last.time ? lines.slice(1) : lines;
  return before.slice(0, 5);
}

/** "Earlier checks" lists those, with the kind of check and the counts each had. */
async function earlierAsAnswered(t, page, history, last) {
  const wanted = earlierOf(history, last);
  const list = card(page, "check").locator("details.hw-earlier");
  if (wanted.length === 0) {
    ensure((await list.count()) === 0, "the card has Earlier checks, and the machine keeps no check before the last");
    return;
  }
  await t.see(list.locator("summary", { hasText: "Earlier checks" }));
  if ((await list.getAttribute("open")) === null) await list.locator("summary").click();
  const lines = await list.locator("li").evaluateAll((items) => items.map((item) => item.textContent.trim()));
  ensure(lines.length === wanted.length, `Earlier checks lists ${lines.length} and ${ROUTES.history} has ${wanted.length} before the last`);
  wanted.forEach((line, at) => {
    const counts = [
      line.passed > 0 && `${line.passed} passed`,
      line.warnings > 0 && counted(line.warnings, "warning"),
      line.failed > 0 && `${line.failed} failed`,
      line.skipped > 0 && `${line.skipped} skipped`,
    ].filter(Boolean).join(", ") || "nothing could be checked";
    const kind = line.full ? "a full check" : "a quick check";
    ensure(lines[at].includes(` · ${kind} · ${counts}`), `Earlier checks' line ${at + 1} says ${JSON.stringify(lines[at])}, not ${kind} with ${counts}`);
  });
}

/** The memory card's Last run is the memory's row of the last check, or says that the last check had none. */
async function memoryRunAsAnswered(page, last) {
  const line = (await card(page, "memory").locator("p", { hasText: /^Last run:/ }).innerText()).trim();
  if (last === null) {
    ensure(line === "Last run: never", `the memory card says ${JSON.stringify(line)} before any check`);
    return;
  }
  const rows = last.checks.filter((check) => check.area === "memory");
  const found = rows.find((check) => check.result === "failed") ?? rows.find((check) => check.result === "warning") ?? rows.find((check) => check.check === "memory test") ?? rows[0];
  if (found === undefined) ensure(line.startsWith("Last run: not in the last check"), `the memory card says ${JSON.stringify(line)}, and the last check has no memory area`);
  else ensure(line.includes(` · ${found.result}`), `the memory card says ${JSON.stringify(line)}, and the memory's check ${found.check} is ${found.result}`);
}

/** Everything that the page shows of the last check is what the machine says, once the page has read it. */
async function checkAsAnswered(t, page, { earlier = true } = {}) {
  await eventually(async () => {
    const last = await readLast(page);
    ensure(last !== null, `${ROUTES.last} says no check is kept after a check`);
    await lastAsAnswered(t, page, last);
    if (earlier) await earlierAsAnswered(t, page, await readHistory(page), last);
    await memoryRunAsAnswered(page, last);
  }, 60);
}

// --- Before the first check ---

/** Nothing is checked yet: the card says so, the line under the head has no check, and the memory has never been tested. */
async function notChecked(t, page, admin) {
  ensure((await readLast(page)) === null, `${ROUTES.last} gives a check before any was made: hardware.toml puts the machine back before each engine`);
  ensure((await readHistory(page)).length === 0, `${ROUTES.history} has lines before any check was made`);
  await t.see(card(page, "check").getByText(NOT_CHECKED, { exact: true }));
  ensure(!(await lead(page, /^Last check/).isVisible()), "the line under the head says a last check, and none was kept");
  ensure((await card(page, "check").locator("li.hw-check").count()) === 0, "the Last check card lists checks, and none was kept");
  await memoryRunAsAnswered(page, null);
  if (!admin) await t.see(card(page, "check").getByText(READER_CHECK, { exact: true }));
}

// --- The check ---

/** Opens the dialog of Check the hardware, which says what a quick check does, and gives the dialog. */
async function openCheck(t, page) {
  await checkNow(page).click();
  const dialog = page.getByRole("dialog", { name: "Check the hardware", exact: true });
  await t.see(dialog);
  await t.see(dialog.getByText(/^A quick check takes about 3 minutes:/));
  return dialog;
}

/** The six areas are boxes, all on; the full check's box brings its words and takes them away; and with no area chosen, Check now sends nothing. */
async function checkDialogAsked(t, page, dialog) {
  for (const area of AREAS) {
    const box = dialog.getByRole("checkbox", { name: area, exact: true });
    await t.see(box);
    ensure(await box.isChecked(), `the box of ${area} is not on at first`);
  }
  const full = dialog.getByRole("checkbox", { name: "A full check, which takes hours", exact: true });
  const fullWords = dialog.getByText(/^A full check takes hours:/);
  ensure(!(await full.isChecked()) && !(await fullWords.isVisible()), "the full check is on at first, or its words show before it is");
  await full.check();
  await t.see(fullWords);
  await lookAll(t, page, "hardware-check-dialog");
  await full.uncheck();
  await t.gone(fullWords);

  for (const area of AREAS) await dialog.getByRole("checkbox", { name: area, exact: true }).uncheck();
  ensure(await sendsNoPost(page, buttonIn(dialog, "Check now"), pathOf(ROUTES.check)), "Check now sent a check with no area chosen");
  await t.see(dialog.getByText(NO_AREA, { exact: true }));
}

/** Chooses the areas that this engine checks. */
async function chooseAreas(dialog, names) {
  for (const area of AREAS) {
    const box = dialog.getByRole("checkbox", { name: area, exact: true });
    if (names.includes(area)) await box.check();
    else await box.uncheck();
  }
}

/**
 * Presses `control`, which starts a check, and gives `{id, body}`: the task
 * that the console started and what the page sent. `body` is the arguments.
 */
async function startsCheck(t, page, control) {
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.check), { timeout: 120_000 });
  sent.catch(() => {});
  const answer = await pressFor(t, page, control, pathOf(ROUTES.check));
  const text = (await sent).postData() ?? "{}";
  let body = {};
  try {
    body = JSON.parse(text);
  } catch {
    ensure(false, `${ROUTES.check} was sent ${JSON.stringify(text)}, which is not JSON`);
  }
  const reply = await answer.json().catch(() => null);
  if (Number.isInteger(reply?.task)) return { id: reply.task, body };
  const list = await fetchAsPage(page, "GET", "/api/1/tasks");
  const mine = (list.json?.tasks ?? []).filter((task) => task.action === "hardware.check").sort((one, other) => other.id - one.id)[0];
  ensure(mine !== undefined, `${ROUTES.check} gave no task: ${JSON.stringify(reply)}`);
  return { id: mine.id, body };
}

/** While a check runs: the line under the head says so, Check the hardware and the memory test are off, and pressing them sends nothing. */
async function runningShown(t, page, { quick = false } = {}) {
  await t.see(lead(page, /^Checking the hardware\./));
  ensure((await checkNow(page).getAttribute("aria-disabled")) === "true", "Check the hardware is on while a check runs");
  const test = buttonIn(card(page, "memory"), "Test part of the memory");
  ensure((await test.getAttribute("aria-disabled")) === "true", "Test part of the memory is on while a check runs");
  await t.see(card(page, "memory").locator("#memory-why", { hasText: WHY_OFF }));
  if (quick) return;
  ensure(await sendsNoPost(page, test, pathOf(ROUTES.check)), "Test part of the memory started a check while one runs");
  await checkNow(page).click({ force: true });
  await sleep(800);
  ensure((await page.getByRole("dialog", { name: "Check the hardware", exact: true }).count()) === 0, "Check the hardware opened its dialog while a check runs");
  await look(t, page, "hardware-running");
}

/** The task ends well, and the page goes back to what it was: the buttons are on, and the line under the head is the last check's. */
async function endsWell(t, page, id, seconds) {
  const task = await taskEnd(page, id, seconds);
  ensure(task.state === "done", `the check, task ${id}, ended ${task.state}: ${JSON.stringify(task)}`);
  await page.waitForFunction(() => !(document.querySelector("#hw-lead")?.textContent ?? "").startsWith("Checking"), null, { timeout: 120_000 });
  await eventually(async () => {
    ensure((await checkNow(page).getAttribute("aria-disabled")) === null, "Check the hardware is still off after the check");
    ensure((await buttonIn(card(page, "memory"), "Test part of the memory").getAttribute("aria-disabled")) === null, "Test part of the memory is still off after the check");
  }, 60);
}

/** The check that the dialog starts, with the areas this engine checks: it runs to its end, and the page shows what it found. */
async function checkToItsEnd(t, page, names) {
  const dialog = await openCheck(t, page);
  await checkDialogAsked(t, page, dialog);
  await chooseAreas(dialog, names);
  const { id, body } = await startsCheck(t, page, buttonIn(dialog, "Check now"));
  if (names.length === AREAS.length) ensure(body.areas === undefined && body.full === undefined, `a check of every area was sent ${JSON.stringify(body)}, which says all by leaving the areas out`);
  else ensure(body.areas === names.map((name) => ({ Memory: "memory", Processors: "cpu" })[name]).join(" ") && body.full === undefined, `a check of ${names.join(" and ")} was sent ${JSON.stringify(body)}`);
  await t.gone(dialog);
  await runningShown(t, page);
  await endsWell(t, page, id, 15 * 60);
  await checkAsAnswered(t, page, { earlier: false });
  const history = await readHistory(page);
  ensure(history.length === 1, `${ROUTES.history} has ${history.length} lines after one check`);
  ensure((await card(page, "check").locator("details.hw-earlier").count()) === 0, "the card has Earlier checks after the first check, which has none before it");
  await lookAll(t, page, "hardware-checked");
}

/** Test part of the memory is the memory area of a check: it runs, ends, and the last run, the card and Earlier checks follow. */
async function memoryTest(t, page) {
  const test = buttonIn(card(page, "memory"), "Test part of the memory");
  const { id, body } = await startsCheck(t, page, test);
  ensure(body.areas === "memory" && body.full === undefined, `Test part of the memory was sent ${JSON.stringify(body)}, not the memory area`);
  await runningShown(t, page, { quick: true });
  await endsWell(t, page, id, 10 * 60);
  await checkAsAnswered(t, page);
  const history = await readHistory(page);
  ensure(history.length === 2, `${ROUTES.history} has ${history.length} lines after two checks`);
  const last = await readLast(page);
  ensure(last.checks.length > 0 && last.checks.every((check) => check.area === "memory"), `the memory test's result has areas other than memory: ${JSON.stringify(last.checks.map((check) => check.area))}`);
  await lookAll(t, page, "hardware-earlier");
}

// --- Restart or shut down ---

/** Restart… and the memory card's Restart now… ask first, and say what the restart is for; Cancel closes the question, and nothing is sent. */
async function restartIsAsked(t, page, host) {
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.restart)) sent += 1;
  };
  page.on("request", count);
  const dialog = page.getByRole("dialog", { name: `Restart ${host}?`, exact: true });

  await opensDialog(t, page, buttonIn(card(page, "power"), "Restart…"), dialog);
  await t.see(dialog.getByText(new RegExp(`^${escaped(host)} restarts in one minute, and you can cancel until then\\.`)));
  await t.see(buttonIn(dialog, "Restart in a minute"));
  await lookAll(t, page, "hardware-restart-dialog");
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);

  await opensDialog(t, page, buttonIn(card(page, "memory"), "Restart now…"), dialog);
  await t.see(dialog.getByText(new RegExp(`^${escaped(MEMORY_RESTART_WHY)} `)));
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);

  await sleep(800);
  page.off("request", count);
  ensure(sent === 0, "a question that was cancelled sent a restart");
  ensure((await readPower(page)) === null, "the machine has a restart set after two questions that were cancelled");
}

/** A restart at a time of day: a time that is not written is refused in words, a time that is set shows its line with Cancel, the machine says the same, and Cancel takes it back. */
async function restartAtATime(t, page, host) {
  const power = card(page, "power");
  const time = power.getByLabel("Restart at", { exact: true });
  const set = buttonIn(power, "Restart at that time");

  // With no time typed the page says how a time is written, and sends nothing.
  ensure(await sendsNoPost(page, set, pathOf(ROUTES.restart)), "Restart at that time sent a restart with no time typed");
  await t.see(page.getByText(BAD_TIME).first());

  // Twelve hours from now by the lab PC's clock: a time of day that is far from a minute away.
  const later = new Date(Date.now() + 12 * 3600 * 1000);
  const at = `${String(later.getUTCHours()).padStart(2, "0")}:${String(later.getUTCMinutes()).padStart(2, "0")}`;
  await time.fill(at);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.restart), { timeout: 120_000 });
  sent.catch(() => {});
  await pressFor(t, page, set, pathOf(ROUTES.restart));
  const body = (await sent).postData() ?? "";
  ensure(JSON.parse(body).at === at, `Restart at that time sent ${JSON.stringify(body)}, not ${at}`);
  await t.see(pendingLine(page));
  await t.see(pendingLine(page).locator("span[role=timer]", { hasText: new RegExp(`^${escaped(host)} restarts at \\d{1,2}:\\d\\d`) }));
  const waiting = await readPower(page);
  ensure(waiting?.end === "reboot" && waiting.in_seconds > 120 && waiting.in_seconds <= 24 * 3600 + 120, `${ROUTES.power} says ${JSON.stringify(waiting)} for a restart set at ${at}`);
  await lookAll(t, page, "hardware-restart-set");

  await pressFor(t, page, buttonIn(pendingLine(page), "Cancel"), pathOf(ROUTES.cancel));
  await t.gone(pendingLine(page));
  ensure((await readPower(page)) === null, "the machine still has a restart set after Cancel");
}

/** Shut down… says that nothing brings the machine back, keeps its button off until the machine's name is typed, sets the shut down, and Cancel takes it back within the minute. */
async function shutDownAndCancel(t, page, host) {
  const dialog = page.getByRole("dialog", { name: `Shut down ${host}?`, exact: true });
  await opensDialog(t, page, buttonIn(card(page, "power"), "Shut down…"), dialog);
  await t.see(dialog.getByText(`${host} does not come back until someone presses its power button. If nobody can reach it, use Restart instead.`));
  const go = buttonIn(dialog, "Shut down in a minute");
  const typed = dialog.getByLabel(`Type ${host} to shut it down`, { exact: true });
  await t.see(typed);
  await t.see(dialog.getByText(`The button stays off until ${host} is typed exactly as it is written.`));
  ensure(await go.isDisabled(), "Shut down in a minute is on before the machine's name is typed");
  await typed.fill(`${host}x`);
  ensure(await go.isDisabled(), "Shut down in a minute is on with a name that is not the machine's");
  await lookAll(t, page, "hardware-shutdown-dialog");
  await typed.fill(host);
  ensure(await go.isEnabled(), "Shut down in a minute is off with the machine's name typed");

  // From here the minute runs: what is read is read at once, and Cancel is pressed within seconds.
  await pressFor(t, page, go, pathOf(ROUTES.shutdown));
  await t.gone(dialog);
  await t.see(pendingLine(page).locator("span[role=timer]", { hasText: /shuts down in \d+ seconds?$/ }));
  const waiting = await readPower(page);
  ensure(waiting !== null && waiting.end !== "reboot" && waiting.in_seconds <= 61, `${ROUTES.power} says ${JSON.stringify(waiting)} for a shut down that was just set`);
  await pressFor(t, page, buttonIn(pendingLine(page), "Cancel"), pathOf(ROUTES.cancel));
  await t.gone(pendingLine(page));
  ensure((await readPower(page)) === null, "the machine still has a shut down set after Cancel");
}

/** The links of the Overview and the palette lead to the cards, and the card's heading has the keyboard. */
async function hashesLeadToCards(t, page) {
  for (const [hash, id] of [["#check", "check-h"], ["#memory", "memory-h"], ["#power", "power-h"]]) {
    // From another page, so that the page is drawn again with the hash in its address.
    await page.goto(new URL("/", page.url()).href);
    await t.see(page.getByRole("heading", { level: 1, name: "Overview", exact: true }));
    await openHardware(t, page, hash);
    await page.waitForFunction((wanted) => document.activeElement?.id === wanted, id, { timeout: 15_000 });
    ensure((await focusedId(page)) === id, `/hardware${hash} leaves the keyboard on ${JSON.stringify(await focusedId(page))}, not ${id}`);
  }
}

// --- The people and the phone ---

/** An administrator: the machine, the slots after Show the modules unlocks, a check to its end, the memory test, and the power questions. */
async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  const host = await hostOf(page);
  await openHardware(t, page);

  // The console is locked: the slots are not read, and a button asks.
  ensure(!a.seen.has(ROUTES.inventory), `the page asked for ${ROUTES.inventory} before the console was unlocked`);
  mustBe(await askFor(page, ROUTES.inventory), 403, "ADM-106", `a locked administrator's ${ROUTES.inventory}`);
  await machineAsAnswered(t, page, "locked");
  await notChecked(t, page, true);
  await lookAll(t, page, "hardware-first");

  const read = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.inventory), { timeout: 120_000 });
  read.catch(() => {});
  await buttonIn(card(page, "machine"), "Show the modules").click();
  await answerUnlock(t, page);
  const inventory = await (await read).json();
  await eventually(async () => {
    ensure((await buttonIn(card(page, "machine"), "Show the modules").count()) === 0, "Show the modules is still there after the slots were read");
  }, 30);
  await machineAsAnswered(t, page, "ok", inventory);
  await lookAll(t, page, "hardware-modules");

  await checkToItsEnd(t, page, t.engine === "chromium" ? AREAS : SOME);
  await memoryTest(t, page);

  await restartIsAsked(t, page, host);
  await restartAtATime(t, page, host);
  await shutDownAndCancel(t, page, host);
  await hashesLeadToCards(t, page);

  mustHaveCalled(a.seen, Object.values(ROUTES));
  await signOut(t, page);
  await page.context().close();
}

/** A reader sees every fact, the last check and what was before it, and no button; is told who may change the machine; and is refused when a route is asked. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openHardware(t, page);
  await t.see(page.getByText(LEAD_READER, { exact: true }));
  await machineAsAnswered(t, page, "reader");
  await checkAsAnswered(t, page);
  await t.see(card(page, "memory").getByText(READER_TEST, { exact: true }));
  await t.see(card(page, "power").getByText(READER_POWER, { exact: true }));
  ensure((await checkNow(page).count()) === 0, "a reader has Check the hardware");
  const buttons = await page.locator("main .hw button").evaluateAll((all) => all.map((one) => one.textContent.trim()));
  ensure(buttons.length === 0, `a reader's page has these buttons: ${buttons.join(", ")}`);
  ensure((await readPower(page)) === null, "the machine has a restart set, which a reader reads and no one set");

  mustBe(await askFor(page, ROUTES.inventory), 403, "ADM-122", "a reader's slots of the memory");
  mustBe(await askFor(page, ROUTES.check, {}), 403, "ADM-122", "a reader's check");
  mustBe(await askFor(page, ROUTES.restart, {}), 403, "ADM-122", "a reader's restart");
  mustBe(await askFor(page, ROUTES.shutdown, {}), 403, "ADM-122", "a reader's shut down");
  mustBe(await askFor(page, ROUTES.cancel, {}), 403, "ADM-122", "a reader's cancel");
  await lookAll(t, page, "hardware-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the page in one column, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  await openHardware(t, page);
  await t.see(buttonIn(card(page, "power"), "Restart…"));
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main button, main summary, main input[type=time], main a.btn");
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "hardware-phone");

  // The question that the head's button asks is read on a phone too, and closed.
  const dialog = await openCheck(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's check dialog scrolls ${await sidewaysScroll(page)} px sideways`);
  await buttonIn(dialog, "Cancel").click();
  await t.gone(dialog);
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The administrator's check comes first: the reader and the phone read what it found.
  await administrator(t);
  await reader(t);
  await phone(t);
}
