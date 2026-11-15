// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Networking page says and decides (design 9): the editor's checks,
// which say what `network set` says (every line of the file that Rust's tests
// read too), the form's arguments, whether a change cuts the browser off, and
// the words of the cards and connections.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  argsOf, cardRow, checkSet, connectionRows, cutsOff, cutWords, endWords, findCard, formOf, leaseWords, newAddressUrl,
  prefixHint, setOf, speedWords, stateOf, trafficChart, wordsOf,
} from '../pages/networking/network-model.js';

const CASES = new URL('../../programs/systools/src/network/cases.txt', import.meta.url);
const KEYS = ['router', 'dns', 'metered'];

/** The words after `network set`, as the command reads them: the card, the mode, the addresses of a fixed card, and the keys in any order. */
function setOfWords(words) {
  const [card, mode, ...rest] = words;
  const set = { card, mode, addresses: [], router: null, dns: [], metered: null };
  let at = 0;
  if (mode === 'fixed') {
    while (at < rest.length && (rest[at].includes('/') || /^[0-9a-f:.]+$/i.test(rest[at]) && !KEYS.includes(rest[at]))) {
      set.addresses.push(rest[at]);
      at += 1;
    }
  }
  while (at < rest.length) {
    const key = rest[at];
    at += 1;
    if (key === 'dns') {
      while (at < rest.length && !KEYS.includes(rest[at])) {
        set.dns.push(rest[at]);
        at += 1;
      }
    } else {
      set[key] = rest[at];
      at += 1;
    }
  }
  return set;
}

test('every case of the shared file is checked as network set checks it', () => {
  let count = 0;
  for (const line of readFileSync(CASES, 'utf8').split('\n')) {
    if (line === '' || line.startsWith('#')) continue;
    const [words, want] = line.split(' => ');
    const bad = checkSet(setOfWords(words.split(/\s+/)));
    assert.equal(bad === null ? 'ok' : `${bad.field}: ${bad.why}`, want, line);
    count += 1;
  }
  assert.ok(count >= 40, `${count} cases`);
});

/** A card as `network show --json` gives it. */
const net0 = (over = {}) => ({
  name: 'net0',
  mac: '52:54:00:12:34:56',
  up: true,
  speed_mbps: 1000,
  addresses: [
    { address: '192.168.1.10', prefix: 24, from: 'dhcp' },
    { address: 'fe80::5054:ff:fe12:3456', prefix: 64, from: 'link-local' },
  ],
  routers: ['192.168.1.1'],
  dns: ['192.168.1.1'],
  settings: { section: '52:54:00:12:34:56', mode: 'dhcp', addresses: [], router: null, dns: [], metered: 'auto', dhcp_server: false, other: ['ipv6'] },
  lease: { address: '192.168.1.10', ends_ms: 1_763_120_000_000 },
  ...over,
});

const SESSION = { from: '192.168.1.20', via: { address: '192.168.1.10', port: 9090, card: 'net0' } };

const fixed = (over = {}) => ({
  mode: 'fixed', addresses: ['192.168.1.10/24'], router: '192.168.1.1', dns: ['192.168.1.1'], metered: 'auto', ...over,
});

test('the editor starts from the settings, and a card that asks DHCP starts a fixed address at the one in use', () => {
  assert.deepEqual(formOf(net0()), {
    mode: 'dhcp', addresses: ['192.168.1.10/24'], router: '192.168.1.1', dns: ['192.168.1.1'], metered: 'auto',
  });
  const own = net0({ settings: { ...net0().settings, mode: 'fixed', addresses: ['10.0.2.15/24', '2001:db8::15/64'], router: '10.0.2.2', dns: ['10.0.2.3'], metered: 'yes' } });
  assert.deepEqual(formOf(own), {
    mode: 'fixed', addresses: ['10.0.2.15/24', '2001:db8::15/64'], router: '10.0.2.2', dns: ['10.0.2.3'], metered: 'yes',
  });
  assert.equal(formOf(net0({ settings: { ...net0().settings, mode: 'none' } })).mode, 'dhcp');
});

test('the arguments of network.set hold what the mode takes, metered only when it changes, and the trial\'s seconds', () => {
  const card = net0();
  assert.deepEqual(argsOf({ ...fixed(), mode: 'dhcp' }, card), { card: 'net0', mode: 'dhcp' });
  assert.deepEqual(argsOf(fixed({ addresses: ['10.0.2.15/24', '', ' 10.0.2.99/24 '], router: ' 10.0.2.2', dns: ['10.0.2.3, 1.1.1.1'] }), card, 120), {
    card: 'net0', mode: 'fixed', addresses: '10.0.2.15/24 10.0.2.99/24', router: '10.0.2.2', dns: '10.0.2.3 1.1.1.1', try: 120,
  });
  assert.deepEqual(argsOf(fixed({ router: '', dns: [''] }), card), { card: 'net0', mode: 'fixed', addresses: '192.168.1.10/24' });
  assert.deepEqual(argsOf({ ...fixed(), mode: 'dhcp', metered: 'yes' }, card), { card: 'net0', mode: 'dhcp', metered: 'yes' });
});

test('the checks run on the form as the command runs them, blank fields left out', () => {
  const card = net0();
  assert.equal(checkSet(setOf(fixed(), card)), null);
  assert.deepEqual(checkSet(setOf(fixed({ addresses: ['', ''] }), card)), { field: 'addresses', why: 'a card has 1 to 4 addresses of its own' });
  assert.deepEqual(checkSet(setOf(fixed({ router: '192.168.9.1' }), card)), {
    field: 'router', why: 'the router 192.168.9.1 is on none of the networks of the card\'s addresses',
  });
  assert.equal(checkSet(setOf({ ...fixed({ addresses: [''] }), mode: 'dhcp' }, card)), null, 'DHCP ignores the fixed fields');
  // A wrong address says its place among the addresses that are filled in, so that the page puts the sentence under its own field.
  assert.equal(checkSet(setOf(fixed({ addresses: ['192.168.1.10/24', '', 'nonsense/24', '10.0.0.1/8'] }), card)).at, 1);
  assert.equal(checkSet(setOf(fixed({ addresses: ['192.168.1.10/24', ' 192.168.1.10/24 '] }), card)).at, 1, 'given twice');
  assert.equal(checkSet(setOf(fixed({ addresses: ['', ''] }), card)).at, undefined, 'a count has no place');
});

test('a change that takes away the address the browser came by, or the router it is answered by, cuts it off', () => {
  const card = net0({ settings: { ...net0().settings, mode: 'fixed', addresses: ['192.168.1.10/24'], router: '192.168.1.1' } });
  assert.equal(cutsOff(fixed(), card, SESSION), null, 'the same settings');
  assert.deepEqual(cutsOff(fixed({ addresses: ['192.168.1.12/24'] }), card, SESSION), { kind: 'address', address: '192.168.1.10' });
  assert.equal(cutsOff(fixed({ addresses: ['192.168.1.12/24', '192.168.1.10/24'] }), card, SESSION), null, 'a second address beside it');
  assert.equal(cutsOff(fixed({ addresses: ['192.168.1.12/24'] }), card, { ...SESSION, via: { ...SESSION.via, card: 'net1' } }), null, 'the browser comes by another card');
  assert.equal(cutsOff(fixed({ addresses: ['192.168.1.12/24'] }), card, { from: '192.168.1.20' }), null, 'a session that says nothing of the way');
  assert.equal(cutsOff(fixed({ addresses: ['192.168.1.12/24'] }), card, { ...SESSION, via: { ...SESSION.via, card: '52:54:00:12:34:56' } })?.kind, 'address', 'a card by its MAC address');
});

test('the router is a cut only when the browser is not on the card\'s own network', () => {
  const card = net0({ settings: { ...net0().settings, mode: 'fixed', addresses: ['192.168.1.10/24'], router: '192.168.1.1' } });
  assert.equal(cutsOff(fixed({ router: '192.168.1.254' }), card, SESSION), null, 'the browser is on the network');
  const far = { ...SESSION, from: '10.8.0.5' };
  assert.deepEqual(cutsOff(fixed({ router: '192.168.1.254' }), card, far), { kind: 'router', router: '192.168.1.1' });
  assert.deepEqual(cutsOff(fixed({ router: '' }), card, far), { kind: 'router', router: '192.168.1.1' });
  assert.equal(cutsOff(fixed(), card, far), null, 'the same router');
  assert.equal(cutsOff(fixed({ dns: ['9.9.9.9'] }), card, far), null, 'a change of the name servers');
  const none = net0({ settings: { ...net0().settings, mode: 'fixed', addresses: ['192.168.1.10/24'], router: null }, routers: [] });
  assert.equal(cutsOff(fixed({ router: '192.168.1.1' }), none, far), null, 'a router added');
});

test('a change to DHCP is taken to change the address, unless the card asks DHCP already', () => {
  const dhcp = { ...fixed(), mode: 'dhcp' };
  assert.deepEqual(cutsOff(dhcp, net0({ settings: { ...net0().settings, mode: 'fixed' } }), SESSION), { kind: 'address', address: '192.168.1.10' });
  assert.equal(cutsOff({ ...dhcp, metered: 'yes' }, net0(), SESSION), null);
});

test('after the change the console is reached at the first new address of the kind it was reached by', () => {
  const place = { protocol: 'https:', port: '9090' };
  assert.equal(newAddressUrl(fixed({ addresses: ['192.168.1.20/24'] }), net0(), SESSION, place), 'https://192.168.1.20:9090/networking');
  assert.equal(newAddressUrl(fixed({ addresses: ['2001:db8::15/64', '192.168.1.20/24'] }), net0(), SESSION, place), 'https://192.168.1.20:9090/networking');
  assert.equal(newAddressUrl(fixed({ addresses: ['2001:db8::15/64'] }), net0(), { ...SESSION, via: { ...SESSION.via, address: '2001:db8::1' } }, { protocol: 'https:', port: '' }), 'https://[2001:db8::15]/networking');
  assert.equal(newAddressUrl({ ...fixed(), mode: 'dhcp' }, net0(), SESSION, place), null, 'DHCP chooses it');
  assert.equal(newAddressUrl(fixed({ addresses: ['2001:db8::15/64'] }), net0(), SESSION, place), null, 'no address of the kind');
});

test('what the page says before Try it names the address, the new place and the certificate', () => {
  const address = cutWords({ kind: 'address', address: '192.168.1.10' }, { host: '192.168.1.10', url: 'https://192.168.1.20:9090/networking' });
  assert.equal(address[0], 'This browser reaches the console at 192.168.1.10, which this change takes away.');
  assert.match(address[1], /^After Try it, open https:\/\/192\.168\.1\.20:9090\/networking in a new tab, sign in, and keep the change there\. The browser asks about the certificate again/);
  const unknown = cutWords({ kind: 'address', address: '192.168.1.10' }, { host: '192.168.1.10', url: null });
  assert.match(unknown[0], /may take away/);
  assert.match(unknown[1], /the address DHCP gives the card/);
  const named = cutWords({ kind: 'address', address: '192.168.1.10' }, { host: 'duke-nas.local', url: null });
  assert.match(named[0], /^This browser reaches the console as duke-nas\.local, which has 192\.168\.1\.10 now/);
  assert.match(cutWords({ kind: 'router', router: '192.168.1.1' }, { host: 'x', url: null })[0], /router of this card, 192\.168\.1\.1,/);
});

test('an address hint says the mask of an IPv4 prefix', () => {
  assert.equal(prefixHint('192.168.1.10/24'), '/24 is 255.255.255.0');
  assert.equal(prefixHint('10.0.0.1/8'), '/8 is 255.0.0.0');
  assert.equal(prefixHint('10.0.0.1/20'), '/20 is 255.255.240.0');
  assert.equal(prefixHint('10.0.0.1/31'), '/31 is 255.255.255.254');
  assert.equal(prefixHint('10.0.0.1/32'), '/32 is 255.255.255.255');
  for (const text of ['', '10.0.0.1', '10.0.0.1/33', '10.0.0.1/0', '2001:db8::1/64', 'x/24']) {
    assert.equal(prefixHint(text), 'Write each address with its prefix, as 192.168.1.10/24.', text);
  }
});

test('a card in words: its link, its addresses and where each came from, its lease', () => {
  const time = { when: (seconds) => `at ${seconds}` };
  const row = cardRow(net0(), SESSION, 1_763_100_000_000, time);
  assert.deepEqual(row.state, { kind: 'ok', word: 'Connected, 1 Gbit/s' });
  assert.deepEqual(row.addresses.map((one) => [one.text, one.from]), [['192.168.1.10/24', 'DHCP'], ['fe80::5054:ff:fe12:3456/64', 'link-local']]);
  assert.equal(row.mode, 'Gets its address by DHCP');
  assert.equal(row.lease, '192.168.1.10 until at 1763120000');
  assert.equal(row.through, true);
  assert.equal(row.href, '/networking/net0');
  assert.equal(cardRow(net0(), { via: { card: 'net1' } }, 0, time).through, false);
  assert.deepEqual(stateOf(net0({ up: false, speed_mbps: null })), { kind: 'warn', word: 'No cable' });
  assert.deepEqual(stateOf(net0({ speed_mbps: null })), { kind: 'ok', word: 'Connected' });
});

test('a speed, a lease and the other ways an address comes are in words', () => {
  assert.deepEqual([speedWords(10), speedWords(100), speedWords(1000), speedWords(2500), speedWords(10_000), speedWords(null), speedWords(0)],
    ['10 Mbit/s', '100 Mbit/s', '1 Gbit/s', '2.5 Gbit/s', '10 Gbit/s', '', '']);
  const time = { when: () => '3:40 PM' };
  assert.equal(leaseWords(null, 0, time), null);
  assert.equal(leaseWords({ address: '10.0.2.15', ends_ms: null }, 5, time), '10.0.2.15 for ever');
  assert.equal(leaseWords({ address: '10.0.2.15', ends_ms: 5 }, 5, time), '10.0.2.15, which has ended');
  assert.equal(leaseWords({ address: '10.0.2.15', ends_ms: 9000 }, 5, time), '10.0.2.15 until 3:40 PM');
  const held = cardRow(net0({ addresses: [{ address: '2001:db8::1', prefix: 64, from: 'router' }, { address: '10.9.9.9', prefix: 8, from: 'other' }, { address: '10.0.0.5', prefix: 8, from: 'fixed' }] }), null, 0, time);
  assert.deepEqual(held.addresses.map((one) => one.from), ['IPv6, from the router', 'set another way', 'fixed']);
});

test('a card is found by its name or its MAC address', () => {
  const show = { cards: [net0(), net0({ name: 'net1', mac: '52:54:00:12:34:57' })] };
  assert.equal(findCard(show, 'net1').mac, '52:54:00:12:34:57');
  assert.equal(findCard(show, '52:54:00:12:34:56').name, 'net0');
  assert.equal(findCard(show, 'wg0'), null);
  assert.equal(findCard({}, 'net0'), null);
});

test('traffic is one card\'s received and sent series, in bits', () => {
  assert.deepEqual(trafficChart('net0').lines.map((line) => line.series), ['net.rx{net0}', 'net.tx{net0}']);
  assert.equal(trafficChart('net0').kind, 'bits');
});

test('connections are the rows of ss, by local port and the other end, and the browser\'s own is marked', () => {
  const end = (address, port) => ({ address, port });
  const rows = connectionRows([
    { tcp: true, state: 'established', local: end('192.168.1.10', 9090), remote: end('192.168.1.20', 51334), send_queue: 0, receive_queue: 0 },
    { tcp: true, state: 'established', local: end('192.168.1.10', 22), remote: end('192.168.1.30', 40000), send_queue: 0, receive_queue: 0 },
    { tcp: true, state: 'close-wait', local: end('fe80::1', 9090), remote: end('fe80::2', 5), send_queue: 0, receive_queue: 0 },
    { tcp: false, state: 'unconn', local: end('0.0.0.0', 53), remote: end('0.0.0.0', 0), send_queue: 0, receive_queue: 0 },
  ], SESSION);
  assert.deepEqual(rows.map((row) => [row.local, row.remote, row.state, row.thisBrowser]), [
    ['192.168.1.10:22', '192.168.1.30:40000', 'Established', false],
    ['192.168.1.10:9090', '192.168.1.20:51334', 'Established', true],
    ['[fe80::1]:9090', '[fe80::2]:5', 'Close wait', false],
  ]);
  assert.deepEqual(connectionRows(null, SESSION), []);
  assert.equal(endWords({ address: '0.0.0.0', port: 0 }), '0.0.0.0:*');
});

test('words are split at spaces and commas', () => {
  assert.deepEqual(wordsOf('1.1.1.1, 8.8.8.8  9.9.9.9'), ['1.1.1.1', '8.8.8.8', '9.9.9.9']);
  assert.deepEqual(wordsOf(null), []);
});
