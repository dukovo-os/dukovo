// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/show-command drives in each engine, with the lab's
// browser runner (lab repository, docs/test-files.md, "Browser tests"): the
// console's Show command (design 6), which says what a button does as a
// command, a call to the API, an Ansible task and the files it writes. An
// administrator unlocks the console and starts the lab's long action,
// `lab.long`, whose task has the one-line command under its card in the Tasks
// drawer. Pressing that line opens the Show command drawer with its four
// tabs, which hold what `POST /api/1/show` answers for the action, word for
// word, and which the arrow keys, Home and End move through. Copy puts the
// shown text on the clipboard, and "Copy as Ansible task" the task's. A tab
// that has nothing to show says so. Each look of the console is checked: dark,
// light, high contrast, forced colours and less motion.
//
// No page of this console has Show commands' switch yet: the page that first
// has buttons that change something adds the check of the switch and of the
// diff under Files, which needs an action that writes a file.

import {
  ensure,
  fetchAsPage,
  lookAll,
  mustHaveCalled,
  openConsole,
  signInDone,
  signOut,
  sleep,
} from "./console.mjs";

// The password that show-command.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The action the card is of, and how long its task runs: long enough to be
// looked at, and stopped at the end.
const ARGS = { seconds: 90 };

// The tabs, in order.
const TABS = ["Command", "API", "Ansible", "Files"];

// The words of a Files tab that has no file.
const NO_FILE = "This call changes no file.";

// The routes that lab/coverage.toml says this test covers.
const ROUTES = ["POST /api/1/show", "GET /api/1/tasks", "POST /api/1/lab/long", "POST /api/1/tasks/{id}/stop"];

/** The Show command drawer, which is the page's complementary landmark called "Show command". */
const drawerOf = (page) => page.getByRole("complementary", { name: "Show command" });

/** The tab called `name` in `drawer`. */
const tabOf = (drawer, name) => drawer.getByRole("tab", { name, exact: true });

/** The text of the panel that `drawer` shows. */
const panelText = async (drawer) => (await drawer.getByRole("tabpanel").innerText()).trim();

/** What `POST /api/1/show` answers for `lab.long` with `ARGS`, which every tab and every copy must match. */
async function shownBy(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/show", { action: "lab.long", args: ARGS });
  ensure(answer.status === 200, `POST /api/1/show was answered ${answer.status}: ${answer.text}`);
  const { command, api, ansible, files } = answer.json;
  ensure(
    typeof command === "string" && command.startsWith("elevate ") && typeof api === "string" && typeof ansible === "string" && Array.isArray(files),
    `POST /api/1/show answered ${answer.text}`,
  );
  return answer.json;
}

/** The tab's text, as `shown` has it. */
const wanted = (shown, tab) => ({ Command: shown.command, API: shown.api, Ansible: shown.ansible, Files: NO_FILE })[tab];

/** The text the browser's clipboard holds, or the text that is selected when the browser left the copy to the person. */
async function copied(page, engine) {
  const status = (await page.getByRole("status").filter({ hasText: /Copied\.|Press Ctrl\+C to copy\./ }).first().innerText()).trim();
  if (status === "Press Ctrl+C to copy.") return page.evaluate(() => getSelection().toString().trim());
  // Only Chromium can be allowed to read the clipboard back.
  return engine === "chromium" ? (await page.evaluate(() => navigator.clipboard.readText())).trim() : null;
}

/**
 * Opens the Tasks drawer on the card of the lab's long task `id`; gives the
 * card's one-line command. The task an engine before this one stopped needs
 * a look in this browser, which the button's name says first, and has a card
 * of the same name: the card is told from it by the number of its task.
 */
async function openOnCard(t, page, id) {
  await page.getByRole("banner").getByRole("button", { name: /^Tasks, (\d+ needs? a look, )?\d+ running/ }).click();
  const tasks = page.getByRole("complementary", { name: "Tasks" });
  await t.see(tasks);
  const card = tasks.getByRole("region", { name: /^Run the lab's long test/ }).filter({ has: page.getByText(new RegExp(`task ${id}$`)) });
  await t.see(card);
  ensure((await card.count()) === 1, `the card of task ${id} is not in the drawer once`);
  const line = card.getByRole("button", { name: /^Show the command: elevate admin lab-steps/ });
  await t.see(line);
  return line;
}

/** The four tabs: what each holds, the keys that move through them, and that the footnote is there. */
async function tabs(t, page, drawer, shown) {
  await t.see(drawer.getByRole("tablist", { name: "Show as" }));
  for (const name of TABS) await t.see(tabOf(drawer, name));
  ensure((await tabOf(drawer, "Command").getAttribute("aria-selected")) === "true", "the Command tab is not the first one shown");
  for (const name of TABS) {
    await tabOf(drawer, name).click();
    ensure((await tabOf(drawer, name).getAttribute("aria-selected")) === "true", `${name} is not selected after it is pressed`);
    // The panel's text is read trimmed, and the Ansible task ends with a line's end.
    const shownText = wanted(shown, name).trim();
    ensure((await panelText(drawer)) === shownText, `the ${name} tab holds "${await panelText(drawer)}", not "${shownText}"`);
  }
  await t.see(drawer.getByText("What you see here is what runs.", { exact: false }));

  // The keys: Right and Left, Home and End, with the selected tab the one a key reaches.
  await tabOf(drawer, "Command").click();
  await tabOf(drawer, "Command").focus();
  await page.keyboard.press("ArrowRight");
  ensure((await tabOf(drawer, "API").getAttribute("aria-selected")) === "true", "ArrowRight did not select the API tab");
  await page.keyboard.press("End");
  ensure((await tabOf(drawer, "Files").getAttribute("aria-selected")) === "true", "End did not select the Files tab");
  await page.keyboard.press("ArrowRight");
  ensure((await tabOf(drawer, "Command").getAttribute("aria-selected")) === "true", "ArrowRight at the end did not go round to Command");
  await page.keyboard.press("ArrowLeft");
  ensure((await tabOf(drawer, "Files").getAttribute("aria-selected")) === "true", "ArrowLeft at the start did not go round to Files");
  await page.keyboard.press("Home");
  ensure((await tabOf(drawer, "Command").getAttribute("aria-selected")) === "true", "Home did not select the Command tab");
}

/** Copy puts the tab's text on the clipboard, and "Copy as Ansible task" the task's. */
async function copying(t, page, drawer, shown, engine) {
  for (const name of TABS) {
    await tabOf(drawer, name).click();
    await drawer.getByRole("button", { name: "Copy", exact: true }).click();
    await t.see(drawer.getByRole("status").filter({ hasText: /Copied\.|Press Ctrl\+C to copy\./ }));
    const text = await copied(page, engine);
    if (text !== null) {
      // The Files tab copies the diffs of its files, which are none here. The
      // clipboard is read trimmed, and the Ansible task ends with a line's end.
      const expected = name === "Files" ? "" : wanted(shown, name).trim();
      ensure(text === expected, `Copy on the ${name} tab put "${text}" on the clipboard, not "${expected}"`);
    }
  }
  await tabOf(drawer, "Command").click();
  await drawer.getByRole("button", { name: "Copy as Ansible task" }).click();
  const text = await copied(page, engine);
  ensure(text === null || text === shown.ansible.trim(), `Copy as Ansible task put "${text}" on the clipboard, not the task`);
  // The button is for the Command tab only.
  await tabOf(drawer, "API").click();
  await t.gone(drawer.getByRole("button", { name: "Copy as Ansible task" }));
}

export default async function check(t) {
  const name = `wc-show-${t.engine}`;
  const permissions = t.engine === "chromium" ? { permissions: ["clipboard-read", "clipboard-write"] } : {};
  const { page, seen } = await openConsole(t, "/", permissions);
  await signInDone(t, page, name, PASSWORD);
  await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  const asked = await fetchAsPage(page, "POST", "/api/1/lab/long", ARGS);
  ensure(asked.status === 202 && Number.isInteger(asked.json?.task), `lab.long was answered ${asked.status} ${asked.text}`);
  const id = asked.json.task;
  const shown = await shownBy(page);

  // The card has the command in one line; pressing it opens the drawer.
  const line = await openOnCard(t, page, id);
  ensure((await line.innerText()).trim() === shown.command, `the card's line is "${await line.innerText()}", not "${shown.command}"`);
  await line.click();
  const drawer = drawerOf(page);
  await t.see(drawer);
  await t.see(drawer.getByRole("heading", { name: "Show command", level: 2 }));
  await tabs(t, page, drawer, shown);
  await copying(t, page, drawer, shown, t.engine);

  // Each look of the console, with the drawer open on the first tab and on the last.
  await tabOf(drawer, "Command").click();
  await lookAll(t, page, "show-command");
  await tabOf(drawer, "Files").click();
  await t.see(drawer.getByText(NO_FILE));
  await lookAll(t, page, "show-command-files");

  // Esc closes the drawer.
  await drawer.getByRole("heading", { name: "Show command", level: 2 }).focus();
  await page.keyboard.press("Escape");
  await t.gone(drawer);

  // The task ends here, so that none is left to run.
  const stopped = await fetchAsPage(page, "POST", `/api/1/tasks/${id}/stop`, {});
  ensure(stopped.status === 200, `the stop was answered ${stopped.status}: ${stopped.text}`);
  await sleep(1000);
  mustHaveCalled(seen, ROUTES.map((route) => route.replace("{id}", String(id))));
  await signOut(t, page);
}
