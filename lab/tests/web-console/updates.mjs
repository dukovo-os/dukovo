// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/updates drives in each engine, with the lab's browser runner
// (lab repository, docs/test-files.md, "Browser tests"): the Updates page at
// /updates, against the test update server that updates.toml has the lab PC
// serve and the machine read. The cards are held to what the machine says: the
// System, Firmware and Apps cards and the Security card to `update last
// --json` (`GET /api/1/updates`) and `update check --json` (`GET
// /api/1/updates/check`, which Check now asks), How this server updates to
// `update settings --json` (`GET /api/1/updates/settings`), the checks after
// an update to `update checks --json`. Save changes the settings and the
// channel (`POST /api/1/updates/settings` and `POST /api/1/updates/channel`)
// and Undo puts them back; Hold this version and Release the hold change the
// system's hold (`POST /api/1/updates/{item}/hold` and `POST
// /api/1/updates/{item}/unhold`), a hold asks "Why (optional)" first and the
// Held card says when, by whom and why; Later puts
// the reminder of an update off for a day or a week and Remind me now brings
// it back (`POST /api/1/updates/{item}/later`), unless the update fixes an
// advisory, which is never put off and says so; Stage it stages the
// system's release, a task (`POST /api/1/updates/{item}/apply`), which the
// banner "Restart to finish the update" tells of, and Take it back unstages it
// (`POST /api/1/updates/unstage`). A check is added and removed, and so is a
// service that must run again (`POST /api/1/updates/checks`, `POST
// /api/1/updates/checks/{name}/remove` and `POST /api/1/updates/essential`).
// The advisories and the history are read when asked for (`GET
// /api/1/updates/advisories` and `GET /api/1/updates/history`), and the
// failed starts as the page opens (`GET /api/1/updates/failed`). An
// app that the check offers an update for is updated from the Apps card, a
// task (`POST /api/1/apps/{id}/update`), and put back through the route that
// the app's own page calls (`POST /api/1/apps/{id}/rollback`); Update all, on
// the Apps page while two apps or more wait for an update, updates every one
// of them as a task (`POST /api/1/apps/update`), and each is put back the same
// way; Tonight asks for it in tonight's window
// (`POST /api/1/apps/{id}/tonight`), Install tonight for every app that has
// one (`POST /api/1/apps/tonight`) and Not tonight takes the request away
// (`POST /api/1/apps/{id}/not-tonight`); the lab machine's catalog may offer
// fewer apps an update than a press needs, and the script says so for each
// press it leaves out.
//
// The restart, the fallback of an update, What happened and the search for a
// bad change are in updates-restart.mjs and updates-bisect.mjs, each in one
// engine, since each restarts the machine. Check now with the update server
// not answering is updates-offline.mjs.
//
// A locked administrator sees "Show the slots" and the console unlocks when it
// is pressed. A reader sees every card and no button but Check now and the
// two that only read. A phone at 390 by 844 shows the cards stacked with no
// sideways scroll and every control 44 px tall. Each look is the console's
// own: dark, light, high contrast, forced colours and less motion, each
// checked by axe (console.mjs's lookAll). The accounts are the two that
// updates.toml made, so no script needs the lab's password.

import {
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustHaveCalled,
  openConsole,
  signIn,
  signOut,
  sidewaysScroll,
  sleep,
} from "./console.mjs";
import { PASSWORD, PHONE, answerUnlock, pressAndAnswer, smallControls, unlockDialog, unlockNow } from "./trial.mjs";
import {
  LOCKED_SLOTS,
  READER_WORDS,
  ROUTES,
  appOffers,
  bannerOf,
  card,
  checkedLine,
  firmwareWords,
  offerOf,
  openUpdates,
  pathOf,
  read,
  routeFor,
  settingsNow,
  signedIn,
  systemWords,
  wordsOf,
  checkNow,
} from "./updates-page.mjs";

const ADMIN = "wc-updates";
const READER = "wc-updates-reader";

// The words the page says where this script looks for them.
const UNSAVED = "Nothing to change.";
const NO_CHECK_NAME = "lab-web";
const NEVER_TOLD = "Nobody is told of security fixes. SEC-131 warns after 7 days.";
const REASON = "lab: waiting for the next alpha";
const NOT_PUT_OFF = /is not put off|so it is not put off/;

// --- Reading the machine ---

/** `update last --json`'s check, `{checked_ms, by, updates}`, the daily check that the steps before kept; null when none is kept. */
async function lastKept(page) {
  const answer = await read(page, ROUTES.last);
  return answer.status === 200 && Array.isArray(answer.json?.updates?.offers) ? answer.json : null;
}

/** Reads the apps until `test(app)` holds for the app `id`, for up to `seconds`, and gives it. */
async function appBecomes(page, id, test, what, seconds = 300) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const answer = await fetchAsPage(page, "GET", "/api/1/apps");
    const app = (answer.json?.bundles ?? []).find((one) => one.id === id);
    if (app !== undefined && test(app)) return app;
    ensure(Date.now() < until, `the app ${id} did not ${what} within ${seconds} seconds`);
    await sleep(2000);
  }
}

// --- The cards ---

/** The System, Firmware and Apps cards say what the last check says, in words. */
async function cardsAsKept(t, page, kept) {
  const offers = kept.updates;
  await t.see(checkedLine(page));
  ensure(/^Checked /.test(await wordsOf(checkedLine(page))), `the checked line says ${await wordsOf(checkedLine(page))}`);
  const system = offerOf(offers, "system");
  await t.see(card(page, "system"));
  ensure(systemWords(system).test(await wordsOf(card(page, "system"))), `the System card says ${await wordsOf(card(page, "system"))}, and the check says ${JSON.stringify(system)}`);
  const firmware = offerOf(offers, "firmware");
  ensure(firmwareWords(firmware).test(await wordsOf(card(page, "firmware"))), `the Firmware card says ${await wordsOf(card(page, "firmware"))}, and the check says ${JSON.stringify(firmware)}`);
  for (const app of appOffers(offers)) {
    ensure((await wordsOf(card(page, "apps"))).includes(app.to), `the Apps card does not say ${app.to} for ${app.item}`);
  }
  // A check that is kept has no part that is "not checked yet".
  for (const id of ["system", "firmware", "apps", "security"]) {
    ensure(!/Not checked yet/.test(await wordsOf(card(page, id))), `the ${id} card says it was not checked`);
  }
  return { system, firmware };
}

/** What the page's settings card has chosen is what `update settings --json` says. */
async function settingsAsAnswered(t, page) {
  const settings = await settingsNow(page);
  const chosen = (name) => card(page, "settings").locator(`input[name="upd-${name}"]:checked`).getAttribute("value");
  ensure((await chosen("check")) === (settings.check ?? "notify"), `the page chose "${await chosen("check")}" to look for updates, and the machine says ${settings.check}`);
  ensure((await chosen("system")) === (settings.unattended ? "auto" : "manual"), "the page's choice for the system is not the machine's");
  ensure((await chosen("apps")) === (settings.apps ?? "notify"), `the page chose "${await chosen("apps")}" for apps, and the machine says ${settings.apps}`);
  ensure((await chosen("channel")) === (settings.channel ?? "stable"), `the page chose "${await chosen("channel")}" as the channel, and the machine says ${settings.channel}`);
  await t.see(card(page, "settings").getByRole("button", { name: "Save", exact: true }));
  return settings;
}

/** The checks after an update are the ones `update checks --json` lists. */
async function checksAsAnswered(t, page) {
  const answer = await read(page, ROUTES.checks);
  ensure(answer.status === 200 && Array.isArray(answer.json?.essential), `${ROUTES.checks} was answered ${answer.status}: ${answer.text}`);
  const words = await wordsOf(card(page, "checks"));
  for (const name of answer.json.essential) ensure(words.includes(name), `the Checks card does not name the service ${name}`);
  for (const one of answer.json.checks ?? []) ensure(words.includes(one.name), `the Checks card does not name the check ${one.name}`);
  await t.see(card(page, "checks").getByRole("button", { name: "Add a check…", exact: true }));
  return answer.json;
}

// --- The administrator ---

/** A locked console reads no slots: the card says to unlock, and Show the slots unlocks and reads them. */
async function slotsUnlock(t, page) {
  await t.see(card(page, "system").getByText(LOCKED_SLOTS));
  await card(page, "system").getByRole("button", { name: "Show the slots", exact: true }).click();
  await answerUnlock(t, page);
  await t.gone(card(page, "system").getByText(LOCKED_SLOTS));
  const slots = await read(page, ROUTES.slots);
  ensure(slots.status === 200 && slots.json !== null, `${ROUTES.slots} was answered ${slots.status}: ${slots.text}`);
  return slots.json;
}

/** Save changes what moved, the machine has it, and Undo puts it back. */
async function saveAndUndo(t, page) {
  const before = await settingsNow(page);
  const box = card(page, "settings");
  const nothing = box.getByRole("button", { name: "Save", exact: true });
  await nothing.click();
  await t.see(box.getByText(UNSAVED));

  // Never to look for updates, and the beta channel: one Save, two routes.
  await box.getByRole("group", { name: "Look for updates", exact: true }).getByRole("radio", { name: "Never", exact: true }).check();
  await t.see(box.getByText(NEVER_TOLD));
  const to = before.channel === "beta" ? "Nightly" : "Beta";
  await box.getByRole("group", { name: "Channel", exact: true }).getByRole("radio", { name: to, exact: true }).check();
  const channelAnswered = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.channel));
  await pressAndAnswer(t, page, nothing, pathOf(ROUTES.set));
  ensure((await channelAnswered).status() === 200, `${ROUTES.channel} was not answered 200`);
  await t.see(box.getByText("Saved."));
  const after = await settingsNow(page);
  ensure(after.check === "off", `the machine looks for updates "${after.check}" after Save`);
  ensure(after.channel === to.toLowerCase(), `the machine's channel is ${after.channel} after Save`);

  // Undo puts back what Save changed, and nothing else.
  const setBack = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.set));
  const channelBack = page.waitForResponse((answer) => answer.request().method() === "POST" && new URL(answer.url()).pathname === pathOf(ROUTES.channel));
  await box.getByRole("button", { name: "Undo", exact: true }).click();
  ensure((await setBack).status() === 200 && (await channelBack).status() === 200, "Undo was not answered 200 by both routes");
  await t.see(box.getByText("Put back as it was."));
  const now = await settingsNow(page);
  ensure(now.check === before.check && now.channel === before.channel && now.apps === before.apps, `Undo left ${JSON.stringify({ check: now.check, channel: now.channel })}, and it was ${JSON.stringify({ check: before.check, channel: before.channel })}`);
}

/** A time that is not HH:MM is said in words and sends nothing. */
async function badTime(t, page) {
  const box = card(page, "settings");
  await box.getByRole("group", { name: "The system", exact: true }).getByRole("radio", { name: "By itself, and restart at", exact: true }).check();
  await box.getByLabel("Restart at", { exact: true }).fill("");
  let sent = 0;
  const count = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === pathOf(ROUTES.set)) sent += 1;
  };
  page.on("request", count);
  await box.getByRole("button", { name: "Save", exact: true }).click();
  await t.see(box.getByText(/^A time is written HH:MM/));
  await sleep(800);
  page.off("request", count);
  ensure(sent === 0, "Save sent a restart time that is not HH:MM");
  await box.getByRole("group", { name: "The system", exact: true }).getByRole("radio", { name: "When I say", exact: true }).check();
}

/** Check now asks the update server, and the cards are what it says. */
async function checkAgain(t, page) {
  const answered = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.check));
  await page.getByRole("button", { name: "Check now", exact: true }).click();
  ensure((await answered).status() === 200, `${ROUTES.check} was not answered 200`);
  await t.see(checkedLine(page).filter({ hasText: /^Checked just now/ }));
  const doc = await checkNow(page);
  const system = offerOf(doc, "system");
  ensure(systemWords(system).test(await wordsOf(card(page, "system"))), `after Check now the System card says ${await wordsOf(card(page, "system"))}, and the server offers ${JSON.stringify(system)}`);
  return doc;
}

/** The apps the check offers an update for, now: the ones with a Later and a Tonight to press. */
const waitingApps = (doc) => appOffers(doc).filter((one) => one.state === "update");

/** Presses `control`, which asks a question of its own: answers the unlock if it comes first, and gives the dialog called `name`. */
async function opensDialog(t, page, control, name) {
  await control.click();
  const asked = await unlockDialog(page).waitFor({ state: "visible", timeout: 2000 }).then(() => true, () => false);
  if (asked) await answerUnlock(t, page);
  const dialog = page.getByRole("dialog", { name, exact: true });
  await t.see(dialog);
  return dialog;
}

/** Hold this version holds what the system is offered, and Release the hold lets it go. */
async function holdAndRelease(t, page, doc) {
  const system = offerOf(doc, "system");
  if (system === null || system.state !== "update") {
    console.log(`the system's offer is ${system?.state ?? "none"}, so Hold this version was not pressed`);
    return false;
  }
  const box = card(page, "system");
  const to = system.to.replace(/\./g, "\\.");
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Hold this version", exact: true }), pathOf(ROUTES.hold, { item: "system" }));
  await t.see(box.getByText(new RegExp(`${to} is held`)));
  await t.see(card(page, "held").getByText(`System ${system.to}`));
  const held = await settingsNow(page);
  ensure(held.hold?.system === system.to, `the machine holds ${held.hold?.system} and the page held ${system.to}`);
  // The Held card's own Release does what the System card's does.
  await pressAndAnswer(t, page, card(page, "held").getByRole("button", { name: "Release", exact: true }), pathOf(ROUTES.unhold, { item: "system" }));
  await t.see(card(page, "held").getByText("Nothing is held."));
  await t.see(box.getByText(new RegExp(`${to} is ready`)));
  ensure((await settingsNow(page)).hold?.system == null, "the machine still holds the system after Release");
  await holdWithReason(t, page, system);
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Release the hold", exact: true }), pathOf(ROUTES.unhold, { item: "system" }));
  await t.see(box.getByText(new RegExp(`${to} is ready`)));
  const after = await settingsNow(page);
  ensure(after.hold?.system == null && after.hold_reasons?.system == null, `the machine keeps ${JSON.stringify(after.hold_reasons)} of a hold that was released`);
  return true;
}

/** Hold a version, from the Held card's own choice, asks why before it holds: Cancel holds nothing, and the reason is kept with the hold and said by the cards. */
async function holdWithReason(t, page, system) {
  const held = card(page, "held");
  const path = pathOf(ROUTES.hold, { item: "system" });
  await held.getByLabel("Hold a version", { exact: true }).selectOption({ label: `The system, ${system.to}` });
  const opener = held.getByRole("button", { name: "Hold it…", exact: true });
  const title = `Hold the system update ${system.to}`;
  let dialog = await opensDialog(t, page, opener, title);
  const field = dialog.getByLabel("Why (optional)", { exact: true });
  await t.see(field);
  ensure((await field.getAttribute("maxlength")) === "200", "the reason may have more than 200 characters");
  await t.see(dialog.getByText("Everyone who can sign in sees the reason, here and with update settings.", { exact: true }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
  ensure((await settingsNow(page)).hold?.system == null, "Cancel in the question that asks why held the system");

  dialog = await opensDialog(t, page, opener, title);
  await dialog.getByLabel("Why (optional)", { exact: true }).fill(REASON);
  const sent = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === path);
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: "Hold it", exact: true }), path);
  const body = (await sent).postData() ?? "";
  ensure(body.includes(`"why":${JSON.stringify(REASON)}`) || body.includes(`"why": ${JSON.stringify(REASON)}`), `${ROUTES.hold} was sent ${JSON.stringify(body)}, without the reason`);
  await t.gone(dialog);
  const settings = await settingsNow(page);
  const kept = settings.hold_reasons?.system;
  ensure(settings.hold?.system === system.to && kept?.text === REASON && kept?.version === system.to, `the machine keeps ${JSON.stringify(settings.hold)} and ${JSON.stringify(settings.hold_reasons)} for a hold with a reason`);
  // The Held card and the System card say when, by whom and why.
  const row = held.locator("li.upd-row").filter({ hasText: `System ${system.to}` });
  await t.see(row);
  const words = await wordsOf(row);
  ensure(/held (?:today|yesterday|on [A-Z][a-z]{2} \d{1,2}) at \d{1,2}:\d{2} [AP]M/.test(words) && words.includes(`: "${REASON}"`), `the Held card says ${words} of a hold made just now for "${REASON}"`);
  ensure((await wordsOf(card(page, "system"))).includes(`: "${REASON}"`), `the System card does not say why it is held: ${await wordsOf(card(page, "system"))}`);
}

/** Later puts the reminder of the system's update off and Remind me now brings it back; an update that fixes an advisory has no Later and says why. */
async function laterAndRemind(t, page, doc) {
  const system = offerOf(doc, "system");
  if (system === null || system.state !== "update") {
    console.log(`the system's offer is ${system?.state ?? "none"}, so Later was not pressed`);
    return false;
  }
  const box = card(page, "system");
  const later = box.getByRole("button", { name: "Later for the system update", exact: true });
  if ((await later.count()) === 0) {
    ensure(NOT_PUT_OFF.test(await wordsOf(box)), `the System card has no Later and does not say why: ${await wordsOf(box)}`);
    console.log("the system's update fixes an advisory, or the machine cannot tell, so Later was not pressed");
    return false;
  }
  await later.click();
  const menu = box.getByRole("menu", { name: "Put off the reminder of the system update", exact: true });
  await t.see(menu);
  const choices = await menu.getByRole("menuitem").evaluateAll((items) => items.map((item) => item.textContent.trim()));
  ensure(JSON.stringify(choices) === JSON.stringify(["Remind me tomorrow", "Remind me in a week"]), `Later offers ${JSON.stringify(choices)}`);
  await pressAndAnswer(t, page, menu.getByRole("menuitem", { name: "Remind me tomorrow", exact: true }), pathOf(ROUTES.later, { item: "system" }));
  const put = box.getByText(/^Put off until /);
  await put.waitFor({ state: "visible", timeout: 30_000 }).catch(async () => {
    throw new Error(`the System card does not say it is put off: ${await wordsOf(box)}; update settings says ${JSON.stringify((await settingsNow(page)).later)}`);
  });
  const entry = ((await settingsNow(page)).later ?? []).find((one) => one.item === "system");
  ensure(entry !== undefined && entry.version === system.to, `update settings lists ${JSON.stringify(entry)} for a reminder put off for ${system.to}`);
  const hours = (entry.until_ms - entry.at_ms) / 3_600_000;
  ensure(hours >= 12 && hours <= 48, `the reminder of a day is put off for ${hours} hours`);
  ensure((await box.getByRole("button", { name: "Later for the system update", exact: true }).count()) === 0, "the System card has Later while the update is put off");

  await pressAndAnswer(t, page, box.getByRole("button", { name: "Remind me now", exact: true }), pathOf(ROUTES.later, { item: "system" }));
  await t.see(box.getByRole("button", { name: "Later for the system update", exact: true }));
  await t.gone(box.getByText(/^Put off until /));
  ensure(((await settingsNow(page)).later ?? []).every((one) => one.item !== "system"), "update settings still lists the reminder of the system after Remind me now");
  return true;
}

/** Stage it stages the release as a task and the page says to restart; Take it back unstages it. */
async function stageAndTakeBack(t, page, doc) {
  const system = offerOf(doc, "system");
  if (system === null || system.state !== "update") {
    console.log(`the system's offer is ${system?.state ?? "none"}, so Stage it was not pressed`);
    return;
  }
  const box = card(page, "system");
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Stage it", exact: true }), pathOf(ROUTES.apply, { item: "system" }), "POST", [202]);
  // The staging fetches the release, which takes as long as the lab network does.
  await page.waitForFunction(() => /is staged in slot [AB]/.test(document.querySelector("main section#system")?.textContent ?? ""), null, { timeout: 8 * 60 * 1000 });
  await t.see(bannerOf(page).getByText("Restart to finish the update"));
  await t.see(box.getByRole("button", { name: "Restart now…", exact: true }));
  const slots = await read(page, ROUTES.slots);
  ensure(slots.json?.staged?.slot !== undefined, `${ROUTES.slots} says nothing is staged: ${slots.text}`);
  // The spare slot is the system's until it has proved itself, so the firmware set waits.
  const firmware = offerOf(doc, "firmware");
  if (firmware?.state === "update") {
    await t.see(card(page, "firmware").getByText("After the system update has proved itself"));
    ensure((await card(page, "firmware").getByRole("button", { name: "Stage it", exact: true }).getAttribute("aria-disabled")) === "true", "the firmware's Stage it is not off while the system is staged");
  }
  await pressAndAnswer(t, page, box.getByRole("button", { name: "Take it back", exact: true }), pathOf(ROUTES.unstage));
  await t.gone(bannerOf(page));
  await t.see(box.getByText(systemWords(system)));
  ensure((await read(page, ROUTES.slots)).json?.staged == null, "the machine still has the update staged after Take it back");
}

/** An update that the check offers for an app is run as a task, and the app goes back by the route its own page calls. */
async function appUpdate(t, page, doc) {
  const [offer] = appOffers(doc).filter((one) => one.state === "update");
  if (offer === undefined) {
    console.log("no app of this machine has an update to take, so Update was not pressed");
    return null;
  }
  const row = card(page, "apps").locator("li.upd-row").filter({ hasText: offer.to });
  await pressAndAnswer(t, page, row.getByRole("button", { name: "Update", exact: true }), `/api/1/apps/${encodeURIComponent(offer.item)}/update`, "POST", [202]);
  await appBecomes(page, offer.item, (app) => app.version === offer.to, `be at ${offer.to}`);
  await t.gone(card(page, "apps").locator("li.upd-row").filter({ hasText: `${offer.from}` }));
  // Going back is the app's own page; its route is the same.
  const back = await fetchAsPage(page, "POST", `/api/1/apps/${encodeURIComponent(offer.item)}/rollback`, {});
  ensure(back.status === 200 || back.status === 202, `POST /api/1/apps/${offer.item}/rollback was answered ${back.status}: ${back.text}`);
  await appBecomes(page, offer.item, (app) => app.version === offer.from, `go back to ${offer.from}`);
  return offer.item;
}

/** The route of an app, `{id}` filled in: the Apps card calls the routes of the app's own page. */
const appRoute = (route, id) => route.slice(route.indexOf(" ") + 1).replace("{id}", encodeURIComponent(id));
const APP_ROUTES = {
  updateAll: "POST /api/1/apps/update",
  tonight: "POST /api/1/apps/{id}/tonight",
  tonightAll: "POST /api/1/apps/tonight",
  notTonight: "POST /api/1/apps/{id}/not-tonight",
};

/** Presses Not tonight on the row of `offer` and waits until the machine no longer has the request. */
async function notTonight(t, page, row, offer) {
  await pressAndAnswer(t, page, row.getByRole("button", { name: "Not tonight", exact: true }), appRoute(APP_ROUTES.notTonight, offer.item));
  await appBecomes(page, offer.item, (app) => app.tonight == null, "be taken off tonight's window", 60);
  await t.see(row.getByRole("button", { name: "Tonight", exact: true }));
  await t.gone(row.getByText("Installs tonight", { exact: true }));
}

/**
 * An app's row has Later with its menu, Tonight with the window in words, and
 * Install tonight asks for every update at once; Not tonight takes each
 * request away. Gives the routes it called, as the page's requests name them.
 */
async function appTonight(t, page, doc) {
  const offers = waitingApps(doc);
  if (offers.length === 0) {
    console.log("no app of this machine has an update to take, so Later, Tonight and Install tonight were not pressed");
    return [];
  }
  const settings = await settingsNow(page);
  const box = card(page, "apps");
  const [offer] = offers;
  const row = box.locator("li.upd-row").filter({ hasText: offer.to }).first();
  const words = await wordsOf(box);
  ensure(words.includes("Installs tonight, between "), `the Apps card does not say the window: ${words}`);
  if (settings.restart == null) ensure(words.includes("Installs tonight, between 2:00 and 3:00 AM."), `the machine has no restart time and the card says ${words}`);
  else ensure(words.includes("the hour before this server's restart time."), `the machine restarts at ${settings.restart} and the card says ${words}`);

  // Later, when apps are told of and the update fixes no advisory.
  const later = row.getByRole("button", { name: /^Later for / });
  const called = [];
  if ((await later.count()) === 0) {
    ensure(settings.apps === "auto" || NOT_PUT_OFF.test(await wordsOf(row)), `the row of ${offer.item} has no Later and does not say why: ${await wordsOf(row)}`);
  } else {
    await later.click();
    await pressAndAnswer(t, page, box.getByRole("menuitem", { name: "Remind me in a week", exact: true }), pathOf(ROUTES.later, { item: offer.item }));
    await t.see(row.getByText(/^Put off until /));
    await pressAndAnswer(t, page, row.getByRole("button", { name: "Remind me now", exact: true }), pathOf(ROUTES.later, { item: offer.item }));
    await t.see(row.getByRole("button", { name: /^Later for / }));
    called.push(`POST ${pathOf(ROUTES.later, { item: offer.item })}`);
  }

  // Tonight asks for the update in the window, and the row says so.
  await pressAndAnswer(t, page, row.getByRole("button", { name: "Tonight", exact: true }), appRoute(APP_ROUTES.tonight, offer.item), "POST", [202]);
  called.push(`POST ${appRoute(APP_ROUTES.tonight, offer.item)}`);
  const app = await appBecomes(page, offer.item, (one) => one.tonight != null || one.version === offer.to, "be asked for tonight or be updated", 120);
  if (app.tonight == null) {
    // The machine's window is open now: a request made inside it runs at once.
    console.log(`${offer.item} was updated at once, so the window is open now; Not tonight was not pressed`);
    const back = await fetchAsPage(page, "POST", `/api/1/apps/${encodeURIComponent(offer.item)}/rollback`, {});
    ensure(back.status === 200 || back.status === 202, `POST /api/1/apps/${offer.item}/rollback was answered ${back.status}: ${back.text}`);
    await appBecomes(page, offer.item, (one) => one.version === offer.from, `go back to ${offer.from}`);
    // Going back holds the app at that version; the update is on offer again once it is let go.
    const free = await fetchAsPage(page, "POST", `/api/1/apps/${encodeURIComponent(offer.item)}/unhold`, {});
    ensure(free.status === 200, `POST /api/1/apps/${offer.item}/unhold was answered ${free.status}: ${free.text}`);
    await page.reload();
    await openUpdates(t, page);
    return called;
  }
  await t.see(row.getByText("Installs tonight", { exact: true }));
  await notTonight(t, page, row, offer);
  called.push(`POST ${appRoute(APP_ROUTES.notTonight, offer.item)}`);

  // Install tonight asks for every update at once; with one app, no button is there, and the route is the same.
  if (offers.length > 1) {
    await pressAndAnswer(t, page, box.getByRole("button", { name: "Install tonight", exact: true }), appRoute(APP_ROUTES.tonightAll));
  } else {
    await t.gone(box.getByRole("button", { name: "Install tonight", exact: true }));
    const asked = await fetchAsPage(page, "POST", appRoute(APP_ROUTES.tonightAll), {});
    ensure(asked.status === 200, `${APP_ROUTES.tonightAll} was answered ${asked.status}: ${asked.text}`);
  }
  called.push(`POST ${appRoute(APP_ROUTES.tonightAll)}`);
  for (const one of offers) await appBecomes(page, one.item, (found) => found.tonight != null, "be asked for tonight", 120);
  await page.reload();
  await openUpdates(t, page);
  for (const one of offers) await notTonight(t, page, box.locator("li.upd-row").filter({ hasText: one.to }).first(), one);
  return called;
}

/** An app goes back by the route its own page calls and is let go: going back holds it at that version, and the update is on offer again once it is let go. */
async function appPutBack(page, id, from) {
  const back = await fetchAsPage(page, "POST", `/api/1/apps/${encodeURIComponent(id)}/rollback`, {});
  ensure(back.status === 200 || back.status === 202, `POST /api/1/apps/${id}/rollback was answered ${back.status}: ${back.text}`);
  await appBecomes(page, id, (app) => app.version === from, `go back to ${from}`);
  const free = await fetchAsPage(page, "POST", `/api/1/apps/${encodeURIComponent(id)}/unhold`, {});
  ensure(free.status === 200, `POST /api/1/apps/${id}/unhold was answered ${free.status}: ${free.text}`);
}

/**
 * Update all is on the Apps page, and only while more than one app waits for
 * an update: it updates every app that waits, as one task, and each app goes
 * back by the route its own page calls. With fewer than two apps waiting the
 * page has no such button, and the script says so. Gives whether it was
 * pressed.
 */
async function appUpdateAll(t, page, doc) {
  await page.goto(new URL("/apps", page.url()).href);
  await t.see(page.getByRole("heading", { level: 1, name: "Apps", exact: true }));
  const drawn = () => page.waitForFunction(() => document.querySelectorAll(".apps-list a.apps-who, .apps-list .apps-empty").length > 0, null, { timeout: 30_000 });
  const waiting = () => page.locator('td[data-updates="true"]').evaluateAll((cells) => cells.map((cell) => cell.getAttribute("data-app")));
  const button = page.getByRole("button", { name: /^Update all/ });
  await drawn();
  const ids = await waiting();
  if (ids.length < 2) {
    ensure((await button.count()) === 0, `${ids.length} apps have an update and the Apps page has Update all`);
    console.log(`${ids.length} apps of this machine have an update to take, and Update all is there for two or more, so it was not pressed`);
    await openUpdates(t, page);
    return false;
  }
  const answer = await fetchAsPage(page, "GET", "/api/1/apps");
  ensure(answer.status === 200 && Array.isArray(answer.json?.bundles), `GET /api/1/apps was answered ${answer.status}: ${answer.text}`);
  const apps = ids.map((id) => {
    const bundle = answer.json.bundles.find((one) => one.id === id);
    return { id, from: bundle?.version, to: offerOf(doc, id)?.to ?? bundle?.update };
  });
  ensure(apps.every((one) => one.from && one.to && one.to !== one.from), `the Apps page says ${ids.join(", ")} wait for an update, and the machine has ${JSON.stringify(apps)}`);
  await t.see(page.getByRole("button", { name: `Update all (${ids.length})`, exact: true }));
  await pressAndAnswer(t, page, button, appRoute(APP_ROUTES.updateAll), "POST", [202]);
  // Each app is at the version its update brings, which takes as long as the lab network does.
  for (const one of apps) await appBecomes(page, one.id, (app) => app.version === one.to, `be at ${one.to}`, 600);
  await page.reload();
  await drawn();
  ensure((await waiting()).length === 0 && (await button.count()) === 0, `after Update all the Apps page still says ${JSON.stringify(await waiting())} wait for an update`);
  for (const one of apps) await appPutBack(page, one.id, one.from);
  await openUpdates(t, page);
  return true;
}

/** The advisories are read when asked for, and the page lists what the machine says. */
async function advisoriesShown(t, page) {
  const box = card(page, "security");
  const answered = page.waitForResponse((answer) => answer.request().method() === "GET" && new URL(answer.url()).pathname === pathOf(ROUTES.advisories));
  await box.getByRole("button", { name: "Show the advisories", exact: true }).click();
  const answer = await answered;
  ensure([200, 204].includes(answer.status()), `${ROUTES.advisories} was answered ${answer.status()}`);
  const doc = await answer.json().catch(() => ({}));
  const count = (doc.system?.length ?? 0) + (doc.apps?.length ?? 0);
  if (count === 0) await t.see(box.getByText("No advisory applies and no flaw is known."));
  else ensure((await box.locator(".upd-full li").count()) === count, `the page lists ${await box.locator(".upd-full li").count()} advisories and the machine says ${count}`);
}

/** The history is read when asked for: the two lists and their headings. */
async function historyShown(t, page) {
  const box = card(page, "history");
  await box.getByRole("button", { name: "Show the history", exact: true }).click();
  await t.see(box.getByRole("heading", { level: 3, name: "The system and the firmware set", exact: true }));
  await t.see(box.getByRole("heading", { level: 3, name: "Apps", exact: true }));
}

/** A check is added with the dialog, listed, and removed; so is a service that must run again. */
async function checksChange(t, page) {
  const box = card(page, "checks");
  await box.getByRole("button", { name: "Add a check…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add a check", exact: true });
  await t.see(dialog);
  // A name that is not one is said in words and sends nothing.
  await dialog.getByRole("button", { name: "Add the check", exact: true }).click();
  await t.see(dialog.getByText(/^Give the check a name/));
  await dialog.getByLabel("What", { exact: true }).selectOption("url");
  await dialog.getByLabel("Address or service", { exact: true }).fill("https://localhost/");
  await dialog.getByLabel("Name", { exact: true }).fill(NO_CHECK_NAME);
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: "Add the check", exact: true }), pathOf(ROUTES.checkAdd));
  await t.gone(dialog);
  await t.see(box.getByText(`${NO_CHECK_NAME} · https://localhost/ answers`));
  const listed = (await read(page, ROUTES.checks)).json;
  ensure((listed.checks ?? []).some((one) => one.name === NO_CHECK_NAME && one.required === false), `the machine lists no wanted check called ${NO_CHECK_NAME}`);
  const row = box.locator("li.upd-row").filter({ hasText: NO_CHECK_NAME });
  await pressAndAnswer(t, page, row.getByRole("button", { name: "Remove", exact: true }), pathOf(ROUTES.checkRemove, { name: NO_CHECK_NAME }));
  await t.gone(box.getByText(`${NO_CHECK_NAME} · `));

  // A service that must run again: one of the machine's that is not one yet.
  await box.getByRole("button", { name: "Add a service…", exact: true }).click();
  const service = page.getByRole("dialog", { name: "Add a service", exact: true });
  await t.see(service);
  const name = (await service.getByLabel("Service", { exact: true }).locator("option").first().textContent()).trim();
  await pressAndAnswer(t, page, service.getByRole("button", { name: "Add the service", exact: true }), pathOf(ROUTES.essential));
  await t.gone(service);
  ensure((await read(page, ROUTES.checks)).json.essential.includes(name), `${name} is not one of the services that must run again`);
  const added = box.locator("li.upd-row").filter({ hasText: new RegExp(`^${name}$`) });
  await pressAndAnswer(t, page, added.getByRole("button", { name: "Remove", exact: true }), pathOf(ROUTES.essential));
  ensure(!(await read(page, ROUTES.checks)).json.essential.includes(name), `${name} still must run again after Remove`);
}

async function administrator(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await openUpdates(t, page);
  const kept = await lastKept(page);
  ensure(kept !== null, "the steps before kept no check: updated --once should have");
  await cardsAsKept(t, page, kept);
  await settingsAsAnswered(t, page);
  await checksAsAnswered(t, page);
  await slotsUnlock(t, page);
  await lookAll(t, page, "updates");

  await saveAndUndo(t, page);
  await badTime(t, page);
  const doc = await checkAgain(t, page);
  await cardsAsKept(t, page, { updates: doc });
  await holdAndRelease(t, page, doc);
  const later = await laterAndRemind(t, page, doc);
  await stageAndTakeBack(t, page, doc);
  const asked = await appTonight(t, page, doc);
  const updatedAll = await appUpdateAll(t, page, doc);
  const app = await appUpdate(t, page, doc);
  await advisoriesShown(t, page);
  await historyShown(t, page);
  await checksChange(t, page);
  await lookAll(t, page, "updates-changed");

  mustHaveCalled(a.seen, [
    routeFor(ROUTES.last),
    routeFor(ROUTES.check),
    routeFor(ROUTES.slots),
    routeFor(ROUTES.history),
    routeFor(ROUTES.advisories),
    routeFor(ROUTES.settings),
    routeFor(ROUTES.set),
    routeFor(ROUTES.channel),
    routeFor(ROUTES.checks),
    routeFor(ROUTES.checkAdd),
    routeFor(ROUTES.checkRemove, { name: NO_CHECK_NAME }),
    routeFor(ROUTES.essential),
    routeFor(ROUTES.failed),
    routeFor(ROUTES.power),
    ...(offerOf(doc, "system")?.state === "update"
      ? [routeFor(ROUTES.hold, { item: "system" }), routeFor(ROUTES.unhold, { item: "system" }), routeFor(ROUTES.apply, { item: "system" }), routeFor(ROUTES.unstage)]
      : []),
    ...(later ? [routeFor(ROUTES.later, { item: "system" })] : []),
    ...(app === null ? [] : [`POST /api/1/apps/${encodeURIComponent(app)}/update`]),
    ...(updatedAll ? [`POST ${appRoute(APP_ROUTES.updateAll)}`] : []),
    ...asked,
  ]);
  await signOut(t, page);
  await page.context().close();
}

// --- The reader and the phone ---

/** A reader sees every card and no button but Check now and the two that only read. */
async function reader(t) {
  const { page } = await signedIn(t, READER);
  await openUpdates(t, page);
  const kept = await lastKept(page);
  await cardsAsKept(t, page, kept);
  await t.see(page.getByText(READER_WORDS));
  const names = await page.locator("main .upd button").evaluateAll((buttons) => buttons.map((one) => one.textContent.trim()));
  const allowed = new Set(["Show the advisories", "Show the history"]);
  const extra = names.filter((name) => !allowed.has(name));
  ensure(extra.length === 0, `a reader's page has the buttons ${extra.join(", ")}`);
  await t.see(page.getByRole("button", { name: "Check now", exact: true }));
  for (const name of ["Stage it", "Save", "Hold this version", "Hold it…", "Add a check…", "Restart now…", "Install tonight", "Tonight", "Not tonight", "Remind me now"]) {
    ensure((await page.getByRole("button", { name, exact: true }).count()) === 0, `a reader's page has ${name}`);
  }
  ensure((await page.getByRole("button", { name: /^Later for / }).count()) === 0, "a reader's page has Later");
  // The slots are an administrator's read.
  const slots = await read(page, ROUTES.slots);
  ensure(slots.status === 403 || slots.status === 401, `${ROUTES.slots} was answered ${slots.status} to a reader`);
  ensure(slots.json?.error?.id === "ADM-122", `${ROUTES.slots} told a reader ${slots.json?.error?.id ?? slots.text}`);
  // The settings are read in words.
  await t.see(card(page, "settings").getByText("Look for updates"));
  await lookAll(t, page, "updates-reader");
  await signOut(t, page);
  await page.context().close();
}

/** At 390 wide: the cards stack, nothing scrolls sideways and each control is a thumb's. */
async function phone(t) {
  const { page } = await openConsole(t, "/", { viewport: PHONE });
  const answer = await signIn(page, ADMIN, PASSWORD);
  ensure(answer.status() === 200, `${ADMIN} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await unlockNow(page);

  await openUpdates(t, page);
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);
  const small = await smallControls(page, "main .upd button, main .upd a.btn, main .upd input, main .upd select");
  ensure(small.length === 0, `these controls of the phone's page are under 44 px tall: ${small.join(", ")}`);
  const order = await page.locator("main section.card[id]").evaluateAll((cards) => cards.map((one) => one.id));
  const wanted = ["system", "apps", "security", "firmware", "held", "settings", "checks", "history"];
  ensure(JSON.stringify(order.filter((id) => wanted.includes(id))) === JSON.stringify(wanted), `the cards come in the order ${order.join(", ")}`);
  await lookAll(t, page, "updates-phone");
  await signOut(t, page);
  await page.context().close();
}

export default async function check(t) {
  await administrator(t);
  await reader(t);
  await phone(t);
}
