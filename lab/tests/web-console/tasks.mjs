// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/tasks drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): a long
// action of the console runs as a task that is admind's and not the page's
// (ADR 0516). An administrator signs in, unlocks the console and asks for
// the lab's long action, `lab.long`, which runs `admin lab-steps` for 150
// seconds, four steps of it, and the console answers 202 with the task's
// number. A second browser, signed in as another administrator, sees the
// task run: its step and words, its log, and `event: task` on the event
// stream, which tells each tab where the task is and holds no log, no
// arguments and no command. The first browser closes, and the console
// locks its session a minute later, as unlock.mjs shows; the task goes on,
// and has moved on when the second browser looks again. The second
// browser's administrator, whose locked session needs no unlock to stop a
// task, stops it: the task is `stopped by` that account, its command has
// ended, and the journal has the action's row with the task's number, as
// the first account's console change.
//
// The second browser also has the Tasks drawer of the console's header: the
// button says how many tasks run, the drawer lists the task under Running
// with its step, its bar, its log and the command it is, and in each of the
// console's looks; the other administrator stops it there, with the dialog's
// question, and the task moves to "Needs a look" with its note, as the count
// on the button says in words. At the phone's width the drawer fills the
// screen as a dialog.
//
// The lab's action is in a build made with systools' `lab` feature, which
// `ob image --lab` turns on for the lab image and no other image has. The
// test needs it: a console that does not offer `lab.long` is a lab image
// built without the feature, and a failure, not a reason to skip. The
// routes of a task that is not there are asked of the console too, as
// ADM-127.

import {
  answerTo,
  ensure,
  fetchAsPage,
  leaveWelcome,
  lookAll,
  mustBe,
  mustHaveCalled,
  openConsole,
  routeOf,
  screenshot,
  signIn,
  signInDone,
  signOut,
  sleep,
} from "./console.mjs";

// The password that tasks.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each, with the number of a
// task or a change for {id}, by the end.
const ROUTES = {
  tasks: "GET /api/1/tasks",
  task: "GET /api/1/tasks/{id}",
  stop: "POST /api/1/tasks/{id}/stop",
  changes: "GET /api/1/changes",
  change: "GET /api/1/changes/{id}",
  events: "GET /api/1/events",
};

// A task that is not there.
const ABSENT = 999999;

// How long the task runs, in seconds, and how long the first browser has
// been closed when the second looks again: more than the minute that the
// console waits before it locks a session with no page.
const SECONDS = 150;
const CLOSED_FOR_SECONDS = 75;

/** The task `id` as `page` reads it, once `ready` holds of it, waiting up to `seconds`. */
async function taskWhere(page, id, ready, what, seconds = 60) {
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const answer = await fetchAsPage(page, "GET", `/api/1/tasks/${id}`);
    ensure(answer.status === 200, `GET /api/1/tasks/${id} was answered ${answer.status}: ${answer.text}`);
    if (ready(answer.json)) return answer.json;
    ensure(Date.now() < until, `task ${id} is not ${what} ${seconds} seconds on: ${answer.text}`);
    await sleep(1000);
  }
}

/** How far a task is, as one number that only grows: its steps and the percent of the step. */
const progressOf = (task) => task.step * 1000 + (task.percent ?? 0);

/**
 * The `event: task` messages that the event stream gives a page that
 * opens it, for `milliseconds`: the console tells a new tab where each live
 * task is, and then each change.
 */
function taskEvents(page, milliseconds) {
  return page.evaluate(
    (wait) =>
      new Promise((resolve) => {
        const events = [];
        const stream = new EventSource("/api/1/events");
        stream.addEventListener("task", (event) => events.push(JSON.parse(event.data)));
        setTimeout(() => {
          stream.close();
          resolve(events);
        }, wait);
      }),
    milliseconds,
  );
}

/** What the console says of a task that is not there: the list is a list, and the task and its stop are ADM-127. */
async function absentTask(page) {
  const list = await fetchAsPage(page, "GET", "/api/1/tasks");
  ensure(list.status === 200 && Array.isArray(list.json?.tasks), `GET /api/1/tasks was answered ${list.text}`);
  mustBe(await fetchAsPage(page, "GET", `/api/1/tasks/${ABSENT}`), 404, "ADM-127", "a task that is not there");
  mustBe(await fetchAsPage(page, "POST", `/api/1/tasks/${ABSENT}/stop`, {}), 404, "ADM-127", "its stop");
}

/** The header's Tasks button, whose name says how many tasks run or need a look. */
const tasksButton = (page, words) => page.getByRole("banner").getByRole("button", { name: words });

// The button's name while the task runs. A task that a test before stopped
// on the same machine needs a look until the drawer is opened, and the name
// says so first.
const RUNNING = /^Tasks, (\d+ needs? a look, )?\d+ running$/;

/**
 * The drawer, and the card of task `id` in it, called `title`. An engine
 * before this one left a stopped task of the same name, which needs a look:
 * the card is told from it by the number of its task.
 */
const drawerOf = (page) => page.getByRole("complementary", { name: "Tasks" });
const cardOf = (drawer, title, id) =>
  drawer.getByRole("region", { name: title }).filter({ has: drawer.page().getByText(new RegExp(`task ${id}$`)) });

// The task's card is named by the action's title and its one argument.
const CARD = `Run the lab's long test · ${SECONDS}`;

/** The drawer shows the running task: its group, step, bar, log and command; its looks; and Esc closes it. */
async function drawerShowsTask(t, page, id, name) {
  const button = tasksButton(page, RUNNING);
  await t.see(button);
  await button.click();
  const drawer = drawerOf(page);
  await t.see(drawer);
  await t.see(drawer.getByRole("heading", { name: /^Running/, level: 3 }));
  const card = cardOf(drawer, CARD, id);
  await t.see(card);
  await t.see(card.getByText(`Started by ${name}`, { exact: false }));
  await t.see(card.getByText(new RegExp(`task ${id}$`)));
  await t.see(card.getByText(/^Step \d of 4/).first());
  const bar = card.getByRole("progressbar");
  await t.see(bar);
  ensure(Number.isInteger(Number(await bar.getAttribute("aria-valuenow"))), "the task's bar has no value");

  // The log, and the command as the console would show it for this task.
  await card.getByRole("button", { name: "Show the log" }).click();
  const log = card.getByLabel(`Log of task ${id}`);
  await t.see(log);
  await page.waitForFunction((label) => document.querySelector(`[aria-label="${label}"]`)?.textContent.trim().length > 0, `Log of task ${id}`);
  await t.see(card.getByRole("button", { name: /^Show the command: elevate admin lab-steps/ }));
  await lookAll(t, page, "tasks-drawer");
  await page.keyboard.press("Escape");
  await t.gone(drawer);
  ensure((await page.evaluate(() => document.activeElement.getAttribute("aria-label") ?? "")).startsWith("Tasks"), "focus is not back on the Tasks button after Esc");
}

/** The other administrator stops the task in the drawer: the question, the stop, the card in its new group and the button's words. */
async function stopInDrawer(t, page, id, other) {
  await tasksButton(page, RUNNING).click();
  const drawer = drawerOf(page);
  await t.see(drawer);
  const card = cardOf(drawer, CARD, id);
  await card.getByRole("button", { name: "Stop", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Stop task ${id}?` });
  await t.see(dialog);
  await screenshot(t, page, "tasks-stop-question");
  const [answer] = await Promise.all([
    answerTo(page, `/api/1/tasks/${id}/stop`),
    dialog.getByRole("button", { name: "Stop the task" }).click(),
  ]);
  ensure(answer.status() === 200, `the drawer's stop was answered ${answer.status()}: ${await answer.text()}`);
  await t.see(page.getByText(`Stopping task ${id}.`));
  await t.see(drawer.getByRole("heading", { name: /^Needs a look/, level: 3 }));
  await t.see(drawer.getByText(`Stopped: stopped by ${other}`));
  await t.gone(drawer.getByRole("heading", { name: /^Running/, level: 3 }));
  // Looking at the list marks what needs a look as seen, so the words say none run.
  await t.see(tasksButton(page, "Tasks, none running"));
  await lookAll(t, page, "tasks-stopped");
  await page.keyboard.press("Escape");
  await t.gone(drawer);
}

/** At the phone's width the drawer is a dialog that fills the screen, and Esc closes it. */
async function drawerOnPhone(t, other) {
  const phone = await openConsole(t, "/", { viewport: { width: 390, height: 844 } });
  const answer = await signIn(phone.page, other, PASSWORD);
  ensure(answer.status() === 200, `${other} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(phone.page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, phone.page);
  // The stopped task needs a look, which this browser has not seen: the
  // Tasks icon is in the header, with its count in its name. A task that a
  // test before stopped on the same machine is counted too.
  const icon = tasksButton(phone.page, /^Tasks, \d+ needs? a look/);
  await t.see(icon);
  await icon.click();
  const dialog = phone.page.getByRole("dialog", { name: "Tasks" });
  await t.see(dialog);
  await t.see(dialog.getByText(`Stopped: stopped by ${other}`));
  const size = phone.page.viewportSize();
  const box = await dialog.boundingBox();
  ensure(box !== null && box.width >= size.width - 1 && box.height >= size.height - 1, `the drawer on a phone is ${JSON.stringify(box)}, not the whole screen`);
  await lookAll(t, phone.page, "tasks-drawer-phone");
  await phone.page.keyboard.press("Escape");
  await t.gone(dialog);
  await phone.page.context().close();
}

export default async function check(t) {
  const name = `wc-tasks-${t.engine}`;
  const other = `wc-tasks-other-${t.engine}`;
  const first = await openConsole(t, "/");
  await signInDone(t, first.page, name, PASSWORD);
  const unlocked = await fetchAsPage(first.page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(unlocked.status === 200, `the unlock was answered ${unlocked.status}: ${unlocked.text}`);

  const listed = await fetchAsPage(first.page, "GET", "/api/1/actions");
  ensure(listed.status === 200 && Array.isArray(listed.json), `GET /api/1/actions was answered ${listed.text}`);
  const action = listed.json.find((each) => each.id === "lab.long");
  ensure(
    action !== undefined,
    "the console does not offer lab.long: this lab image was built without systools' lab feature, which `ob image --lab` turns on",
  );
  ensure(action.changes && action.long, `lab.long is listed as ${JSON.stringify(action)}, a long change`);
  await absentTask(first.page);

  // The action answers at once with the task's number.
  const asked = await fetchAsPage(first.page, "POST", action.path, { seconds: SECONDS });
  ensure(
    asked.status === 202 && Number.isInteger(asked.json?.task),
    `lab.long was answered ${asked.status} ${asked.text}`,
  );
  const id = asked.json.task;
  const running = await taskWhere(first.page, id, (task) => task.state === "running" && task.step >= 1, "running");
  ensure(
    running.who === name && running.how === "console" && running.action === "lab.long",
    `the task is ${JSON.stringify(running)}`,
  );
  ensure(running.steps === 4 && running.words !== "", `the task says no step: ${JSON.stringify(running)}`);

  // A second browser, another administrator, sees it run.
  const second = await openConsole(t, "/");
  await signInDone(t, second.page, other, PASSWORD);
  const tasks = await fetchAsPage(second.page, "GET", "/api/1/tasks");
  ensure(
    tasks.status === 200 && tasks.json.tasks.some((each) => each.id === id && each.state === "running"),
    `the list the second browser reads: ${tasks.text}`,
  );
  const seen = await taskWhere(
    second.page,
    id,
    (task) => task.state === "running" && task.log.length > 0,
    "running with a log",
  );
  ensure(seen.who === name, `the second browser reads the task as ${seen.who}'s, not ${name}'s`);
  const events = (await taskEvents(second.page, 6000)).filter((event) => event.id === id);
  ensure(events.length > 0, "the event stream did not tell the second browser where the task is");
  ensure(
    events.length <= 4 * 6 + 4,
    `the event stream sent ${events.length} events of the task in 6 seconds, at most 4 a second`,
  );
  for (const event of events) {
    ensure(
      !("log" in event) && !("args" in event) && !("argv" in event),
      `an event of the task holds more than where it is: ${JSON.stringify(event)}`,
    );
    ensure(event.action === "lab.long" && event.steps === 4, `an event of the task is ${JSON.stringify(event)}`);
  }

  // The first browser closes. Its session locks a minute later, and the
  // task goes on.
  const closedAt = Date.now();
  await first.page.context().close();
  // The drawer is looked at while the wait goes on, which the task needs.
  await drawerShowsTask(t, second.page, id, name);
  await sleep(Math.max(0, CLOSED_FOR_SECONDS * 1000 - (Date.now() - closedAt)));
  const later = await taskWhere(second.page, id, () => true, "read");
  ensure(
    later.state === "running",
    `the task is ${later.state} ${CLOSED_FOR_SECONDS} seconds after its browser closed: ${JSON.stringify(later)}`,
  );
  ensure(
    progressOf(later) > progressOf(running),
    `the task has not moved on: ${JSON.stringify(running)} and then ${JSON.stringify(later)}`,
  );

  // Stop, by the other administrator, who needs no unlock for it: in the
  // drawer, with its question.
  await stopInDrawer(t, second.page, id, other);
  const ended = await taskWhere(second.page, id, (task) => task.state !== "running", "ended", 40);
  ensure(
    ended.state === "stopped" && ended.note === `stopped by ${other}` && ended.ended !== null,
    `the stopped task is ${JSON.stringify(ended)}`,
  );
  const again = await fetchAsPage(second.page, "POST", `/api/1/tasks/${id}/stop`, {});
  mustBe(again, 409, "ADM-128", "a stop of the ended task");

  // The journal has the change, made by the first account.
  const changes = await fetchAsPage(second.page, "GET", "/api/1/changes?limit=20");
  ensure(changes.status === 200, `GET /api/1/changes was answered ${changes.status}: ${changes.text}`);
  const row = changes.json.changes.find((each) => each.task === id);
  ensure(row !== undefined, `the journal has no row for task ${id}: ${changes.text}`);
  ensure(
    row.action === "lab.long" && row.who === name && row.how === "console",
    `the row of the task is ${JSON.stringify(row)}`,
  );
  const detail = await fetchAsPage(second.page, "GET", `/api/1/changes/${row.id}`);
  ensure(
    detail.status === 200 && detail.json.task === id,
    `GET /api/1/changes/${row.id} was answered ${detail.status}: ${detail.text}`,
  );

  mustHaveCalled(second.seen, [
    ROUTES.tasks,
    routeOf(ROUTES.task, id),
    routeOf(ROUTES.stop, id),
    ROUTES.changes,
    routeOf(ROUTES.change, row.id),
    ROUTES.events,
  ]);
  mustHaveCalled(first.seen, ["POST /api/1/lab/long"]);
  await drawerOnPhone(t, other);
  await signOut(t, second.page);
}
