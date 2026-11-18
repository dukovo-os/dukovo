// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/report drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Report a problem
// page at /report. A crash that crash-test left, which reportd keeps and
// `GET /api/1/reports/crashes` lists, is a choice of its own; choosing it, or
// opening /report?about=crash&id=, has reportd draft the report
// (`POST /api/1/reports/drafts`, which keeps the draft) and the page shows every
// line of the text, a row for each, with what redaction took out marked. What
// the system gathered can be cut: Cut line on a line, and Cut these words on
// words that are selected (`POST /api/1/reports/drafts/{draft}/redact`, which
// takes the whole list of cuts, against the lines as they were first drafted,
// and the hash of the text that was read). Edit your answers writes the three
// answers again, Undo all my changes asks for the list with no cuts and the
// first text comes back, and Draft again asks first when it would bring cuts
// back. The security box keeps the text, and Save to a file and Download by
// QR code are off while it is checked, since such a report is sent encrypted
// only, to the key Send names. Send (`POST /api/1/reports/send`) gives reportd the draft and the
// hash of the text read, and the staging server of the lab PC
// (lab/tests/reports/report-server.py, which report.toml starts) keeps the bytes
// of the text that was on the screen, byte for byte. Save to a file saves the
// text on the screen, Download by QR code
// (`POST /api/1/reports/drafts/{draft}/phone`) draws a link as a code with the
// certificate's fingerprint, and `GET /once/{token}/report.txt` gives the text
// to the first request and refuses the next. The table of Reports from this
// server (`GET /api/1/reports`) lists the report sent, Check the status asks the
// server (`GET /api/1/reports/status`) and Delete takes it off the server and
// the table (`POST /api/1/reports/{number}/delete`). A reader is offered
// neither a failed start nor a service, and a phone at 390 by 844 shows the
// page with no sideways scroll and every control 44 px tall. Each look is the
// console's own: dark, light, high contrast, forced colours and less motion,
// each checked by axe (console.mjs's lookAll).
//
// The script runs once for each engine, report.toml having left a crash for it
// and the staging server running. The QR code is held to the console's own
// encoder, loaded in the page, since the lab has no decoder: the page's picture
// must be the one the encoder makes of the link it shows. The accounts are the
// two that report.toml made, so no script needs the lab's password.
//
// A report that waits for the network, and Retry now, are report-queued.mjs and
// report-retry.mjs, which use the helpers that this script exports.

import { Buffer } from "node:buffer";
import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { promisify } from "node:util";
import {
  ensure,
  fetchAsPage,
  lookAll,
  openConsole,
  originOf,
  servedCertificate,
  signIn,
  signInDone,
  signOut,
  sidewaysScroll,
  sleep,
  leaveWelcome,
} from "./console.mjs";
import { PASSWORD, PHONE, pressAndAnswer, smallControls, unlockNow } from "./trial.mjs";

const run = promisify(execFile);

const ADMIN = "wc-report";
const READER = "wc-report-reader";

// What report.toml writes for the script: the staging server's folder, and
// report-server.py, which sets what a report's status says.
const DESK_FILE = "/tmp/lab-report-desk";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them. `{draft}`, `{token}` and `{number}` are the ID, the 32 hex
// digits and the number that the page and the machine give.
const ROUTES = {
  crashes: "GET /api/1/reports/crashes",
  draft: "POST /api/1/reports/drafts",
  redact: "POST /api/1/reports/drafts/{draft}/redact",
  phone: "POST /api/1/reports/drafts/{draft}/phone",
  once: "GET /once/{token}/report.txt",
  send: "POST /api/1/reports/send",
  list: "GET /api/1/reports",
  status: "GET /api/1/reports/status",
  remove: "POST /api/1/reports/{number}/delete",
};

const DRAFT = /^\/api\/1\/reports\/drafts$/;
const REDACT = /^\/api\/1\/reports\/drafts\/[0-9a-f]{32}\/redact$/;
const PHONE_LINK = /^\/api\/1\/reports\/drafts\/[0-9a-f]{32}\/phone$/;
const SEND = "/api/1/reports/send";
const DELETE = /^\/api\/1\/reports\/\d+\/delete$/;

// The words the page says where this test looks for them.
const NOTHING_CHOSEN = "Choose what went wrong to see the report.";
const ADMIN_ONLY = "A report about a failed start or about a service is made by an administrator.";
const FIXED_CHOICES = ["The hardware", "Installing or updating Dukovo", "Something else"];

// --- The staging server of the lab PC ---

/** Where report.toml says the staging server keeps its reports, and where its script is: `{folder, script}`. */
async function desk() {
  const [folder, script] = (await readFile(DESK_FILE, "utf8")).trim().split("\n");
  ensure(folder && script, `${DESK_FILE} does not give the staging server's folder and script`);
  return { folder, script };
}

/** What the staging server holds of report `number`, as bytes decoded as UTF-8, or null when it holds none. */
export async function deskCopy(number) {
  const { folder } = await desk();
  const file = `${folder}/${number}.txt`;
  return existsSync(file) ? readFile(file, "utf8") : null;
}

/** Has the staging server say that report `number` is fixed in 0.16.1, as the people who handle reports do. */
async function markFixed(number) {
  const { folder, script } = await desk();
  await run("python3", ["-I", script, "set-status", folder, String(number), "fixed", "--fixed-in", "0.16.1"]);
}

// --- Reading the machine ---

/** The crash of crash-test that reportd keeps and has not had reported, waited for. Gives its record. */
async function unreportedCrash(page, seconds = 90) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const answer = await fetchAsPage(page, "GET", "/api/1/reports/crashes");
    ensure(answer.status === 200, `${ROUTES.crashes} was answered ${answer.status}: ${answer.text}`);
    const crashes = Array.isArray(answer.json) ? answer.json : answer.json?.crashes;
    ensure(Array.isArray(crashes), `${ROUTES.crashes} gives no list of crashes: ${answer.text}`);
    const found = crashes
      .filter((crash) => typeof crash?.program === "string" && crash.program.includes("crash-test") && !crash.reported)
      .sort((a, b) => b.at_ms - a.at_ms)[0];
    if (found) return found;
    ensure(Date.now() < until, `reportd kept no crash of crash-test that is not reported within ${seconds} seconds`);
    await sleep(2000);
  }
}

/** The words of a size as the page writes them. */
const sizeWords = (bytes) => (bytes < 1000 ? `${bytes} B` : `${(bytes / 1000).toFixed(1)} KB`);

// --- Signing in ---

/** Opens the console in a context of its own and signs in as `name`. */
export async function signedIn(t, name, options = {}) {
  const { page, seen } = await openConsole(t, "/", options);
  await signInDone(t, page, name, PASSWORD);
  return { page, seen };
}

/** The requests the page makes from now on, as `{method, path, body}`. */
function watch(page) {
  const calls = [];
  page.on("request", (request) => {
    calls.push({ method: request.method(), path: new URL(request.url()).pathname, body: request.postData() ?? "" });
  });
  return calls;
}

/** The call of `method` to a path that `pattern` matches, the last one, or undefined. */
const lastCall = (calls, method, pattern) => [...calls].reverse().find((call) => call.method === method && pattern.test(call.path));

// --- The page ---

export const card = (page, heading) => page.locator("section.card").filter({ has: page.getByRole("heading", { name: heading, level: 2 }) });
export const questions = (page) => card(page, "What went wrong?");
const text = (page) => page.locator("pre.rp-text");
const cutOf = (page, name) => text(page).getByRole("button", { name, exact: true });
export const choiceOf = (page, title) => page.locator("label.radio-card", { hasText: title }).first();

/** The lines of the text on the screen, each as its row's words. */
const linesOf = (page) => text(page).locator(".rl-text").evaluateAll((cells) => cells.map((cell) => cell.textContent));

/** The text on the screen as it would be saved or sent: its lines, each ended by a line feed. */
async function shown(page) {
  return `${(await linesOf(page)).join("\n")}\n`;
}

/** Waits until the page is not drafting: the text it holds is the one reportd last answered. */
async function settled(page) {
  await page.waitForFunction(() => document.querySelector("pre.rp-text")?.getAttribute("aria-busy") === "false", null, { timeout: 60_000 });
}

/** Opens `/report` with `query` and waits for the page. */
export async function openReport(t, page, query = "") {
  await page.goto(new URL(`/report${query}`, page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Report a problem", exact: true }));
  await t.see(questions(page));
}

/** Waits for a text that has been drafted, and gives the lines of it. */
export async function draftShown(t, page) {
  await t.see(text(page));
  await settled(page);
  return linesOf(page);
}

/**
 * Does `act`, which makes the page ask for a text with some cuts, and gives
 * what it asked: `{body, edits}`, the request's JSON and the edit list in it.
 */
async function editsAfter(page, act) {
  const isRedact = (request) => request.method() === "POST" && REDACT.test(new URL(request.url()).pathname);
  const asked = page.waitForRequest(isRedact);
  const answered = page.waitForResponse((reply) => isRedact(reply.request()));
  await act();
  const body = JSON.parse((await asked).postData() ?? "{}");
  const reply = await answered;
  ensure(reply.status() === 200, `${ROUTES.redact} was answered ${reply.status()}: ${await reply.text()}`);
  ensure(typeof body.edits === "string" && /^[0-9a-f]{64}$/.test(body.sha256), `${ROUTES.redact} was sent ${JSON.stringify(body)}, not the hash of the text read and the edit list as a JSON text`);
  await settled(page);
  return { body, edits: JSON.parse(body.edits) };
}

/** Whether the Draft again question is shown, and the words in it. */
const question = (page) => page.getByRole("dialog", { name: "Draft again?", exact: true });

// --- The choices ---

/** The choices are the machine's: a card for the crash, the three that are always there at the end, and nothing for a failed start or a service where there is none. */
async function choicesAsRead(t, page, crash) {
  const titles = await page.locator("label.radio-card b").evaluateAll((nodes) => nodes.map((node) => node.textContent));
  ensure(titles.includes(`${crash.program} stopped with an error`), `the choices ${JSON.stringify(titles)} have none for the crash of ${crash.program}`);
  ensure(JSON.stringify(titles.slice(-3)) === JSON.stringify(FIXED_CHOICES), `the last choices are ${JSON.stringify(titles.slice(-3))}, not ${JSON.stringify(FIXED_CHOICES)}`);
  await t.see(page.getByText(NOTHING_CHOSEN, { exact: true }));
  for (const name of ["Send", "Save to a file…", "Download by QR code…"]) {
    ensure(await page.getByRole("button", { name, exact: true }).isDisabled(), `${name} is on before anything is drafted`);
  }
}

/** /report?about=crash&id= chooses the crash and drafts it at once, with the note after the choice's own line. */
async function chosenByAddress(t, page, calls, crash) {
  await openReport(t, page, `?about=crash&id=${encodeURIComponent(crash.id)}&note=${encodeURIComponent("Found by the lab.")}`);
  await draftShown(t, page);
  const draft = lastCall(calls, "POST", DRAFT);
  ensure(draft !== undefined, `the page asked for no draft: it never called ${ROUTES.draft}`);
  const asked = JSON.parse(draft.body);
  ensure(asked.about === "crash" && asked.crash === crash.id, `the draft was asked for as ${draft.body}, not the crash ${crash.id}`);
  ensure(asked.happened.startsWith(`${crash.program} stopped with an error`) && asked.happened.endsWith(" Found by the lab."), `the first answer is ${JSON.stringify(asked.happened)}, the choice's line and the note`);
  await t.see(choiceOf(page, `${crash.program} stopped with an error`).locator("input:checked"));
}

// --- The text ---

/** What the text on the screen is: the first line, the part of the crash with its frames, the answers marked as typed, the lines that can be cut each with Cut line, and the size in words. */
async function textAsDrafted(t, page, crash) {
  const lines = await linesOf(page);
  ensure(lines[0] === "Dukovo problem report", `the text begins with ${JSON.stringify(lines[0])}`);
  for (const wanted of ["== What happened", "== Crash"]) ensure(lines.includes(wanted), `the text has no ${wanted} line: ${JSON.stringify(lines)}`);
  ensure(lines.some((line) => /SIGSEGV/.test(line)) && lines.some((line) => line.includes("fault_here")), `the text of the crash of ${crash.program} has no signal or no frame of its own`);
  const gathered = await text(page).locator(".rl.gathered").count();
  ensure(gathered > 0, "no line of the text can be cut");
  ensure((await text(page).getByRole("button", { name: /^Cut line \d+ of /, includeHidden: true }).count()) === gathered, "a line the system gathered has no Cut line");
  await t.see(text(page).locator(".rl-tag", { hasText: "Typed by you" }).first());
  for (const heading of ["== What happened"]) {
    const row = text(page).locator(".rl.heading", { hasText: heading });
    ensure((await row.locator("button").count()) === 0, `${heading} is a line that can be cut`);
  }
  // The size in the card's heading is the bytes of the text.
  const size = await card(page, /^Exactly what is sent/).locator("h2 .more").innerText();
  const bytes = Buffer.byteLength(await shown(page));
  ensure(size === sizeWords(bytes), `the card says ${JSON.stringify(size)} for ${bytes} bytes`);
  await t.see(card(page, /^What the report holds/));
  await t.see(page.getByText(/^(Taken out before anything is shown: |Nothing needed to be taken out)/));
  await t.see(page.getByText(/^A mark such as \[user\] shows what was taken out/));
}

/** The marks: words typed that name an account and an address are cut out of the text and shown as marks, in the card's count too. */
async function marksShown(t, page) {
  await choiceOf(page, "Something else").click();
  const happened = questions(page).getByLabel("What happened?", { exact: true });
  await t.see(happened);
  await happened.fill(`${ADMIN} saw the address 10.1.2.3 on the screen`);
  await happened.blur();
  await draftShown(t, page);
  const marks = await text(page).locator("mark.taken").evaluateAll((nodes) => nodes.map((node) => node.textContent));
  ensure(marks.includes("[user]") && marks.includes("[IPv4]"), `the marks in the text are ${JSON.stringify(marks)}, not [user] and [IPv4] for what was typed`);
  const lines = await linesOf(page);
  ensure(lines.includes("[user] saw the address [IPv4] on the screen"), `the typed line reads ${JSON.stringify(lines)}`);
  const said = await card(page, /^What the report holds/).innerText();
  ensure(/account name/.test(said), `What the report holds does not count the account name: ${JSON.stringify(said)}`);
  ensure(/address/.test(said), `What the report holds does not count the address: ${JSON.stringify(said)}`);
  await lookAll(t, page, "report-marks");
  await page.getByRole("button", { name: "Discard", exact: true }).click();
  await t.see(page.getByText(NOTHING_CHOSEN, { exact: true }));
}

// --- The cuts ---

/** Selects the characters `from` to `to` of the words of the row `n` as a person drags over them. */
async function select(page, n, from, to) {
  await page.evaluate(
    ({ n, from, to }) => {
      const node = document.querySelector(`pre.rp-text .rl[data-n="${n}"] .rl-text`).firstChild;
      const range = document.createRange();
      range.setStart(node, from);
      range.setEnd(node, to);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    },
    { n, from, to },
  );
}

/**
 * The lines the system gathered whose words are one piece of text with no
 * mark in it, as `{n, words, line, title}`: `n` is the row, `line` the number
 * Cut line says for it (from 1) in the part named `title`. The row `except`
 * is left out.
 */
async function pickable(page, except = -1) {
  const found = await text(page).locator(".rl.gathered").evaluateAll(
    (nodes, skip) =>
      nodes
        .map((node) => {
          const cell = node.querySelector(".rl-text");
          const label = /^Cut line (\d+) of (.*)$/.exec(node.querySelector("button")?.getAttribute("aria-label") ?? "");
          return { n: Number(node.getAttribute("data-n")), plain: cell.childNodes.length === 1 && cell.firstChild.nodeType === 3, words: cell.textContent, line: Number(label?.[1]), title: label?.[2] ?? "" };
        })
        .filter((row) => row.n !== skip && row.plain && row.words.length >= 12)
        .map(({ n, words, line, title }) => ({ n, words, line, title })),
    except,
  );
  ensure(found.length >= 2, `the text has ${found.length} lines of plain words that can be cut, and the test needs two`);
  return found;
}

/** Presses Cut line of the line `picked` as a person does, pointing at it first, and gives the edit list that was asked for. */
async function pressCutLine(page, picked) {
  await text(page).locator(`.rl[data-n="${picked.n}"]`).hover();
  const { edits } = await editsAfter(page, () => cutOf(page, `Cut line ${picked.line} of ${picked.title}`).click());
  return edits;
}

/** Selected words are cut with Cut these words: the edit list has the characters of the drafted line, and the line reads [cut] in their place. Gives the cut. */
async function cutWords(t, page, picked) {
  const [from, to] = [0, 5];
  await select(page, picked.n, from, to);
  const button = page.getByRole("button", { name: "Cut these words", exact: true });
  await t.see(button);
  const { edits } = await editsAfter(page, () => button.click());
  const [cut] = edits.cuts;
  ensure(edits.cuts.length === 1 && cut.line === picked.line - 1 && cut.from === from && cut.to === to && typeof cut.section === "string", `Cut these words asked for ${JSON.stringify(edits)}, not characters ${from} to ${to} of line ${picked.line - 1}`);
  ensure(edits.answers === undefined, `Cut these words asked for ${JSON.stringify(edits)}, which has answers in it`);
  // Found by its Cut line, which names the line as drafted: the head gains a line that counts the cuts, which moves the rows.
  const row = text(page).locator(`.rl:has(> .rl-cut[aria-label=${JSON.stringify(`Cut line ${picked.line} of ${picked.title}`)}]) .rl-text`);
  ensure((await row.innerText()) === `[cut]${picked.words.slice(to)}`, `the line reads ${JSON.stringify(await row.innerText())} after its words were cut`);
  await t.see(page.getByText("You cut 1 part of a line.", { exact: true }));
  ensure((await linesOf(page)).some((line) => /^Cut by the sender: 1 part of a line/.test(line)), "the head of the text does not say what was cut");
  return cut;
}

/** Cut line takes a line out of the text, and the list keeps the words cut before as they were. Gives the list. */
async function cutLine(t, page, picked, before) {
  const edits = await pressCutLine(page, picked);
  const whole = edits.cuts.filter((cut) => cut.from === undefined);
  const words = edits.cuts.filter((cut) => cut.from !== undefined);
  ensure(edits.cuts.length === 2 && whole.length === 1 && whole[0].line === picked.line - 1 && (whole[0].through === undefined || whole[0].through === whole[0].line), `Cut line asked for ${JSON.stringify(edits)}, not line ${picked.line - 1} and the words cut before`);
  ensure(JSON.stringify(words[0]) === JSON.stringify(before), `the words cut before are ${JSON.stringify(words)} in the list, not ${JSON.stringify(before)}`);
  const lines = await linesOf(page);
  ensure(!lines.includes(picked.words), `the line ${JSON.stringify(picked.words)} is still in the text`);
  await t.see(page.getByText("You cut 1 line and 1 part of a line.", { exact: true }));
  ensure(lines.some((line) => /^Cut by the sender: 1 line, 1 part of a line/.test(line)), `the head of the text does not count the cuts: ${JSON.stringify(lines.slice(0, 8))}`);
  return edits;
}

/** The three answers are written again in place: the form has what they were, Done asks for the text with the new ones and the cuts kept, and what is typed is cut like the rest. */
async function answersWrittenAgain(t, page, cuts) {
  await page.getByRole("button", { name: "Edit your answers", exact: true }).click();
  const form = page.locator("form.rp-editor");
  await t.see(form);
  const happened = form.getByLabel("What happened?", { exact: true });
  ensure((await happened.inputValue()).trim() !== "", "the first answer is empty in the form that writes it again");
  await happened.fill(`${ADMIN} wrote this again`);
  await form.getByLabel("What did you expect?", { exact: true }).fill("It does not stop");
  const { edits } = await editsAfter(page, () => form.getByRole("button", { name: "Done", exact: true }).click());
  ensure(edits.answers?.happened === `${ADMIN} wrote this again` && edits.answers?.expected === "It does not stop" && edits.answers?.again === "", `the answers asked for were ${JSON.stringify(edits.answers)}`);
  ensure(JSON.stringify(edits.cuts) === JSON.stringify(cuts), `the cuts were ${JSON.stringify(edits.cuts)} after the answers were written again, not ${JSON.stringify(cuts)}`);
  await t.gone(form);
  const lines = await linesOf(page);
  ensure(lines.includes("[user] wrote this again") && lines.includes("It does not stop"), `the answers read ${JSON.stringify(lines)} after they were written again, with the name cut out`);
}

/** Undo all my changes asks for no cuts and no answers, and the first text is back, line for line. */
async function undone(t, page, first) {
  const { edits } = await editsAfter(page, () => page.getByRole("button", { name: "Undo all my changes", exact: true }).click());
  ensure(Array.isArray(edits.cuts) && edits.cuts.length === 0 && edits.answers === undefined, `Undo all my changes asked for ${JSON.stringify(edits)}`);
  ensure(JSON.stringify(await linesOf(page)) === JSON.stringify(first), "the text after Undo all my changes is not the text that was first drafted");
  await t.gone(page.getByRole("button", { name: "Undo all my changes", exact: true }));
}

/** The drafts the page has asked for so far. */
const draftsOf = (calls) => calls.filter((call) => call.method === "POST" && DRAFT.test(call.path)).length;

/** Draft again with cuts asks first, in the words of what comes back: Keep my cuts leaves everything, choosing something else asks the same, and Draft again drafts anew with the cut line back. */
async function draftAgainAsks(t, page, calls, crash, first) {
  await pressCutLine(page, (await pickable(page))[0]);
  const cutText = await shown(page);
  const drafts = draftsOf(calls);
  const dialog = question(page);

  await page.getByRole("button", { name: "Draft again", exact: true }).first().click();
  await t.see(dialog);
  await t.see(dialog.getByText("Drafting again brings back the 1 line you cut. Draft again?", { exact: true }));
  await lookAll(t, page, "report-question");
  await dialog.getByRole("button", { name: "Keep my cuts", exact: true }).click();
  await t.gone(dialog);
  ensure((await shown(page)) === cutText, "the text changed when the cuts were kept");
  ensure(draftsOf(calls) === drafts, "a draft was asked for when the cuts were kept");

  // Choosing something else asks the same question, and keeping the cuts puts the choice back.
  await choiceOf(page, "The hardware").click();
  await t.see(dialog);
  await dialog.getByRole("button", { name: "Keep my cuts", exact: true }).click();
  await t.gone(dialog);
  await t.see(choiceOf(page, `${crash.program} stopped with an error`).locator("input:checked"));
  ensure(draftsOf(calls) === drafts, "a draft was asked for when the choice was put back");

  await page.getByRole("button", { name: "Draft again", exact: true }).first().click();
  await t.see(dialog);
  await dialog.getByRole("button", { name: "Draft again", exact: true }).click();
  await t.gone(dialog);
  await draftShown(t, page);
  ensure(draftsOf(calls) === drafts + 1, "Draft again did not ask for a new draft");
  ensure((await linesOf(page)).length === first.length, "the new draft is not as long as the first, with the cut line back");
  await t.gone(page.getByRole("button", { name: "Undo all my changes", exact: true }));
}

// --- The security box ---

/**
 * A security report is for the team's key, which Send names: the box keeps
 * the text and asks for no new draft, Send stays on, and Save to a file and
 * Download by QR code are off, with why. The box can be taken off again.
 */
async function securityBox(t, page, calls) {
  const box = questions(page).getByLabel(/^This is a security problem\./);
  const drafts = draftsOf(calls);
  const before = await shown(page);
  await box.check();
  await t.see(page.getByText("A security report is sent encrypted only", { exact: false }));
  await settled(page);
  ensure(draftsOf(calls) === drafts, "the security box asked for a new draft");
  ensure((await shown(page)) === before, "the text changed when the security box was checked");
  ensure(await page.getByRole("button", { name: "Send", exact: true }).isEnabled(), "Send is off for a security report");
  for (const name of ["Save to a file…", "Download by QR code…"]) {
    ensure(await page.getByRole("button", { name, exact: true }).isDisabled(), `${name} is on for a security report`);
  }
  await lookAll(t, page, "report-security");
  await box.uncheck();
  await draftShown(t, page);
  ensure(await page.getByRole("button", { name: "Send", exact: true }).isEnabled(), "Send is off again after the box was taken off");
  ensure(await page.getByRole("button", { name: "Save to a file…", exact: true }).isEnabled(), "Save to a file is off again after the box was taken off");
}

// --- Save to a file, and the QR code ---

/** Save to a file saves the text on the screen under the day's name, and sends nothing. */
async function savedToAFile(t, page, calls) {
  const before = calls.filter((call) => call.method === "POST" && call.path === SEND).length;
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 30_000 }), page.getByRole("button", { name: "Save to a file…", exact: true }).click()]);
  const file = await download.path();
  ensure(file !== null, `the browser kept no file for ${download.suggestedFilename()}`);
  const named = /^report-(\d{4}-\d{2}-\d{2})\.txt$/.exec(download.suggestedFilename());
  ensure(named !== null, `the saved file is called ${download.suggestedFilename()}, not report-YYYY-MM-DD.txt`);
  ensure(named[1] === new Date().toISOString().slice(0, 10), `the saved file is named for ${named[1]}, which is not today in UTC`);
  const saved = await readFile(file, "utf8");
  ensure(saved === (await shown(page)), "the saved file is not the text on the screen");
  await t.see(page.getByText(`Saved as ${download.suggestedFilename()}. Nothing was sent.`, { exact: true }));
  ensure(calls.filter((call) => call.method === "POST" && call.path === SEND).length === before, "Save to a file sent the report");
  return saved;
}

/**
 * Asks for the one-time link at `address`, as a phone does. admind counts
 * each request for a link with the sign-in requests of the address, so that
 * a link cannot be guessed, and in the second engine this script's own
 * sign-ins from the lab PC have used the minute up. A 429 is waited out for
 * the seconds its Retry-After gives and the same request is made once more:
 * the limit is checked before the link is looked at, so the refusal has not
 * used the link up.
 */
async function askedFor(page, address) {
  const answer = await page.request.get(address);
  if (answer.status() !== 429) return answer;
  await sleep(Number.parseInt(answer.headers()["retry-after"], 10) * 1000);
  return page.request.get(address);
}

/** The dialog: a QR code that is the one the console's encoder makes of the link it shows, the link as text, the fingerprint of the certificate the console serves, and a countdown. The first request for the link gets the text on the screen and the next is refused. */
async function qrCode(t, page, calls, wanted) {
  const asked = page.waitForResponse((reply) => reply.request().method() === "POST" && PHONE_LINK.test(new URL(reply.url()).pathname));
  await page.getByRole("button", { name: "Download by QR code…", exact: true }).click();
  const reply = await asked;
  ensure(reply.status() === 200, `${ROUTES.phone} was answered ${reply.status()}: ${await reply.text()}`);
  const body = JSON.parse(lastCall(calls, "POST", PHONE_LINK).body);
  ensure(/^[0-9a-f]{64}$/.test(body.sha256), `${ROUTES.phone} was sent ${JSON.stringify(body)}, not the hash of the text read`);
  const dialog = page.getByRole("dialog", { name: "Download by QR code", exact: true });
  await t.see(dialog);
  const link = await dialog.locator("p.cmdtext").first().innerText();
  const found = /^https:\/\/\S+\/once\/([0-9a-f]{32})\/report\.txt$/.exec(link.trim());
  ensure(found !== null, `the dialog shows the link ${JSON.stringify(link)}, not https://ADDRESS/once/TOKEN/report.txt with a token of 32 hex digits`);
  await t.see(dialog.getByText(/^The link ends in /));
  await t.see(dialog.getByRole("timer"));
  ensure((await dialog.getByText(/does not know this server's certificate/).count()) > 0, "the dialog does not say why the phone warns of the certificate");

  // The picture is the encoder's own, of the link as written.
  const shownPath = await dialog.locator("svg path").last().getAttribute("d");
  const made = await page.evaluate(async (address) => {
    const { qrSvg } = await import("/qr.js");
    return qrSvg(address, "check").querySelector("path").getAttribute("d");
  }, link.trim());
  ensure(shownPath === made, "the picture in the dialog is not the QR code of the link it shows");

  // The fingerprint is the certificate the console serves.
  const groups = (await dialog.locator("p.cmdtext").nth(1).innerText()).trim().split(/\s+/);
  const served = (await servedCertificate(originOf(page))).fingerprint256.replaceAll(":", "").toUpperCase();
  ensure(groups.join(":").replaceAll(":", "").toUpperCase() === served, `the dialog shows the fingerprint ${groups.join(" ")}, and the console's certificate has ${served}`);
  ensure(groups.length === 4 && groups.every((group) => /^([0-9A-F]{2}:){7}[0-9A-F]{2}$/.test(group)), `the fingerprint is not in four groups of eight bytes: ${groups.join(" ")}`);
  await lookAll(t, page, "report-qr");

  // The lab PC stands in for the phone: it asks for the link by the address it reached the console by.
  const address = `${originOf(page)}/once/${found[1]}/report.txt`;
  const first = await askedFor(page, address);
  ensure(first.status() === 200, `the first request for the link was answered ${first.status()}`);
  ensure(/^text\/plain/.test(first.headers()["content-type"] ?? "") && /attachment/.test(first.headers()["content-disposition"] ?? "") && /report\.txt/.test(first.headers()["content-disposition"] ?? ""), `the link is served as ${first.headers()["content-type"]} with ${first.headers()["content-disposition"]}`);
  ensure(/no-store/.test(first.headers()["cache-control"] ?? ""), "the link's answer may be kept");
  ensure((await first.text()) === wanted, "the link gives a text that is not the one on the screen");
  const second = await askedFor(page, address);
  ensure(second.status() === 410 && (await second.text()).includes("ADM-140"), `the second request for the link was answered ${second.status()}, not 410 with ADM-140`);
  const unknown = await askedFor(page, `${originOf(page)}/once/${"0".repeat(32)}/report.txt`);
  ensure(unknown.status() === 404, `a token that was never made was answered ${unknown.status()}, not 404`);

  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
}

// --- Send, and the table ---

/** Send gives reportd the draft and the hash of the text read, says the number, and starts the page over; the staging server holds the bytes that were on the screen. Gives the report's number. */
async function sent(t, page, calls, wanted, cutLineWords, crash) {
  await pressAndAnswer(t, page, page.getByRole("button", { name: "Send", exact: true }), SEND);
  const body = JSON.parse(lastCall(calls, "POST", /^\/api\/1\/reports\/send$/).body);
  ensure(/^[0-9a-f]{32}$/.test(body.draft) && /^[0-9a-f]{64}$/.test(body.sha256), `${ROUTES.send} was sent ${JSON.stringify(body)}, not a draft and the hash of the text read`);
  ensure(body.crash === crash.id && typeof body.title === "string" && body.title.length > 0 && [...body.title].length <= 80, `${ROUTES.send} was sent ${JSON.stringify(body)}, not the crash and a title of at most 80 characters`);
  const status = page.locator(".rp-status");
  await t.see(status.getByText(/^Sent as report \d+\.$/));
  const number = Number(/\d+/.exec(await status.innerText())[0]);
  await t.see(page.getByText(NOTHING_CHOSEN, { exact: true }));
  await t.gone(choiceOf(page, `${crash.program} stopped with an error`));

  // What the staging server keeps is what the person read.
  const copy = await deskCopy(number);
  ensure(copy !== null, `the staging server holds no report ${number}`);
  ensure(copy === wanted, `the staging server's copy of report ${number} is not the text that was on the screen`);
  ensure(/^Cut by the sender: 1 line/m.test(copy) && !copy.includes(cutLineWords), `the staging server's copy of report ${number} lacks the line that says what was cut, or has the line that was cut`);
  return number;
}

/** The table has the report with its title, Check the status asks the server and shows what it says, and the status changes with the server's. */
async function tableAsSent(t, page, calls, number, crash) {
  const table = card(page, "Reports from this server");
  const row = table.locator("tbody tr").filter({ has: page.getByRole("button", { name: `Delete report ${number}`, exact: true }) });
  await t.see(row);
  ensure((await row.innerText()).includes(`${crash.program} stopped with an error`), `the row of report ${number} does not have its title`);
  ensure(lastCall(calls, "GET", /^\/api\/1\/reports$/) !== undefined, `the page never called ${ROUTES.list}`);
  await table.getByRole("button", { name: "Check the status", exact: true }).click();
  await t.see(row.getByText("Received", { exact: true }));
  ensure(lastCall(calls, "GET", /^\/api\/1\/reports\/status$/) !== undefined, `Check the status never called ${ROUTES.status}`);
  await lookAll(t, page, "report-table");

  await markFixed(number);
  await table.getByRole("button", { name: "Check the status", exact: true }).click();
  await t.see(row.getByText("Fixed in 0.16.1"));

  // Delete asks first, and then takes the report off the server and the table.
  const dialog = page.getByRole("dialog", { name: `Delete report ${number}?`, exact: true });
  await row.getByRole("button", { name: `Delete report ${number}`, exact: true }).click();
  await t.see(dialog);
  await dialog.getByRole("button", { name: "Keep it", exact: true }).click();
  await t.gone(dialog);
  ensure((await deskCopy(number)) !== null, "Keep it took the report off the staging server");
  await row.getByRole("button", { name: `Delete report ${number}`, exact: true }).click();
  await t.see(dialog);
  const deleted = page.waitForResponse((reply) => reply.request().method() === "POST" && DELETE.test(new URL(reply.url()).pathname));
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  const answer = await deleted;
  ensure(answer.status() === 200, `${ROUTES.remove} was answered ${answer.status()}: ${await answer.text()}`);
  await t.gone(row);
  ensure((await deskCopy(number)) === null, `report ${number} is still on the staging server after Delete`);
}

// --- The people and the phone ---

/** An administrator: the choices and the marks, a draft by the address, the cuts, the answers written again, what is saved and what the phone gets, Undo, the Draft again question, a security report, Send, and the table. */
async function administrator(t) {
  const { page } = await signedIn(t, ADMIN);
  const calls = watch(page);
  await unlockNow(page);
  const crash = await unreportedCrash(page);

  await openReport(t, page);
  await choicesAsRead(t, page, crash);
  await lookAll(t, page, "report-start");
  await marksShown(t, page);

  await chosenByAddress(t, page, calls, crash);
  await textAsDrafted(t, page, crash);
  const first = await linesOf(page);
  await lookAll(t, page, "report-drafted");

  // Words, then a line: the list keeps both against the lines as first drafted.
  const [one] = await pickable(page);
  const words = await cutWords(t, page, one);
  const [two] = await pickable(page, one.n);
  const edits = await cutLine(t, page, two, words);
  await lookAll(t, page, "report-cut");
  await answersWrittenAgain(t, page, edits.cuts);

  // What is saved, and what the phone gets, is what is on the screen with the cuts and the answers in it.
  const wanted = await shown(page);
  ensure((await savedToAFile(t, page, calls)) === wanted, "Save to a file did not save the text on the screen");
  await qrCode(t, page, calls, wanted);

  await undone(t, page, first);
  await draftAgainAsks(t, page, calls, crash, first);
  await securityBox(t, page, calls);

  // Sent with a line cut: the staging server has the head's count and lacks the line.
  const [last] = await pickable(page);
  await pressCutLine(page, last);
  const toSend = await shown(page);
  const number = await sent(t, page, calls, toSend, last.words, crash);
  await tableAsSent(t, page, calls, number, crash);

  for (const [route, pattern] of [
    [ROUTES.crashes, /^\/api\/1\/reports\/crashes$/],
    [ROUTES.draft, DRAFT],
    [ROUTES.redact, REDACT],
    [ROUTES.phone, PHONE_LINK],
    [ROUTES.send, /^\/api\/1\/reports\/send$/],
    [ROUTES.list, /^\/api\/1\/reports$/],
    [ROUTES.status, /^\/api\/1\/reports\/status$/],
    [ROUTES.remove, DELETE],
  ]) {
    ensure(calls.some((call) => call.method === route.split(" ")[0] && pattern.test(call.path)), `the test never called ${route}`);
  }
  await signOut(t, page);
  await page.context().close();
}

/** A reader is offered neither the failed start nor a service, is told so, can draft what they are offered, and is refused a service by the route. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openReport(t, page);
  const titles = await page.locator("label.radio-card b").evaluateAll((nodes) => nodes.map((node) => node.textContent));
  ensure(JSON.stringify(titles.slice(-3)) === JSON.stringify(FIXED_CHOICES), `a reader's last choices are ${JSON.stringify(titles.slice(-3))}`);
  ensure(titles.every((title) => !/ failed$|^Start \d+ failed$|was taken back$/.test(title)), `a reader is offered ${JSON.stringify(titles)}, which has a failed start or a service`);
  await t.see(page.getByText(ADMIN_ONLY, { exact: true }));

  await openReport(t, page, "?about=failure");
  await t.see(page.getByText(/is an administrator's to send/));
  ensure((await page.locator("label.radio-card input:checked").count()) === 0, "a reader's address chose a failed start");

  await openReport(t, page);
  await choiceOf(page, "The hardware").click();
  const happened = questions(page).getByLabel("What happened?", { exact: true });
  await happened.fill("The lab's reader asks");
  await happened.blur();
  await draftShown(t, page);
  ensure(await page.getByRole("button", { name: "Send", exact: true }).isEnabled(), "a reader cannot send the report");

  const refused = await fetchAsPage(page, "POST", "/api/1/reports/drafts", { about: "service", name: "nginx", happened: "x" });
  ensure(refused.status >= 400 && refused.json?.text === undefined, `a reader's draft about a service was answered ${refused.status}: ${refused.text.slice(0, 200)}`);
  await lookAll(t, page, "report-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the choices, a draft and its text, with controls a thumb reaches and no sideways scroll. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  const controls = "main .rp button, main .rp .btn, main .rp .radio-card, main .rp summary, main .rp-table button";
  await openReport(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, controls);
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  await lookAll(t, page, "report-phone-start");

  await choiceOf(page, "Something else").click();
  const happened = questions(page).getByLabel("What happened?", { exact: true });
  await happened.fill("The lab's phone asks");
  await happened.blur();
  await draftShown(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's text scrolls the page ${await sidewaysScroll(page)} px sideways`);
  const smallText = await smallControls(page, controls);
  ensure(smallText.length === 0, `these controls of the phone's report are under 44 px tall: ${smallText.join(", ")}`);
  for (const name of ["Send", "Save to a file…", "Download by QR code…"]) {
    const box = await page.getByRole("button", { name, exact: true }).boundingBox();
    ensure(box !== null && box.width >= 300, `${name} is ${box?.width} px wide on a phone, not the width of the screen`);
  }
  await lookAll(t, page, "report-phone-drafted");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  // The reader first: the administrator sends the crash.
  await reader(t);
  await administrator(t);
  await phone(t);
}
