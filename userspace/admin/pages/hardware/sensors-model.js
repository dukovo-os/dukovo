// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Temperatures and fans card says, as plain values: a row for each
// temperature the kernel reads, each disk diskhealth reads and each thermal
// zone, with what each is now, the highest of 30 days and its limit, and a
// line for each fan. The answers are `hardware sensors` and `metrics show
// temp --highest`, as they print; a member that is not there is left out.

const MINUTE = 60;
const DAY = 24 * 60 * MINUTE;

/** How close to a limit, in degrees Celsius, a sensor is called near it. */
const NEAR = 5;
/** How long a disk may go unread before it is said so; diskhealth looks every ten minutes. */
const STALE = 20 * MINUTE;
/** How far from the highest a start or an update may be, to be called its cause. */
const DURING = 30 * MINUTE;
/** The minute files reach back this far; before it the highest is the hour's. */
const MINUTES_KEPT = 14 * DAY;

export const UNIT_KEY = 'hardware.temps.unit';
export const UNITS = Object.freeze(['C', 'F']);

/** The unit to use for what was kept: Celsius unless it says Fahrenheit. */
export const unitOf = (kept) => (kept === 'F' ? 'F' : 'C');

/** "41 °C" or "106 °F": whole degrees, from Celsius. */
export const degrees = (celsius, unit) => `${Math.round(unit === 'F' ? (celsius * 9) / 5 + 32 : celsius)} °${unit}`;

const list = (value) => (Array.isArray(value) ? value : []);
const number = (value) => (Number.isFinite(value) ? value : null);
// An alarm or a fault is true, or 1, when it is set.
const flag = (value) => value > 0;

const KINDS = Object.freeze({ hdd: 'for a hard disk', ssd: 'for a solid-state disk', nvme: 'for an NVMe drive' });

/** The tone and words of a temperature `celsius` against the one limit `limit` of a sensor, or null for neither. */
function nearLimit(celsius, limit) {
  if (limit === null) return null;
  if (celsius >= limit) return { tone: 'bad', words: 'at its limit' };
  return limit - celsius <= NEAR ? { tone: 'warn', words: 'near its limit' } : null;
}

/** How a BMC sensor's status reads: the tone and the words, or null for `ok`. */
export const BMC_STATUS = Object.freeze({
  warning: { tone: 'warn', words: 'warning' },
  critical: { tone: 'bad', words: 'critical' },
  'non-recoverable': { tone: 'bad', words: 'non-recoverable' },
  unavailable: { tone: 'off', words: 'unavailable' },
  'not-reachable': { tone: 'off', words: 'not reachable' },
});

/** The same against a disk's warning and critical limits. */
function overLimit(celsius, warning, critical) {
  if (critical !== null && celsius >= critical) return { tone: 'bad', words: 'over its critical limit' };
  return warning !== null && celsius >= warning ? { tone: 'warn', words: 'over its warning limit' } : null;
}

/** "at 4:10 AM" for a time today, and the day and clock for any other, to follow "read". */
const at = (seconds, time) => {
  const words = time.when(seconds);
  return /^\d/.test(words) ? `at ${words}` : words.replace(/^Yesterday /, 'yesterday ');
};

/**
 * The cells of the rows of `answer` (`hardware sensors`): `[{key, name, small,
 * now, highest, limit}]` where each of the last three is `{text, note, tone,
 * words, muted}`. `highest` is the answer of `metrics show temp --highest`,
 * `starts` the times of the system's changes in seconds, `unit` is `C` or
 * `F`, `time` is lib/time.js's and `now` is Unix seconds.
 */
export function tempRows(answer, highest, {
  unit, time, now, starts = [],
}) {
  const peaks = new Map(list(highest).map((one) => [one.series, one.highest]));
  const deg = (celsius) => degrees(celsius, unit);

  /** The highest of a series: "63 °C", and when it was, and whether an update was on. */
  function peakOf(series) {
    const point = peaks.get(series);
    if (!Array.isArray(point) || !Number.isFinite(point[0]) || !Number.isFinite(point[1])) return { text: '—', muted: true };
    const hour = now - point[0] > MINUTES_KEPT;
    const during = starts.some((start) => Math.abs(start - point[0]) <= DURING);
    return {
      text: deg(point[1]),
      note: [hour ? `in the hour from ${time.when(point[0])}` : time.when(point[0]), during && 'during the update'].filter(Boolean).join(', '),
    };
  }

  /** A sensor of the kernel's, or a thermal zone: "now" against its one limit. */
  function sensorRow(sensor, name, small) {
    const value = number(sensor.value);
    const crit = number(sensor.crit);
    const limit = crit ?? number(sensor.high);
    const heat = value === null ? null : nearLimit(value, limit);
    return {
      key: sensor.series ?? `${sensor.chip}/${sensor.label}`,
      name,
      small,
      now: value === null ? { text: '—', muted: true } : { text: deg(value), ...heat },
      highest: peakOf(sensor.series),
      limit: limit === null ? { text: '—', muted: true } : { text: deg(limit), note: crit === null ? 'high' : 'critical' },
    };
  }

  function diskRow(disk) {
    const warning = number(disk.warning_c);
    const critical = number(disk.critical_c);
    const celsius = number(disk.temperature_c);
    const checked = number(disk.checked_ms);
    let current;
    if (number(disk.asleep_ms) !== null) {
      current = { text: `Asleep since ${time.when(disk.asleep_ms / 1000)}: not read, so as not to wake it`, muted: true };
    } else if (checked !== null && now - checked / 1000 >= STALE) {
      current = { tone: 'warn', words: `Last read ${at(checked / 1000, time)}` };
    } else if (celsius === null) {
      current = { text: 'The disk gives no temperature', muted: true };
    } else {
      current = { text: deg(celsius), note: checked === null ? null : `read ${at(checked / 1000, time)}`, ...overLimit(celsius, warning, critical) };
    }
    const kind = disk.limits_from === 'disk' ? 'the disk\'s own' : KINDS[disk.kind];
    return {
      key: disk.series ?? `disk/${disk.disk}`,
      name: `Disk ${disk.disk}${disk.model ? ` · ${disk.model}` : ''}`,
      now: current,
      highest: peakOf(disk.series),
      limit: warning === null || critical === null ? { text: '—', muted: true } : { text: `${deg(warning)}, critical ${deg(critical)}`, note: kind },
    };
  }

  /** A temperature of the BMC, named as the BMC names it, with the status the BMC gives it. */
  function bmcRow(sensor) {
    const value = number(sensor.value);
    const state = BMC_STATUS[sensor.status] ?? null;
    return {
      key: sensor.series ?? `bmc/${sensor.name}`,
      name: sensor.name,
      small: 'BMC',
      now: value === null ? { text: '—', muted: true, ...state } : { text: deg(value), ...state },
      highest: { text: '—', muted: true },
      limit: { text: '—', muted: true },
    };
  }

  const PROCESSOR = /^(coretemp|k10temp|zenpower)/;
  const temps = list(answer?.sensors).filter((sensor) => sensor.kind === 'temp');
  const sensorsOf = (processor) => temps.filter((sensor) => PROCESSOR.test(sensor.chip ?? '') === processor)
    .map((sensor) => (processor ? sensorRow(sensor, `Processor (${sensor.label})`) : sensorRow(sensor, sensor.label, sensor.chip)));
  const bmcTemps = list(answer?.bmc?.temperatures).map(bmcRow);
  const zones = list(answer?.zones).map((zone) => sensorRow(zone, `Zone ${/^temp\{zone\/(.*)\}$/.exec(zone.series ?? '')?.[1] ?? ''}`.trim()));
  return {
    // The disks come after the processor's rows and before the thermal zones'.
    rows: [...sensorsOf(true), ...list(answer?.disks).map(diskRow), ...sensorsOf(false), ...bmcTemps, ...zones],
    // A machine that reads no temperature says so, and the disks' rows stand alone.
    none: temps.length === 0 && zones.length === 0 && bmcTemps.length === 0,
  };
}

const rpm = (value) => `${new Intl.NumberFormat('en-US').format(Math.round(value))} RPM`;

/** A fan's line, from the member of `hardware sensors`: `{key, name, words, tone}`; tone is `bad` for one that stopped and `off` for one that is not judged. */
function fanOf(fan) {
  const value = number(fan.value) ?? 0;
  const minimum = number(fan.min) > 0 ? fan.min : null;
  let words = rpm(value);
  let tone = 'ok';
  if (flag(fan.fault) || flag(fan.alarm) || (value === 0 && minimum !== null)) {
    words = 'stopped';
    tone = 'bad';
  } else if (value === 0) {
    // As `hardware check` judges: 0 RPM with no minimum is a header with no fan on it.
    words = 'not judged: no fan on this header';
    tone = 'off';
  } else if (minimum !== null && value < minimum) {
    words = `${rpm(value)}, under its minimum of ${rpm(minimum)}`;
    tone = 'bad';
  }
  return {
    key: fan.series ?? `${fan.chip}/${fan.label}`, name: fan.label, words, tone,
  };
}

/** A fan of the BMC: its speed, or what the BMC says is wrong with it. */
function bmcFanOf(fan) {
  const value = number(fan.value);
  const state = BMC_STATUS[fan.status] ?? null;
  let words = value === null ? 'no reading' : rpm(value);
  if (state !== null) words = value === null ? state.words : `${words}, ${state.words}`;
  return {
    key: fan.series ?? `bmc/${fan.name}`, name: `${fan.name} (BMC)`, words, tone: state?.tone ?? 'ok',
  };
}

/** The fans of `answer` as lines, in the order the kernel lists them, and then the BMC's. */
export const fanRows = (answer) => [
  ...list(answer?.sensors).filter((sensor) => sensor.kind === 'fan').map(fanOf),
  ...list(answer?.bmc?.fans).map(bmcFanOf),
];

export const NO_TEMPERATURE = 'The kernel reads no temperature on this machine.';
export const NO_FAN = 'No fan speed can be read on this machine.';

/** The line under the table; the disks' sentence comes only when the table has a disk. */
export const ALERT_WORDS = 'An alert goes out when a sensor stays within 5 °C of its critical limit for 5 minutes (metricsd).';
export const ALERT_WITH_DISKS = 'Disks are read every 10 minutes, but a sleeping one. An alert goes out when a sensor stays within 5 °C of its critical limit, or a disk at its warning limit, for 5 minutes (metricsd).';

/** The line about the disks that keep no health record, from `storage show`; null when every disk keeps one. */
export function noRecordWords(storage) {
  const count = list(storage?.disks).filter((disk) => !disk.health).length;
  if (count === 0) return null;
  return count === 1 ? '1 disk keeps no health record (USB or virtual) and is not read.' : `${count} disks keep no health record (USB or virtual) and are not read.`;
}

/** The times, in seconds, of the system's changes in `update history`: each is a start that changed it. */
export const startTimes = (history) => list(history?.changes).map((change) => Date.parse(change.time) / 1000).filter(Number.isFinite);
