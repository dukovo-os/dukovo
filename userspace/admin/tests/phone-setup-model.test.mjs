// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the code for a phone holds, and when the page draws the steps: the
// address with the machine's own certificate and with another, the
// fingerprints compared with and without colons, an IPv6 machine, the port,
// and whether push can work there.

import assert from 'node:assert/strict';
import test from 'node:test';

import { PHONE_DONE_KEY, wantsCard, wantsPanel } from '../pages/alerts/phone-flag.js';
import {
  addressOf, certificateWords, fingerprintGroups, firstAddress, isNamedHost, servesOwnCertificate,
} from '../pages/alerts/phone-setup-model.js';
import { qrMatrix } from '../qr.js';

const FINGERPRINT = 'AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99';
const about = (more = {}) => ({
  host: 'duke-nas', addresses: ['10.0.0.5', 'fd00::5'], port: 8443, fingerprint: FINGERPRINT, ...more,
});
const machine = (fingerprint = FINGERPRINT) => ({ name: 'this-machine', fingerprint_sha256: fingerprint });

test('the fingerprints are compared with colons and case left out', () => {
  assert.equal(servesOwnCertificate(FINGERPRINT, FINGERPRINT.toLowerCase().replaceAll(':', '')), true);
  assert.equal(servesOwnCertificate(FINGERPRINT, FINGERPRINT.replace('AA', 'AB')), false);
  assert.equal(servesOwnCertificate(null, null), false, 'two that are not known are not the same');
  assert.equal(servesOwnCertificate(FINGERPRINT, undefined), false);
});

test('with the machine\'s own certificate the code holds the first private address, even when reached by a name', () => {
  const found = addressOf({ about: about(), machine: machine(), hostname: 'duke-nas.example.org' });
  assert.equal(found.url, 'https://10.0.0.5:8443/alerts?phone=1');
  assert.equal(found.own, true);
  assert.equal(found.push, false);
  assert.equal(found.named, false);
});

test('with another certificate reached by a name, the code holds that name and push can work', () => {
  const found = addressOf({ about: about(), machine: machine('00:11'), hostname: 'duke-nas.example.org' });
  assert.equal(found.url, 'https://duke-nas.example.org:8443/alerts?phone=1');
  assert.deepEqual([found.own, found.named, found.push], [false, true, true]);
});

test('another certificate reached by an address, `localhost` or a .local name holds the address, and push cannot work', () => {
  for (const hostname of ['10.0.0.5', 'localhost', 'duke-nas.local', 'DUKE-NAS.LOCAL', '[fd00::5]', '']) {
    const found = addressOf({ about: about(), machine: machine('00:11'), hostname });
    assert.equal(found.url, 'https://10.0.0.5:8443/alerts?phone=1', hostname);
    assert.equal(found.push, false, hostname);
    assert.equal(found.own, false, hostname);
  }
});

test('a certificate that cannot be compared holds the address and does not promise push', () => {
  const found = addressOf({ about: about(), machine: null, hostname: 'duke-nas.example.org' });
  assert.equal(found.url, 'https://10.0.0.5:8443/alerts?phone=1');
  assert.deepEqual([found.own, found.push], [false, false]);
});

test('a machine with only an IPv6 address has it in brackets, and the port is the console\'s', () => {
  assert.equal(addressOf({ about: about({ addresses: ['fd00::5'], port: 443 }), machine: machine(), hostname: 'x' }).url, 'https://[fd00::5]:443/alerts?phone=1');
  assert.equal(addressOf({ about: about({ addresses: ['2001:db8::7'], port: 8080 }), machine: machine(), hostname: 'x' }).url, 'https://[2001:db8::7]:8080/alerts?phone=1');
  assert.equal(addressOf({ about: about({ port: null }), machine: machine(), hostname: 'x' }).url, 'https://10.0.0.5/alerts?phone=1', 'no port is the default one');
});

test('the first private address is the one that a phone on this network reaches', () => {
  assert.equal(firstAddress(['203.0.113.9', '192.168.1.4', '10.0.0.5']), '192.168.1.4');
  assert.equal(firstAddress(['2001:db8::1', 'fd12:3456::1']), 'fd12:3456::1');
  assert.equal(firstAddress(['172.16.0.2']), '172.16.0.2');
  assert.equal(firstAddress(['172.32.0.2', '203.0.113.9']), '172.32.0.2', 'with none private, the first');
  assert.equal(firstAddress([]), null);
  assert.equal(firstAddress(null), null);
});

test('a machine with no address gives none', () => {
  assert.equal(addressOf({ about: about({ addresses: [] }), machine: machine(), hostname: 'x' }), null);
  assert.equal(addressOf({ about: null, machine: null, hostname: 'x' }), null);
});

test('a name is one that a certificate can name', () => {
  assert.deepEqual(['duke-nas.example.org', 'nas'].map(isNamedHost), [true, true]);
  assert.deepEqual(['10.0.0.5', '::1', 'localhost', 'x.local', '', null].map(isNamedHost), [false, false, false, false, false, false]);
});

test('the fingerprint is in groups of eight bytes, and the words say what the phone does', () => {
  assert.deepEqual(fingerprintGroups(FINGERPRINT), [
    'AA:BB:CC:DD:EE:FF:00:11', '22:33:44:55:66:77:88:99', 'AA:BB:CC:DD:EE:FF:00:11', '22:33:44:55:66:77:88:99',
  ]);
  assert.deepEqual(fingerprintGroups('aabb'), ['AA:BB']);
  assert.deepEqual(fingerprintGroups(null), []);
  assert.match(certificateWords(true), /^The phone warns that it does not know this server's certificate\./);
  assert.match(certificateWords(false), /^The phone should trust this server's certificate\. If it warns, compare/);
});

test('the panel is for an administrator who followed the code, once', () => {
  const query = (text) => new URLSearchParams(text);
  assert.equal(wantsPanel({ query: query('phone=1'), done: null, admin: true }), true);
  assert.equal(wantsPanel({ query: query('phone=1'), done: true, admin: true }), false, 'not now was pressed');
  assert.equal(wantsPanel({ query: query('phone=2'), done: null, admin: true }), false, 'any other value is ignored');
  assert.equal(wantsPanel({ query: query('phone='), done: null, admin: true }), false);
  assert.equal(wantsPanel({ query: query(''), done: null, admin: true }), false);
  assert.equal(wantsPanel({ query: query('phone=1'), done: null, admin: false }), false, 'a reader has no panel');
  assert.equal(PHONE_DONE_KEY, 'alerts.phone.done');
});

test('the card of steps is for a computer, not for the phone it is for', () => {
  assert.equal(wantsCard({ narrow: false, admin: true }), true);
  assert.equal(wantsCard({ narrow: true, admin: true }), false);
  assert.equal(wantsCard({ narrow: false, admin: false }), false);
});

test('the longest address the card makes fits one QR code at level M', () => {
  const longest = addressOf({
    about: about({ addresses: ['2001:0db8:85a3:08d3:1319:8a2e:0370:7348'], port: 65535 }), machine: machine(), hostname: 'x',
  }).url;
  assert.equal(longest, 'https://[2001:0db8:85a3:08d3:1319:8a2e:0370:7348]:65535/alerts?phone=1');
  assert.equal(longest.length, 70);
  assert.equal(qrMatrix(longest).version, 5);
});
