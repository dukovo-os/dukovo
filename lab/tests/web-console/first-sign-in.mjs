// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/first-sign-in drives in each engine, with the lab's
// browser runner (lab repository, docs/test-files.md, "Browser tests"): the
// first sign-in of a new account (design 12.1), and /setup, where an
// administrator goes on from it (design 12.2). The browser is in
// Europe/Prague, so the time zone step has a zone of its own to offer.
//
// An administrator signs in at /account and is sent to /welcome, which keeps
// /account as the page that was asked for. The time zone step says that
// times are shown in the browser's zone and in what zone the server's clock
// is; a zone the server does not have is said under the field, and "Use the
// server's" takes the server's own in one press; a zone it has is kept by
// Next. The display step previews a choice and keeps it at Next. The second
// step is the account's own cards. Skipping it ends the wizard, and since
// pages have steps to set the machine up with (the lab's long action),
// the end is the offer to set it up, which leads to /setup: the step's form,
// the command it runs, Set up, which asks to unlock first, and the step
// done once the task ends. The alert email step is one of the steps: it says
// that it needs the unlock to tell whether email is set up, its form is the
// email's fields, and a Set up with a field wrong says which and runs nothing.
// The alert email step is never set up here, so an administrator's wizard
// ends at the offer in each engine; a lab step that an engine before ran is
// shown as ended at /setup. An account that is
// no administrator adds an authenticator app in the frame, which marks the
// step done, and goes to the page it asked for, and is told that /setup is
// for administrators. An account that presses Skip all keeps the zone that
// is shown, and /welcome does not come back.
//
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll). Each engine
// has accounts of its own, which first-sign-in.toml made, so no script needs
// the lab's password. The lab's long action is in a build made with systools'
// `lab` feature, which `ob image --lab` turns on for the lab image and no
// other image has: a console that does not offer it is a lab image built
// without the feature, and a failure.

import {
  addApp,
  ensure,
  fetchAsPage,
  look,
  lookAll,
  mustHaveCalled,
  mustRefuse,
  openConsole,
  press,
  sessionOf,
  signIn,
  signOut,
  signOutButton,
} from "./console.mjs";
import { sendsNothing } from "./trial.mjs";

// The password that first-sign-in.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The browser's time zone, which no server's clock is in.
const ZONE = "Europe/Prague";

// A zone that no machine has.
const REFUSED = "Mars/Olympus";

// The action that the lab's step runs, and the step's own words.
const LAB_ACTION = "lab.long";
const STEP_TITLE = "Run the lab's long test";

// The alert email step: its title, what it says while the console is locked,
// and the route that Set up must not call while a field is wrong.
const ALERT_TITLE = "Get an email when something needs you";
const ALERT_LOCKED = "Unlock to see if it is set up";
const ALERT_ROUTE = "/api/1/alerts/email";

// The offer says how many steps there are, in words: the lab's has the steps of
// every page that offers one on this machine.
const OFFER_WORDS = /^(One|Two|Three|Four|Five|Six|Seven|Eight|Nine|Ten) steps?, a few minutes each\.$/;

// The calls the test must have made by the end.
const ROUTES = ["GET /api/1/account/preferences", "POST /api/1/account/preferences", "GET /api/1/console/about", "GET /api/1/account/second-step"];

/** Waits for focus to be on `heading`, which is where a step or a page puts it. */
const seeFocusOn = (page, heading) =>
  page.waitForFunction(
    (wanted) => {
      const element = document.activeElement;
      return element !== null && /^H[12]$/.test(element.tagName) && element.textContent.trim() === wanted;
    },
    heading,
    { timeout: 15_000 },
  );

/** The path and the `then` of the address that the page is at. */
function placeOf(page) {
  const url = new URL(page.url());
  return { path: url.pathname, then: url.searchParams.get("then") };
}

/** What the account's preferences are now, as the console answers them to the page. */
async function preferencesOf(page) {
  const answer = await fetchAsPage(page, "GET", "/api/1/account/preferences");
  ensure(answer.status === 200, `GET /api/1/account/preferences answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The classes of the root, which say how the console looks. */
const rootClasses = (page) => page.evaluate(() => [...document.documentElement.classList]);

/** The steps list at the left of the wizard, as its items' words with their spaces made one. */
async function stepsOf(page) {
  const items = await page.getByRole("list", { name: "Steps" }).getByRole("listitem").allInnerTexts();
  return items.map((text) => text.replace(/\s+/g, " ").trim());
}

/** The step of the list that the wizard shows now, by the words of its title. */
const currentStep = (page) => page.locator('[aria-current="step"]');

/** Signs in at `path`, which the console asks for first, and gives what `openConsole` gives. */
async function signInAt(t, path, name) {
  const opened = await openConsole(t, path, { timezoneId: ZONE });
  const answer = await signIn(opened.page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  await t.see(signOutButton(opened.page));
  return opened;
}

/** The wizard is at /welcome, with the page that was asked for in its address and its h1 in focus. */
async function seeWelcome(t, page, host, asked) {
  await t.see(page.getByRole("heading", { name: `Welcome to ${host}`, level: 1 }));
  const place = placeOf(page);
  ensure(place.path === "/welcome", `the first page is ${place.path}, not /welcome`);
  ensure(place.then === asked, `/welcome keeps ${place.then} as the page asked for, not ${asked}`);
  await seeFocusOn(page, `Welcome to ${host}`);
  ensure(
    (await page.title()) === `Welcome to ${host} · ${host} · Dukovo Admin`,
    `the tab is called "${await page.title()}"`,
  );
}

/** The time zone step: the browser's zone, the server's clock, a zone that is refused and one that is kept. */
async function zoneStep(t, page, server) {
  await t.see(page.getByText(`Times in the console are shown in ${ZONE}, your browser's time zone.`, { exact: true }));
  await t.see(page.getByText(`The server's clock is in ${server}.`, { exact: true }));
  const steps = await stepsOf(page);
  ensure(
    steps.length === 3 && steps[0].includes("Time zone") && steps[1].includes("Display") && steps[2].includes("Second step"),
    `the steps are ${steps.join(" | ")}, not Time zone, Display and Second step`,
  );
  ensure((await currentStep(page).innerText()).includes("Time zone"), "the first step is not the one marked as current");
  ensure((await preferencesOf(page)).zone === "", "the account had a time zone before it was asked for one");
  await lookAll(t, page, "welcome-zone");

  // A zone the server does not have is said under the field, and nothing is kept.
  await page.getByRole("button", { name: "Change", exact: true }).click();
  const field = page.getByRole("combobox", { name: "Time zone" });
  await t.see(field);
  ensure(await field.evaluate((element) => element === document.activeElement), "focus is not in the zone field after Change");
  await field.fill(REFUSED);
  const refused = await press(page, page.getByRole("button", { name: "Next", exact: true }), "/api/1/account/preferences");
  await mustRefuse(refused, 400, "ADM-125");
  await t.see(page.getByRole("alert").filter({ hasText: /zone/i }));
  ensure((await field.getAttribute("aria-invalid")) === "true", "the zone field is not marked as wrong");
  ensure((await preferencesOf(page)).zone === "", "a zone the server refused was kept");
  const useServer = page.getByRole("button", { name: `Use the server's: ${server}`, exact: true });
  await t.see(useServer);
  await look(t, page, "welcome-zone-refused");

  // The server's own zone is one press, and goes on to the display.
  await press(page, useServer, "/api/1/account/preferences");
  await t.see(page.getByRole("heading", { name: "Display", level: 2 }));
  ensure((await preferencesOf(page)).zone === server, `the server's zone ${server} was not kept`);

  // Back shows the zone that was kept, and a zone the server has is kept by Next.
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await t.see(page.getByText(`Times in the console are shown in ${server}.`, { exact: true }));
  await field.fill(ZONE);
  const kept = await press(page, page.getByRole("button", { name: "Next", exact: true }), "/api/1/account/preferences");
  ensure(kept.status() === 200, `the zone ${ZONE} was answered ${kept.status()}`);
  await t.see(page.getByRole("heading", { name: "Display", level: 2 }));
  ensure((await preferencesOf(page)).zone === ZONE, `the zone ${ZONE} was not kept`);
}

/** The display step: a choice shows at once and is kept by Next. */
async function displayStep(t, page) {
  const display = page.getByRole("region", { name: "Display" });
  await t.see(display);
  await seeFocusOn(page, "Display");
  ensure((await currentStep(page).innerText()).includes("Display"), "the display is not the step marked as current");
  await lookAll(t, page, "welcome-display");

  // Light shows at once, and is not kept before Next.
  await display.getByText("Light", { exact: true }).click();
  ensure((await rootClasses(page)).includes("light"), "the console is not light after the choice");
  ensure((await preferencesOf(page)).theme !== "light", "a choice that was only previewed was kept");
  const kept = await press(page, page.getByRole("button", { name: "Next", exact: true }), "/api/1/account/preferences");
  ensure(kept.status() === 200, `the display was answered ${kept.status()}`);
  await t.see(page.getByRole("heading", { name: "Second step", level: 2 }));
  ensure((await preferencesOf(page)).theme === "light", "Next did not keep the light colours");
  ensure((await rootClasses(page)).includes("light"), "the console is not light after the choice was kept");
}

/** The second step's cards, which an account with none is offered. */
async function secondStep(t, page) {
  await seeFocusOn(page, "Second step");
  await t.see(page.getByRole("heading", { name: "Authenticator app" }));
  ensure((await currentStep(page).innerText()).includes("Second step"), "the second step is not the step marked as current");
  await lookAll(t, page, "welcome-second");
}

/** Whether the lab's step has been done already, by an engine before this one; fails on an image that does not offer it. */
async function stepDone(page) {
  const listed = await fetchAsPage(page, "GET", "/api/1/actions");
  ensure(listed.status === 200 && Array.isArray(listed.json), `GET /api/1/actions was answered ${listed.text}`);
  ensure(
    listed.json.some((each) => each.id === LAB_ACTION),
    `the console does not offer ${LAB_ACTION}: this lab image was built without systools' lab feature, which \`ob image --lab\` turns on`,
  );
  const tasks = await fetchAsPage(page, "GET", "/api/1/tasks");
  ensure(tasks.status === 200 && Array.isArray(tasks.json?.tasks), `GET /api/1/tasks was answered ${tasks.text}`);
  return tasks.json.tasks.some((task) => task.action === LAB_ACTION && task.state === "done");
}

/** The offer that ends an administrator's wizard, and the press that leads to /setup. */
async function offer(t, page, host) {
  await t.see(page.getByRole("heading", { name: `Set up ${host} now?`, level: 2 }));
  await t.see(page.getByText(OFFER_WORDS));
  await seeFocusOn(page, `Set up ${host} now?`);
  await lookAll(t, page, "welcome-offer");
  await page.getByRole("button", { name: "Set up", exact: true }).click();
}

/** The step of a list's item, by the words of its title. */
const stepItem = (page, title) => page.getByRole("list", { name: "Steps" }).locator("button.step", { hasText: title });

/**
 * The alert email step: in the list, saying the unlock is needed to know whether
 * email is set up, with the email's fields as its form (no kinds, which the
 * step leaves at every one), and Set up with a field wrong names the field and
 * sends nothing. The step that was shown is shown again after.
 */
async function alertStep(t, page) {
  const item = stepItem(page, ALERT_TITLE);
  await t.see(item);
  await t.see(item.getByText(ALERT_LOCKED, { exact: true }));
  await item.click();
  await t.see(page.getByRole("heading", { name: ALERT_TITLE, level: 2 }));
  for (const label of ["Send to", "Mail server", "Port", "Sign in as", "Password", "From (optional)"]) {
    await t.see(page.getByLabel(label, { exact: true }));
  }
  ensure((await page.getByText("What it is told of", { exact: true }).count()) === 0, "the alert email step asks what it is told of, which it leaves at every kind");

  // Nothing typed, and then a word that is no address: each is said, and nothing is sent.
  const run = page.getByRole("button", { name: `Set up: ${ALERT_TITLE}`, exact: true });
  ensure(await sendsNothing(page, run, ALERT_ROUTE), "Set up with no address sent the email's settings");
  await t.see(page.getByText("Type at least one address.", { exact: true }));
  await page.getByLabel("Send to", { exact: true }).fill("nobody");
  ensure(await sendsNothing(page, run, ALERT_ROUTE), "Set up with a word that is no address sent the email's settings");
  await t.see(page.getByText("nobody is not an email address.", { exact: true }));
  await look(t, page, "setup-alert-email");

  await page.getByLabel("Send to", { exact: true }).fill("");
  await stepItem(page, STEP_TITLE).click();
  await t.see(page.getByRole("heading", { name: "This step runs" }));
}

/** /setup: the lab's step, the command it runs, and Set up, which asks to unlock and runs a task that ends. */
async function setup(t, page, host, done) {
  await t.see(page.getByRole("heading", { name: `Make ${host} safe in a few minutes`, level: 1 }));
  ensure(placeOf(page).path === "/setup", `the page is at ${placeOf(page).path}, not /setup`);
  ensure(
    (await page.title()) === `Make ${host} safe in a few minutes · ${host} · Dukovo Admin`,
    `the tab is called "${await page.title()}"`,
  );
  const steps = page.getByRole("list", { name: "Steps" });
  await t.see(steps.getByText(STEP_TITLE));
  await t.see(steps.getByText(done ? "It ran to its end" : "It has not run yet", { exact: true }));
  // The page shows the first step that is not done, which is another page's when it has one: the lab's step is chosen.
  await stepItem(page, STEP_TITLE).click();
  await t.see(page.getByRole("heading", { name: "This step runs" }));
  await t.see(page.getByText(/lab-steps 3/).first());
  await lookAll(t, page, done ? "setup-done" : "setup");
  await alertStep(t, page);
  if (done) {
    await t.see(page.getByRole("button", { name: `Set up again: ${STEP_TITLE}` }));
    return;
  }

  // Set up asks to unlock, and then the task runs and the step is marked.
  const run = page.getByRole("button", { name: `Set up: ${STEP_TITLE}` });
  await run.click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  const started = await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  ensure(started.status() === 200, `the unlock was answered ${started.status()}`);
  await t.see(page.getByText("Started. Its progress is in Tasks.", { exact: true }));
  await t.see(steps.getByText("It ran to its end", { exact: true }));
  ensure((await stepDone(page)) === true, "the step is marked done and the lab's task is not");
  // The console is unlocked now, and the step reads whether email is set up: it is not.
  await t.see(stepItem(page, ALERT_TITLE).getByText("No email yet", { exact: true }));
  await t.see(page.getByRole("button", { name: `Set up again: ${STEP_TITLE}` }));
  await look(t, page, "setup-ran");
}

/** An administrator: the whole wizard, then the offer or the page asked for, and /setup. */
async function administrator(t, name) {
  const { page, seen } = await signInAt(t, "/account", name);
  const session = await sessionOf(page);
  ensure(session.name === name && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);
  const { host } = session;
  const about = await fetchAsPage(page, "GET", "/api/1/console/about");
  ensure(about.status === 200 && typeof about.json?.zone === "string", `GET /api/1/console/about was answered ${about.text}`);
  // The test needs a server clock in a zone of its own, to tell the server's from the browser's.
  ensure(about.json.zone !== "" && about.json.zone !== ZONE, `the server's clock is in "${about.json.zone}", which is empty or the browser's own`);
  const done = await stepDone(page);

  await seeWelcome(t, page, host, "/account");
  await zoneStep(t, page, about.json.zone);
  await displayStep(t, page);
  await secondStep(t, page);

  // The account keeps no second step, and the wizard ends at the offer: the
  // alert email step is left, even when an engine before ran the lab's.
  await page.getByRole("button", { name: "Skip for now", exact: true }).click();
  await offer(t, page, host);
  await setup(t, page, host, done);

  // The wizard is done: the Overview is the first page now, and /welcome is not.
  await page.goto(new URL("/welcome", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  ensure(placeOf(page).path === "/", `/welcome shows ${placeOf(page).path} once the zone is kept, not /`);
  mustHaveCalled(seen, ROUTES);
  await signOut(t, page);
  await page.context().close();
}

/** An account that is no administrator: the second step is added in the frame, and the page asked for follows. */
async function reader(t, name) {
  const { page, seen } = await signInAt(t, "/account", name);
  const { host } = await sessionOf(page);
  await seeWelcome(t, page, host, "/account");
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await t.see(page.getByRole("heading", { name: "Display", level: 2 }));
  ensure((await preferencesOf(page)).zone === ZONE, `the browser's zone ${ZONE} was not kept by Next`);
  await page.getByRole("button", { name: "Skip for now", exact: true }).click();
  await t.see(page.getByRole("heading", { name: "Second step", level: 2 }));

  // An app is added in the frame, which marks the step done and leaves nothing to skip.
  await addApp(t, page, PASSWORD);
  await page.getByRole("button", { name: "I have saved them", exact: true }).click();
  // The frame's page asks the console whether the step is done without the
  // click waiting for the answer, so the list reads "Not added yet" for a
  // moment: it is asked again for up to 15 s.
  let steps = await stepsOf(page);
  for (let end = Date.now() + 15000; !steps[2]?.includes("Added") && Date.now() < end; ) {
    await page.waitForTimeout(250);
    steps = await stepsOf(page);
  }
  ensure(steps[2]?.includes("Added"), `the second step is "${steps[2]}" after the app was added`);
  ensure((await page.locator(".step.done").count()) >= 1, "no step is marked done after the app was added");
  await t.gone(page.getByRole("button", { name: "Skip for now", exact: true }));
  await look(t, page, "welcome-second-done");
  await page.getByRole("button", { name: "Finish", exact: true }).click();

  // No steps are offered to an account that cannot set the machine up: the page asked for follows.
  await t.see(page.getByRole("heading", { name: "Your account", level: 1 }));
  ensure(placeOf(page).path === "/account", `the wizard ended at ${placeOf(page).path}, not /account`);
  ensure((await preferencesOf(page)).theme !== "light", "the display was kept by Skip for now");
  await page.goto(new URL("/setup", page.url()).href);
  await t.see(page.getByText("This page is for administrators."));
  mustHaveCalled(seen, ROUTES);
  await signOut(t, page);
  await page.context().close();
}

/** Skip all keeps the zone that is shown, and ends at the page asked for, and /welcome does not come back. */
async function skipAll(t, name) {
  const { page } = await signInAt(t, "/account", name);
  const { host } = await sessionOf(page);
  await seeWelcome(t, page, host, "/account");
  const kept = await press(page, page.getByRole("button", { name: "Skip all", exact: true }), "/api/1/account/preferences");
  ensure(kept.status() === 200, `Skip all was answered ${kept.status()}`);
  await t.see(page.getByRole("heading", { name: "Your account", level: 1 }));
  ensure((await preferencesOf(page)).zone === ZONE, `Skip all did not keep the zone ${ZONE} that was shown`);
  await page.goto(new URL("/", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t, `wc-welcome-${t.engine}`);
  await reader(t, `wc-welcome-reader-${t.engine}`);
  await skipAll(t, `wc-welcome-skip-${t.engine}`);
}
