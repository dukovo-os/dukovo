// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/errors drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): where the
// console puts an error (design 9.1) and the pages that say what each error
// ID means (9.2), which are files on the machine. An administrator opens
// /help/errors from the Help menu, filters the list, opens one page,
// /help/errors/{id}, and follows a link in it to another; an ID that has no
// page is told so. On the
// account page, a time zone that the machine does not have is refused by
// admind with ADM-125, and the words stand under the field with
// `aria-invalid`, focus in the field, until the person types again.
//
// No page of this console runs an action that fails in the other ways yet, so
// the answers that admind gives for them, in the shape docs/errors says, are
// made by this test as the answer to the account page's one call, the save of
// a time zone: ADM-123 as the error view, with the program's own words and the
// link to its page, ADM-122 as a banner, ADM-129 as plain words in place,
// ADM-105 as a countdown, ADM-112 as the helper that stopped, and ADM-106 as the
// unlock dialog and then the same call. Each is shown in each look of the
// console: dark, light, high contrast, forced colours and less motion. The
// check with the lab's actions that fail on purpose, `lab.missing` among them,
// comes with the first page that runs an action.

import {
  ensure,
  leaveWelcome,
  look,
  lookAll,
  mustHaveCalled,
  openAccountMenu,
  openConsole,
  press,
  signInDone,
  signOut,
  accountItem,
} from "./console.mjs";

// The password that errors.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// A zone that is real, and differs from the account's, so that a save sends a call.
const ZONE = "Pacific/Auckland";

// The routes that lab/coverage.toml says this test covers.
const ROUTES = ["POST /api/1/account/preferences", "POST /api/1/unlock"];

/** Waits for focus to be on the h1 of the page, which is where each route leaves it. */
const seeFocusOnHeading = (page, words) =>
  page.waitForFunction(
    (wanted) => {
      const heading = document.querySelector("main h1");
      return heading !== null && document.activeElement === heading && heading.textContent.trim() === wanted;
    },
    words,
    { timeout: 15_000 },
  );

/** The time zone field of the account page, and the button that saves it. */
const zoneField = (page) => page.getByRole("combobox", { name: "Time zone" });
const saveZone = (page) => page.getByRole("button", { name: "Save the time zone" });

/** The account page. */
async function openAccount(t, page) {
  await accountItem(await openAccountMenu(t, page), "Your account").click();
  await seeFocusOnHeading(page, "Your account");
}

/** Types `zone` in the field and presses the button. */
async function saveWith(page, zone) {
  await zoneField(page).fill(zone);
  await saveZone(page).click();
}

/** Has the next POST to the preferences answered by `status` and `error`, as admind answers, and no other call. */
async function answerOnce(page, status, error, headers = {}) {
  await page.route("**/api/1/account/preferences", (call) => {
    if (call.request().method() !== "POST") return call.fallback();
    return call.fulfill({ status, headers, json: { error } });
  }, { times: 1 });
}

/** The help pages: the list by area with its filter, one page, a link between two, and an ID with no page. */
async function helpPages(t, page) {
  await page.getByRole("banner").getByRole("button", { name: "Help", exact: true }).click();
  await page.getByRole("menuitem", { name: "What error messages mean" }).click();
  await seeFocusOnHeading(page, "What error messages mean");
  ensure(new URL(page.url()).pathname === "/help/errors", `the address is ${page.url()}`);
  // The list is grouped on the area text of the build's index, the README's up to its first colon.
  const area = page.getByRole("heading", { name: "The web console, Dukovo Admin", level: 2 });
  await t.see(area);
  ensure((await page.getByRole("region", { name: "The web console, Dukovo Admin" }).getByRole("link", { name: /^ADM-123 / }).count()) === 1, "ADM-123 is not under its area");
  const link = page.getByRole("link", { name: /^ADM-123 / });
  await t.see(link);
  await lookAll(t, page, "help-list");

  const filter = page.getByLabel("Filter by ID or words");
  await filter.fill("adm-123");
  await t.see(page.getByText("1 page", { exact: true }));
  ensure((await page.getByRole("link", { name: /^ADM-/ }).count()) === 1, "the filter left more than the one page");
  await filter.fill("no such words anywhere");
  await t.see(page.getByText("No page matches.", { exact: true }));
  await filter.fill("");

  await link.click();
  await seeFocusOnHeading(page, "ADM-123: The command did not work");
  ensure(new URL(page.url()).pathname === "/help/errors/ADM-123", `the address is ${page.url()}`);
  ensure((await page.title()).startsWith("ADM-123: The command did not work"), `the tab is called "${await page.title()}"`);
  await t.see(page.getByRole("heading", { name: "What to do", level: 2 }));
  await lookAll(t, page, "help-page");

  // A page that names another error has a link to that page.
  await page.goto(new URL("/help/errors/ADM-129", page.url()).href);
  await seeFocusOnHeading(page, "ADM-129: This server does not have that command yet");
  await page.getByRole("link", { name: "ADM-123", exact: true }).click();
  await seeFocusOnHeading(page, "ADM-123: The command did not work");

  // The crumb goes back to the list, and an ID with no page is told so.
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "What error messages mean" }).click();
  await seeFocusOnHeading(page, "What error messages mean");
  await page.goto(new URL("/help/errors/ADM-999", page.url()).href);
  await seeFocusOnHeading(page, "No such help page");
  await t.see(page.getByText("There is no help page for ADM-999."));
  await look(t, page, "help-none");
}

/** ADM-125 from admind stands under its field, which has focus and is marked; typing takes the words away. */
async function underField(t, page) {
  await saveWith(page, "Mars/Olympus");
  const words = page.getByRole("alert").filter({ hasText: /zone/i });
  await t.see(words);
  ensure((await zoneField(page).getAttribute("aria-invalid")) === "true", "the field is not marked as wrong");
  ensure((await zoneField(page).getAttribute("aria-describedby")).includes("problem"), "the field does not point to its words");
  ensure(await zoneField(page).evaluate((element) => element === document.activeElement), "focus is not in the field");
  await lookAll(t, page, "field-error");
  await zoneField(page).pressSequentially("x");
  await t.gone(words);
  ensure((await zoneField(page).getAttribute("aria-invalid")) === null, "the field is still marked after the person typed");

  // A zone that the machine has is kept.
  await zoneField(page).fill(ZONE);
  await saveZone(page).click();
  await t.see(page.getByText("Saved.", { exact: true }).first());
}

/** ADM-123 with the program's own words: the error view, its link, its details and its disclosure. */
async function errorView(t, page, engine) {
  await answerOnce(page, 500, {
    id: "ADM-123",
    title: "The command did not work",
    detail: "lab: it failed on purpose",
    fix: "Read the program's own words under the message.",
    command: { argv: ["lab", "fail"], status: 1, stderr: "lab: it failed on purpose\nthe second line" },
  });
  await saveWith(page, "Europe/Prague");
  const view = page.getByRole("alert", { name: "The command did not work" });
  await t.see(view);
  await t.see(view.getByText("lab: it failed on purpose", { exact: true }).first());
  await t.see(view.getByText("Read the program's own words under the message.", { exact: true }));
  await t.see(view.getByText("ADM-123", { exact: true }));
  await view.getByText("The program's own message", { exact: true }).click();
  await t.see(view.getByRole("group").getByText("the second line", { exact: false }));
  await lookAll(t, page, "error-view");

  // Copy the details: the ID, the words, the command, its status, the page and the time.
  await view.getByRole("button", { name: "Copy the details" }).click();
  await t.see(view.getByRole("status").filter({ hasText: /Copied\.|Press Ctrl\+C to copy\./ }));
  if (engine === "chromium") {
    const text = await page.evaluate(() => navigator.clipboard.readText());
    for (const part of ["ADM-123: The command did not work", "Command: lab fail", "Status: 1", "HTTP status: 500", "Page: /account", "Time: "]) {
      ensure(text.includes(part), `the details copied have no "${part}":\n${text}`);
    }
  }

  // The ID's link opens the page that says what it means.
  await view.getByRole("link", { name: "What ADM-123 means" }).click();
  await seeFocusOnHeading(page, "ADM-123: The command did not work");
  await page.goBack();
  await seeFocusOnHeading(page, "Your account");
}

/** An account that may look and not change gets admind's words as a banner, and no error view. */
async function readerBanner(t, page) {
  await answerOnce(page, 403, {
    id: "ADM-122",
    title: "This account can look but not change",
    detail: "The account is not an administrator.",
    fix: "Ask an administrator, or ask to be made one.",
  });
  await saveWith(page, "Europe/Prague");
  const banner = page.getByRole("status").filter({ hasText: "This account can look but not change" });
  await t.see(banner);
  await t.see(banner.getByText("Ask an administrator, or ask to be made one.", { exact: false }));
  ensure((await page.getByRole("alert", { name: "This account can look but not change" }).count()) === 0, "ADM-122 is shown as an error view");
  await lookAll(t, page, "reader-banner");
}

/** ADM-129 is said in place, in plain words that name the program. */
async function missingProgram(t, page) {
  await answerOnce(page, 501, {
    id: "ADM-129",
    title: "This server does not have that command yet",
    detail: "`lab-missing` is in neither /system/commands nor /applications/commands.",
    fix: "Update the server.",
  });
  await saveWith(page, "Europe/Prague");
  await t.see(page.getByRole("status").filter({ hasText: "This needs lab-missing, which this server does not have yet." }));
  await look(t, page, "in-place");
}

/** Too many tries: the words count down, and the button waits as long. */
async function waitCountdown(t, page) {
  await answerOnce(page, 429, {
    id: "ADM-105",
    title: "Too many tries: wait a moment",
    detail: "This address made too many requests.",
    fix: "Wait for the time it says.",
  }, { "Retry-After": "4" });
  await saveWith(page, "Europe/Prague");
  const banner = page.getByRole("status").filter({ hasText: "Too many tries. Try again in" });
  await t.see(banner);
  ensure(await saveZone(page).isDisabled(), "the button that was refused does not wait");
  await look(t, page, "wait");
  await t.gone(banner);
  ensure(!(await saveZone(page).isDisabled()), "the button still waits after the time is up");
}

/** The helper for the session stopped: the words and a button that looks at the session again. */
async function helperStopped(t, page) {
  await answerOnce(page, 502, {
    id: "ADM-112",
    title: "The session's worker did not answer",
    detail: "The process that works for the session stopped.",
    fix: "Try again.",
  });
  await saveWith(page, "Europe/Prague");
  const banner = page.getByRole("status").filter({ hasText: "The console's helper for your session stopped." });
  await t.see(banner);
  await look(t, page, "helper-stopped");
  await banner.getByRole("button", { name: "Try again" }).click();
  await t.gone(banner);
}

/** A locked console asks to unlock, and says to press the button again once it is. */
async function locked(t, page) {
  await answerOnce(page, 403, {
    id: "ADM-106",
    title: "The console is locked",
    detail: "Changes need the console unlocked.",
    fix: "Unlock it.",
  });
  await saveWith(page, "Europe/Prague");
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.gone(dialog);
  await t.see(page.getByRole("status").filter({ hasText: "The console is unlocked. Press the button again." }));
  await saveZone(page).click();
  await t.see(page.getByText("Saved.", { exact: true }).first());
  await page.getByRole("banner").getByRole("button", { name: "Lock", exact: true }).click();
}

export default async function check(t) {
  const name = `wc-errors-${t.engine}`;
  const permissions = t.engine === "chromium" ? { permissions: ["clipboard-read", "clipboard-write"] } : {};
  const { page, seen } = await openConsole(t, "/", permissions);
  await signInDone(t, page, name, PASSWORD);
  await leaveWelcome(t, page);

  await helpPages(t, page);
  await openAccount(t, page);
  await underField(t, page);
  await errorView(t, page, t.engine);
  await readerBanner(t, page);
  await missingProgram(t, page);
  await waitCountdown(t, page);
  await helperStopped(t, page);
  await locked(t, page);
  mustHaveCalled(seen, ROUTES);
  await signOut(t, page);
}
