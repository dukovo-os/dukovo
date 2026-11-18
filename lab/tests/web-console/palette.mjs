// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/palette drives in Chromium, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the console's
// command palette (design 11). Ctrl+K opens it with the focus in its field;
// Esc closes it and the focus goes back to where it was. A page found by its
// words is gone to, and so is a card of the Alerts page: "alerts in this
// browser" finds Get alerts in this browser, which goes to /alerts#browser.
// "rest diskh" finds Restart diskhealth first, with the
// command it runs in one line under the row, equal to what
// `POST /api/1/show` answers; Tab opens the form with the Show command panel
// under it; Enter asks to unlock, a cancel keeps the form, and Enter after
// the unlock runs the restart, closes the palette and says it is done. The
// next opening has the Recent group. `state:failed` finds the lab's failed
// service. A read that needs a name says so under its field, and then shows
// the answer; a read that needs nothing shows its table. A reader sees
// "Administrators only" on a change and has no Run button. A phone at 390
// by 844 has rows and buttons of 44 px. Each look is dark, light, high
// contrast, forced colours and less motion, checked by axe.
//
// Chromium only: the shell's test is the one that runs in every engine.
//
// The service that is restarted is diskhealth, which every image has. The
// failed one is lab-broken, which palette.toml puts in place and starts
// before this script runs, so that Spark has given up on it.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
  lookAll,
  mustHaveCalled,
  openConsole,
  press,
  sidewaysScroll,
  signIn,
  signInDone,
  signOut,
  signOutButton,
} from "./console.mjs";

// The password that palette.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The administrator, and the one reader, which palette.toml makes.
const ADMIN = "wc-palette-chromium";
const READER = "wc-palette-reader";

// The service that the restart is of, and the one that has failed.
const SERVICE = "diskhealth";
const BROKEN = "lab-broken";

// The phone of section 14: 390 by 844.
const PHONE = { width: 390, height: 844 };

// The calls that lab/coverage.toml says this test covers, which the browser
// must have made by the end. `{name}` is `SERVICE`.
const ROUTES = [
  "GET /api/1/actions",
  "GET /api/1/services",
  "GET /api/1/certificates",
  "GET /api/1/services/{name}/explain",
  "POST /api/1/show",
  "POST /api/1/unlock",
  "POST /api/1/services/{name}/restart",
];

/** The route as the browser calls it: both the explanation and the restart are of `SERVICE`. */
const called = (route) => route.replace("{name}", SERVICE);

/** The palette: the page's dialog called what the palette's own code calls it. */
const paletteOf = (page) => page.getByRole("dialog", { name: "Search or run a command" });

/** The palette's field, which is a combobox. */
const fieldOf = (palette) => palette.getByRole("combobox");

/** The rows the palette lists. */
const rowsOf = (palette) => palette.getByRole("option");

/** The heading of a view that the palette opened in itself. */
const headingOf = (palette, title) => palette.getByRole("heading", { name: title, level: 2, exact: true });

/** The field for a service's name, which the form labels "Name (needed)". */
const nameFieldOf = (palette) => palette.getByRole("textbox", { name: /^Name/ });

/** The notice that the shell shows when a change is done. */
const noticeOf = (page, words) => page.locator(".notice-words", { hasText: words });

/**
 * Opens the palette with Ctrl+K and checks that its field has the focus.
 * Gives the palette.
 */
async function openPalette(t, page) {
  await page.keyboard.press("Control+k");
  const palette = paletteOf(page);
  await t.see(palette);
  await t.see(fieldOf(palette));
  const inField = await fieldOf(palette).evaluate((field) => field === document.activeElement);
  ensure(inField, "the palette's field does not have the focus when it opens");
  return palette;
}

/** The label of the row in hand, which is the first option unless a key moved it. */
async function rowInHand(palette) {
  const label = await palette.locator("[role=option][aria-selected=true]").first().getAttribute("aria-label");
  ensure(label !== null, "the palette has no row in hand");
  return label;
}

/** What `POST /api/1/show` answers for the restart of `SERVICE`, which the palette's command must equal. */
async function shownBy(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/show", { action: "service.restart", args: { name: SERVICE } });
  ensure(answer.status === 200, `POST /api/1/show was answered ${answer.status}: ${answer.text}`);
  const { command } = answer.json;
  ensure(
    typeof command === "string" && command.startsWith("elevate ") && command.includes(SERVICE),
    `POST /api/1/show answered ${answer.text}`,
  );
  return answer.json;
}

/** Esc closes the palette, and the focus goes back to what had it before Ctrl+K. */
async function closesWithEsc(t, page) {
  await page.evaluate(() => { window.labOpener = document.activeElement; });
  const palette = await openPalette(t, page);
  const opener = await page.evaluate(() => window.labOpener?.tagName ?? null);
  ensure(opener !== null, "the page had no element in focus to give the focus back to");
  ensure((await palette.getByRole("group", { name: "Recent" }).count()) === 0, "the palette has a Recent group before anything was run");
  await page.keyboard.press("Escape");
  await t.gone(palette);
  const back = await page.evaluate(() => document.activeElement === window.labOpener);
  ensure(back, "the focus did not go back to where it was when the palette closed");
}

/**
 * A page is found by its words, and Enter goes to it: the section of the
 * account page, by its hash. "your" keeps out the action "Remove an
 * account's second steps", whose group comes before the pages'.
 */
async function goesToAPage(t, page) {
  const palette = await openPalette(t, page);
  await fieldOf(palette).fill("your second step");
  const row = rowsOf(palette).first();
  await t.see(row);
  const label = await rowInHand(palette);
  ensure(/^Your account › Second step/.test(label), `"your second step" has "${label}" first, not the section of the account page`);
  await page.keyboard.press("Enter");
  await t.gone(palette);
  await t.see(page.getByRole("heading", { name: "Your account", level: 1 }));
  ensure(new URL(page.url()).hash === "#second-step", `Enter went to ${page.url()}, not the second step's section`);
  // The router puts the focus on the h1 once the page has drawn its cards,
  // which is after the heading shows: wait for it.
  const focused = await page.waitForFunction(() => document.activeElement?.tagName === "H1", null, { timeout: 15_000 }).then(
    () => true,
    () => false,
  );
  ensure(focused, "the page does not have the focus after the palette took the person to it");
}

/** The Alerts page's cards are found by their words, for an administrator: "Get alerts in this browser" goes to the card of that name, by its hash. */
async function findsAlerts(t, page) {
  const palette = await openPalette(t, page);
  await fieldOf(palette).fill("alerts in this browser");
  const row = palette.getByRole("option", { name: /^Get alerts in this browser, A notice on this device/ });
  await t.see(row.first());
  ensure((await row.count()) === 1, `"alerts in this browser" has ${await row.count()} rows of the Alerts page's, not one`);
  await row.click();
  await t.gone(palette);
  await t.see(page.getByRole("heading", { name: "Alerts", level: 1, exact: true }));
  const where = new URL(page.url());
  ensure(where.pathname === "/alerts" && where.hash === "#browser", `the row went to ${where.pathname}${where.hash}, not /alerts#browser`);
}

/** "rest diskh" finds the restart first, with its command under the row, and Tab opens the form with the panel. */
async function findsTheRestart(t, page, shown) {
  const palette = await openPalette(t, page);
  await fieldOf(palette).fill("rest diskh");
  await t.see(rowsOf(palette).filter({ hasText: `Restart ${SERVICE}` }).first());
  const label = await rowInHand(palette);
  ensure(label.startsWith(`Restart ${SERVICE}`), `"rest diskh" has "${label}" in hand, not Restart ${SERVICE}`);
  ensure(label.includes("Needs Unlock"), `the row says "${label}", which does not say that it needs the unlock`);
  ensure(
    (await fieldOf(palette).getAttribute("aria-activedescendant")) === (await palette.locator("[role=option][aria-selected=true]").first().getAttribute("id")),
    "aria-activedescendant does not name the row in hand",
  );

  // The command is in one line under the row in hand, and is what admind shows.
  const line = palette.locator("[role=option][aria-selected=true] .cmd-line");
  await t.see(line.filter({ hasText: shown.command }));
  ensure((await line.innerText()).trim() === shown.command, `the row's command is "${await line.innerText()}", not "${shown.command}"`);
  await lookAll(t, page, "palette-list");

  // Tab opens the form of the action, filled from the service, with the panel open.
  await page.keyboard.press("Tab");
  await t.see(headingOf(palette, `Restart ${SERVICE}`));
  const name = nameFieldOf(palette);
  await t.see(name);
  ensure((await name.inputValue()) === SERVICE, `the form's name is "${await name.inputValue()}", not "${SERVICE}"`);
  await t.see(palette.getByRole("tablist", { name: "Show as" }));
  const panel = palette.getByRole("tabpanel");
  await t.see(panel.filter({ hasText: shown.command }));
  ensure((await panel.innerText()).trim() === shown.command, `the panel's command is "${await panel.innerText()}", not "${shown.command}"`);
  await t.see(palette.getByRole("button", { name: shown.command, exact: true }));
  await lookAll(t, page, "palette-run");
  return palette;
}

/** Enter asks to unlock, a cancel keeps the form, and Enter after the unlock runs the restart. */
async function runsAfterTheUnlock(t, page, palette) {
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog);
  await t.see(dialog.getByText(`restart ${SERVICE}`, { exact: true }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  await t.see(headingOf(palette, `Restart ${SERVICE}`));
  ensure((await nameFieldOf(palette).inputValue()) === SERVICE, "a cancelled unlock lost what the form held");

  await page.keyboard.press("Enter");
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.gone(dialog);
  await t.gone(palette);
  await t.see(noticeOf(page, `Restart ${SERVICE}: done.`));
  // Outside the dialog, which has closed and left the page's notice to be read.
  await page.locator("body > [role=status]").filter({ hasText: `Restart ${SERVICE}: done.` }).waitFor({ state: "attached" });
}

/** The next opening has what was run in a group of its own, first with the latest, and the account has kept it. */
async function remembers(t, page) {
  const palette = await openPalette(t, page);
  const groups = await palette.getByRole("group").evaluateAll((all) => all.map((group) => group.getAttribute("aria-label")));
  ensure(groups[0] === "Recent", `the first group is "${groups[0]}", not Recent: ${groups.join(", ")}`);
  const label = await rowInHand(palette);
  ensure(label.startsWith(`Restart ${SERVICE}`), `the Recent group has "${label}" first, not Restart ${SERVICE}`);
  const kept = await page.evaluate(() => Object.keys(localStorage).filter((key) => key.includes("recent.v1.")));
  ensure(kept.some((key) => key.endsWith(`recent.v1.${ADMIN}`)), `the recent rows are not kept under the account's name: ${kept.join(", ")}`);
  return palette;
}

/** `state:failed` lists only what has failed, and the lab's service is there. */
async function findsTheFailed(t, page, palette) {
  await fieldOf(palette).fill("state:failed");
  await t.see(rowsOf(palette).filter({ hasText: BROKEN }).first());
  const labels = await rowsOf(palette).evaluateAll((all) => all.map((row) => row.getAttribute("aria-label")));
  ensure(labels.some((label) => label.startsWith(BROKEN)), `state:failed does not list ${BROKEN}: ${labels.join("; ")}`);
  const others = labels.filter((label) => !/Failed/.test(label));
  ensure(others.length === 0, `state:failed lists what has not failed: ${others.join("; ")}`);
}

/**
 * A read that needs a name says so under its field; with the name it shows
 * what admind explains. The service is one that runs: `service NAME explain`
 * ends with 3 for one that does not, and admind answers that as ADM-123.
 */
async function readsWithAName(t, page, palette) {
  await fieldOf(palette).fill("explain a");
  await t.see(rowsOf(palette).filter({ hasText: "Explain a service" }).first());
  await page.keyboard.press("Enter");
  await t.see(headingOf(palette, "Explain a service"));
  const name = nameFieldOf(palette);
  await t.see(name);
  await page.keyboard.press("Enter");
  await t.see(palette.getByRole("alert").filter({ hasText: /Name is needed/ }));
  ensure((await name.getAttribute("aria-invalid")) === "true", "the field that is needed is not marked invalid");
  ensure(await name.evaluate((field) => field === document.activeElement), "the field that is needed does not have the focus");

  await name.fill(SERVICE);
  await page.keyboard.press("Enter");
  await t.see(palette.getByText("Root", { exact: true }));
  const text = await palette.innerText();
  ensure(new RegExp(`Root\\s+${SERVICE}`).test(text), `the answer does not name ${SERVICE} as the root: ${text}`);
  await lookAll(t, page, "palette-answer");

  // Esc goes back to the list with what was typed, and again closes.
  await page.keyboard.press("Escape");
  await t.see(fieldOf(palette));
  ensure((await fieldOf(palette).inputValue()) === "explain a", "going back lost what was typed");
}

/** A read that needs nothing is run from the list and shows its table. */
async function readsAList(t, page, palette) {
  await fieldOf(palette).fill("list the services");
  await t.see(rowsOf(palette).filter({ hasText: "List the services" }).first());
  await page.keyboard.press("Enter");
  const table = palette.getByRole("table");
  await t.see(table);
  await t.see(table.getByRole("row").filter({ hasText: SERVICE }).first());
  await lookAll(t, page, "palette-table");
  await page.keyboard.press("Escape");
  await t.see(fieldOf(palette));
  await page.keyboard.press("Escape");
  await t.gone(palette);
}

/** A reader sees "Administrators only" on a change, is told why, and has no Run button. */
async function aReader(t, page) {
  await signOut(t, page);
  const answer = await signIn(page, READER, PASSWORD);
  ensure(answer.status() === 200, `${READER} was not signed in: the console answered ${answer.status()}`);
  await t.see(signOutButton(page));
  await leaveWelcome(t, page);

  const palette = await openPalette(t, page);
  await fieldOf(palette).fill("rest");
  await t.see(rowsOf(palette).filter({ hasText: "Restart" }).first());
  const label = await rowInHand(palette);
  ensure(/^Restart .*Administrators only/.test(label), `the reader's row is "${label}", which does not say Administrators only`);
  ensure(!label.includes("Needs Unlock"), `the reader's row is "${label}", which offers an unlock that is not hers`);
  await look(t, page, "palette-reader-list");

  await page.keyboard.press("Tab");
  await t.see(palette.getByRole("status").filter({ hasText: /can look but not change/ }));
  await t.see(palette.getByRole("status").filter({ hasText: /needs an administrator/ }));
  await t.gone(palette.getByRole("button", { name: "Run", exact: true }));
  await lookAll(t, page, "palette-reader");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await t.gone(palette);
  await signOut(t, page);
}

/** On a phone the icon opens the palette, which fills the screen with rows and buttons of 44 px. */
async function onAPhone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);

  await page.getByRole("button", { name: "Search or run a command", exact: true }).click();
  const palette = paletteOf(page);
  await t.see(palette);
  await fieldOf(palette).fill("rest diskh");
  await t.see(rowsOf(palette).filter({ hasText: `Restart ${SERVICE}` }).first());
  const small = async (what) => {
    const found = await palette.evaluate((dialog) =>
      [...dialog.querySelectorAll("button, [role=option], input, summary")]
        .filter((element) => element.offsetParent !== null)
        .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
        .filter(({ box }) => box.width > 0 && box.height < 43.5)
        .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
    );
    ensure(found.length === 0, `${what}: these controls of the palette are under 44 px high on a phone: ${found.join(", ")}`);
    ensure((await sidewaysScroll(page)) === 0, `${what}: the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  };
  await small("the list");
  await lookAll(t, page, "palette-phone-list");

  await page.keyboard.press("Tab");
  await t.see(headingOf(palette, `Restart ${SERVICE}`));
  await t.see(palette.getByRole("tablist", { name: "Show as" }));
  await small("the form");
  await lookAll(t, page, "palette-phone-run");

  // A table is a card to a row on a phone.
  await page.keyboard.press("Escape");
  await fieldOf(palette).fill("list the services");
  await t.see(rowsOf(palette).filter({ hasText: "List the services" }).first());
  await page.keyboard.press("Enter");
  await t.see(palette.locator("table"));
  await small("the table");
  await lookAll(t, page, "palette-phone-table");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await t.gone(palette);
  await signOut(t, page);
}

export default async function check(t) {
  if (t.engine !== "chromium") t.skip("the palette is checked in Chromium, as its board is; the shell's test runs in every engine");

  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, ADMIN, PASSWORD);
  const shown = await shownBy(page);

  await closesWithEsc(t, page);
  await goesToAPage(t, page);
  await findsAlerts(t, page);
  let palette = await findsTheRestart(t, page, shown);
  await runsAfterTheUnlock(t, page, palette);
  palette = await remembers(t, page);
  await findsTheFailed(t, page, palette);
  await readsWithAName(t, page, palette);
  await readsAList(t, page, palette);
  mustHaveCalled(seen, ROUTES.map(called));

  await aReader(t, page);
  await onAPhone(t);
}
