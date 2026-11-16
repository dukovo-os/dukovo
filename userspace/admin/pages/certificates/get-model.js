// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the form that gets a certificate from Let's Encrypt says and sends,
// as plain values: the names typed, what the checks before an order found
// (`cert get --check --json`) in the words a person reads and the button
// each offers, the arguments of the order, the TXT records an order by DNS
// asks for, and how a task that ran ended.

import { endWords as taskEnd } from '../../lib/tasks-words.js';

/** The most names one certificate has. */
export const NAMES_MOST = 20;

/** The most characters of a certificate's name. */
export const NAME_MOST = 64;

/** How long the open port 80 is tried for, in seconds: it must outlast an order. */
export const PORT_TRY_SECONDS = 600;

/** The proof by port 80 and the proof by DNS, as the form's two choices. */
export const PROOFS = Object.freeze([
  { id: 'http', label: 'Through port 80', hint: 'Let\'s Encrypt connects to this server on port 80.' },
  { id: 'dns', label: 'Through DNS, for private services', hint: 'You add a TXT record for each name. A wildcard needs this.' },
]);

/** What the form says about the contact email. */
export const EMAIL_HINT = 'Let\'s Encrypt keeps it; it no longer mails notices of expiry, and this server warns you itself. Left empty, the account has none.';

/** What the checks say before an order by DNS. */
export const DNS_LINE = 'You add a TXT record for each name at your DNS provider; this server waits up to 10 minutes for them to show.';

/** What a person is told of a certificate proved by DNS. */
export const DNS_RENEWAL = 'certd does not renew a certificate proved by DNS. It warns you 21 days and again 7 days before it runs out; get it again before then.';

/** What the check of port 80 cannot tell, and the button that asks. */
export const TRY_WORDS = 'Whether port 80 reaches this server from outside can only be told by asking. Try it first asks Let\'s Encrypt\'s staging authority, installs nothing and does not count against Let\'s Encrypt\'s limits.';

/** What the staging test needs when the names are proved by DNS. */
export const TRY_DNS_WORDS = 'By DNS the test has records of its own to add: the staging authority asks for other values than the real one.';

/** The names typed in `text`: split at spaces, commas and line ends, in small letters, each once. */
export function namesOf(text) {
  const names = String(text ?? '').split(/[\s,]+/).map((name) => name.trim().toLowerCase()).filter(Boolean);
  return [...new Set(names)];
}

/** What is wrong with the name `name`, as `cert get` takes it, or ''. `dns` says whether a wildcard may be asked for. */
export function nameProblem(name, dns) {
  const wildcard = name.startsWith('*.');
  const host = wildcard ? name.slice(2) : name;
  if (wildcard && !dns) return `${name} is a wildcard, which Let's Encrypt proves only through DNS: choose that proof.`;
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(host) || host.includes(':')) return `${name} is an IP address: Let's Encrypt issues to names here.`;
  const labels = host.split('.');
  const fits = (label) => /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(label);
  if (host.length > 253 || labels.length < 2 || !labels.every(fits)) {
    return `${name} is not a name Let's Encrypt issues for: it has lowercase letters, digits and - in words of up to 63 characters, with a dot between them. Write a name that is not ASCII in punycode, as xn--…`;
  }
  return '';
}

/** What is wrong with the names `names`, or ''. */
export function namesProblem(names, dns) {
  if (names.length === 0) return 'Give the name of the site, such as shop.example.org.';
  if (names.length > NAMES_MOST) return `A certificate has 1 to ${NAMES_MOST} names, not ${names.length}.`;
  for (const name of names) {
    const problem = nameProblem(name, dns);
    if (problem) return problem;
  }
  return '';
}

/** The name a certificate for `names` has: the first, without a wildcard's `*.`; null when it is too long. */
export function certName(names) {
  const first = (names[0] ?? '').replace(/^\*\./, '');
  return first === '' || first.length > NAME_MOST ? null : first;
}

/** The address `certificate.check` is read from, for `names` proved through DNS or port 80. */
export function checkPath(names, dns) {
  const as = certName(names);
  const query = new URLSearchParams();
  if (as !== null && as !== names[0]) query.set('as', as);
  if (dns) query.set('dns', 'true');
  const text = query.toString();
  return `/api/1/certificates/check/${encodeURIComponent(names.join(','))}${text ? `?${text}` : ''}`;
}

/** "a", "a and b", "a, b and c". */
export const listWords = (items) => (items.length < 2 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`);

/** `words` with a capital where a sentence starts with a plain word; a name or a program's keeps its letters. */
const sentence = (words) => (/^(?:port|the|a) /.test(words) ? words[0].toUpperCase() + words.slice(1) : words);

/** What the command says, in the page's words: "the authority" is Let's Encrypt, "machine" is "server". */
const plainly = (words) => sentence(String(words ?? '').replace(/\bthe authority\b/g, 'Let\'s Encrypt').replace(/\bmachine\b/g, 'server'));

/** The tones a row has, which the page draws as a mark. */
const TONES = Object.freeze({
  passed: 'ok', warning: 'warn', failed: 'bad', skipped: 'skip',
});

/**
 * One check in the page's words: `{key, tone, words, id, name, button}`.
 * `button` is `open-port` for the firewall that closes port 80, which the
 * page opens for a while. `report` is the whole answer, for the firewall
 * rule's number and the program that holds port 80.
 */
export function checkRow(row, report, at) {
  const tone = TONES[row.result] ?? 'warn';
  const name = row.name ?? null;
  const base = {
    key: `${at}-${row.id ?? row.result}-${name ?? ''}`, tone, id: row.id ?? null, name, button: null,
  };
  const found = /leads to ([^\s,]+)/.exec(row.words ?? '');
  switch (row.id) {
    case 'CRT-101':
      return { ...base, words: `${name} does not resolve to an address: give it an A record (and AAAA for IPv6) that leads to this server` };
    case 'CRT-102':
      return { ...base, words: `${name} leads to ${found?.[1] ?? 'a private address'}, a private address Let's Encrypt cannot reach: give it this server's public address, or prove it by DNS` };
    case 'CRT-103':
      return { ...base, words: 'The firewall closes port 80, which Let\'s Encrypt must reach', button: 'open-port' };
    case 'CRT-104': {
      const own = /\(it has ([^)]+)\)/.exec(row.words ?? '');
      if (own) {
        const first = /points to ([^\s,]+),/.exec(row.words ?? '');
        return { ...base, words: `${name}'s AAAA record points to ${first?.[1] ?? 'another address'}, which is not this server (it has ${own[1]}). Let's Encrypt tries IPv6 first, so it would check that machine. Remove the AAAA record or point it to ${own[1].split(', ')[0]}` };
      }
      const first = /if (\S+) is not this machine/.exec(row.words ?? '');
      return { ...base, words: `${name} has an IPv6 address${first ? `, ${first[1]}` : ''}, and this server has none: if it is not this server, Let's Encrypt, which tries IPv6 first, may not reach it` };
    }
    case 'CRT-105': {
      const day = /\(([^)]+)\)/.exec(row.words ?? '');
      return { ...base, words: `The clock is wrong${day ? ` (${day[1]})` : ''}: it must be right first` };
    }
    case 'CRT-106':
      return { ...base, words: `Port 80 is held by ${report?.port80_holder ?? 'a program'}, which is not nginx or certd: stop it for the few minutes the order takes, or prove the names by DNS` };
    default:
      break;
  }
  if (/^port 80 is open/.test(row.words ?? '')) {
    const rule = report?.firewall_rule;
    return { ...base, words: `Port 80 is open (${Number.isInteger(rule) ? `firewall rule ${rule}` : 'no firewall rule closes it'})` };
  }
  return { ...base, words: plainly(row.words) };
}

/**
 * The checks of `report` (`certificate.check`'s answer) as rows to draw, for
 * names proved through port 80 or, when `dns`, through DNS, where the checks
 * of the names and the port are left out and one line says what to do.
 */
export function checkRows(report, dns) {
  const checks = Array.isArray(report?.checks) ? report.checks : [];
  const rows = checks.filter((row) => row.result !== 'skipped').map((row, at) => checkRow(row, report, at));
  if (dns) rows.push({
    key: 'dns', tone: 'ok', id: null, name: null, button: null, words: DNS_LINE,
  });
  return rows;
}

/** Whether `report` is an answer of the check, which has a list of checks. */
export const isReport = (report) => report !== null && typeof report === 'object' && Array.isArray(report.checks);

/** The address of the terms of service, when it is one a link may go to. */
export function termsLink(report) {
  try {
    const url = new URL(report?.terms_url ?? '');
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * What the button that gets the certificate does with the checks: `{can,
 * names, dropped, label, why}`. A name that fails alone is left out, and the
 * button says so; a failure that is not one name's (the firewall, the clock,
 * the program on port 80) stops it. Before the checks are in, it is off.
 */
export function plan(rows, names) {
  const base = {
    can: false, names, dropped: [], label: 'Get the certificate', why: '',
  };
  if (rows === null) return { ...base, why: 'The checks are not in yet.' };
  const failed = rows.filter((row) => row.tone === 'bad');
  if (failed.some((row) => row.name === null)) return { ...base, why: 'Put right what failed first, then check again.' };
  const dropped = [...new Set(failed.map((row) => row.name))];
  if (dropped.length === 0) return { ...base, can: true };
  const kept = names.filter((name) => !dropped.includes(name));
  if (kept.length === 0) return { ...base, dropped, why: 'No name passes the checks.' };
  return {
    can: true, names: kept, dropped, label: `Get it without ${listWords(dropped)}`, why: '',
  };
}

/** The arguments of `certificate.get` for `names`; `dryRun` asks the staging authority and installs nothing. */
export function getArgs({
  names, dns = false, dryRun = false, email = '',
}) {
  const args = { name: certName(names), names: names.join('\n') };
  if (dns) args.dns = true;
  if (dryRun) args.dry_run = true;
  if (email.trim() !== '') args.email = email.trim();
  args.agree_terms = true;
  return args;
}

/** The arguments of `firewall.add` that open port 80 for a while. */
export const openPortArgs = (seconds = PORT_TRY_SECONDS) => ({
  action: 'allow', protocol: 'tcp', ports: '80', try: seconds,
});

/** The TXT records the log of an order by DNS asks for, `[{name, value}]`: each is a line `add: NAME TXT "VALUE"`. */
export function recordsOf(log) {
  const found = [];
  for (const line of Array.isArray(log) ? log : []) {
    const record = /^add:\s+(\S+)\s+TXT\s+"(.*)"\s*$/.exec(String(line));
    if (record && !found.some((one) => one.name === record[1] && one.value === record[2])) found.push({ name: record[1], value: record[2] });
  }
  return found;
}

/** The line a record is copied as: its name, then its value. */
export const recordLine = (record) => `${record.name} TXT "${record.value}"`;

/** Whether the order waits for the records, from the words of its step. */
export const waitsForRecords = (task) => /waiting for the TXT records/.test(task?.words ?? '');

/** How a task of the order ended, in words: `{ok, words}`. `dryRun` says it was the staging test. */
export function endedWords(task, { dryRun, name }) {
  if (task.state === 'done') {
    return {
      ok: true,
      words: dryRun
        ? 'The staging authority accepted the order, so this order should work. Nothing was installed.'
        : `${name} is got and installed.`,
    };
  }
  return { ok: false, words: `The order did not finish. ${taskEnd(task)}.` };
}

/** What names `names` would be used by: the services of the certificate called `name` that is there already, as words. */
export function whereUsed(name, list, names) {
  const there = (Array.isArray(list) ? list : []).find((certificate) => certificate.name === name);
  const users = there?.used_by ?? [];
  if (users.length > 0) return `${name} is used by ${listWords(users)}.`;
  return `No nginx site answers for ${names[0] ?? name} yet. The certificate is kept until a site uses it.`;
}
