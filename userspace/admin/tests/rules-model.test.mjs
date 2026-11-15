// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The firewall's rules as the Firewall page holds them (design 10): read and
// written as firewall(8) does, in words, which rule decides a connection
// (every question of the file that firewall.rs's tests read too), whether a
// change cuts the browser off, and the form that makes a rule.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { parseAddress } from '../lib/address-model.js';
import {
  actionOf, ADMIND, argsOfRule, blankForm, browserPacket, canChange, connectionAfter, decides, formOfRule, hereOf, parsePorts,
  parseRule, ruleOf, ruleOfForm, rulesAfter, rulesOf, ruleText, sameRule, wordsOf,
} from '../pages/firewall/rules-model.js';

const CASES = new URL('../../programs/systools/src/firewall/cases.txt', import.meta.url);

const rule = (text) => {
  const found = parseRule(text.split(/\s+/));
  assert.ok(found.rule, `${text}: ${found.why}`);
  return found.rule;
};

/** A question of the file: `tcp|udp PORT` or `ping`, then `from ADDRESS on CARD [program PATH]`. */
function asked(words) {
  const [protocol, ...rest] = words.split(/\s+/);
  const port = protocol === 'ping' ? 0 : Number(rest.shift());
  assert.equal(rest[0], 'from');
  assert.equal(rest[2], 'on');
  return {
    protocol, port, from: parseAddress(rest[1]), on: rest[3], program: rest[4] === 'program' ? rest[5] : undefined,
  };
}

test('every question of the shared file is decided as firewall test decides it', () => {
  let count = 0;
  for (const block of readFileSync(CASES, 'utf8').split('\n\n')) {
    const rules = [];
    for (const line of block.split('\n').filter((one) => one !== '' && !one.startsWith('#'))) {
      if (!line.startsWith('> ')) {
        rules.push(rule(line));
        continue;
      }
      const [question, want] = line.slice(2).split(' => ');
      const found = decides(rules, asked(question));
      assert.equal(found.number === null ? 'none' : `${found.number} ${found.action}`, want, `${rules.map(ruleText).join('; ')}: ${line}`);
      count += 1;
    }
  }
  assert.ok(count >= 30, `${count} questions`);
});

test('a rule is read and written back as it is, in any order of its words', () => {
  for (const text of [
    'allow tcp 22', 'deny udp 5353 on net1', 'allow tcp 8000-8100 from 192.168.1.0/24', 'allow from 2001:db8::/32', 'deny ping from 10.0.0.1',
    'allow program /users/alice/bin/my server', 'allow tcp 8080 on br0 program /applications/commands/web', 'deny',
  ]) {
    assert.equal(ruleText(rule(text)), text);
  }
  assert.equal(ruleText(rule('allow 22 tcp')), 'allow tcp 22');
  assert.equal(ruleText(rule('allow any 1-65535')), 'allow 1-65535');
  assert.equal(ruleText(rule('allow tcp 22 from 192.168.1.9/32')), 'allow tcp 22 from 192.168.1.9');
});

test('a wrong rule says why, in the words of the command', () => {
  const why = (text) => parseRule(text === '' ? [] : text.split(' ')).why;
  assert.equal(why(''), 'a rule starts with allow or deny');
  assert.equal(why('open 22'), 'a rule starts with allow or deny, not `open`');
  assert.equal(why('allow tcp 99999'), '`99999` is not a port, 1 to 65535, or a range of them');
  assert.equal(why('allow tcp 0'), '`0` is not a port, 1 to 65535, or a range of them');
  assert.equal(why('allow tcp 90-80'), '`90-80` is not a port, 1 to 65535, or a range of them');
  assert.equal(why('allow tcp 22-'), '`22-` is not a port, 1 to 65535, or a range of them');
  assert.equal(why('allow tcp 22 80'), '`80` is not a word of a rule here');
  assert.equal(why('allow tcp tcp'), '`tcp` is not a word of a rule here');
  assert.equal(why('allow from'), '`from` needs a network, such as 192.168.1.0/24 after it');
  assert.equal(why('allow from 10.0.0.0/33'), '`10.0.0.0/33` is not an address or network');
  assert.equal(why('allow from 10.0.0.1 from 10.0.0.2'), '`from` is not a word of a rule here');
  assert.equal(why('allow on'), '`on` needs an interface\'s name after it');
  assert.equal(why('allow on net/0'), '`net/0` is not an interface\'s name');
  assert.equal(why('allow on abcdefghijklmnop'), '`abcdefghijklmnop` is not an interface\'s name');
  assert.equal(why('allow program'), '`program` needs a program\'s path after it');
  assert.equal(why('allow program bin/x'), '`program` takes an absolute path, not `bin/x`');
  assert.equal(why('allow ping 22'), 'a ping has no port');
  assert.equal(why('allow ping program /x'), 'a ping reaches no program');
  assert.equal(why('allow nonsense'), '`nonsense` is not a word of a rule here');
});

test('ports are one or a range, from 1 to 65535', () => {
  assert.deepEqual(parsePorts('22'), [22, 22]);
  assert.deepEqual(parsePorts('8000-8100'), [8000, 8100]);
  assert.deepEqual(parsePorts('65535'), [65_535, 65_535]);
  for (const text of ['0', '65536', '70000', '-5', '5-', '1-2-3', 'x', '', '22 ', '8100-8000']) assert.equal(parsePorts(text), null, text);
});

test('a rule in words', () => {
  const words = {
    'allow tcp 443 from 192.168.1.0/24': 'Allow TCP port 443 from 192.168.1.0/24',
    'deny ping on net1': 'Deny pings on net1',
    'allow udp 51820': 'Allow UDP port 51820',
    'allow tcp 8000-8100': 'Allow TCP ports 8000 to 8100',
    'allow tcp': 'Allow TCP on every port',
    'allow 22': 'Allow TCP and UDP port 22',
    deny: 'Deny everything',
    'allow from 10.0.0.0/8': 'Allow everything from 10.0.0.0/8',
    'allow program /system/commands/admind': 'Allow everything for the program /system/commands/admind',
    'deny tcp 22 on net0 program /x': 'Deny TCP port 22 on net0 for the program /x',
  };
  for (const [text, said] of Object.entries(words)) assert.equal(wordsOf(rule(text)), said, text);
});

const LISTED = {
  rules: [
    { number: 1, text: 'allow tcp 22', action: 'allow', protocol: 'tcp', ports: [22, 22], from: null, on: null, program: null },
    { number: 2, text: 'allow tcp 8000-8100 from 192.168.1.0/24 on net0', action: 'allow', protocol: 'tcp', ports: [8000, 8100], from: '192.168.1.0/24', on: 'net0', program: null },
    { number: 3, text: 'deny ping', action: 'deny', protocol: 'ping', ports: null, from: null, on: null, program: null },
    { number: 4, text: 'deny program /users/alice/bin/server', action: 'deny', protocol: 'any', ports: null, from: null, on: null, program: '/users/alice/bin/server' },
    { number: 5, text: 'allow', action: 'allow', protocol: 'any', ports: null, from: null, on: null, program: null },
  ],
  problems: [],
  trial: null,
};

test('the rules of firewall list --json are the rules their words make', () => {
  for (const entry of LISTED.rules) assert.deepEqual(ruleOf(entry), rule(entry.text), entry.text);
  assert.equal(rulesOf(LISTED).length, 5);
  assert.deepEqual(rulesOf(null), []);
  assert.deepEqual(rulesOf({}), []);
});

const SESSION = { from: '192.168.1.20', via: { address: '192.168.1.10', port: 9090, card: 'net0' } };

test('the browser\'s connection is a new TCP connection to the console\'s port from its address, by its card, to admind', () => {
  const packet = browserPacket(SESSION);
  assert.equal(packet.protocol, 'tcp');
  assert.equal(packet.port, 9090);
  assert.equal(packet.on, 'net0');
  assert.equal(packet.program, ADMIND);
  assert.deepEqual(packet.from, parseAddress('192.168.1.20'));
  assert.equal(browserPacket({ from: '192.168.1.20' }), null);
  assert.equal(browserPacket({ via: SESSION.via }), null);
  assert.equal(browserPacket({ from: 'host', via: SESSION.via }), null);
  assert.equal(browserPacket(null), null);
});

test('rules after an add are the rules and the new one at the end; after a replace, the new one in place; after a remove, without it', () => {
  const rules = [rule('allow tcp 22'), rule('deny tcp 22'), rule('allow tcp 22')];
  const next = rule('allow tcp 80');
  assert.deepEqual(rulesAfter(rules, { kind: 'add', rule: next }).map(ruleText), ['allow tcp 22', 'deny tcp 22', 'allow tcp 22', 'allow tcp 80']);
  assert.deepEqual(rulesAfter(rules, { kind: 'replace', old: rule('deny tcp 22'), rule: next }).map(ruleText), ['allow tcp 22', 'allow tcp 80', 'allow tcp 22']);
  assert.deepEqual(rulesAfter(rules, { kind: 'remove', old: rule('allow tcp 22') }).map(ruleText), ['deny tcp 22', 'allow tcp 22'], 'the first rule that is equal');
  assert.equal(rulesAfter(rules, { kind: 'remove', old: rule('allow tcp 23') }), rules, 'a rule that is not there');
  assert.equal(rulesAfter(rules, { kind: 'replace', old: rule('allow tcp 23'), rule: next }), rules);
  assert.equal(sameRule(rule('allow 22 tcp'), rule('allow tcp 22')), true, 'by their words as a rule, in any order');
});

test('a rule that would deny the browser\'s connection is a cut, with the number of the rule that denies it', () => {
  const rules = [rule('allow tcp 22'), rule('allow tcp 9090 from 192.168.1.0/24')];
  const cut = connectionAfter(rules, { kind: 'replace', old: rules[1], rule: rule('deny tcp 9090 from 192.168.1.0/24') }, SESSION);
  assert.equal(cut.state, 'denied');
  assert.equal(cut.number, 2);
  assert.equal(cut.words, 'This would cut off your own connection. The console reaches you from 192.168.1.20 through net0 on port 9090, and this rule would deny it.');
  // Added after the allow: the first that fits decides, and it allows.
  assert.equal(connectionAfter(rules, { kind: 'add', rule: rule('deny tcp 9090') }, SESSION).state, 'allowed');
  // Taking the allow away leaves the deny that was behind it.
  const behind = [rule('allow tcp 9090 from 192.168.1.20'), rule('deny tcp 9090 from 192.168.1.0/24')];
  const removed = connectionAfter(behind, { kind: 'remove', old: behind[0] }, SESSION);
  assert.equal(removed.state, 'denied');
  assert.equal(removed.number, 2, 'the place it has in the rules now, not after the one taken out');
  assert.match(removed.words, /and rule 2 would deny it\.$/);
  // A rule before the one taken out keeps its place.
  const before = [rule('deny tcp 9090'), rule('allow tcp 22')];
  assert.equal(connectionAfter(before, { kind: 'remove', old: before[1] }, SESSION).number, 1);
});

test('a rule of another port, card or network does not cut the browser off, and a rule for a program fits the console\'s', () => {
  const add = (text) => connectionAfter([], { kind: 'add', rule: rule(text) }, SESSION).state;
  assert.equal(add('deny tcp 22'), 'allowed');
  assert.equal(add('deny tcp 9090 on net1'), 'allowed');
  assert.equal(add('deny tcp 9090 from 10.0.0.0/8'), 'allowed');
  assert.equal(add('deny udp 9090'), 'allowed');
  assert.equal(add('deny ping'), 'allowed');
  assert.equal(add('deny tcp 9000-9100'), 'denied');
  assert.equal(add('deny tcp 9090 from 192.168.1.20'), 'denied');
  assert.equal(add('deny tcp 9090 on net0'), 'denied');
  assert.equal(add('deny'), 'denied');
  assert.equal(add('deny program /system/commands/admind'), 'denied');
  assert.equal(add('deny program /system/commands/sshd'), 'allowed');
  assert.equal(add('deny 9090'), 'denied');
});

test('a session that does not say where the browser comes from has no cut to find', () => {
  assert.deepEqual(connectionAfter([], { kind: 'add', rule: rule('deny') }, {}), { state: 'unknown' });
});

test('the form makes a rule, and a value that cannot work says which field and why', () => {
  const form = (over = {}) => ({ ...blankForm(), ports: '22', ...over });
  assert.equal(ruleText(ruleOfForm(form(), null).rule), 'allow tcp 22');
  assert.equal(ruleText(ruleOfForm(form({ action: 'deny', protocol: 'udp', ports: ' 53 ', on: 'net1' }), null).rule), 'deny udp 53 on net1');
  assert.equal(ruleText(ruleOfForm(form({ from: { kind: 'address', text: ' 203.0.113.9 ' } }), null).rule), 'allow tcp 22 from 203.0.113.9');
  assert.equal(ruleText(ruleOfForm(form({ from: { kind: 'network', text: '' } }), '192.168.1.0/24').rule), 'allow tcp 22 from 192.168.1.0/24');
  assert.equal(ruleText(ruleOfForm(form({ protocol: 'ping', ports: '7' }), null).rule), 'allow ping', 'a ping has no port');
  assert.equal(ruleText(ruleOfForm(form({ protocol: 'any', ports: '1-1024' }), null).rule), 'allow 1-1024');
  assert.deepEqual(ruleOfForm(form({ ports: '70000' }), null), { field: 'ports', why: '`70000` is not a port, 1 to 65535, or a range of them' });
  assert.deepEqual(ruleOfForm(form({ ports: '' }), null), { field: 'ports', why: 'Name a port or a range of ports, as 22 or 8000-8100.' });
  assert.deepEqual(ruleOfForm(form({ ports: '22 80' }), null).field, 'ports');
  assert.deepEqual(ruleOfForm(form({ from: { kind: 'address', text: 'laptop' } }), null), { field: 'from', why: '`laptop` is not an address or network' });
  assert.deepEqual(ruleOfForm(form({ from: { kind: 'address', text: '' } }), null), { field: 'from', why: 'Write an address or a network, such as 192.168.1.0/24.' });
  assert.equal(ruleOfForm(form({ from: { kind: 'network', text: '' } }), null).field, 'from', 'no network of this card is known');
  assert.deepEqual(ruleOfForm(form({ on: 'net 0' }), null), { field: 'on', why: '`net 0` is not an interface\'s name' });
});

test('a rule opened for Change fills the form, and "this network" is told from an address', () => {
  assert.deepEqual(formOfRule(rule('allow tcp 8000-8100 from 192.168.1.0/24 on net0'), '192.168.1.0/24'), {
    action: 'allow', protocol: 'tcp', ports: '8000-8100', from: { kind: 'network', text: '' }, on: 'net0',
  });
  assert.deepEqual(formOfRule(rule('deny ping from 10.0.0.1'), '192.168.1.0/24'), {
    action: 'deny', protocol: 'ping', ports: '', from: { kind: 'address', text: '10.0.0.1' }, on: '',
  });
  assert.deepEqual(formOfRule(rule('deny udp 53'), null).from, { kind: 'anywhere', text: '' });
  const form = formOfRule(rule('allow tcp 22 from 10.0.0.0/8'), null);
  assert.equal(ruleText(ruleOfForm(form, null).rule), 'allow tcp 22 from 10.0.0.0/8', 'the form makes the rule it was opened for');
});

test('the actions take a rule\'s fields, a replace takes two with old_ and new_, and the trial\'s seconds', () => {
  const tcp = rule('allow tcp 443 from 192.168.1.0/24 on net0');
  assert.deepEqual(argsOfRule(tcp), { action: 'allow', protocol: 'tcp', ports: '443', from: '192.168.1.0/24', on: 'net0' });
  assert.deepEqual(argsOfRule(rule('deny ping')), { action: 'deny', protocol: 'ping' });
  assert.deepEqual(actionOf({ kind: 'add', rule: rule('allow tcp 7') }), { action: 'firewall.add', args: { action: 'allow', protocol: 'tcp', ports: '7' } });
  assert.deepEqual(actionOf({ kind: 'add', rule: rule('allow tcp 7') }, 60).args.try, 60);
  assert.deepEqual(actionOf({ kind: 'remove', old: rule('allow tcp 7') }, 120), { action: 'firewall.remove', args: { action: 'allow', protocol: 'tcp', ports: '7', try: 120 } });
  assert.deepEqual(actionOf({ kind: 'replace', old: rule('allow tcp 7'), rule: rule('deny udp 8 on net1') }, 300), {
    action: 'firewall.replace',
    args: {
      old_action: 'allow', old_protocol: 'tcp', old_ports: '7', new_action: 'deny', new_protocol: 'udp', new_ports: '8', new_on: 'net1', try: 300,
    },
  });
});

test('a rule that names a program is not one the page changes', () => {
  assert.equal(canChange(rule('allow tcp 22')), true);
  assert.equal(canChange(rule('allow program /x')), false);
});

test('"this network" is the network of the card the browser came through that holds its address', () => {
  const show = { cards: [{ name: 'net1', addresses: [] }, { name: 'net0', addresses: [{ address: '192.168.1.10', prefix: 24, from: 'dhcp' }] }] };
  assert.equal(hereOf(show, SESSION), '192.168.1.0/24');
  assert.equal(hereOf(show, { via: { address: '192.168.1.99', card: 'net0' } }), null);
  assert.equal(hereOf(show, { via: { address: '192.168.1.10', card: 'wg0' } }), null);
  assert.equal(hereOf({}, SESSION), null);
  assert.equal(hereOf(show, null), null);
});
