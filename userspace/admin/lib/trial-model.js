// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A tried change, as plain values (design 8): what Spark's document says of
// the trial, the words of its countdown, who began it, how it ended, the
// windows a person may choose, and the machine that finds out whether the
// browser still reaches the console over the new settings. lib/trial-banner.js
// draws it. The trials are `network` and `firewall`. The words of the header's
// chip, which shows every running trial on every page (shell/trial-chip.js),
// are here too.

import { addressOfHost } from './address-model.js';

/** The windows of an undo, in the order they are offered. */
export const WINDOWS = Object.freeze([
  { seconds: 60, label: '60 seconds' },
  { seconds: 120, label: '2 minutes' },
  { seconds: 300, label: '5 minutes' },
  { seconds: 600, label: '10 minutes' },
]);

/** The window that is chosen when a change takes nothing away from the browser. */
export const DEFAULT_WINDOW = 60;

/**
 * The windows to offer, and the one chosen, for a change that takes away
 * the address the browser reached the console by (`cuts`, a boolean) when
 * the browser came to `host`, as `location.hostname` gives it (design 8.5):
 * by an address, there is a new tab to open, so 5 and 10 minutes; by a name,
 * the new address is found by name, so 2, 5 and 10; otherwise all four.
 */
export function windowsFor(cuts, host) {
  if (!cuts) return { choices: WINDOWS, chosen: DEFAULT_WINDOW };
  const wanted = addressOfHost(host) !== null ? [300, 600] : [120, 300, 600];
  return { choices: WINDOWS.filter((one) => wanted.includes(one.seconds)), chosen: 300 };
}

// ---- Spark's document --------------------------------------------------------

/** The trial `id` of the document `show` (`GET /api/1/services`): `{id, files, services, user, left_ms}`, or null. */
export function trialOf(show, id) {
  const trials = Array.isArray(show?.trials) ? show.trials : [];
  return trials.find((trial) => trial?.id === id && Number.isFinite(trial.left_ms)) ?? null;
}

/** The times, by Spark's clock, at which trials `id` were undone, among the last eight the document keeps. */
export function undoneAt(show, id) {
  const undone = Array.isArray(show?.undone) ? show.undone : [];
  return undone.filter((entry) => entry?.id === id).map((entry) => entry.at_ms);
}

/**
 * How the trial `id` ended, from the document `show` read after it was
 * gone, and `before`, what `undoneAt` said while it ran: `{how: 'undone',
 * why}` when Spark has undone a trial of that name since (`why` is its
 * words, "it was not kept"), else `{how: 'kept'}`.
 */
export function endedHow(show, id, before) {
  const found = (Array.isArray(show?.undone) ? show.undone : []).find((entry) => entry?.id === id && !before.includes(entry.at_ms));
  if (!found) return { how: 'kept' };
  return { how: 'undone', why: typeof found.why === 'string' && found.why !== '' ? found.why : 'it was not kept' };
}

// ---- the countdown -----------------------------------------------------------

/** The whole seconds left of `ms`, rounded up, so that the clock reads 0:00 only when the time is out. */
const secondsOf = (ms) => Math.max(0, Math.ceil(ms / 1000));

/** The time left as a clock, "4:47". */
export function clockWords(ms) {
  const seconds = secondsOf(ms);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

/** `count` and its noun, with an s when it is not one. */
const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** The time left in words, as a screen reader says it: "4 minutes 47 seconds", "30 seconds". */
export function leftWords(ms) {
  const seconds = secondsOf(ms);
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes === 0) return counted(rest, 'second');
  return rest === 0 ? counted(minutes, 'minute') : `${counted(minutes, 'minute')} ${counted(rest, 'second')}`;
}

/** The shares of the window left, 0 to 100, for the meter; whole steps when `steps` says motion is to be kept small. */
export function percentLeft(leftMs, windowMs, steps = false) {
  if (!(windowMs > 0)) return 0;
  const share = Math.min(100, Math.max(0, (leftMs / windowMs) * 100));
  return steps ? Math.ceil(share / 10) * 10 : share;
}

/** The times left at which the polite region speaks, in milliseconds, the biggest first. */
export const MOMENTS = Object.freeze([60_000, 30_000, 10_000]);

/**
 * The moment of `MOMENTS` that the time left, `ms`, has come to: the
 * smallest that is not under it, or null while there is more time than the
 * first. The page keeps the last it said and says only a new one, so a tab
 * that was not running for a while says the one it has come to, not each it
 * missed.
 */
export function announceAt(ms) {
  const reached = MOMENTS.filter((moment) => ms <= moment);
  return reached.length === 0 ? null : Math.min(...reached);
}

/** What the polite region says at a moment (design 8.9). */
export const momentWords = (moment) => `${leftWords(moment)} left to keep the change.`;

// ---- who began it ------------------------------------------------------------

/** The settings files and the actions that try them, for each trial. */
const FAMILIES = Object.freeze({
  network: { files: ['/system/settings/network'], begins: ['network.set'] },
  firewall: { files: ['/system/settings/firewall'], begins: ['firewall.add', 'firewall.replace', 'firewall.remove'] },
  ssh: { files: ['/system/settings/ssh/sshd_config', '/system/settings/spark/services/sshd.toml'], begins: ['ssh.set', 'ssh.off'] },
  console: { files: ['/system/settings/admin.toml'], begins: ['console.reach'] },
});

/** The seconds of a `--try SECONDS` in `argv`, or null. */
function trySeconds(argv) {
  const at = Array.isArray(argv) ? argv.indexOf('--try') : -1;
  const seconds = at < 0 ? Number.NaN : Number(argv[at + 1]);
  return Number.isFinite(seconds) ? seconds : null;
}

/**
 * Who began the running trial `id`, from the journal's rows (`GET
 * /api/1/changes`, newest first): the newest row that names one of the
 * trial's files or one of its actions decides, a refused one aside. A row of
 * an action that tries the files began it: `{who, how, seconds, card}`
 * (`card` is the network card, null for the others); a keep, an undo or a
 * change that was not tried, or a change of a file by hand, by another
 * action or by Spark's undo, means a trial now running began at a terminal,
 * which is null. A trial this page does not know, as a service's, is
 * undefined: who began it is not said.
 */
export function triedBy(rows, id) {
  const family = Object.hasOwn(FAMILIES, id) ? FAMILIES[id] : undefined;
  if (family === undefined) return undefined;
  for (const row of Array.isArray(rows) ? rows : []) {
    // A change that was refused changed nothing, and says nothing of the trial.
    if (typeof row.action === 'string' && row.status !== 0) continue;
    if (typeof row.action === 'string' && row.action.startsWith(`${id}.`)) {
      const seconds = family.begins.includes(row.action) ? trySeconds(row.argv) : null;
      if (seconds === null) return null;
      return { who: row.who, how: row.how, seconds, card: id === 'network' ? (row.argv[2] ?? null) : null };
    }
    if (Array.isArray(row.files) && row.files.some((file) => family.files.includes(file.path))) return null;
  }
  return null;
}

/** Who began a trial, in words: "alice in this console", "ci with the API token ci", "a terminal". */
export function begunWords(began) {
  if (began === null) return 'at a terminal';
  const token = /^token (.+)$/.exec(began.how ?? '');
  return token ? `by ${began.who} with the API token ${token[1]}` : `by ${began.who} in this console`;
}

// ---- does the browser reach the console --------------------------------------

/** How many times in a row `fresh` is asked while it says the connection is an old one (design 23.3: the answer closes it, so the next asks on a new one). */
export const ASKS_AT_ONCE = 3;

/** How long after an answer that is not final `fresh` is asked again, and how long one ask may take, in milliseconds. */
export const RETRY_MS = 3000;
export const ASK_MS = 5000;

/** What an error of `GET /api/1/console/fresh/{trial}` says: `old` (ADM-133), `gone` (ADM-135) or `cannot`, for no answer, a late one and any other. */
function kindOf(error) {
  if (error?.status === 409 || error?.id === 'ADM-133') return 'old';
  if (error?.status === 404 || error?.id === 'ADM-135') return 'gone';
  return 'cannot';
}

/** `promise`, or a rejection that says `{status: 0, id: null, timeout: true}` when it takes more than `ms`; `schedule(fn, ms)` gives what cancels the timer. */
function inTime(promise, ms, schedule) {
  return new Promise((resolve, reject) => {
    const cancel = schedule(() => reject(Object.assign(new Error('no answer in time'), { status: 0, id: null, timeout: true })), ms);
    promise.then(resolve, reject).finally(cancel);
  });
}

/**
 * Asks whether this browser reaches the console over a connection opened
 * since the change (design 8.4): `ask()` is the call, which resolves with
 * `{fresh, from, via}` or rejects. At most `asks` in a row while the answer
 * is that the connection is an old one. Gives `{kind: 'reached', from, via}`,
 * `{kind: 'gone'}` (nothing is tried by that name), `{kind: 'cannot'}` (no
 * answer, or a failed one) or `{kind: 'unsure'}` (every ask was an old
 * connection).
 */
export async function reachRound(ask, { asks = ASKS_AT_ONCE, timeoutMs = ASK_MS, schedule }) {
  for (let tried = 0; tried < asks; tried += 1) {
    let answer;
    try {
      answer = await inTime(ask(), timeoutMs, schedule);
    } catch (error) {
      const kind = kindOf(error);
      if (kind === 'old') continue;
      return { kind };
    }
    if (answer?.fresh === true) return { kind: 'reached', from: answer.from ?? null, via: answer.via ?? null };
    return { kind: 'cannot' };
  }
  return { kind: 'unsure' };
}

/**
 * The reach machine: `start()` asks in rounds, again after `RETRY_MS` while
 * the answer is not final, until the browser has reached the console, nothing
 * is tried by that name, `over()` says the window has ended, or `stop()`.
 * `onChange(state)` hears each state: `{kind}` is `idle`, `asking`,
 * `reached` (with `from` and `via`), `cannot`, `unsure`, `gone` or `over`.
 * `check()` is one round on its own, for a page that did not begin the trial
 * and asks when its person presses Keep. `schedule(fn, ms)` is the timer and
 * gives what cancels it; tests replace it.
 */
export function makeReach({
  ask, schedule, onChange = () => {}, over = () => false, asks = ASKS_AT_ONCE, retryMs = RETRY_MS, timeoutMs = ASK_MS,
}) {
  let state = { kind: 'idle' };
  let generation = 0;
  let cancel = null;
  const options = { asks, timeoutMs, schedule };

  const set = (next) => {
    state = next;
    onChange(state);
  };

  async function run(mine) {
    if (state.kind === 'idle') set({ kind: 'asking' });
    const result = await reachRound(ask, options);
    if (mine !== generation) return;
    set(result);
    if (result.kind !== 'cannot' && result.kind !== 'unsure') return;
    if (over()) {
      set({ kind: 'over' });
      return;
    }
    cancel = schedule(() => run(mine), retryMs);
  }

  return {
    state: () => state,
    start() {
      generation += 1;
      cancel?.();
      state = { kind: 'idle' };
      return run(generation);
    },
    stop() {
      generation += 1;
      cancel?.();
      cancel = null;
    },
    check: () => reachRound(ask, options),
  };
}

/**
 * What the banner says of the reach, as `{words, keep}`: the sentence, and
 * the reason Keep is off (`keep` is null when it is on). `ends` is the time
 * the change is undone, as the clock reads it ("2:32 PM").
 */
export function reachWords(state, ends) {
  const cannot = `This browser cannot reach the console over the new settings. It is undone at ${ends} unless it is kept.`;
  switch (state.kind) {
    case 'reached':
      return { words: 'This browser reached the console over the new settings.', keep: null };
    case 'cannot':
      return { words: cannot, keep: cannot };
    case 'unsure':
      return {
        words: 'Cannot tell yet whether this browser reaches the console over the new settings. Asking again.',
        keep: 'Keep is on once this browser has reached the console over the new settings.',
      };
    case 'gone':
      return { words: 'Nothing is being tried any more.', keep: 'Nothing is being tried any more.' };
    case 'over':
      return { words: 'The time to keep the change is over.', keep: 'The time to keep the change is over.' };
    default:
      return {
        words: 'Checking that this browser still reaches the console over the new settings…',
        keep: 'Keep is on once this browser has reached the console over the new settings.',
      };
  }
}

// ---- the header's chip -------------------------------------------------------

/** How long after its end a trial is still shown, in milliseconds: admind says that it ended within its 30-second round, so a longer silence means the stream missed it. */
export const OVERDUE_MS = 60_000;

/**
 * The trials of a `trial` event, `[{id, left_ms, page, what}]`, as the chip
 * holds them: `{page, what, endsMs}`, with the time each ends by this
 * browser's clock, `now` being when the event came. An entry that is not
 * `{left_ms, page, what}` is left out.
 */
export function heldTrials(list, now) {
  return (Array.isArray(list) ? list : [])
    .filter((trial) => Number.isFinite(trial?.left_ms) && typeof trial.page === 'string' && typeof trial.what === 'string')
    .map((trial) => ({ page: trial.page, what: trial.what, endsMs: now + trial.left_ms }));
}

/** The trials of `held` that are not over by more than `OVERDUE_MS` at `now`. */
export const stillShown = (held, now) => held.filter((trial) => trial.endsMs + OVERDUE_MS > now);

/** `what` as it reads after "Trying:": a first word of one capital and small letters ("The", "Who") starts with a small one; "SSH's" and a service's name stay as they are. */
const inSentence = (what) => (/^[A-Z][a-z]*(?:\s|$)/.test(what) ? what[0].toLowerCase() + what.slice(1) : what);

/** When a trial is undone, as the person's clock reads it: "undone at 2:32 PM"; `clock(seconds)` is the person's zone's clock (lib/time.js). */
export const endsWords = (endsMs, clock) => `undone at ${clock(Math.floor(endsMs / 1000))}`;

/** The part of `page` before the `#`: the route that owns a trial. */
const routeOf = (page) => page.split('#')[0];

/**
 * What the header's chip says for the trials `held` (`heldTrials`) at `now`
 * on the route `route`; a trial whose own page is the route is left out,
 * since the page has its banner. Gives null when none is left; else
 * `{words, clock, label, href, items}`:
 *
 * - `words` and `clock`: what is seen, "Trying: the network settings" and
 *   "4:12", the time left of the soonest. With more than one trial `words` is
 *   "Trying 2 changes";
 * - `label`: the accessible name, which holds the words and the time of the
 *   end, since the seconds are hidden from a screen reader;
 * - `href`: where the chip goes, null with more than one trial, which it
 *   lists in `items`, `{label, href}` for each, the soonest first.
 */
export function chipWords(held, { route, now, clock }) {
  const shown = held.filter((trial) => routeOf(trial.page) !== route).sort((a, b) => a.endsMs - b.endsMs);
  if (shown.length === 0) return null;
  const [soonest] = shown;
  const several = shown.length > 1;
  const words = several ? `Trying ${shown.length} changes` : `Trying: ${inSentence(soonest.what)}`;
  const label = `${words}, ${several ? 'the soonest ' : ''}${endsWords(soonest.endsMs, clock)} unless kept`;
  return {
    words,
    clock: clockWords(soonest.endsMs - now),
    label,
    href: several ? null : soonest.page,
    items: shown.map((trial) => ({ label: `${trial.what}, ${endsWords(trial.endsMs, clock)}`, href: trial.page })),
  };
}
