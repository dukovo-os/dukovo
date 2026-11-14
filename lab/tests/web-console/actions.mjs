// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/actions drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): every
// action of the console, called from the console's own page as its
// buttons call it. An administrator signs in and unlocks the console, and
// the page, with fetch() and nothing the page may not do (the content
// security policy stays on), reads the list of actions from
// GET /api/1/actions and calls the route of each with the arguments of
// its first good example, with the session's cookie and token. A read is
// answered 200 with JSON. A change is answered 200 and done, and says
// whether it changed anything; the action that its examples name puts back
// what it did: the service the examples use is as it was at the end, by
// the change itself where the undo left it elsewhere (a service that runs
// is started, and an undo stops it).
//
// The list is the one admin actions --json prints in a terminal, which the
// boot test admin-actions holds to the route's; this test takes what the
// page reads, so an action is tried as soon as it is registered.
//
// certificate.renew is not called. It makes the machine's own certificate
// anew, which admind then serves, and the browser has the old one
// accepted: the page's next request would be a new warning, in an engine
// that asks. The boot test admin-actions renews it and sees the console
// answer with the new certificate.

import {
  ensure,
  fetchAsPage,
  look,
  mustHaveCalled,
  openConsole,
  signInDone,
  signOut,
} from "./console.mjs";

// The password that actions.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The actions this test leaves alone, each with its reason above.
// A long action, which a lab build has, answers 202 and runs as a task
// (web-console/tasks), so it is left alone too.
const SKIPPED = new Set(["certificate.renew"]);

// The reads that may be answered with the machine's own words, by error
// ID, where the rest are 200: checks.last runs dukovo-check, which a
// machine has only once an update brings the checks (ADM-129, "This server
// does not have that command yet"); lab.missing, which a lab build has, is
// a read of a command that no image has (ADM-129); metrics.usage reads the
// history that metricsd writes every 10 minutes, which a machine that has
// run for less has no points of (ADM-123).
const MAY_REFUSE = new Map([
  ["checks.last", "ADM-129"],
  ["lab.missing", "ADM-129"],
  ["metrics.usage", "ADM-123"],
]);

/** `text` with every byte outside `A-Za-z0-9-._~` written as `%XX`. */
const percentEncode = (text) =>
  Array.from(new TextEncoder().encode(text), (byte) =>
    /[A-Za-z0-9\-._~]/.test(String.fromCharCode(byte))
      ? String.fromCharCode(byte)
      : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`,
  ).join("");

/** The address of `template`, with each `{name}` part given by `args`, and the arguments the address does not take. */
function addressOf(template, args) {
  const rest = { ...args };
  const path = template
    .split("/")
    .map((part) => {
      const name = /^\{(.*)\}$/.exec(part)?.[1];
      if (name === undefined) return part;
      ensure(name in rest, `${template} wants ${name}, which the example does not give`);
      const value = rest[name];
      delete rest[name];
      return percentEncode(String(value));
    })
    .join("/");
  return { path, rest };
}

/** The state of the service `name` as the console lists it: `state` and whether it starts at boot. */
function serviceState(listed, name) {
  const service = listed.json?.services?.find((each) => each.name === name);
  ensure(service !== undefined, `the list of services has no ${name}`);
  return `${service.state}, ${service.enabled ? "enabled" : "not enabled"}`;
}

export default async function check(t) {
  const name = `wc-actions-${t.engine}`;
  const { page, seen } = await openConsole(t, "/");
  await signInDone(t, page, name, PASSWORD);
  const unlocked = await fetchAsPage(page, "POST", "/api/1/unlock", { kind: "password", password: PASSWORD });
  ensure(unlocked.status === 200, `the page's own unlock was answered ${unlocked.status}: ${unlocked.text}`);

  const listed = await fetchAsPage(page, "GET", "/api/1/actions");
  ensure(listed.status === 200, `GET /api/1/actions was answered ${listed.status}: ${listed.text}`);
  const actions = listed.json;
  ensure(Array.isArray(actions) && actions.length > 0, `GET /api/1/actions gave ${listed.text}`);
  const called = ["GET /api/1/actions"];

  /** Calls the route of `action` with `args` from the page, and gives the answer. */
  async function call(action, args) {
    const { path, rest } = addressOf(action.path, args);
    ensure(action.changes || Object.keys(rest).length === 0, `${action.id}'s example has arguments its address does not take`);
    const method = action.changes ? "POST" : "GET";
    called.push(`${method} ${path}`);
    return fetchAsPage(page, method, path, action.changes ? rest : null);
  }

  /** Calls the change `id`, which the list has, with `args`: done. */
  async function change(id, args) {
    const action = actions.find((each) => each.id === id);
    ensure(action?.changes, `${id} is no change in the list`);
    const answer = await call(action, args);
    ensure(answer.status === 200, `${id} from the page was answered ${answer.status}: ${answer.text}`);
    ensure(answer.json?.done === true, `${id} from the page was answered ${answer.text}, which does not say it is done`);
    ensure(typeof answer.json?.changed === "boolean", `${id} from the page was answered ${answer.text}, which does not say whether it changed anything`);
  }

  // The service the examples of the services' actions use, if there is one,
  // and how the console lists it, to see it put back.
  const lister = actions.find((each) => each.id === "service.list");
  const service = actions.find((each) => each.area === "services" && each.changes && !each.long)?.examples.good[0]?.name;
  const stateNow = async () => (lister && service ? serviceState(await call(lister, lister.examples.good[0]), service) : null);

  // The reads first.
  for (const action of actions.filter((each) => !each.changes)) {
    const answer = await call(action, action.examples.good[0]);
    if (MAY_REFUSE.has(action.id) && answer.json?.error?.id === MAY_REFUSE.get(action.id)) continue;
    ensure(answer.status === 200, `${action.id} from the page was answered ${answer.status}: ${answer.text}`);
    ensure(answer.json !== null, `${action.id} from the page was answered ${answer.text}, which is not JSON`);
  }

  // The console's own reads, which the Overview shows, have the shapes it
  // reads. GET /api/1/console/about names the keys of `admin about --json`,
  // GET /api/1/console/notices a list of notices, and GET /api/1/metrics/now
  // is JSON. GET /api/1/metrics/usage/{range} takes a word of a list, 1h,
  // 24h, 7d or 1y, and is answered with the points or, on a machine that has
  // kept no history yet, with metrics's own words (ADM-123); any other word
  // is ADM-125 with the field named.
  const own = async (path, check) => {
    called.push(`GET ${path}`);
    const answer = await fetchAsPage(page, "GET", path, null);
    ensure(answer.status === 200, `GET ${path} was answered ${answer.status}: ${answer.text}`);
    check(answer.json, answer.text);
  };
  await own("/api/1/console/about", (json, text) => {
    const keys = ["host", "addresses", "port", "fingerprint", "release", "fallback", "machine", "processor", "memory", "up", "zone", "clock", "ssh_host_key", "last_good_start", "sign_ins"];
    for (const key of keys) ensure(json !== null && key in json, `GET /api/1/console/about has no ${key}: ${text}`);
  });
  await own("/api/1/console/notices", (json, text) => {
    ensure(Array.isArray(json?.notices), `GET /api/1/console/notices does not give a list of notices: ${text}`);
    for (const notice of json.notices) ensure(typeof notice.name === "string" && Array.isArray(notice.lines) && "at" in notice, `a notice is ${JSON.stringify(notice)}`);
  });
  await own("/api/1/metrics/now", (json, text) => ensure(json !== null && typeof json === "object", `GET /api/1/metrics/now gave ${text}`));
  for (const range of ["1h", "24h", "7d", "1y"]) {
    const path = `/api/1/metrics/usage/${range}`;
    called.push(`GET ${path}`);
    const answer = await fetchAsPage(page, "GET", path, null);
    ensure(answer.status === 200 || answer.json?.error?.id === "ADM-123", `GET /api/1/metrics/usage/{range} for ${range} was answered ${answer.status}: ${answer.text}`);
  }
  called.push("GET /api/1/metrics/usage/2h");
  const odd = await fetchAsPage(page, "GET", "/api/1/metrics/usage/2h", null);
  ensure(odd.status === 400 && odd.json?.error?.id === "ADM-125" && odd.json?.error?.field === "range", `GET /api/1/metrics/usage/{range} for 2h was answered ${odd.status}: ${odd.text}`);
  // GET /api/1/checks/last is dukovo-check's last result, or ADM-129 where
  // the machine does not have dukovo-check yet.
  called.push("GET /api/1/checks/last");
  const checks = await fetchAsPage(page, "GET", "/api/1/checks/last", null);
  ensure(checks.status === 200 || checks.json?.error?.id === "ADM-129", `GET /api/1/checks/last was answered ${checks.status}: ${checks.text}`);

  // Then each change, and what puts it back.
  const before = await stateNow();
  for (const action of actions.filter((each) => each.changes && !each.long && !SKIPPED.has(each.id))) {
    const args = action.examples.good[0];
    const undo = action.examples.undo;
    if (undo === null) {
      await change(action.id, args);
      ensure((await stateNow()) === before, `${action.id} left ${await stateNow()}, and the service was ${before}`);
      continue;
    }
    // From the state the undo leaves, so that the change has something to do.
    await change(undo.action, undo.args);
    await change(action.id, args);
    await change(undo.action, undo.args);
    if ((await stateNow()) !== before) await change(action.id, args);
    const now = await stateNow();
    ensure(now === before, `after ${action.id} and ${undo.action} the service is ${now}, and it was ${before}`);
  }

  mustHaveCalled(seen, called);
  // The page's own calls broke no part of the content security policy, and
  // the page is still one that passes the accessibility check.
  await look(t, page, "actions");
  await signOut(t, page);
}
