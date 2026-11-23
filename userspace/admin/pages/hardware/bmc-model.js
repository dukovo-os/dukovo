// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the BMC card says, as plain values: who the BMC is, its power,
// current and voltage sensors with the ones that are not well first and the
// rest to open, and the last entries of its event log. The answers are
// `hardware show` as an administrator gets it (its `bmc` member),
// `hardware sensors` as an administrator gets it (its `bmc` group) and
// `bmc events`, as they print. The BMC's temperatures and fans are not
// here: the Temperatures and fans card has them (sensors-model.js).

import { BMC_STATUS } from './sensors-model.js';

/** How many entries of the log the card shows. */
export const EVENTS_SHOWN = 10;

/** The kinds this card lists, by their member of the group, in the order shown. */
const KINDS = Object.freeze([['power', 'Power'], ['currents', 'Currents'], ['voltages', 'Voltages']]);

const list = (value) => (Array.isArray(value) ? value : []);

const numberWords = (value) => new Intl.NumberFormat('en-US', { maximumFractionDigits: 3 }).format(value);

/** The card's first lines, `[label, words]`, from the `bmc` member of `hardware show`; empty when the machine has none. */
export function bmcRows(bmc) {
  if (!bmc) return [];
  return [
    ['Maker', bmc.maker],
    ['Firmware', bmc.firmware],
    ['IPMI', bmc.ipmi_version],
    ['LAN address', bmc.lan_address ?? 'none set'],
  ].filter(([, words]) => words);
}

/** One sensor as a line: `{name, words, tone, well}`; `tone` is null for one that is well. */
function itemOf(sensor) {
  const state = BMC_STATUS[sensor.status] ?? null;
  const value = Number.isFinite(sensor.value) ? `${numberWords(sensor.value)} ${sensor.unit ?? ''}`.trim() : null;
  let words = value ?? 'no reading';
  if (state !== null) words = value === null ? state.words : `${value}, ${state.words}`;
  return {
    name: sensor.name, words, tone: state?.tone ?? null, well: state === null,
  };
}

/**
 * The BMC's power, currents and voltages from `answer` (`hardware sensors`
 * as an administrator gets it): `{total, attention, groups}`. `attention` is
 * every sensor that is not well, whatever its kind, each with the kind's
 * name in `kind`; `groups` is every sensor by kind, `{id, title, items}`,
 * in the order of the BMC, for the list that opens. A kind with none is
 * left out.
 */
export function sensorGroups(answer) {
  const groups = KINDS
    .map(([id, title]) => ({ id, title, items: list(answer?.bmc?.[id]).map(itemOf) }))
    .filter((group) => group.items.length > 0);
  const attention = groups.flatMap((group) => group.items.filter((item) => !item.well).map((item) => ({ ...item, kind: group.title })));
  return { total: groups.reduce((sum, group) => sum + group.items.length, 0), attention, groups };
}

/** The words that name how many sensors there are and open the list. */
export const showAllWords = (total) => `Show all ${total} sensors`;

const clock = (seconds) => {
  const pad = (value) => String(value).padStart(2, '0');
  return `${Math.floor(seconds / 3600)}:${pad(Math.floor((seconds % 3600) / 60))}:${pad(seconds % 60)}`;
};

const EVENT_TONES = Object.freeze({ warning: 'warn', critical: 'bad', 'non-recoverable': 'bad' });

/**
 * The last entries of the log in `answer` (`bmc events`), newest first, as
 * `{key, when, words, tone}`. `time` is lib/time.js's. An entry whose
 * clock was not set says how long after the BMC started it came.
 */
export function eventRows(answer, time, shown = EVENTS_SHOWN) {
  return list(answer?.events).slice(-shown).reverse().map((entry) => {
    let when = '—';
    if (entry.time) when = time.when(Date.parse(entry.time) / 1000);
    else if (Number.isFinite(entry.since_start)) when = `start +${clock(entry.since_start)}`;
    const source = entry.sensor ?? (entry.sensor_type ? `${entry.sensor_type} #${entry.sensor_number}` : null);
    const what = [entry.event, entry.direction].filter(Boolean).join(', ');
    return {
      key: entry.id, when, words: source ? `${source}: ${what}` : what, tone: EVENT_TONES[entry.severity] ?? null,
    };
  });
}

export const NO_EVENTS = 'The BMC\'s event log is empty.';
export const NO_SENSORS = 'The BMC lists no power, current or voltage sensor.';
export const IDENTIFY_WORDS = 'Lights the identify light for 15 seconds, to find this machine in a rack.';
