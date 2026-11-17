// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/terminal drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the
// Terminal page, which shows the person's own shells on the terminals admind
// holds. An administrator signs in, and the page makes a terminal and shows
// a shell, where `printf` is typed and its output is read. Read the screen
// has the text. The page splits into panes, takes each layout, moves between
// panes by Alt+Shift and leaves a pane for the tabs by Alt+Shift+Up. The
// page reloaded shows the output again, and so does a second browser of the
// same account. Announcements reach the live region, and a pasted list of
// lines asks first. After Unlock a pane is an administrator's, where `id -u`
// says 0, and Lock asks before it ends the pane. Sessions lists, renames and
// ends terminals, and Settings keeps the preferences. A phone has one pane
// at a time, its key bar and targets of 44 px. A page of another origin
// cannot open the socket, and an account that is not the first one's gets
// ADM-131 for the first one's terminal and pane.
//
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll). Each engine
// has an administrator and an account that is no administrator, which
// terminal.toml made, so no script needs the lab's password.

import {
  answerTo,
  ensure,
  fetchAsPage,
  look,
  lookAll,
  mustHaveCalled,
  openConsole,
  originOf,
  press,
  sessionOf,
  sidewaysScroll,
  signIn,
  signInDone,
  sleep,
  leaveWelcome,
} from "./console.mjs";

// The password that terminal.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The phone of section 14: 390 by 844.
const PHONE = { width: 390, height: 844 };

// The calls that lab/coverage.toml says this test covers, as the page
// makes them: the ids in a path are written {id} and {pane}.
const ROUTES = [
  "GET /api/1/terminals",
  "POST /api/1/terminals",
  "POST /api/1/terminals/{id}/panes",
  "POST /api/1/terminals/{id}/layout",
  "POST /api/1/terminals/{id}/rename",
  "POST /api/1/terminals/{id}/end",
  "POST /api/1/terminals/panes/{pane}/end",
  "GET /api/1/terminal",
];

/** `METHOD /path` with the ids of the terminal routes written as {id} and {pane}. */
const templateOf = (method, path) =>
  `${method} ${path.replace(/^\/api\/1\/terminals\/panes\/\d+\//, "/api/1/terminals/panes/{pane}/").replace(/^\/api\/1\/terminals\/\d+\//, "/api/1/terminals/{id}/")}`;

/** Notes the calls of the context's pages in `seen` as ROUTES names them, and the sockets the pages open as GET. */
function watch(page, seen) {
  const note = (candidate) => {
    candidate.on("websocket", (socket) => seen.add(`GET ${new URL(socket.url()).pathname}`));
  };
  page.context().on("request", (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith("/api/1/terminals")) seen.add(templateOf(request.method(), pathname));
  });
  note(page);
  page.context().on("page", note);
}

/** The pane that has focus, and its screen as the text a person sees. */
const focusedPane = (page) => page.locator(".tr-pane.tr-focused");

/** Waits until the focused pane's screen has `words`. */
const seeOnScreen = (page, words) =>
  page.waitForFunction(
    (wanted) => document.querySelector(".tr-pane.tr-focused .tr-screen")?.innerText.includes(wanted) === true,
    words,
    { timeout: 30_000 },
  );

// How long the focused pane's screen stays the same before a line is typed.
const QUIET_MS = 2_000;

/**
 * Waits until the focused pane's screen shows something and has not changed
 * for QUIET_MS, as a person waits for the prompt. A shell draws its prompt
 * again once the page gives its pane a size, and keys typed while it does may
 * be lost (docs/deferred.md).
 */
async function quiet(page) {
  const screen = focusedPane(page).locator(".tr-screen");
  const end = Date.now() + 30_000;
  let last = null;
  let since = Date.now();
  while (Date.now() < end) {
    const text = await screen.innerText();
    if (text !== last) {
      last = text;
      since = Date.now();
    } else if (text.trim() !== "" && Date.now() - since >= QUIET_MS) {
      return;
    }
    await sleep(100);
  }
  ensure(false, `the focused pane's screen did not stay the same for ${QUIET_MS} ms in 30 s: ${String(last).slice(-200)}`);
}

/**
 * Waits until no pane's screen has changed for QUIET_MS, as a person looks at
 * a layout before choosing the next. Each pane's shell draws its prompt again
 * for the pane's new size, and one that is resized again while it waits for
 * the cursor's answer may draw its prompt over the lines above
 * (docs/deferred.md).
 */
async function settled(page) {
  const screens = page.locator(".tr-pane .tr-screen");
  const end = Date.now() + 30_000;
  let last = null;
  let since = Date.now();
  while (Date.now() < end) {
    const text = (await screens.allInnerTexts()).join("\f");
    if (text !== last) {
      last = text;
      since = Date.now();
    } else if (Date.now() - since >= QUIET_MS) {
      return;
    }
    await sleep(100);
  }
  ensure(false, `the panes' screens did not stay the same for ${QUIET_MS} ms in 30 s`);
}

/** Types a line into the focused pane once its screen is quiet, as a person does. */
async function typeLine(page, text) {
  await quiet(page);
  await page.keyboard.type(text);
  await page.keyboard.press("Enter");
}

/** Clicks into pane number `index` (from 0) of the screen, which puts focus in it. */
async function enter(page, index = 0) {
  await page.locator(".tr-pane .tr-body").nth(index).click();
  await page.waitForFunction(
    (at) => document.querySelectorAll(".tr-pane")[at]?.classList.contains("tr-focused") === true,
    index,
  );
}

/** The boxes of the panes that are shown, left to right and top to bottom as the page has them. */
const paneBoxes = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll(".tr-pane")].map((pane) => {
      const { x, y, width, height } = pane.getBoundingClientRect();
      return { x: Math.round(x), y: Math.round(y), width: Math.round(width), height: Math.round(height) };
    }),
  );

/** The role of the element that has focus. */
const focusedRole = (page) => page.evaluate(() => document.activeElement.getAttribute("role"));

/** The New split button's arrow, which opens the menu of the other things a terminal does. */
const moreActions = (page) => page.getByRole("button", { name: "More terminal actions" });

/** Opens the New menu and chooses `name`; `radio` for a layout. */
async function fromNewMenu(t, page, name, radio = false) {
  await moreActions(page).click();
  const item = page.getByRole(radio ? "menuitemradio" : "menuitem", { name, exact: true });
  await t.see(item);
  await item.click();
}

/** The answer of the console to the next POST whose path matches `pattern`, which the terminal routes with an id in them need. */
const postTo = (page, pattern) =>
  page.waitForResponse((answer) => answer.request().method() === "POST" && pattern.test(new URL(answer.url()).pathname));

/** Ends the pane that has focus from its menu, and confirms when a program runs in it. */
async function endFocusedPane(t, page) {
  await focusedPane(page).getByRole("button", { name: "Pane menu" }).click();
  const item = page.getByRole("menuitem", { name: "End this pane", exact: true });
  await t.see(item);
  const answered = postTo(page, /^\/api\/1\/terminals\/panes\/\d+\/end$/);
  await item.click();
  // A program that runs in the pane makes the page ask; a shell at its prompt does not.
  const dialog = page.getByRole("dialog", { name: "End this pane?" });
  if (await dialog.waitFor({ state: "visible", timeout: 1500 }).then(() => true, () => false)) {
    await dialog.getByRole("button", { name: "End the pane" }).click();
  }
  const answer = await answered;
  ensure(answer.status() === 200, `ending a pane was answered ${answer.status()}`);
}

/** Opens the terminal page, with a terminal made for the account when it has none. */
async function openTerminal(t, page, host, name) {
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.getByRole("tab", { name: `${name}@${host}`, exact: false }).first());
  await t.see(page.locator(".tr-pane").first());
}

/** The first terminal: a shell, `printf`, and its output on the screen. */
async function firstTerminal(t, page, host, name) {
  await page.goto(new URL("/terminal", page.url()).href);
  const tab = page.getByRole("tab", { name: `${name}@${host}`, exact: false });
  await t.see(tab.first());
  ensure((await tab.count()) === 1, `the account has ${await tab.count()} tabs, not the one that the first visit makes`);
  ensure((await page.getByRole("tablist", { name: "Terminals" }).count()) === 1, "the page has no tab bar called Terminals");
  const group = page.getByRole("group", { name: `Pane 1 of 1: ${name}@${host}`, exact: true });
  await t.see(group);
  await enter(page);
  await typeLine(page, "printf 'lab-%s\\n' output");
  await seeOnScreen(page, "lab-output");
  ensure((await page.locator(".tr-screen .tr-row").first().getAttribute("aria-hidden")) === "true", "the drawn rows are not hidden from a screen reader");
  await t.see(page.getByRole("status").filter({ hasText: "Connected" }));
  await lookAll(t, page, "terminal");
}

/** Read the screen has what the screen shows, as text a screen reader can move through. */
async function readTheScreen(t, page) {
  await focusedPane(page).getByRole("button", { name: "Pane menu" }).click();
  await page.getByRole("menuitem", { name: "Read the screen", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Read the screen" });
  await t.see(dialog);
  const text = await dialog.getByRole("textbox", { name: "The screen and the lines above it" }).inputValue();
  ensure(text.includes("lab-output"), `Read the screen has no "lab-output": ${text.slice(0, 200)}`);
  await look(t, page, "terminal-read-the-screen");
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await t.gone(dialog);
}

/** Split, the layouts, Alt+Shift between panes, and the way out of a pane. */
async function splitting(t, page) {
  await fromNewMenu(t, page, "Split right");
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane").length === 2);
  await settled(page);
  let boxes = await paneBoxes(page);
  ensure(boxes[0].y === boxes[1].y && boxes[0].x < boxes[1].x, `a split to the right does not put the panes side by side: ${JSON.stringify(boxes)}`);
  await t.see(page.getByRole("group", { name: /^Pane 2 of 2: / }));
  ensure((await page.getByRole("separator", { name: "Resize panes" }).count()) === 1, "two panes have no separator between them");

  // The second pane has a shell of its own.
  await enter(page, 1);
  await typeLine(page, "printf 'lab-%s\\n' second");
  await seeOnScreen(page, "lab-second");

  // Alt+Shift and an arrow move between the panes.
  await page.keyboard.press("Alt+Shift+ArrowLeft");
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane")[0].classList.contains("tr-focused"));
  await page.keyboard.press("Alt+Shift+ArrowRight");
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane")[1].classList.contains("tr-focused"));

  // The separator moves by the arrow keys, which is what dragging has as an alternative.
  const separator = page.getByRole("separator", { name: "Resize panes" });
  const before = Number(await separator.getAttribute("aria-valuenow"));
  await separator.focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction((was) => Number(document.querySelector(".tr-sep").getAttribute("aria-valuenow")) > was, before);
  await settled(page);

  // A third pane, and each layout.
  await enter(page, 0);
  await fromNewMenu(t, page, "Split down");
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane").length === 3);
  await settled(page);
  await fromNewMenu(t, page, "Rows", true);
  await page.waitForFunction(() => {
    const boxes = [...document.querySelectorAll(".tr-pane")].map((pane) => Math.round(pane.getBoundingClientRect().x));
    return boxes.length === 3 && boxes.every((x) => x === boxes[0]);
  });
  boxes = await paneBoxes(page);
  ensure(boxes[0].y < boxes[1].y && boxes[1].y < boxes[2].y, `Rows does not stack the panes: ${JSON.stringify(boxes)}`);
  await settled(page);
  await fromNewMenu(t, page, "Columns", true);
  await page.waitForFunction(() => {
    const tops = [...document.querySelectorAll(".tr-pane")].map((pane) => Math.round(pane.getBoundingClientRect().y));
    return tops.length === 3 && tops.every((y) => y === tops[0]);
  });
  await settled(page);
  await fromNewMenu(t, page, "Grid", true);
  await page.waitForFunction(() => new Set([...document.querySelectorAll(".tr-pane")].map((pane) => Math.round(pane.getBoundingClientRect().y))).size === 2);
  await settled(page);
  await look(t, page, "terminal-grid");

  // The way out of a pane: Alt+Shift+Up in a top pane puts focus on the tabs.
  await enter(page, 0);
  await page.keyboard.press("Alt+Shift+ArrowUp");
  await page.waitForFunction(() => document.activeElement.getAttribute("role") === "tab");
  ensure((await focusedRole(page)) === "tab", "Alt+Shift+Up did not leave the pane for the tabs");

  // Back to one pane: the two that were added end, from their menus.
  await enter(page, 2);
  await endFocusedPane(t, page);
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane").length === 2);
  await settled(page);
  await enter(page, 1);
  await endFocusedPane(t, page);
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane").length === 1);
  await settled(page);
  await enter(page);
  await seeOnScreen(page, "lab-output");
}

/** The page closed and opened again shows the output, and so does a second browser of the account. */
async function reopened(t, page, host, name) {
  await page.goto(new URL("/", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await openTerminal(t, page, host, name);
  await enter(page);
  await seeOnScreen(page, "lab-output");

  const other = await openConsole(t, "/");
  watch(other.page, other.seen);
  await signInDone(t, other.page, name, PASSWORD);
  await openTerminal(t, other.page, host, name);
  await enter(other.page);
  await seeOnScreen(other.page, "lab-output");
  // What one browser types shows in the other.
  await typeLine(other.page, "printf 'lab-%s\\n' twice");
  await seeOnScreen(other.page, "lab-twice");
  await enter(page);
  await seeOnScreen(page, "lab-twice");
  await other.page.context().close();
}

/** With "Read new output aloud" on, the output of a program reaches the live region. */
async function announcements(t, page) {
  await page.goto(new URL("/terminal/settings", page.url()).href);
  const box = page.getByLabel("Read new output aloud");
  await t.see(box);
  const saved = answerTo(page, "/api/1/account/preferences");
  await box.check();
  ensure((await saved).status() === 200, "the console did not keep the choice to read new output aloud");
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.locator(".tr-pane").first());
  await enter(page);
  await typeLine(page, "printf 'lab-%s\\n' announced");
  await page.waitForFunction(
    () => [...document.querySelectorAll(".tr-foot [aria-live=polite]")].some((region) => region.textContent.includes("lab-announced")),
    undefined,
    { timeout: 30_000, polling: 50 },
  );
  // And off again.
  await page.goto(new URL("/terminal/settings", page.url()).href);
  const off = answerTo(page, "/api/1/account/preferences");
  await page.getByLabel("Read new output aloud").uncheck();
  await off;
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.locator(".tr-pane").first());
}

/**
 * Pasted lines: a shell that asked for bracketed paste (nukesh does) takes
 * them as one paste that runs nothing, with no question; a program that did
 * not ask reads each line as typed, so the page asks first, and nothing is
 * pasted when the person says no.
 */
async function pasting(t, page) {
  await enter(page);
  // Firefox makes a ClipboardEvent's data from members of its own and not
  // from `clipboardData`, so the data is given to the event as its own.
  const paste = (text) =>
    page.evaluate((words) => {
      const data = new DataTransfer();
      data.setData("text/plain", words);
      const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "clipboardData", { value: data });
      document.querySelector(".tr-pane.tr-focused .tr-input").dispatchEvent(event);
    }, text);
  const screen = () => focusedPane(page).locator(".tr-screen").innerText();
  const dialog = page.getByRole("dialog", { name: "Paste?" });

  // At nukesh's prompt: no question, and printf runs only on Enter. Ctrl+C
  // clears the line.
  await paste("printf 'lab-%s\\n' bracketed\nprintf 'lab-%s\\n' bracketed\n");
  await sleep(1000);
  ensure((await dialog.count()) === 0, "a paste into a shell that asked for bracketed paste asked first");
  ensure(!(await screen()).includes("lab-bracketed"), "a bracketed paste ran before Enter");
  await page.keyboard.press("Control+C");
  // The terminal drops what is typed while Ctrl+C ends the line, so cat is
  // typed once the prompt is back, with the status of the line it ended.
  await seeOnScreen(page, "\u2717 130");

  // cat asks for nothing: each pasted line would be read as typed.
  await typeLine(page, "cat");
  await sleep(1000);
  await paste("lab-pasted-no\nlab-pasted-no\nlab-pasted-no\n");
  await t.see(dialog);
  await t.see(dialog.getByText("Paste 3 lines? Each line may run as a command."));
  await look(t, page, "terminal-paste");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  await sleep(1000);
  ensure(!(await screen()).includes("lab-pasted-no"), "a pasted list that the person refused was pasted");

  // One line is pasted without a question; several, once the person agrees.
  await paste("lab-pasted-one");
  await t.gone(dialog);
  await page.keyboard.press("Enter");
  await seeOnScreen(page, "lab-pasted-one");
  await paste("lab-pasted-yes\nlab-pasted-yes\n");
  await t.see(dialog);
  await dialog.getByRole("button", { name: "Paste", exact: true }).click();
  await seeOnScreen(page, "lab-pasted-yes");
  // cat ends at the end of its input, and the shell's prompt is back.
  await page.keyboard.press("Control+D");
}

/** Before Unlock the console refuses an administrator's pane; after it a pane's `id -u` says 0, and Lock asks before it ends the pane. */
async function administratorPane(t, page) {
  const header = page.getByRole("banner");
  const refused = await fetchAsPage(page, "POST", "/api/1/terminals", { admin: true, cols: 80, rows: 24 });
  ensure(refused.status === 403 && refused.json?.error?.id === "ADM-106", `an administrator's terminal while locked was answered ${refused.status} ${refused.json?.error?.id ?? refused.text}, not 403 ADM-106`);

  await header.getByRole("button", { name: "Unlock", exact: true }).click();
  const unlock = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(unlock);
  await unlock.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, unlock.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.see(header.getByText("Unlocked ·"));

  await moreActions(page).click();
  await page.getByRole("menuitem", { name: "New administrator terminal", exact: true }).click();
  const tab = page.getByRole("tab", { name: /^root@.*, administrator/ });
  await t.see(tab);
  const pane = page.getByRole("group", { name: /administrator$/ });
  await t.see(pane);
  await t.see(pane.getByText(/Administrator · ends when the console locks, in \d+ min/));
  await enter(page);
  await typeLine(page, "id -u");
  await page.waitForFunction(
    () => document.querySelector(".tr-pane.tr-focused .tr-screen").innerText.split("\n").some((line) => line.trim() === "0"),
    undefined,
    { timeout: 30_000 },
  );
  await lookAll(t, page, "terminal-administrator");

  // Lock asks first. Cancel leaves the console unlocked and the pane running.
  const question = page.getByRole("dialog", { name: "Lock and end 1 administrator shell?" });
  // It opens on Cancel, so a key pressed by chance ends nothing, and
  // closes with focus back on Lock.
  const lock = header.getByRole("button", { name: "Lock", exact: true });
  await lock.click();
  await t.see(question);
  await look(t, page, "terminal-lock-question");
  const cancel = question.getByRole("button", { name: "Cancel", exact: true });
  ensure(await cancel.evaluate((button) => button === document.activeElement), "the lock question did not open on Cancel");
  await cancel.click();
  await t.gone(question);
  await t.see(header.getByText("Unlocked ·"));
  ensure(await lock.evaluate((button) => button === document.activeElement), "focus did not come back to Lock");
  await enter(page);
  await typeLine(page, "printf 'lab-%s\\n' still-here");
  await seeOnScreen(page, "lab-still-here");

  // Lock and end: the console locks, and the pane says why it ended.
  await header.getByRole("button", { name: "Lock", exact: true }).click();
  await t.see(question);
  const [locked] = await Promise.all([answerTo(page, "/api/1/lock"), question.getByRole("button", { name: "Lock and end", exact: true }).click()]);
  ensure(locked.status() === 200, `Lock was answered ${locked.status()}`);
  await t.see(header.getByText("Limited access"));
  await t.see(pane.getByText("The console locked, which ends administrator panes."));
  await look(t, page, "terminal-administrator-ended");

  // The pane that ended stays until it is closed, and then its tab goes.
  await pane.getByRole("button", { name: "Pane menu" }).click();
  await page.getByRole("menuitem", { name: "Close this pane", exact: true }).click();
  await t.gone(tab);
}

/** Sessions lists the terminals, and renames and ends them. */
async function sessions(t, page, host, name) {
  const address = new URL("/terminal/sessions", page.url()).href;
  await page.goto(address);
  await t.see(page.getByRole("heading", { name: "Sessions", level: 1 }));
  const first = `${name}@${host}`;
  await t.see(page.getByRole("button", { name: `Rename ${first}`, exact: true }));
  await t.see(page.getByRole("link", { name: `Open ${first}`, exact: true }));
  await lookAll(t, page, "terminal-sessions");

  // A second terminal, made by the page's own link.
  await page.getByRole("link", { name: "New terminal", exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll(".tr-tabs [role=tab]").length === 2);
  await page.goto(address);
  await page.waitForFunction(() => document.querySelectorAll("tbody tr").length === 2);

  // The newest is the first row. It is renamed, and then ended: nothing runs in it, so nothing asks.
  await page.locator("tbody tr").first().getByRole("button", { name: /^Rename / }).click();
  const dialog = page.getByRole("dialog", { name: "Rename the terminal" });
  await t.see(dialog);
  await dialog.getByLabel("Name", { exact: true }).fill("lab-second");
  const [renamed] = await Promise.all([postTo(page, /^\/api\/1\/terminals\/\d+\/rename$/), dialog.getByRole("button", { name: "Rename", exact: true }).click()]);
  ensure(renamed.status() === 200, `renaming a terminal was answered ${renamed.status()}`);
  const end = page.getByRole("button", { name: "End lab-second", exact: true });
  await t.see(end);
  const [ended] = await Promise.all([postTo(page, /^\/api\/1\/terminals\/\d+\/end$/), end.click()]);
  ensure(ended.status() === 200, `ending a terminal was answered ${ended.status()}`);
  await page.waitForFunction(() => document.querySelectorAll("tbody tr").length === 1);
}

/** Settings keeps each choice at once, refuses a number out of range, and the text size reaches the page. */
async function settings(t, page) {
  await page.goto(new URL("/terminal/settings", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Terminal settings", level: 1 }));
  const size = page.getByLabel("Text size", { exact: false });
  const saved = answerTo(page, "/api/1/account/preferences");
  await size.fill("16");
  await size.press("Tab");
  ensure((await saved).status() === 200, "the console did not keep the text size");
  const kept = await fetchAsPage(page, "GET", "/api/1/account/preferences");
  ensure(kept.json?.terminal?.size === 16, `the text size kept is ${JSON.stringify(kept.json?.terminal)}`);

  // A number out of range is named in words and not kept.
  const lines = page.getByLabel("Lines kept", { exact: false });
  await lines.fill("500");
  await lines.press("Tab");
  await t.see(page.getByText("Lines kept is a whole number from 1000 to 50000 lines.", { exact: false }));
  ensure((await lines.getAttribute("aria-invalid")) === "true", "the field that is out of range is not marked");
  await lookAll(t, page, "terminal-settings");
  await lines.fill("10000");
  await lines.press("Tab");
  await t.gone(page.getByText("Lines kept is a whole number from 1000 to 50000 lines.", { exact: false }));
  ensure((await fetchAsPage(page, "GET", "/api/1/account/preferences")).json.terminal.scrollback === 10000, "the scrollback that was kept is not 10000");

  // The keys table is a table with the keys named.
  await t.see(page.getByRole("table").filter({ hasText: "Alt+Shift+Up" }));

  // The size reaches the page, and A- and A+ move it by 2.
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.locator(".tr-pane").first());
  const pixels = () => page.locator(".tr-screen").first().evaluate((screen) => Number.parseFloat(getComputedStyle(screen).fontSize));
  ensure((await pixels()) === 16, `the screen's text is ${await pixels()} px, not the 16 that was chosen`);
  await press(page, page.getByRole("button", { name: "Smaller text" }), "/api/1/account/preferences");
  await page.waitForFunction(() => Number.parseFloat(getComputedStyle(document.querySelector(".tr-screen")).fontSize) === 14);
  ensure((await fetchAsPage(page, "GET", "/api/1/account/preferences")).json.terminal.size === 14, "A- did not keep the smaller size");
}

/** A phone: one pane at a time, the key bar, sticky Ctrl, and targets of 44 px. */
async function phone(t, name) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.locator(".tr-pane").first());
  const keys = page.getByRole("toolbar", { name: "Terminal keys" });
  await t.see(keys);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);

  // Two panes show one at a time, with a second row of tabs.
  await fromNewMenu(t, page, "Split right");
  await page.waitForFunction(() => document.querySelectorAll(".tr-pane").length === 1 && document.querySelectorAll(".tr-second [role=tab]").length === 2);
  const second = page.getByRole("tablist", { name: "Panes of this terminal" });
  await t.see(second);
  await second.getByRole("tab", { name: "Pane 1" }).click();
  await t.see(page.getByRole("group", { name: /^Pane 1 of 2: / }));

  // Every control a thumb presses is at least 44 px each way.
  const small = await page.evaluate(() =>
    [...document.querySelectorAll("main button, main a[href], main input, main select, main summary")]
      .filter((element) => element.offsetParent !== null && !element.classList.contains("skip") && !element.classList.contains("tr-input"))
      .map((element) => ({ name: element.getAttribute("aria-label") || element.title || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && (box.height < 43.5 || box.width < 43.5))
      .map(({ name: words, box }) => `${words} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );
  ensure(small.length === 0, `these controls of the terminal are under 44 px on a phone: ${small.join(", ")}`);
  await lookAll(t, page, "terminal-phone");

  // Sticky Ctrl: a program is stopped by Ctrl, then c, and the shell answers again.
  await enter(page);
  await typeLine(page, "sleep 60");
  await sleep(500);
  await keys.getByRole("button", { name: "Ctrl, for the next key" }).click();
  ensure((await keys.getByRole("button", { name: "Ctrl, for the next key" }).getAttribute("aria-pressed")) === "true", "Ctrl on the key bar does not say it is waiting for a key");
  await page.keyboard.press("c");
  await typeLine(page, "printf 'lab-%s\\n' phone");
  await seeOnScreen(page, "lab-phone");
  ensure((await keys.getByRole("button", { name: "Ctrl, for the next key" }).getAttribute("aria-pressed")) === "false", "Ctrl is still waiting after the key it was for");

  // The extra pane ends from its menu, so that the other tests find one.
  await second.getByRole("tab", { name: "Pane 2" }).click();
  await enter(page);
  await endFocusedPane(t, page);
  await page.waitForFunction(() => document.querySelector(".tr-second").hidden);
  await page.context().close();
}

/** A page of another origin cannot open the socket, even with the session's token. */
async function fromAnotherOrigin(t, page) {
  const { hostname, port } = new URL(page.url());
  if (hostname !== "127.0.0.1") {
    console.log(`${hostname} has no other name that this lab PC reaches it by: the page of another origin is not tried`);
    return;
  }
  await page.goto(new URL("/terminal", page.url()).href);
  await t.see(page.locator(".tr-pane").first());
  const { csrf } = await sessionOf(page);
  const attacker = await page.context().newPage();
  const attackerOrigin = `https://localhost:${port}`;
  const attackerPage = `${attackerOrigin}/lab-another-site.html`;
  await attacker.route(attackerPage, (route) =>
    route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>Another site</title>" }),
  );
  await page
    .context()
    .grantPermissions(["local-network-access"], { origin: attackerOrigin })
    .catch(() => undefined);
  await attacker.goto(attackerPage);
  // The strongest case: the page of the other site has the session's token.
  const result = await attacker.evaluate(
    ({ url, token }) =>
      new Promise((resolve) => {
        const socket = new WebSocket(url, ["dukovo-terminal-1", `csrf.${token}`]);
        const done = (what) => {
          resolve(what);
          try {
            socket.close();
          } catch {
            // Closed already.
          }
        };
        socket.onopen = () => done("open");
        socket.onerror = () => done("refused");
        socket.onclose = () => done("closed");
        setTimeout(() => done("no answer"), 20_000);
      }),
    { url: `wss://${hostname}:${port}/api/1/terminal`, token: csrf },
  );
  ensure(result === "refused" || result === "closed", `a page of another origin opened the terminal socket: ${result}`);
  await attacker.close();
  // The person's own page still has its socket.
  await t.see(page.getByRole("status").filter({ hasText: "Connected" }));
}

/** An account that is not the first one's is told there is no such terminal, for its terminal and its pane. */
async function otherAccount(t, page, other) {
  const listed = await fetchAsPage(page, "GET", "/api/1/terminals");
  ensure(listed.status === 200 && listed.json.terminals.length > 0, `the first account has no terminal to ask for: ${listed.status} ${listed.text}`);
  const [{ id, panes }] = listed.json.terminals;
  const pane = panes[0].id;

  const second = await openConsole(t, "/");
  watch(second.page, second.seen);
  await signInDone(t, second.page, other, PASSWORD);
  const mine = await fetchAsPage(second.page, "GET", "/api/1/terminals");
  ensure(mine.status === 200 && !mine.json.terminals.some((terminal) => terminal.id === id), "the second account's list has the first account's terminal");
  for (const [route, body] of [
    [`/api/1/terminals/${id}/panes`, { cols: 80, rows: 24 }],
    [`/api/1/terminals/${id}/layout`, { layout: "columns" }],
    [`/api/1/terminals/${id}/rename`, { name: "taken" }],
    [`/api/1/terminals/${id}/end`, {}],
    [`/api/1/terminals/panes/${pane}/end`, {}],
  ]) {
    const answer = await fetchAsPage(second.page, "POST", route, body);
    ensure(answer.status === 404 && answer.json?.error?.id === "ADM-131", `POST ${route} as another account was answered ${answer.status} ${answer.json?.error?.id ?? answer.text}, not 404 ADM-131`);
  }
  // The socket says the same of the pane.
  const { csrf } = await sessionOf(second.page);
  const said = await second.page.evaluate(
    ({ token, paneId }) =>
      new Promise((resolve) => {
        const socket = new WebSocket(`wss://${location.host}/api/1/terminal`, ["dukovo-terminal-1", `csrf.${token}`]);
        // The socket's own messages (admind's terminal/protocol.rs), not the page's names for them.
        socket.onopen = () => socket.send(JSON.stringify({ attach: paneId, cols: 80, rows: 24 }));
        socket.onmessage = (event) => {
          if (typeof event.data !== "string") return;
          const message = JSON.parse(event.data);
          if ("error" in message || "attached" in message) {
            resolve(message.error ? message.error.id : "attached");
            socket.close();
          }
        };
        socket.onerror = () => resolve("socket error");
        socket.onclose = (event) => resolve(`a close with ${event.code}`);
        setTimeout(() => resolve("no answer"), 20_000);
      }),
    { token: csrf, paneId: pane },
  );
  ensure(said === "ADM-131", `the socket of another account answered ${said} to the first account's pane, not ADM-131`);
  // Nothing of the first account's changed.
  const still = await fetchAsPage(page, "GET", "/api/1/terminals");
  ensure(still.json.terminals.some((terminal) => terminal.id === id), "the first account's terminal went when another account asked for it");
  await second.page.context().close();
}

/** Ends every terminal the account has, so that the account can be removed. */
async function tidy(page) {
  const listed = await fetchAsPage(page, "GET", "/api/1/terminals");
  for (const terminal of listed.json?.terminals ?? []) {
    await fetchAsPage(page, "POST", `/api/1/terminals/${terminal.id}/end`, {});
  }
}

export default async function check(t) {
  const name = `wc-term-${t.engine}`;
  const other = `wc-term-other-${t.engine}`;
  const { page, seen } = await openConsole(t, "/");
  watch(page, seen);
  await signInDone(t, page, name, PASSWORD);
  const session = await sessionOf(page);
  ensure(session.name === name && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);
  ensure(session.terminal === true, `GET /api/1/session says terminal ${session.terminal}: the account has no terminal`);
  const { host } = session;
  const origin = originOf(page);
  ensure(origin.startsWith("https://"), `the console is at ${origin}, not over HTTPS`);

  try {
    await firstTerminal(t, page, host, name);
    await readTheScreen(t, page);
    await splitting(t, page);
    await reopened(t, page, host, name);
    await announcements(t, page);
    await pasting(t, page);
    await settings(t, page);
    await administratorPane(t, page);
    await sessions(t, page, host, name);
    await phone(t, name);
    await fromAnotherOrigin(t, page);
    await otherAccount(t, page, other);
  } finally {
    await tidy(page).catch(() => undefined);
  }
  mustHaveCalled(seen, ROUTES);
}
