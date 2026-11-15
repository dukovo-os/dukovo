// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Firewall page says of what the machine reports (design 10.3), as
// plain values: the pill of the head from the `firewall` service, the rules of
// `firewall list --json` as rows, the lines of the file that are no rule, what
// was dropped (`firewall dropped --json`), what listens (`firewall status
// --json`) and the answer of `firewall test --json` in words.

import { parseAddress } from '../../lib/address-model.js';
import { isCardName, ruleOf, ruleText, wordsOf } from './rules-model.js';

/** The pill of the page's head, from the `firewall` service of Spark's document: `{words, kind, explain}`, or null when Spark lists none. */
export function headPill(show) {
  const service = (Array.isArray(show?.services) ? show.services : []).find((one) => one?.name === 'firewall');
  if (!service) return null;
  if (service.state === 'done' || service.state === 'running') return { words: 'On', kind: 'ok', explain: null };
  if (service.state === 'failed') {
    const why = typeof service.why === 'string' && service.why !== '' ? service.why : service.last_end;
    return { words: 'The rules did not load', kind: 'bad', explain: typeof why === 'string' && why !== '' ? why : null };
  }
  return { words: 'Off', kind: 'off', explain: null };
}

/** The rows of the rules table, from `firewall list --json`: `{number, words, text, program}`. */
export const ruleRows = (listed) => (Array.isArray(listed?.rules) ? listed.rules : []).map((entry) => ({
  number: entry.number, words: wordsOf(ruleOf(entry)), text: entry.text, program: entry.program ?? null,
}));

/** The lines of the file that are no rule, in words: "Line 7 is not a rule: why". */
export const problemWords = (listed) => (Array.isArray(listed?.problems) ? listed.problems : []).map((problem) => ({
  line: problem.line, text: problem.text, words: `Line ${problem.line} is not a rule: ${problem.why}`,
}));

/** The seconds left of the trial in `listed` (`firewall list --json`), or null while none runs. */
export const trialLeft = (listed) => (Number.isFinite(listed?.trial?.left_ms) ? listed.trial.left_ms : null);

// ---- what was dropped --------------------------------------------------------------

/** What the kernel says after the card when a drop is a brute-force block (thread A's PR 5), which belongs on Security. */
export const BLOCKED_WHY = 'the address is blocked';

/** The first letter of `words` in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** What the kernel's `why` of a drop means, in words. */
export function whyWords(why) {
  if (why === null || why === undefined || why === '') return 'No rule lets it in, and no service listens there with the right to be reached.';
  const process = /^process (\d+) listens for this machine alone$/.exec(why);
  if (process) return `A program (process ${process[1]}) listens there for this machine alone, and no rule lets others reach it.`;
  return capital(why.endsWith('.') ? why : `${why}.`);
}

/** The rule that lets what a group tried in, as the add form takes it: `allow tcp 22 from 203.0.113.9`. */
export function allowTextOf(group) {
  const from = parseAddress(group.from);
  const source = from === null ? group.from : `from ${group.from}`;
  if (group.protocol === 'ping') return `allow ping ${source}`;
  return `allow ${group.protocol} ${group.port} ${source}`;
}

/** What a group tried, in words: "203.0.113.9 tried port 22 on net0 at least 14 times, last 4 minutes ago". `time` is lib/time.js's. */
export function dropWords(group, time) {
  const times = `at least ${group.count} ${group.count === 1 ? 'time' : 'times'}`;
  const what = group.protocol === 'ping' ? `pinged ${group.card}` : `tried ${group.protocol === 'udp' ? 'UDP ' : ''}port ${group.port} on ${group.card}`;
  return `${group.from} ${what} ${times}, last ${time.relative(Math.floor(group.last_ms / 1000))}`;
}

/**
 * The groups of `firewall dropped --json`, as `{groups, blocked}`, each
 * `{key, words, full, why, allow}` (`full` is the time of the last drop, to
 * the second): the ones whose reason is a block for guessing passwords are
 * apart in `blocked` (`allow` is null there, since a block is not undone by a
 * rule), the rest are in the order the command gave, the most counted first.
 */
export function dropRows(answer, time) {
  const rows = { groups: [], blocked: [] };
  for (const group of Array.isArray(answer?.groups) ? answer.groups : []) {
    const key = [group.from, group.protocol, group.port, group.card, group.why].join('|');
    const blocked = group.why === BLOCKED_WHY;
    const row = {
      key,
      words: dropWords(group, time),
      full: time.full(Math.floor(group.last_ms / 1000)),
      why: blocked ? '' : whyWords(group.why),
      allow: blocked ? null : allowTextOf(group),
    };
    (blocked ? rows.blocked : rows.groups).push(row);
  }
  return rows;
}

// ---- what listens ------------------------------------------------------------------

/** Who reaches a listener from other machines, in words, by `reach` and the number of the rule. */
export function reachWords(reach, rule) {
  const by = rule === null || rule === undefined ? '' : ` by rule ${rule}`;
  switch (reach) {
    case 'everyone': return `Every machine reaches it${by}.`;
    case 'some': return `Some machines or cards reach it${by}.`;
    case 'service': return 'Its service has the right to be reached, so every machine does.';
    case 'denied': return `No other machine reaches it${by}.`;
    default: return 'This machine only.';
  }
}

/** The rows of What listens, from `firewall status --json`: `{key, port, program, words}`. */
export const listenerRows = (status) => (Array.isArray(status?.listening) ? status.listening : []).map((one) => ({
  key: `${one.tcp ? 'tcp' : 'udp'}${one.port}-${one.process}`,
  port: `${one.port}/${one.tcp ? 'TCP' : 'UDP'}`,
  program: one.program ?? 'a program that is not found',
  words: reachWords(one.reach, one.rule),
}));

// ---- the test form -----------------------------------------------------------------

/** The form of a test: TCP, no port yet, no address, no card. */
export const blankTest = () => ({
  protocol: 'tcp', port: '', from: '', on: '',
});

/**
 * The arguments of `firewall.test` for `form`, as `{args}`, or `{field, why}`
 * for the first value that cannot work. `from` is an address, as the command
 * asks; a network is not one.
 */
export function checkTest(form) {
  const port = form.port.trim();
  if (!/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65_535) {
    return { field: 'port', why: port === '' ? 'Write a port, 1 to 65535.' : `\`${port}\` is not a port, 1 to 65535.` };
  }
  const from = form.from.trim();
  if (from === '') return { field: 'from', why: 'Write the address the connection comes from, such as 192.168.1.20.' };
  if (parseAddress(from) === null) return { field: 'from', why: `\`${from}\` is not an address` };
  const on = form.on.trim();
  if (on !== '' && !isCardName(on)) return { field: 'on', why: `\`${on}\` is not an interface's name` };
  return { args: { protocol: form.protocol, port: Number(port), from, ...(on === '' ? {} : { on }) } };
}

/** The answer of `firewall test --json` in words: what decides, and the rules passed over because they name a program. `rules` is the page's list, for the passed rules' words. */
export function testWords(answer, port, rules) {
  const lines = [];
  if (answer?.rule === null || answer?.rule === undefined) {
    lines.push(`No rule decides: it reaches port ${port} only while a service listens there with the right to be reached.`);
  } else {
    lines.push(`Rule ${answer.rule} decides: ${answer.allowed ? 'allowed' : 'denied'}. ${answer.text ?? ''}`.trim());
  }
  const passed = Array.isArray(answer?.passed) ? answer.passed : [];
  if (passed.length > 0) {
    const named = passed.map((number) => `rule ${number}${rules[number - 1] ? ` (${ruleText(rules[number - 1])})` : ''}`);
    lines.push(`Passed over, since they name what a test does not know: ${named.join(', ')}.`);
  }
  return lines;
}
