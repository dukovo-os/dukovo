// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Updates page says of the last check, as plain values with no DOM:
// the checked line, the System, Firmware, Apps and Security cards by the state
// each offer is in, the holds, and the numbers the navigation and the
// Overview count. The documents are those of `update check --json`, `update
// last --json`, `update settings --json` and `sysupdate status --json`; a
// field that is missing is left out of the words and never said as "null".

import { kindOfUpdate, versionPair } from '../../lib/version-words.js';

const list = (value) => (Array.isArray(value) ? value : []);
const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);
const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** The size of a download in whole megabytes, "412 MB" (a thousand to a unit, as the disks count). */
const megabytes = (bytes) => `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;

/** The release a server runs, from `release` in `console.about`'s answer ("Dukovo Server 1.0.0-alpha.2"): its last word, or null. */
export const runningOf = (about) => (typeof about?.release === 'string' ? about.release.trim().split(/\s+/).at(-1) || null : null);

/** `words` as a sentence: one that ends in a full stop, a question mark or an exclamation mark is left as it is. */
export const ended = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/** "3:00 AM" for "03:00", the time of day a restart is set for. */
export function clockWords(hhmm) {
  const [hour, minute] = String(hhmm).split(':').map(Number);
  return `${hour % 12 === 0 ? 12 : hour % 12}:${String(minute).padStart(2, '0')} ${hour < 12 ? 'AM' : 'PM'}`;
}

/** "on Nov 13 at 3:06 AM", "today at 3:06 AM": when something was done, from the person's clock `time` (lib/time.js). */
export function atWords(ms, time) {
  const words = time.when(ms / 1000);
  if (/^\d/.test(words)) return `today at ${words}`;
  if (words.startsWith('Yesterday ')) return `yesterday at ${words.slice(10)}`;
  const found = /^(.+?) (\d{1,2}:\d{2} [AP]M)$/.exec(words);
  return found ? `on ${found[1]} at ${found[2]}` : `on ${words}`;
}

// ---- the last check ---------------------------------------------------------

/**
 * The check that an answer holds: `updates.last`'s (`{checked_ms, by,
 * updates}`) or `updates.check`'s own document, which was made `at` ms. Gives
 * null for none (the last check was never kept: exit 3).
 */
export function checkOf(answer, at = Date.now()) {
  const doc = answer?.updates ?? (Array.isArray(answer?.offers) ? answer : null);
  if (!doc || !Array.isArray(doc.offers)) return null;
  return {
    checkedMs: answer.updates ? (Number.isFinite(answer.checked_ms) ? answer.checked_ms : null) : at,
    by: answer.updates ? text(answer.by) : null,
    offers: doc.offers.filter((offer) => typeof offer?.item === 'string'),
    withdrawn: doc.running_withdrawn?.version ? doc.running_withdrawn : null,
    unchecked: list(doc.unchecked).filter((one) => typeof one?.item === 'string'),
    notes: list(doc.notes).filter((note) => typeof note === 'string'),
    advisories: list(doc.security?.system),
    appFlaws: Number.isFinite(doc.security?.app_flaws) ? doc.security.app_flaws : 0,
  };
}

const BY = { updated: ' by the daily check', 'update check': ' by update check' };

/** The line under the head: "Checked 12 minutes ago by the daily check.", "Checked just now.", "Not checked yet." */
export function checkedWords(check, time) {
  if (!check) return 'Not checked yet.';
  if (check.checkedMs === null) return 'Checked at a time that was not kept.';
  return `Checked ${time.relative(check.checkedMs / 1000)}${BY[check.by] ?? ''}.`;
}

/** The offer for `item` (`system`, `firmware` or an app's ID), or null. */
export const offerOf = (check, item) => check?.offers.find((offer) => offer.item === item) ?? null;

/** The offers that are an app's. */
export const appOffers = (check) => (check ? check.offers.filter((offer) => offer.item !== 'system' && offer.item !== 'firmware') : []);

/** Why `item` could not be checked: the command's words, `''` when it gave none, or null when it could. */
export function whyUnchecked(check, item) {
  const found = check?.unchecked.find((one) => one.item === item);
  return found ? text(found.why) ?? '' : null;
}

/** "Could not check: WHY." The page never says "up to date" for a part it could not check. */
export const couldNot = (why) => (why ? `Could not check: ${ended(why)}` : 'Could not check.');

// ---- advisories -------------------------------------------------------------

const RANK = { critical: 4, high: 3, medium: 2, low: 1 };

/** The worst severity of `flaws`, or null when none says one. */
export function worstOf(flaws) {
  return flaws.map((flaw) => flaw.severity).filter((severity) => RANK[severity]).sort((a, b) => RANK[b] - RANK[a])[0] ?? null;
}

/** "DKSA-2026-0007, high: summary". */
export const flawWords = (flaw) => `${flaw.id}${flaw.severity ? `, ${flaw.severity}` : ''}${flaw.summary ? `: ${flaw.summary}` : ''}`;

const FIX = {
  offered: (flaw) => `${flaw.release ?? 'A newer release'} has the fix and is offered.`,
  staged: () => 'The fix is staged: restart to finish the update.',
  waiting: () => 'The fix reaches this machine in a later wave.',
  held: () => 'The fix is held until you release it.',
  none: () => 'No release has the fix yet.',
};

/** What is done about an advisory of the system, from its `fix`: "1.0.0-alpha.2 has the fix and is offered." */
export const fixWords = (flaw) => (FIX[flaw.fix] ?? FIX.none)(flaw);

// ---- the System card --------------------------------------------------------

const PILL = {
  current: ['ok', 'Up to date'], update: ['info', 'Update ready'], staged: ['info', 'Staged'], waiting: ['', 'Waiting'],
  held: ['warn', 'Held'], withdrawn: ['bad', 'Withdrawn'], unchecked: ['warn', 'Could not check'], none: ['', 'Not checked yet'],
};

/** The version in slot `slot` of `slots` (`sysupdate status --json`), or null. */
export const slotVersion = (slots, slot) => text(list(slots?.slots).find((one) => one.slot === slot)?.version);

/** "A" for "a": how the slots are named to a person. */
export const slotName = (slot) => String(slot).toUpperCase();

/**
 * The System card: `{state, pill: {kind, word}, version, title, lines, buttons,
 * time}`. `buttons` are ids the page draws (`stage`, `hold`, `restart`,
 * `unstage`, `release`, `happened`, `goback`). `slots` is what an
 * administrator reads (null for anyone else), `settings` is
 * `update settings --json`, `failed` is `changes --failed --json`, and
 * `running` is the version this system runs when no check says.
 */
export function systemCard({
  check, slots, settings, failed, running, time,
}) {
  const offer = offerOf(check, 'system');
  const version = offer?.from ?? running ?? null;
  const done = (state, title, lines = [], buttons = []) => ({
    state, pill: { kind: PILL[state][0], word: PILL[state][1] }, version, title, lines, buttons,
  });
  const staged = slots?.staged;
  if (staged) {
    const next = slotVersion(slots, staged.slot);
    const tries = Number.isFinite(staged.tries_left) ? ` and has ${staged.tries_left === 1 ? '1 try' : `${staged.tries_left} tries`} to prove itself` : '';
    return done('staged', `${next ?? 'The update'} is staged in slot ${slotName(staged.slot)}`, [`It starts at the next restart${tries}.`], ['restart', 'unstage']);
  }
  const why = whyUnchecked(check, 'system');
  if (why !== null) return done('unchecked', couldNot(why));
  if (!offer) return done('none', check ? 'Nothing is offered.' : 'Not checked yet.');
  const to = offer.to ?? 'The update';
  switch (offer.state) {
    case 'current': {
      const channel = text(settings?.channel);
      return done('current', 'Up to date', [channel ? `The ${channel} channel offers nothing newer.` : 'Nothing newer is offered.']);
    }
    case 'update': {
      const size = Number.isFinite(offer.download?.bytes) ? ` · ${megabytes(offer.download.bytes)} to download` : '';
      const fixes = check.advisories.filter((flaw) => flaw.fix === 'offered').map((flaw) => `Fixes ${flawWords(flaw)}`);
      return done('update', `${to} is ready${size}`, [...fixes, offer.bridge && `It goes through ${offer.bridge} first.`].filter(Boolean), ['stage', 'hold']);
    }
    case 'waiting':
      return done('waiting', `${to} reaches this machine on day ${offer.wave_day ?? '?'} of its waves`, [text(offer.waves_held) && `Its waves are paused: ${ended(offer.waves_held)}`].filter(Boolean), ['hold']);
    case 'held': {
      const fell = offer.held_because === 'fell-back';
      const held = Date.parse(offer.held_at ?? '');
      const cause = text(failed?.check) ? `: it failed its check (${failed.check})` : '';
      const lines = fell
        ? [`It was taken back${Number.isFinite(held) ? ` ${atWords(held, time)}` : ''}${cause}. It is not staged again until you release it.`]
        : ['It is held until you release it. A held version gets no security fixes.'];
      return done('held', `${to} is held`, lines, [fell && 'happened', 'release'].filter(Boolean));
    }
    case 'withdrawn':
      return done('withdrawn', `${to} was withdrawn${text(offer.withdrawn) ? `: ${ended(offer.withdrawn)}` : '.'}`);
    default:
      return done('none', 'Nothing is offered.');
  }
}

/** The lines about the release this system runs when the release keys withdrew it, with the button its advice has. */
export function withdrawnWords(check) {
  const one = check?.withdrawn;
  if (!one) return null;
  const advice = one.advice === 'rollback' ? 'Go back to the version before.' : 'Stay on it until the next release.';
  return { lines: [`The release this system runs, ${one.version}, was withdrawn: ${ended(one.reason ?? 'no reason was given')} ${advice}`], buttons: one.advice === 'rollback' ? ['goback'] : [] };
}

/** "Kept for going back: 1.0.0-alpha.0 in slot A", or null when no version is kept. */
export function keptWords(slots) {
  const version = slotVersion(slots, slots?.previous);
  return version ? { version, text: `Kept for going back: ${version} in slot ${slotName(slots.previous)}` } : null;
}

// ---- the Firmware card ------------------------------------------------------

/** What the machine's own firmware is, and is not, Dukovo's to update. */
export const MAKER_WORDS = 'The machine\'s own firmware is its maker\'s to update; Dukovo updates the firmware set its drivers load.';

/** The Firmware card: `{state, pill, title, buttons, reason}`; `reason` says why Stage it is off. */
export function firmwareCard({ check, slots, running }) {
  const offer = offerOf(check, 'firmware');
  const set = offer?.from ?? running ?? null;
  const done = (state, title, extra = {}) => ({
    state, pill: { kind: PILL[state][0], word: PILL[state][1] }, title, buttons: [], reason: null, ...extra,
  });
  const why = whyUnchecked(check, 'firmware');
  if (why !== null) return done('unchecked', couldNot(why));
  if (!offer) return done('none', set ? `Firmware set ${set}` : 'Not checked yet.');
  const was = set ? `Firmware set ${set}` : 'No firmware set';
  switch (offer.state) {
    case 'update': {
      const blocked = slots?.staged ? 'After the system update has proved itself' : null;
      return done('update', `${was} · ${offer.to} is offered`, { buttons: ['stage'], reason: blocked });
    }
    case 'held':
      return done('held', `${was} · ${offer.to} is held`, { buttons: ['release'] });
    default:
      return done('current', `${was} · up to date`);
  }
}

// ---- the Apps card ----------------------------------------------------------

/** "Fixes 1 security advisory, high" for the flaws of `item`'s installed version that the offered version fixes, or null. */
export function fixesFor(item, to, flaws) {
  const found = list(flaws).filter((flaw) => flaw.item === item && flaw.fixed_by_update === to);
  if (found.length === 0) return null;
  const worst = worstOf(found);
  return `Fixes ${found.length === 1 ? '1 security advisory' : `${found.length} security advisories`}${worst ? `, ${worst}` : ''}`;
}

/**
 * The rows of the Apps card: each app the check offers an update for, joined
 * with `apps` (`bundle list --json`'s `bundles`) for its name and with
 * `flaws` (`update advisories --json`'s `apps`) for what the update fixes.
 */
export function appRows(check, apps, flaws) {
  const bundles = list(apps).filter((one) => one.for_everyone !== false);
  return appOffers(check).map((offer) => {
    const name = bundles.find((one) => one.id === offer.item)?.name ?? offer.item;
    const pair = versionPair(offer.from, offer.to);
    const kind = kindOfUpdate(offer.from, offer.to);
    const state = offer.state === 'held' ? 'held' : offer.state === 'waiting' ? 'waiting' : 'update';
    const lines = state === 'held' ? [`Held at ${offer.from}`] : state === 'waiting' ? ['Reaches this machine soon'] : [fixesFor(offer.item, offer.to, flaws)].filter(Boolean);
    return {
      id: offer.item, name, pair, kind, state, lines,
    };
  });
}

/** The Apps card's pill and title: "3 waiting", "Up to date", or why it could not be checked. */
export function appsHead(check, rows) {
  const why = whyUnchecked(check, 'apps');
  if (why !== null) return { kind: 'warn', word: 'Could not check', lines: [couldNot(why)] };
  if (!check) return { kind: '', word: 'Not checked yet', lines: [] };
  const waiting = rows.filter((row) => row.state === 'update').length;
  return waiting > 0 ? { kind: 'info', word: `${waiting} waiting`, lines: [] } : { kind: 'ok', word: 'Up to date', lines: [] };
}

// ---- the Security card ------------------------------------------------------

/** The Security card: `{title, rows}`; each row is an advisory of the system. */
export function securityCard(check) {
  if (!check) return { title: 'Not checked yet.', rows: [] };
  const why = whyUnchecked(check, 'security');
  if (why !== null) return { title: couldNot(why), rows: [] };
  const rows = check.advisories.map((flaw) => ({ id: flaw.id, severity: flaw.severity, line: flawWords(flaw), fix: fixWords(flaw) }));
  const flaws = check.appFlaws > 0 ? `${counted(check.appFlaws, 'flaw')} in apps` : null;
  if (rows.length === 0 && !flaws) return { title: 'No advisory applies to this system.', rows };
  return { title: [rows.length > 0 && `${rows.length === 1 ? '1 advisory applies' : `${rows.length} advisories apply`} to this system`, flaws].filter(Boolean).join(' · '), rows };
}

// ---- holds ------------------------------------------------------------------

/**
 * What is held: the system and the firmware set by `settings.hold` (a version
 * each, or null), each app of `apps` with `held`. Each is `{kind, item, title,
 * line}`; `release` runs `updates.unhold` for the system and the firmware, and
 * `apps.unhold` for an app.
 */
export function heldRows(settings, apps) {
  const rows = [];
  const held = settings?.held ?? null;
  if (text(settings?.hold?.system)) {
    const fell = held?.version === settings.hold.system && held.why === 'fell-back';
    rows.push({ kind: 'system', item: 'system', title: `System ${settings.hold.system}`, line: fell ? 'It was taken back, so it is not staged again until you release it.' : 'It is not staged until you release it.' });
  }
  if (text(settings?.hold?.firmware)) rows.push({ kind: 'firmware', item: 'firmware', title: `Firmware set ${settings.hold.firmware}`, line: 'It is not staged until you release it.' });
  for (const app of list(apps).filter((one) => one.held && one.for_everyone !== false)) {
    rows.push({ kind: 'app', item: app.id, title: `${app.name ?? app.id} ${app.version}`, line: `Held at ${app.version}.` });
  }
  return rows;
}

/** What a person may hold: the system's offered release, the firmware set, each app not held, as `{item, kind, label, action, args}`. */
export function holdChoices(check, settings, apps) {
  const choices = [];
  const system = offerOf(check, 'system');
  if (system && ['update', 'waiting'].includes(system.state) && !text(settings?.hold?.system)) choices.push({ item: 'system', kind: 'system', label: `The system, ${system.to}` });
  const firmware = offerOf(check, 'firmware');
  if (firmware?.state === 'update' && !text(settings?.hold?.firmware)) choices.push({ item: 'firmware', kind: 'firmware', label: `The firmware set, ${firmware.to}` });
  for (const app of list(apps).filter((one) => one.for_everyone !== false && !one.held)) choices.push({ item: app.id, kind: 'app', label: `${app.name ?? app.id}, held at ${app.version}` });
  return choices;
}

// ---- what the navigation and the Overview count -----------------------------

/** How many parts have an update ready: the system, the firmware set and each app. */
export const readyCount = (check) => (check ? check.offers.filter((offer) => offer.state === 'update').length : 0);

/** The days a check may be old before the Overview says so. */
export const STALE_DAYS = 7;
