// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware page gives the Overview: the checks of the last result
// that failed or warned, each with its ID, a restart or shut down that waits,
// and the row that says what the machine is and when it was last checked.
// Every read is the person's own. The part imports nothing, since the
// Overview reads it on every visit.

const read = (cx, path) => cx.cache(path, 5000, () => cx.api.get(path).catch(() => null));
const checksOf = (last) => (Array.isArray(last?.checks) ? last.checks : []);

/** "Restarts in 52 seconds" for a minute or less, "Shuts down at 3:00 AM" for longer; `shutdown` is `power.status`'s, null for none. */
function waitingWords(shutdown, time) {
  if (!shutdown) return null;
  const what = shutdown.end === 'reboot' ? 'Restarts' : 'Shuts down';
  const left = shutdown.in_seconds;
  if (typeof left === 'number' && left <= 0) return `${what} now`;
  if (typeof left === 'number' && left <= 60) return `${what} in ${Math.ceil(left)} ${Math.ceil(left) === 1 ? 'second' : 'seconds'}`;
  return Number.isFinite(shutdown.at_ms) ? `${what} at ${time.clock(shutdown.at_ms / 1000)}` : what;
}

/** The IDs of the checks of `last` that ended with `result`, each once. */
const idsOf = (last, result) => [...new Set(checksOf(last).filter((one) => one.result === result && one.id).map((one) => one.id))];

/** The things to look at from the last check `last` and the restart `shutdown` that waits, with `time` as lib/time.js's. */
export function needsOf(last, shutdown, time) {
  const found = [];
  for (const [result, severity] of [['failed', 'bad'], ['warning', 'warn']]) {
    const ids = idsOf(last, result);
    if (ids.length > 0) {
      const advice = checksOf(last).find((one) => one.result === result && one.advice)?.advice;
      found.push({
        id: `hardware-${result}`, severity, title: `Hardware check found ${ids.join(', ')}`, detail: advice ?? 'See the Last check card.', href: '/hardware#check', icon: 'hardware', at: last.time,
      });
    }
  }
  const waiting = waitingWords(shutdown, time);
  if (waiting) {
    found.push({
      id: 'hardware-power', severity: 'info', title: waiting, detail: 'Cancel it from Hardware.', href: '/hardware#power', icon: 'power', at: null,
    });
  }
  return found;
}

/** The things to look at, for the Overview. */
export async function needs(cx) {
  const [last, power] = await Promise.all([read(cx, '/api/1/hardware/check'), read(cx, '/api/1/power')]);
  return needsOf(last, power?.shutdown ?? null, cx.time);
}

/** The row of the server card: the machine and when its hardware was last checked. */
export async function rows(cx) {
  const [machine, last] = await Promise.all([read(cx, '/api/1/hardware'), read(cx, '/api/1/hardware/check')]);
  const name = [machine?.maker, machine?.model].filter((word) => typeof word === 'string' && word.trim() !== '').join(' ');
  if (name === '') return [];
  const checked = Number.isFinite(last?.time) ? `checked ${cx.time.relative(last.time)}` : 'not checked yet';
  return [{ label: 'Machine', value: `${name} · ${checked}`, order: 35 }];
}
