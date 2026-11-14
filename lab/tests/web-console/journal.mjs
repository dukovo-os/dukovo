// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/journal drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): a hand edit
// of a service file, as the console's journal tells it (ADR 0516). The root
// step of journal.toml before each engine has added a comment line naming
// the engine to the service file of diskhealth, which no console change
// explains. An administrator signs in and unlocks the console. The round
// that looks for hand edits runs every 30 seconds, so the script asks for
// GET /api/1/changes every few seconds, for up to 90, until the newest
// `by hand` row whose diff of the file adds the engine's line is there. The
// row says that `lab` did it: the lab runs a root step as `elevate -S sh
// -c ...`, which authd's audit log has with the file's path as a word of the
// command, and that is how admind finds who.
//
// Keep it is refused while the console is locked, as Put back is, and is
// refused for a row that the console made itself. Unlocked, it marks the
// row seen by the administrator, and a second Keep it leaves it as it was.
// Put back answers with the command that ran, `elevate admin put-back N`,
// says that Spark reads the service file when the service next starts, and
// has a row of its own that names the change it puts back. That row has the
// file as admind read it when the command ended, and it is the file as it
// was before the hand edit, without the line. A change that is not there is
// ADM-127 for the change, for Keep it and for Put back.

import {
  ensure,
  fetchAsPage,
  mustBe,
  mustHaveCalled,
  openConsole,
  routeOf,
  signInDone,
  signOut,
  sleep,
} from "./console.mjs";

// The password that journal.toml gives the account it makes.
const PASSWORD = "lab-console-password";
const NAME = "wc-journal";

// The file that journal.toml changes: the service file of diskhealth, which
// every image runs and nothing needs.
const FILE = "/system/settings/spark/services/diskhealth.toml";

// The account that runs the lab's root steps, and so the person whose
// elevated command names the file.
const EDITOR = "lab";

// What a put back says of a service file: Spark is asked nothing, since it
// reads the file when the service next starts.
const READ_WHEN_STARTED =
  "Spark reads diskhealth's service file when diskhealth next starts: `service diskhealth restart` puts it to use now";

// The routes that lab/coverage.toml says this test covers, as the console's
// table names them: the browser must have called each, with the number of
// a change for {id}, by the end.
const ROUTES = {
  changes: "GET /api/1/changes",
  change: "GET /api/1/changes/{id}",
  keep: "POST /api/1/changes/{id}/keep",
  putBack: "POST /api/1/changes/{id}/put-back",
};

// A change that is not there.
const ABSENT = 999999;

// How long a hand edit is waited for, and how often the journal is asked
// meanwhile: the round that finds hand edits runs every 30 seconds.
const WAIT_SECONDS = 90;
const ASK_EVERY_MILLISECONDS = 3000;

/** Unlocks the console for the session of `page`, with the account's password. */
async function unlock(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(answer.status === 200, `the unlock was answered ${answer.status}: ${answer.text}`);
}

/** Locks the console for the session of `page`. */
async function lock(page) {
  const answer = await fetchAsPage(page, "POST", "/api/1/lock", {});
  ensure(answer.status === 200, `the lock was answered ${answer.status}: ${answer.text}`);
}

/** Change `id` as `GET /api/1/changes/{id}` gives it, with the diff of each file. */
async function readChange(page, id) {
  const answer = await fetchAsPage(page, "GET", `/api/1/changes/${id}`);
  ensure(answer.status === 200, `GET /api/1/changes/${id} was answered ${answer.status}: ${answer.text}`);
  return answer.json;
}

/** The entry of the service file among the files of `change`. */
function fileOf(change) {
  const file = change.files.find((each) => each.path === FILE);
  ensure(file !== undefined, `change ${change.id} does not name ${FILE}: ${JSON.stringify(change.files)}`);
  return file;
}

/**
 * The lines that the diff of the service file in `change` adds, for `sign`
 * `+`, or removes, for `-`, without their sign.
 */
function linesOf(change, sign) {
  const header = sign.repeat(3);
  return (fileOf(change).diff ?? "")
    .split("\n")
    .filter((text) => text.startsWith(sign) && !text.startsWith(`${header} `))
    .map((text) => text.slice(1));
}

/**
 * The newest row made `by hand` whose diff of the service file adds `line`,
 * as `GET /api/1/changes/{id}` gives it, waited for up to `WAIT_SECONDS`.
 */
async function handEditAdding(page, line) {
  const until = Date.now() + WAIT_SECONDS * 1000;
  const others = new Set();
  for (;;) {
    const list = await fetchAsPage(page, "GET", "/api/1/changes?limit=200");
    ensure(
      list.status === 200 && Array.isArray(list.json?.changes),
      `GET /api/1/changes was answered ${list.status}: ${list.text}`,
    );
    for (const row of list.json.changes) {
      if (row.how !== "by hand" || others.has(row.id) || !row.files.some((file) => file.path === FILE)) continue;
      const change = await readChange(page, row.id);
      if (linesOf(change, "+").includes(line)) return change;
      others.add(row.id);
    }
    ensure(
      Date.now() < until,
      `no row made by hand adds "${line}" to ${FILE} within ${WAIT_SECONDS} seconds`,
    );
    await sleep(ASK_EVERY_MILLISECONDS);
  }
}

/** What the console says of a change that is not there: the change, its Keep it and its Put back are ADM-127. */
async function absentChange(page) {
  mustBe(await fetchAsPage(page, "GET", `/api/1/changes/${ABSENT}`), 404, "ADM-127", "a change that is not there");
  mustBe(await fetchAsPage(page, "POST", `/api/1/changes/${ABSENT}/keep`, {}), 404, "ADM-127", "its Keep it");
  mustBe(await fetchAsPage(page, "POST", `/api/1/changes/${ABSENT}/put-back`, {}), 404, "ADM-127", "its Put back");
}

/**
 * Marks change `id` seen by the account, and again a moment later, which
 * changes nothing, and checks that the journal says so.
 */
async function keepIt(page, id) {
  const route = `/api/1/changes/${id}/keep`;
  const kept = await fetchAsPage(page, "POST", route, {});
  ensure(kept.status === 200, `Keep it was answered ${kept.status}: ${kept.text}`);
  const marked = kept.json.seen;
  ensure(
    marked?.by === NAME && Number.isInteger(marked.at),
    `Keep it marked change ${id} as ${JSON.stringify(marked)}, not as seen by ${NAME}`,
  );
  // A second Keep it that wrote the time again would show a second later.
  await sleep(2000);
  const again = await fetchAsPage(page, "POST", route, {});
  ensure(
    again.status === 200 && JSON.stringify(again.json.seen) === JSON.stringify(marked),
    `Keep it again was answered ${again.status} ${again.text}, after ${JSON.stringify(marked)}`,
  );
  const listed = await readChange(page, id);
  ensure(
    JSON.stringify(listed.seen) === JSON.stringify(marked),
    `GET /api/1/changes/${id} says ${JSON.stringify(listed.seen)} after Keep it, not ${JSON.stringify(marked)}`,
  );
}

/**
 * Puts back `edit`, a hand edit that added `line` to the service file, and
 * checks what the answer and the row of the put back say. Gives that row.
 */
async function putBack(page, edit, line) {
  const id = edit.id;
  const answer = await fetchAsPage(page, "POST", `/api/1/changes/${id}/put-back`, {});
  ensure(answer.status === 200, `the put back of change ${id} was answered ${answer.status}: ${answer.text}`);
  const told = answer.json;
  ensure(
    told.done === true && told.command === `elevate admin put-back ${id}` && Number.isInteger(told.change),
    `the put back of change ${id} was answered ${answer.text}`,
  );
  ensure(
    told.output.includes(`put back ${FILE}`) && told.output.includes(READ_WHEN_STARTED),
    `the put back says ${JSON.stringify(told.output)}, not that it wrote the file and when Spark reads it`,
  );

  // Its row names the change it puts back and the file as it was left.
  const row = await readChange(page, told.change);
  ensure(
    row.action === "changes.put-back" &&
      row.title === "Put a change back" &&
      row.who === NAME &&
      row.how === "console" &&
      row.status === 0 &&
      row.note === `puts back change ${id}`,
    `the put back's own row is ${JSON.stringify(row)}`,
  );
  const [hand, back] = [fileOf(edit), fileOf(row)];
  ensure(
    back.before.sha256 === hand.after.sha256 && back.after.sha256 === hand.before.sha256,
    `the put back took ${FILE} from ${back.before.sha256} to ${back.after.sha256}, not from ${hand.after.sha256} back to ${hand.before.sha256}`,
  );
  ensure(linesOf(row, "-").includes(line), `the put back's row does not remove "${line}" from ${FILE}`);
  return row;
}

/** The test, run once for each engine of the step that names it. */
export default async function check(t) {
  const line = `# lab journal ${t.engine}`;
  const { page, seen: called } = await openConsole(t, "/");
  await signInDone(t, page, NAME, PASSWORD);
  await unlock(page);
  await absentChange(page);

  // The hand edit, as the journal tells it.
  const edit = await handEditAdding(page, line);
  const id = edit.id;
  ensure(
    edit.who === EDITOR,
    `the row of the hand edit names ${edit.who}, not ${EDITOR}: admind could not tell who wrote ${FILE} from authd's audit lines`,
  );
  ensure(
    edit.action === null && edit.status === null && edit.seen === null,
    `a hand edit has no action, no status and no mark, and this one is ${JSON.stringify(edit)}`,
  );
  ensure(
    edit.put_back.possible === true && edit.put_back.command === `elevate admin put-back ${id}`,
    `the hand edit cannot be put back as elevate admin put-back ${id}: ${JSON.stringify(edit.put_back)}`,
  );

  // Locked, the console shows the row and keeps and puts back nothing.
  await lock(page);
  mustBe(await fetchAsPage(page, "POST", `/api/1/changes/${id}/keep`, {}), 403, "ADM-106", "Keep it, locked");
  mustBe(await fetchAsPage(page, "POST", `/api/1/changes/${id}/put-back`, {}), 403, "ADM-106", "Put back, locked");
  ensure((await readChange(page, id)).seen === null, "a locked session marked the change seen");
  await unlock(page);

  await keepIt(page, id);
  const row = await putBack(page, edit, line);
  // The put back is a change of the console's, which is in the journal
  // already: only a hand edit is marked seen.
  const keptPutBack = await fetchAsPage(page, "POST", `/api/1/changes/${row.id}/keep`, {});
  mustBe(keptPutBack, 409, "ADM-128", "Keep it on the put back's row");

  mustHaveCalled(called, [
    ROUTES.changes,
    routeOf(ROUTES.change, id),
    routeOf(ROUTES.keep, id),
    routeOf(ROUTES.putBack, id),
  ]);
  await signOut(t, page);
}
