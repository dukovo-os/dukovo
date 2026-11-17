// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the memory card says of a test that is asked for and of the last one
// at a start, from `hardware memtest --status --json`: the asked line, and
// the start's line with its tone and its offer to report. Most machines
// never ran one, so the card reads this file when it draws, not with the
// page.

import { fill } from '../../dom.js';
import { dayWords } from '../../lib/snapshot-words.js';
import { upWords } from './model.js';

const GIB = 2 ** 30;
const NOT_READ = 'The memory test at the start could not be read.';

/** "Nov 16, 3:10 PM": the day and the clock, `ms` in Unix milliseconds and `now` in seconds. */
const dayAndClock = (ms, time, now) => `${dayWords(ms, time.zone, now * 1000)}, ${time.clock(ms / 1000)}`;

/** "The memory is tested at the next restart (asked Nov 16, 3:10 PM by admin)", from the `asked` of the answer. */
export function askedWords(asked, time, now) {
  const when = Number.isFinite(asked?.at_ms) ? dayAndClock(asked.at_ms, time, now) : '';
  const name = asked?.by_name;
  const by = typeof name === 'string' && name !== '' ? `by ${name}` : '';
  const said = [when, by].filter(Boolean).join(' ');
  return `The memory is tested at the next restart${said ? ` (asked ${said})` : ''}`;
}

/** "15.2 of 16 GiB": how much was tested, in the unit of the whole. */
function testedWords(tested, total) {
  const unit = total >= GIB ? GIB : 2 ** 20;
  const say = (bytes) => Number((bytes / unit).toFixed(1));
  return `${say(tested)} of ${say(total)} ${unit === GIB ? 'GiB' : 'MiB'}`;
}

const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/**
 * What the last test at a start found, from the `last` of the answer:
 * `{words, tone, report}`. `tone` is `bad` for errors, `warn` for a test that
 * did not finish and `off` for one the start menu did not take; `report` says
 * whether a report about the hardware is offered. Null when there is no
 * record, or one whose result this page does not know. `now` is in seconds.
 */
export function startRun(last, time, now) {
  if (!Number.isFinite(last?.at_ms)) return null;
  const day = dayWords(last.at_ms, time.zone, now * 1000);
  switch (last.result) {
    case 'passed': {
      const took = Number.isFinite(last.seconds) ? ` in ${upWords(last.seconds)}` : '';
      const tested = last.tested_bytes > 0 && last.total_bytes > 0 ? ` · ${testedWords(last.tested_bytes, last.total_bytes)} tested${took}` : '';
      return { words: `${day}, at the start: passed${tested}`, tone: null, report: false };
    }
    case 'errors': {
      const errors = Number.isFinite(last.errors) ? counted(last.errors, 'error') : 'errors';
      const places = Array.isArray(last.bad) && last.bad.length > 0 ? ` in ${counted(last.bad.length, 'place')}` : '';
      return {
        words: `${day}, at the start: ${errors}${places}. Those parts are out of use until the next restart. The memory should be replaced (HW-103).`,
        tone: 'bad',
        report: true,
      };
    }
    case 'unfinished':
      return { words: `${day}: the test at the start did not finish (HW-104). The power went or the machine stopped during it, which failing memory can cause.`, tone: 'warn', report: false };
    case 'not-taken':
      return { words: `${day}: the start menu did not run the test: an update was tried first. Ask again.`, tone: 'off', report: false };
    default:
      return null;
  }
}

/**
 * Draws the asked line and the start's line, with the offer to report, from
 * what `ctx` has read. `lines` are the card's `{asked, start, report}`.
 */
export function drawLines(ctx, { asked, start, report }) {
  const { memtest } = ctx.data;
  const now = Math.floor(Date.now() / 1000);
  const run = memtest === undefined ? { words: NOT_READ, tone: 'off', report: false } : startRun(memtest?.last, ctx.view.time, now);
  start.hidden = run === null;
  report.hidden = run?.report !== true;
  start.setAttribute('class', run?.tone === 'off' ? 'muted' : `hw-start ${run?.tone ?? ''}`.trim());
  fill(start, run?.words);
  asked.hidden = !memtest?.asked;
  fill(asked, memtest?.asked && askedWords(memtest.asked, ctx.view.time, now));
}
