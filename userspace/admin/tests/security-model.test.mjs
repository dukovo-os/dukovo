// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Security page decides (design 8): a button for every check the
// score weighs, what a change of SSH's settings takes away (held against
// the cases that ssh-server(8)'s own test reads, so that the page and the
// command say the same), whether a change of the console's reach would cut
// the browser off, and the rows of the blocked addresses.

import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { PREFIX_WEIGHTS, WEIGHTS } from '../lib/health.js';
import {
  blockedRows, firstSentence, fixesFor, isOutside, narrows, networksOf, nobody, reachCutsOff,
} from '../pages/security/model.js';
import { at, time } from './snapshots-fixtures.mjs';

const REPO = new URL('../../../', import.meta.url);

// ---- the buttons of the findings ----------------------------------------------------

const everyPage = () => true;
const noPage = () => false;
const finding = (id, detail = '', fix = 'Do this.') => ({ id, detail, fix });

test('every check that the score weighs by name has a button, whether or not the page it names is in the console', () => {
  for (const id of Object.keys(WEIGHTS)) {
    for (const has of [everyPage, noPage]) {
      const buttons = fixesFor(finding(id, 'alice has no password'), has);
      assert.ok(buttons.length > 0, `${id} has a button`);
      for (const button of buttons) assert.ok(button.label.length > 0, `${id}'s button has words`);
    }
  }
});

test('every warning of the early warnings and the hardware check that the machine has a page for has a button', () => {
  const ids = readdirSync(new URL('docs/errors/', REPO)).map((name) => name.replace(/\.md$/, ''));
  const families = PREFIX_WEIGHTS.map((entry) => entry.prefix);
  const named = ids.filter((id) => families.some((prefix) => id.startsWith(prefix)));
  assert.ok(named.length > 15, 'the error pages of both families are read');
  for (const id of named) {
    assert.ok(fixesFor(finding(id), noPage).length > 0, `${id} has a button`);
    assert.ok(fixesFor(finding(id), everyPage).length > 0, `${id} has a button with every page there`);
  }
});

test('each button goes where design 8.3 says', () => {
  const only = (id, detail, has = everyPage) => {
    const buttons = fixesFor(finding(id, detail), has);
    assert.equal(buttons.length, 1, id);
    return buttons[0];
  };
  assert.deepEqual(only('SEC-101'), { kind: 'link', label: 'See the rule', href: '/firewall' });
  assert.deepEqual(only('SEC-102'), {
    kind: 'action', label: 'Start the firewall', action: 'service.start', args: { name: 'firewall' }, words: { unlock: 'start the firewall', done: 'The firewall is started. Check again to update the score.' },
  });
  assert.deepEqual(only('SEC-110'), { kind: 'ssh', label: 'Keys only…', preset: 'keys-only' });
  assert.deepEqual(only('SEC-111'), { kind: 'ssh', label: 'Choose who may sign in…', preset: 'who' });
  assert.equal(only('SEC-112').kind, 'how');
  assert.match(only('SEC-112').words, /\/system\/settings\/blocking\.toml/);
  assert.deepEqual(only('SEC-120', 'alice has no password'), { kind: 'link', label: 'Set a password…', href: '/accounts/alice' });
  assert.deepEqual(only('SEC-121'), { kind: 'link', label: 'Choose one…', href: '/accounts' });
  assert.deepEqual(only('SEC-130'), { kind: 'link', label: 'Updates', href: '/updates' });
  assert.deepEqual(only('SEC-140'), { kind: 'how', label: 'Show how', words: 'The time service says what it tried.', command: 'timedatectl timesync-status' });
  assert.deepEqual(only('SEC-150', 'web gave up after failed starts in a row'), { kind: 'link', label: 'See it', href: '/services/web' });
  assert.deepEqual(only('SEC-160'), { kind: 'link', label: 'Snapshots', href: '/snapshots' });
  assert.deepEqual(only('MET-101'), { kind: 'link', label: 'See it', href: '/storage' });
  assert.deepEqual(only('MET-105'), { kind: 'link', label: 'See it', href: '/' });
});

test('the updates have a command until the Updates page is in the console', () => {
  const [now] = fixesFor(finding('SEC-130'), noPage);
  assert.equal(now.kind, 'how');
  assert.equal(now.command, 'elevate update apply');
  assert.equal(fixesFor(finding('SEC-131'), noPage)[0].command, 'elevate update check');
});

test('a hardware check goes to Hardware when there is such a page, else a disk to Storage and the rest to its advice', () => {
  assert.deepEqual(fixesFor(finding('HW-132'), everyPage), [{ kind: 'link', label: 'See it', href: '/hardware' }]);
  assert.deepEqual(fixesFor(finding('HW-111'), noPage), [{ kind: 'link', label: 'See it', href: '/storage' }]);
  assert.equal(fixesFor(finding('HW-132', '', 'Check the fan.'), noPage)[0].kind, 'how');
  assert.deepEqual(fixesFor(finding('HW-132', '', ''), noPage), []);
});

test('a check with several names has a button for each, and one the table does not know has its advice or nothing', () => {
  assert.deepEqual(
    fixesFor(finding('SEC-120', 'alice, bob and carol have no password'), everyPage).map((one) => one.href),
    ['/accounts/alice', '/accounts/bob', '/accounts/carol'],
  );
  assert.equal(fixesFor(finding('SEC-120', 'alice and bob have no password'), everyPage)[1].label, 'Set a password for bob…');
  assert.deepEqual(fixesFor(finding('SEC-150', 'web and backup gave up after failed starts in a row'), everyPage).map((one) => one.href), ['/services/web', '/services/backup']);
  assert.equal(fixesFor(finding('SEC-120', 'someone'), everyPage)[0].href, '/accounts');
  assert.equal(fixesFor(finding('SEC-150', 'something'), everyPage)[0].href, '/services');
  assert.equal(fixesFor(finding('NEW-001', '', 'Ask for help.'), everyPage)[0].kind, 'how');
  assert.deepEqual(fixesFor(finding('NEW-001', '', ''), everyPage), []);
  assert.deepEqual(fixesFor(finding(null, '', ''), everyPage), []);
});

test('a name with a dot in it goes to a service address that the server does not take for a file', () => {
  assert.equal(fixesFor(finding('SEC-150', 'my.web gave up after failed starts in a row'), everyPage)[0].href, '/services/my%2Eweb');
});

test('the first sentence of an advice ends at its first full stop followed by a space', () => {
  assert.equal(firstSentence('Remove that rule with elevate firewall remove NUMBER, and open only the ports that need it. Then run it.'), 'Remove that rule with elevate firewall remove NUMBER, and open only the ports that need it.');
  assert.equal(firstSentence('Set it in /system/settings/ssh/sshd_config.'), 'Set it in /system/settings/ssh/sshd_config.');
  assert.equal(firstSentence('No full stop'), 'No full stop');
  assert.equal(firstSentence(null), '');
});

// ---- SSH: what a change takes away --------------------------------------------------------

const ssh = (more = {}) => ({
  on: true, passwords: false, keys: true, allow_users: [], allow_groups: [], ...more,
});

test('turning SSH off, a way in off or a name off a list narrows who signs in', () => {
  assert.equal(narrows(ssh(), ssh({ on: false })), true);
  assert.equal(narrows(ssh({ passwords: true }), ssh({ passwords: false })), true);
  assert.equal(narrows(ssh(), ssh({ keys: false, passwords: true })), true);
  assert.equal(narrows(ssh(), ssh({ allow_users: ['alice'] })), true);
  assert.equal(narrows(ssh(), ssh({ allow_groups: ['admins'] })), true);
  assert.equal(narrows(ssh({ allow_users: ['alice', 'bob'] }), ssh({ allow_users: ['alice'] })), true);
  assert.equal(narrows(ssh({ allow_users: ['alice'] }), ssh({ allow_users: ['bob'] })), true);
  assert.equal(narrows(ssh({ allow_groups: ['admins', 'staff'] }), ssh({ allow_groups: ['staff'] })), true);
});

test('turning SSH on, a way in on or a name on a list, or a list taken away, does not narrow', () => {
  assert.equal(narrows(ssh({ on: false }), ssh()), false);
  assert.equal(narrows(ssh({ on: false }), ssh({ passwords: true, allow_users: ['alice'] })), false);
  assert.equal(narrows(ssh(), ssh({ passwords: true })), false);
  assert.equal(narrows(ssh({ allow_users: ['alice'] }), ssh({ allow_users: ['alice', 'bob'] })), false);
  assert.equal(narrows(ssh({ allow_users: ['alice'] }), ssh()), false);
  assert.equal(narrows(ssh({ allow_groups: ['admins'] }), ssh({ allow_groups: [] })), false);
  assert.equal(narrows(ssh(), ssh()), false);
  assert.equal(narrows(ssh({ allow_users: ['alice', 'bob'] }), ssh({ allow_users: ['bob', 'alice'] })), false, 'the same names in another order let in the same people');
});

test('a change that widens one way in and narrows another narrows', () => {
  assert.equal(narrows(ssh({ passwords: false, keys: true }), ssh({ passwords: true, keys: false })), true);
  assert.equal(narrows(ssh({ allow_users: ['alice'] }), ssh({ allow_users: ['bob', 'alice'], passwords: false, keys: false })), true);
});

test('a file that lets nobody in has no one to cut off', () => {
  assert.equal(narrows(ssh({ passwords: false, keys: false }), ssh({ passwords: true, allow_users: ['alice'] })), false);
  assert.equal(narrows(ssh({ on: false }), ssh({ allow_users: ['alice'] })), false);
});

test('nobody signs in over SSH when neither passwords nor keys do', () => {
  assert.equal(nobody(ssh({ passwords: false, keys: false })), true);
  assert.equal(nobody(ssh({ passwords: true, keys: false })), false);
  assert.equal(nobody(ssh({ passwords: false, keys: true })), false);
});

/**
 * Whether `settings` let a person with the account `name`, in the groups
 * `groups`, sign in with `method` (`password` or `key`): what sshd does with
 * these four settings, written out for one person and one way, so that a
 * test can try every person and every way against `narrows`.
 */
function lets(settings, name, groups, method) {
  if (settings.on === false) return false;
  if (!(method === 'password' ? settings.passwords : settings.keys)) return false;
  if (settings.allow_users.length > 0 && !settings.allow_users.includes(name)) return false;
  return settings.allow_groups.length === 0 || groups.some((group) => settings.allow_groups.includes(group));
}

/** Every subset of `names`. */
const subsets = (names) => names.reduce((all, name) => [...all, ...all.map((one) => [...one, name])], [[]]);

/** Whether some person who signs in with `before` does not with `after`, among the accounts and the groups they may be in. */
function takesAWayIn(before, after, accounts, groups) {
  return accounts.some((name) => subsets(groups).some((member) => ['password', 'key'].some((method) => lets(before, name, member, method) && !lets(after, name, member, method))));
}

test('narrows agrees with trying every person and every way in, for every pair of a spread of settings', () => {
  const accounts = ['alice', 'bob', 'mallory', 'zed'];
  const groups = ['admins', 'staff'];
  const lists = [[], ['alice'], ['bob'], ['alice', 'bob'], ['alice', 'bob', 'mallory']];
  const spread = [];
  for (const on of [true, false]) {
    for (const passwords of [true, false]) {
      for (const keys of [true, false]) {
        for (const allow_users of lists) {
          for (const allow_groups of [[], ['admins'], ['staff'], ['admins', 'staff']]) spread.push({ on, passwords, keys, allow_users, allow_groups });
        }
      }
    }
  }
  let narrowed = 0;
  for (const before of spread) {
    for (const after of spread) {
      const expected = takesAWayIn(before, after, accounts, groups);
      assert.equal(narrows(before, after), expected, `${JSON.stringify(before)} to ${JSON.stringify(after)}`);
      if (expected) narrowed += 1;
    }
  }
  assert.ok(narrowed > 1000, 'the spread has many changes that narrow, and many that do not');
});

// ---- SSH: the cases that ssh-server(8) makes of sshd_config -------------------------------

const CASES = new URL('userspace/programs/systools/src/ssh_server/cases.txt', REPO);
const COMMAND_WAITS = existsSync(CASES) ? false : 'waits for userspace/programs/systools/src/ssh_server/cases.txt, which ssh-server(8) brings';

/** The words after a keyword of a line of sshd_config: after spaces or one `=`, a word in double quotes may hold spaces, and `#` starts a comment at the start of a word. */
function wordsOf(rest) {
  const words = [];
  let at = rest.replace(/^\s*=?\s*/, '');
  while (at !== '' && !at.startsWith('#')) {
    const quoted = /^"([^"]*)"\s*/.exec(at);
    const plain = /^(\S+)\s*/.exec(at);
    const found = quoted ?? plain;
    words.push(found[1]);
    at = at.slice(found[0].length);
  }
  return words;
}

/** The four settings of the text `lines` of sshd_config(5), as the command reads them: the first value of a flag wins, the lists add up, passwords are off and keys on until the file says otherwise. */
function settingsOf(lines) {
  const settings = { on: true, passwords: false, keys: true, allow_users: [], allow_groups: [] };
  const seen = new Set();
  for (const line of lines) {
    const found = /^\s*([A-Za-z]+)(.*)$/.exec(line);
    if (!found) continue;
    const keyword = found[1].toLowerCase();
    const words = wordsOf(found[2]);
    if (keyword === 'allowusers') settings.allow_users.push(...words);
    if (keyword === 'allowgroups') settings.allow_groups.push(...words);
    if ((keyword === 'passwordauthentication' || keyword === 'pubkeyauthentication') && !seen.has(keyword)) {
      seen.add(keyword);
      settings[keyword === 'passwordauthentication' ? 'passwords' : 'keys'] = words[0] === 'yes';
    }
  }
  return settings;
}

/** The cases of the file: `{accounts, groups, before, change, after, refusal}` for each, `after` being the lines the file has then, or null when it is refused. */
function casesOf(text) {
  let accounts = [];
  let groups = [];
  const found = [];
  for (const block of text.split(/\n\s*\n/)) {
    const lines = block.split('\n').filter((line) => line !== '' && !line.startsWith('#'));
    if (lines.length === 0) continue;
    if (lines[0].startsWith('accounts:')) {
      accounts = lines[0].slice('accounts:'.length).trim().split(/\s+/);
      groups = (lines[1] ?? 'groups:').slice('groups:'.length).trim().split(/\s+/);
      continue;
    }
    const kept = { accounts, groups, before: [], change: '', after: null, refusal: null };
    let same = false;
    let empty = false;
    const after = [];
    for (const line of lines) {
      if (line.startsWith('\\')) continue;
      const text1 = line.length > 2 ? line.slice(2) : '';
      if (line.startsWith('<')) kept.before.push(text1);
      else if (line.startsWith('+ ')) kept.change = line.slice(2);
      else if (line.startsWith('>')) after.push(text1);
      else if (line === '=') same = true;
      else if (line === '-') empty = true;
      else if (line.startsWith('! ')) kept.refusal = line.slice(2);
    }
    if (kept.refusal === null) kept.after = same ? kept.before : empty ? [] : after;
    found.push(kept);
  }
  return found;
}

/** The settings that the words after `ssh-server set` ask for, laid over `settings`: `{passwords, keys, allow_users, allow_groups}`. */
function laidOver(settings, change) {
  const next = { ...settings, allow_users: [...settings.allow_users], allow_groups: [...settings.allow_groups] };
  const words = change.split(/\s+/);
  const keywords = ['passwords', 'keys', 'allow-users', 'allow-groups'];
  for (let at = 0; at < words.length;) {
    let end = at + 1;
    while (end < words.length && !keywords.includes(words[end])) end += 1;
    const values = words.slice(at + 1, end);
    if (words[at] === 'passwords') next.passwords = values[0] === 'yes';
    if (words[at] === 'keys') next.keys = values[0] === 'yes';
    if (words[at] === 'allow-users') next.allow_users = values[0] === 'everyone' ? [] : values;
    if (words[at] === 'allow-groups') next.allow_groups = values[0] === 'everyone' ? [] : values;
    at = end;
  }
  return next;
}

test('the cases of the command are read as a person reads them', { skip: COMMAND_WAITS }, () => {
  const found = casesOf(readFileSync(CASES, 'utf8'));
  assert.ok(found.length > 30, `${found.length} cases`);
  assert.deepEqual(found[0].accounts, ['alice', 'bob', 'carol', 'root']);
  assert.deepEqual(found[0].groups, ['admins', 'staff']);
  assert.ok(found.filter((one) => one.refusal === 'nobody').length >= 4, 'the refusals for no way in are read');
});

test('what the page takes the settings to be after a change is what the command writes, and the change it refuses is the one with no way in', { skip: COMMAND_WAITS }, () => {
  const all = casesOf(readFileSync(CASES, 'utf8'));
  let compared = 0;
  let refused = 0;
  for (const one of all) {
    if (one.refusal !== null && one.refusal !== 'nobody') continue;
    const before = settingsOf(one.before);
    const wanted = laidOver(before, one.change);
    if (one.refusal === 'nobody') {
      assert.equal(nobody(wanted), true, `${one.change} is refused for no way in`);
      refused += 1;
    } else {
      assert.deepEqual(settingsOf(one.after), wanted, `${one.change} on ${JSON.stringify(one.before)}`);
      assert.equal(nobody(wanted), false, `${one.change} leaves a way in`);
      compared += 1;
    }
  }
  assert.ok(compared > 20, `${compared} changes held`);
  assert.equal(refused, all.filter((one) => one.refusal === 'nobody').length, 'every refusal for no way in is one that nobody() sees');
});

test('narrows agrees, for each change of the command, with trying every person and every way in', { skip: COMMAND_WAITS }, () => {
  for (const one of casesOf(readFileSync(CASES, 'utf8'))) {
    if (one.refusal !== null) continue;
    const before = settingsOf(one.before);
    const after = settingsOf(one.after);
    assert.equal(narrows(before, after), takesAWayIn(before, after, [...one.accounts, 'mallory', 'zed'], one.groups), `${one.change} on ${JSON.stringify(one.before)}`);
  }
});

// ---- the console's reach (design 8.7) ----------------------------------------------------------

test('a browser on a private, shared or local address is inside, and one anywhere else is outside', () => {
  for (const inside of ['192.168.1.25', '10.8.0.2', '172.16.5.5', '172.31.255.254', '100.64.0.1', '100.127.255.254', '169.254.3.4', '127.0.0.1', '::1', 'fd12:3456::1', 'fe80::1', '::ffff:192.168.1.25']) {
    assert.equal(isOutside(inside), false, inside);
  }
  for (const outside of ['203.0.113.9', '172.32.0.1', '100.128.0.1', '192.169.0.1', '11.0.0.1', '2001:db8::1', 'fec0::1', '::ffff:203.0.113.9']) {
    assert.equal(isOutside(outside), true, outside);
  }
  assert.equal(isOutside('not an address'), false, 'text that is no address is not said to be outside');
});

test('a network that is not whole inside a private range is outside', () => {
  assert.equal(isOutside('192.168.1.0/24'), false);
  assert.equal(isOutside('10.0.0.0/8'), false);
  assert.equal(isOutside('10.0.0.0/7'), true);
  assert.equal(isOutside('0.0.0.0/0'), true);
  assert.equal(isOutside('fd00::/8'), false);
  assert.equal(isOutside('fc00::/6'), true);
});

test('the networks of the cards are given as networks, with the host bits cleared', () => {
  const show = {
    cards: [
      { name: 'net0', addresses: [{ address: '192.168.1.10', prefix: 24 }, { address: 'fe80::1', prefix: 64 }] },
      { name: 'net1', addresses: [{ address: '203.0.113.7', prefix: 29 }] },
      { name: 'lo', addresses: [] },
    ],
  };
  assert.deepEqual(networksOf(show), ['192.168.1.0/24', 'fe80::/64', '203.0.113.0/29']);
  assert.deepEqual(networksOf(null), []);
  assert.deepEqual(networksOf({ cards: [{ addresses: [{ address: 'x', prefix: 1 }] }] }), []);
});

test('a browser from outside is cut off by a private reach unless it is on one of the machine\'s own networks', () => {
  const networks = ['192.168.1.0/24', '203.0.113.0/29'];
  assert.equal(reachCutsOff('203.0.113.9', networks), true);
  assert.equal(reachCutsOff('198.51.100.9', networks), true);
  assert.equal(reachCutsOff('203.0.113.3', networks), false, 'its own network, though a public one');
  assert.equal(reachCutsOff('192.168.1.25', networks), false);
  assert.equal(reachCutsOff('10.8.0.2', networks), false, 'a private range of its own');
  assert.equal(reachCutsOff('::ffff:203.0.113.3', networks), false);
  assert.equal(reachCutsOff('203.0.113.9', []), true);
  assert.equal(reachCutsOff('', networks), false, 'an address that is not given is not said to be cut off');
  assert.equal(reachCutsOff(undefined, networks), false);
});

// ---- the blocked addresses (design 8.5) ---------------------------------------------------------

test('the rows say the address, whether it is outside, why it was blocked and until when', () => {
  const answer = {
    on: true,
    never: ['192.168.1.0/24'],
    blocks: [
      { address: '203.0.113.41/32', until_ms: at(2026, 11, 14, 8, 19), today: 2, why: '3 wrong passwords over SSH in 2 minutes, for admin' },
      { address: '192.168.1.77/32', until_ms: at(2026, 11, 15, 9, 2), today: 1, why: '12 console sign-ins' },
      { address: '2001:db8::/64', until_ms: at(2026, 11, 14, 11, 0), today: 1, why: '5 wrong passwords over SSH' },
    ],
  };
  const rows = blockedRows(answer, time);
  assert.deepEqual(rows.map((row) => [row.address, row.outside]), [['203.0.113.41', true], ['192.168.1.77', false], ['2001:db8::/64', true]]);
  assert.equal(rows[0].why, '3 wrong passwords over SSH in 2 minutes, for admin');
  assert.equal(rows[0].until, '8:19 AM');
  assert.equal(rows[1].until, 'Tomorrow 9:02 AM'.replace('Tomorrow', 'Nov 15'));
  assert.match(rows[0].full, /Nov 14, 2026/);
  assert.deepEqual(rows.map((row) => row.key), ['203.0.113.41/32', '192.168.1.77/32', '2001:db8::/64'], 'the key is the address as the firewall gave it');
});

test('no blocks, or an answer that is not one, give no rows', () => {
  assert.deepEqual(blockedRows({ on: true, never: [], blocks: [] }, time), []);
  assert.deepEqual(blockedRows(null, time), []);
  assert.deepEqual(blockedRows({}, time), []);
});

test('an address the page cannot read is shown as it was given', () => {
  const [row] = blockedRows({ blocks: [{ address: 'somewhere', until_ms: at(2026, 11, 14, 9, 0), why: 'x' }] }, time);
  assert.equal(row.address, 'somewhere');
  assert.equal(row.outside, false);
});
