// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Every time the console shows, in the person's zone and one set of forms:
// "3:06 AM" today, "Yesterday 7:40 PM", "Nov 11 7:40 AM" this year, "Nov 11,
// 2025" in another; durations as "2 min" and "3 h 5 min"; relative words
// ("4 minutes ago") only in lists. The server gives Unix seconds.

const LOCALE = 'en-US';

const SECOND = 1;
const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

/** The time zone this browser is set to, or UTC when it says none. */
export function browserZone() {
  return new Intl.DateTimeFormat(LOCALE).resolvedOptions().timeZone || 'UTC';
}

/** Whether `zone` is one that this browser knows. */
export function isZone(zone) {
  if (typeof zone !== 'string' || zone === '') return false;
  try {
    new Intl.DateTimeFormat(LOCALE, { timeZone: zone });
    return true;
  } catch {
    return false;
  }
}

/** The time zones this browser knows, for a person to choose from. */
export function zones() {
  const known = typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
  return known.includes('UTC') ? known : ['UTC', ...known];
}

/** The year, month, day and clock of the Unix time `ms` in `zone`. */
export function partsOf(zone, ms) {
  const formatter = new Intl.DateTimeFormat(LOCALE, {
    timeZone: zone, hourCycle: 'h23', year: 'numeric', month: 'numeric', day: 'numeric',
    hour: 'numeric', minute: 'numeric', second: 'numeric',
  });
  const found = {};
  for (const { type, value } of formatter.formatToParts(ms)) found[type] = Number(value);
  return found;
}

/** How far `zone`'s clock is ahead of UTC at the Unix time `ms`, in milliseconds. */
export function offsetOf(zone, ms) {
  const p = partsOf(zone, ms);
  return Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(ms / 1000) * 1000;
}

/** The days since 1970 that a date is, for counting from one date to another. */
const dayNumber = ({ year, month, day }) => Date.UTC(year, month - 1, day) / (DAY * 1000);

/** `text` with the narrow no-break space that newer ICU puts before AM and PM made a plain space, so that search and tests see one kind. */
const plain = (text) => text.replace(/[\u202f\u00a0]/g, ' ');

/** `count` and its noun, with an s when it is not one. */
const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** The length of `seconds`: "45 s", "2 min", "3 h 5 min", "2 d 3 h". */
export function duration(seconds) {
  const rounded = Math.max(0, Math.round(seconds));
  if (rounded < MINUTE) return `${rounded} s`;
  const minutes = Math.round(rounded / MINUTE);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(rounded / HOUR);
  if (hours < 24) {
    const rest = Math.round((rounded - hours * HOUR) / MINUTE);
    return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
  }
  const wholeDays = Math.floor(rounded / DAY);
  const left = Math.round((rounded - wholeDays * DAY) / HOUR);
  return left === 0 ? `${wholeDays} d` : `${wholeDays} d ${left} h`;
}

/**
 * The formatters for `zone`, the person's; an empty or unknown one is the
 * browser's own, which a first sign-in shows until the person chooses.
 * `clock` gives the time now in milliseconds; tests replace it.
 */
export function makeTime(zone, clock = () => Date.now()) {
  const chosen = isZone(zone) ? zone : browserZone();
  const format = (options) => new Intl.DateTimeFormat(LOCALE, { timeZone: chosen, ...options });
  const hour = format({ hour: 'numeric', minute: '2-digit', hour12: true });
  const day = format({ month: 'short', day: 'numeric' });
  const date = format({ month: 'short', day: 'numeric', year: 'numeric' });
  const whole = format({ dateStyle: 'medium', timeStyle: 'medium' });

  const days = (seconds) => dayNumber(partsOf(chosen, seconds * 1000)) - dayNumber(partsOf(chosen, clock()));

  const time = {
    zone: chosen,

    /** The clock of `seconds`: "3:06 AM". */
    clock: (seconds) => plain(hour.format(seconds * 1000)),

    /** The time of `seconds` as a list shows it: the clock today, then "Yesterday 7:40 PM", the day and clock this year, the date in another. */
    when(seconds) {
      const ms = seconds * 1000;
      const away = days(seconds);
      if (away === 0) return plain(hour.format(ms));
      if (away === -1) return plain(`Yesterday ${hour.format(ms)}`);
      if (partsOf(chosen, ms).year !== partsOf(chosen, clock()).year) return plain(date.format(ms));
      return plain(`${day.format(ms)} ${hour.format(ms)}`);
    },

    /** The whole time, to the second, for a `title` and an accessible name. */
    full: (seconds) => plain(whole.format(seconds * 1000)),

    /** The length of `seconds`, as `duration` gives it. */
    duration,

    /** How long before or after now `seconds` is: "just now", "4 minutes ago", "in 2 hours"; a week or more is `when`'s. */
    relative(seconds) {
      const apart = Math.floor(clock() / 1000) - seconds;
      const size = Math.abs(apart);
      const say = (count, noun) => (apart >= 0 ? `${counted(count, noun)} ago` : `in ${counted(count, noun)}`);
      if (size < MINUTE * SECOND) return 'just now';
      if (size < HOUR) return say(Math.floor(size / MINUTE), 'minute');
      if (size < DAY) return say(Math.floor(size / HOUR), 'hour');
      if (size < 7 * DAY) return say(Math.floor(size / DAY), 'day');
      return time.when(seconds);
    },

    /** The Unix time of the start of the day that holds `seconds` (now when left out). */
    midnight(seconds = Math.floor(clock() / 1000)) {
      const p = partsOf(chosen, seconds * 1000);
      const local = Date.UTC(p.year, p.month - 1, p.day);
      // The clocks may have changed since, so the offset is asked again at the guess.
      let start = local - offsetOf(chosen, seconds * 1000);
      start = local - offsetOf(chosen, start);
      return Math.round(start / 1000);
    },
  };
  return time;
}
