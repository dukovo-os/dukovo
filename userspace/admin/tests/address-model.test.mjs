// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// IP addresses and networks as the commands read and write them: what an
// address is, how it is printed, and which network holds it.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addressOfHost, formatAddress, formatNetwork, networkOf, parseAddress, parseNetwork, sameAddress, within,
} from '../lib/address-model.js';

const round = (text) => {
  const address = parseAddress(text);
  return address === null ? null : formatAddress(address);
};

test('an IPv4 address is four numbers to 255 with no leading zero', () => {
  assert.equal(round('192.168.1.10'), '192.168.1.10');
  assert.equal(round('0.0.0.0'), '0.0.0.0');
  for (const text of ['', '1.2.3', '1.2.3.4.5', '256.1.1.1', '01.2.3.4', '1.2.3.-4', '1.2.3.x', ' 1.2.3.4', '1.2.3.4 ', '1..3.4', '300.1.1.1']) {
    assert.equal(parseAddress(text), null, text);
  }
});

test('an IPv6 address has up to eight groups, one :: and perhaps an IPv4 end, and is written as Rust writes it', () => {
  const same = {
    '2001:db8::15': '2001:db8::15',
    '2001:DB8:0:0:0:0:0:15': '2001:db8::15',
    '::': '::',
    '::1': '::1',
    '1::': '1::',
    'fe80::5054:ff:fe12:3456': 'fe80::5054:ff:fe12:3456',
    '2001:db8:0:0:1:0:0:1': '2001:db8::1:0:0:1',
    '2001:0:0:1:0:0:0:1': '2001:0:0:1::1',
    '1:0:2:0:3:0:4:0': '1:0:2:0:3:0:4:0',
    '::ffff:192.168.1.20': '::ffff:192.168.1.20',
    '::ffff:c0a8:114': '::ffff:192.168.1.20',
    '64:ff9b::1.2.3.4': '64:ff9b::102:304',
    '1:2:3:4:5:6:7::': '1:2:3:4:5:6:7:0',
    '::2:3:4:5:6:7:8': '0:2:3:4:5:6:7:8',
  };
  for (const [text, written] of Object.entries(same)) assert.equal(round(text), written, text);
  for (const text of ['1::2::3', ':1:2:3:4:5:6:7', '1:2:3:4:5:6:7', '1:2:3:4:5:6:7:8:9', '12345::1', ':::', '1:2:3:4:5:6:7:8::', 'g::1', '1.2.3.4::1', '::1.2.3', '1::1.2.3.4:5', 'fe80::1%eth0']) {
    assert.equal(parseAddress(text), null, text);
  }
});

test('an address is told from a name in what a browser gives as its host', () => {
  assert.equal(addressOfHost('192.168.1.10').version, 4);
  assert.equal(addressOfHost('[fe80::1]').version, 6);
  assert.equal(addressOfHost('2001:db8::1').version, 6);
  assert.equal(addressOfHost('duke-nas.local'), null);
  assert.equal(addressOfHost('localhost'), null);
  assert.equal(addressOfHost(undefined), null);
});

test('a network is an address and a prefix, the whole length for an address alone, and is written as a rule writes it', () => {
  assert.equal(formatNetwork(parseNetwork('192.168.1.0/24')), '192.168.1.0/24');
  assert.equal(formatNetwork(parseNetwork('192.168.1.9/32')), '192.168.1.9');
  assert.equal(formatNetwork(parseNetwork('192.168.1.9')), '192.168.1.9');
  assert.equal(formatNetwork(parseNetwork('2001:DB8::/32')), '2001:db8::/32');
  assert.equal(formatNetwork(parseNetwork('::1')), '::1');
  assert.equal(parseNetwork('0.0.0.0/0').prefix, 0);
  for (const text of ['192.168.1.0/33', '2001:db8::/129', '192.168.1.0/', '192.168.1.0/x', '192.168.1.0/24/1', 'host/24', '/24']) {
    assert.equal(parseNetwork(text), null, text);
  }
});

test('an address is within a network of its own kind only', () => {
  const inside = (address, network) => within(parseAddress(address), parseNetwork(network));
  assert.equal(inside('192.168.1.9', '192.168.1.0/24'), true);
  assert.equal(inside('192.168.2.9', '192.168.1.0/24'), false);
  assert.equal(inside('10.1.2.3', '0.0.0.0/0'), true);
  assert.equal(inside('192.168.1.9', '192.168.1.9'), true);
  assert.equal(inside('192.168.1.10', '192.168.1.9'), false);
  assert.equal(inside('192.168.1.255', '192.168.1.128/25'), true);
  assert.equal(inside('192.168.1.127', '192.168.1.128/25'), false);
  assert.equal(inside('2001:db8::9', '2001:db8::/32'), true);
  assert.equal(inside('2001:db9::9', '2001:db8::/32'), false);
  assert.equal(inside('fe80::1', '::/0'), true);
  assert.equal(inside('::ffff:192.168.1.9', '192.168.1.0/24'), false);
  assert.equal(inside('192.168.1.9', '::/0'), false);
});

test('two addresses are the same when their kind and bytes are', () => {
  assert.equal(sameAddress(parseAddress('2001:db8::1'), parseAddress('2001:0db8:0:0:0:0:0:1')), true);
  assert.equal(sameAddress(parseAddress('192.168.1.1'), parseAddress('::ffff:192.168.1.1')), false);
});

test('the network that holds an address has its host bits cleared', () => {
  const first = (text) => formatNetwork(networkOf(parseNetwork(text)));
  assert.equal(first('192.168.1.10/24'), '192.168.1.0/24');
  assert.equal(first('192.168.1.10/20'), '192.168.0.0/20');
  assert.equal(first('10.1.2.3/8'), '10.0.0.0/8');
  assert.equal(first('10.1.2.3/32'), '10.1.2.3');
  assert.equal(first('10.1.2.3/0'), '0.0.0.0/0');
  assert.equal(first('2001:db8:1:2::15/64'), '2001:db8:1:2::/64');
  assert.equal(first('2001:db8:ffff::1/33'), '2001:db8:8000::/33');
});
