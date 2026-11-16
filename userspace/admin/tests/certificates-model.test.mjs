// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Certificates page says, from its model files with no document:
// the rows of the list and their words, the buttons each kind has and which
// certificates the console may serve, the history and the failure of a
// renewal, the checks before an order and the button each offers, the lines
// the rename dialog says for each address the browser may come by, and what
// the upload checks before it sends.

import assert from 'node:assert/strict';
import test from 'node:test';

import * as get from '../pages/certificates/get-model.js';
import * as past from '../pages/certificates/history-model.js';
import * as model from '../pages/certificates/model.js';
import * as name from '../pages/certificates/name-model.js';
import * as upload from '../pages/certificates/upload-model.js';
import {
  about, byDns, certificate, closed, FINGERPRINT, history, list, machine, machineWithDomain, NOW, oneMissing, own, report,
  SHOWN_FINGERPRINT, time,
} from './certificates-fixtures.mjs';

const rows = () => model.rowsOf(list(), time, { serving: 'this-machine', now: NOW });
const row = (id) => rows().find((one) => one.name === id);

test('a date is written without its year in this year and with it in another', () => {
  assert.equal(model.dateWords(Date.UTC(2026, 11, 30) / 1000, time, NOW), 'Dec 30');
  assert.equal(model.dateWords(Date.UTC(2027, 0, 28) / 1000, time, NOW), 'Jan 28, 2027');
  assert.equal(model.dateWords(Number.NaN, time, NOW), '');
  assert.equal(model.secondsOf('2026-12-30T03:00:00Z'), Date.UTC(2026, 11, 30, 3) / 1000);
  assert.equal(model.secondsOf('soon'), null);
  assert.equal(model.secondsOf(undefined), null);
});

test('an issuer is a distinguished name: Let\'s Encrypt, the organisation, the common name, or this server', () => {
  const from = (issuer, more = {}) => model.fromWords(certificate({ issuer, ...more }));
  assert.deepEqual(from('C=US, O=Let\'s Encrypt, CN=R11'), { words: 'Let\'s Encrypt', note: null });
  assert.equal(from('C=US, O=(STAGING) Let\'s Encrypt, CN=(STAGING) Ersatz Edamame E1').words, 'Let\'s Encrypt (test authority)');
  assert.equal(from('O=Example Corp, CN=Example Intranet CA', { trusted: false }).words, 'Example Corp');
  assert.equal(from('O=Example Corp, CN=Example Intranet CA', { trusted: false }).note, 'Not from an authority this server trusts');
  assert.equal(from('CN=Pebble Intermediate CA 3a1f').words, 'Pebble Intermediate CA 3a1f');
  assert.equal(from('O=Example\\, Inc.+CN=CA').words, 'Example, Inc.');
  assert.equal(from('O=Caf\\C3\\A9 CA').words, 'Café CA');
  assert.equal(from('nothing to read').words, 'nothing to read');
  assert.deepEqual(model.fromWords(own()), { words: 'This server', note: null });
  assert.deepEqual(model.dnParts('C=US, O=Let\'s Encrypt'), [['C', 'US'], ['O', 'Let\'s Encrypt']]);
  assert.deepEqual(model.dnParts(undefined), []);
});

test('this server\'s own certificate ends 800 days after it was made, and says its date in full', () => {
  assert.equal(model.lifeDays(own()), model.SELF_LIFE_DAYS);
  assert.equal(model.SELF_LIFE_DAYS, 800);
  assert.deepEqual(row('this-machine').end, { date: 'Jan 13, 2029', note: null, tone: null });
  assert.equal(model.lifeDays({ not_before: 'x' }), null);
});

test('Good until says the day, then when it renews by itself or that it cannot', () => {
  assert.deepEqual(row('shop.example.org').end, { date: 'Jan 28, 2027', note: 'renews Dec 30 by itself', tone: null });
  assert.deepEqual(row('photos.example.org').end, { date: 'Nov 25', note: '11 days left, cannot renew by itself', tone: 'warn' });
  assert.deepEqual(row('intranet.example.org').end, { date: 'Jun 2, 2027', note: '200 days left, cannot renew by itself', tone: null });
  assert.deepEqual(row('blog.example.org').end, { date: 'Jan 28, 2027', note: 'revoked', tone: 'bad' });
  const ended = model.endWords(certificate({ days_left: -3, not_after: '2026-11-11T00:00:00Z', renews: undefined }), time, { now: NOW });
  assert.deepEqual(ended, { date: 'Nov 11', note: 'ended 3 days ago', tone: 'bad' });
  const last = model.endWords(certificate({ days_left: 5, renews: undefined, kind: 'added' }), time, { now: NOW });
  assert.equal(last.tone, 'bad');
  assert.equal(last.note, '5 days left, cannot renew by itself');
  assert.equal(model.endWords(certificate({ days_left: 1, renews: undefined, kind: 'added' }), time, { now: NOW }).note, '1 day left, cannot renew by itself');
});

test('a certificate from an authority that certd has no day for, near its end, is told to be one it cannot renew', () => {
  const near = certificate({ days_left: 20, renews: undefined });
  assert.equal(model.cannotRenew(near), true);
  assert.equal(model.cannotRenew(certificate({ days_left: 22, renews: undefined })), false);
  assert.equal(model.cannotRenew(certificate({ days_left: 20 })), false, 'certd has a day for it');
  assert.equal(model.cannotRenew(certificate({ days_left: 60, renews: undefined }), new Set(['shop.example.org'])), true, 'a refused renewal said it was proved by DNS');
  assert.equal(model.cannotRenew(certificate({ kind: 'added' })), true);
  assert.equal(model.cannotRenew(own()), false);
  assert.equal(model.cannotRenew(certificate({ days_left: 3, renews: undefined, revoked: true })), false);
  const proved = model.endWords(certificate({ days_left: 60, renews: undefined }), time, { now: NOW, byDns: new Set(['shop.example.org']) });
  assert.equal(proved.note, '60 days left, cannot renew by itself');
});

test('the rows have a name, how many more it holds, who made it, what uses it and its state', () => {
  assert.deepEqual(rows().map((one) => [one.name, one.kind, one.state, one.console]), [
    ['this-machine', 'machine', 'valid', true],
    ['shop.example.org', 'authority', 'valid', false],
    ['photos.example.org', 'authority', 'expiring', false],
    ['intranet.example.org', 'added', 'valid', false],
    ['blog.example.org', 'authority', 'revoked', false],
  ]);
  assert.equal(row('shop.example.org').first, 'shop.example.org');
  assert.equal(row('shop.example.org').more, 1);
  assert.equal(row('photos.example.org').more, 0);
  assert.deepEqual(row('shop.example.org').usedBy, ['nginx']);
  assert.equal(row('this-machine').unused, null);
  assert.equal(model.unusedWords(own({ used_by: [] })), 'kept for use by name on the local network');
  assert.equal(model.unusedWords(certificate({ used_by: [] })), 'no service uses it yet');
  assert.deepEqual(model.rowsOf(undefined, time), []);
  assert.deepEqual(model.rowsOf([null, { name: '' }, {}], time), []);
  assert.equal(model.serviceHref('nginx'), '/services/nginx');
});

test('the account line says when it was made and whether it has a contact, and never an address', () => {
  assert.equal(model.accountWords(list(), time, NOW), 'Let\'s Encrypt account: made on Nov 2, with no contact email');
  const withContact = [certificate({ account: { made_ms: Date.UTC(2026, 10, 2), contact: true } })];
  assert.equal(model.accountWords(withContact, time, NOW), 'Let\'s Encrypt account: made on Nov 2, with a contact email');
  assert.equal(model.accountWords([own()], time, NOW), null);
  assert.equal(model.accountWords(undefined, time, NOW), null);
});

test('a fingerprint is shown as browsers show it', () => {
  assert.equal(model.fingerprintWords(FINGERPRINT), SHOWN_FINGERPRINT);
  assert.equal(model.fingerprintWords('ab:cd:ef'), 'AB:CD:EF');
  assert.equal(model.fingerprintWords(undefined), '');
});

test('a certificate\'s name covers an address as a browser checks it', () => {
  assert.equal(model.covers('duke-nas.example.org', 'DUKE-NAS.example.org'), true);
  assert.equal(model.covers('duke-nas.example.org', 'duke-nas.example.org.'), true);
  assert.equal(model.covers('192.168.1.10', '192.168.1.10'), true);
  assert.equal(model.covers('192.168.1.10', '192.168.1.11'), false);
  assert.equal(model.covers('2001:db8::5', '[2001:0db8:0:0:0:0:0:5]'), true);
  assert.equal(model.covers('*.example.org', 'nas.example.org'), true);
  assert.equal(model.covers('*.example.org', 'example.org'), false);
  assert.equal(model.covers('*.example.org', 'a.nas.example.org'), false);
  assert.equal(model.covers('*.example.org', '192.168.1.10'), false);
  assert.equal(model.covers('example.org', 'nas.example.org'), false);
  assert.equal(model.covers('', 'x'), false);
  assert.equal(model.covers('x', ''), false);
  assert.equal(model.namesCover(['a.example.org', 'b.example.org'], 'b.example.org'), true);
  assert.equal(model.namesCover(undefined, 'b.example.org'), false);
});

test('Use for this console is offered where the certificate names the address this browser comes by', () => {
  const where = { host: '192.168.1.10', full: 'duke-nas.example.org', port: 9090, serving: 'this-machine' };
  const offer = (certificateRow, more = {}) => model.useOffer(certificateRow, { ...where, ...more });
  // A certificate that names the browser's own address.
  const named = certificate({ names: ['192.168.1.10', 'shop.example.org'] });
  assert.deepEqual(offer(named), { offer: 'use', words: 'Use for this console' });
  // One that names the machine's full name: the browser has to be there first.
  const full = offer(certificate({ names: ['duke-nas.example.org'] }));
  assert.equal(full.offer, 'open');
  assert.equal(full.url, 'https://duke-nas.example.org:9090/');
  assert.equal(full.words, 'Open the console at https://duke-nas.example.org:9090/ and choose it there');
  // One that names neither.
  const none = offer(certificate());
  assert.equal(none.offer, 'none');
  assert.match(none.words, /^This certificate does not name this machine\. Give the machine that name first \(Name and domain, below\)/);
  // This server's own one is offered again once the console serves another.
  assert.equal(offer(own(), { serving: 'shop.example.org' }).offer, 'again');
  assert.equal(offer(own(), { serving: 'shop.example.org' }).words, 'Use this server\'s own certificate again');
  // The one it serves, a revoked one and an ended one are not offered.
  assert.equal(offer(own()).offer, null);
  assert.equal(offer(certificate({ revoked: true, names: ['192.168.1.10'] })).offer, null);
  assert.equal(offer(certificate({ days_left: -1, names: ['192.168.1.10'] })).offer, null);
  // On the standard port the address has no port.
  assert.equal(offer(certificate({ names: ['duke-nas.example.org'] }), { port: 443 }).url, 'https://duke-nas.example.org/');
  assert.equal(model.servingWords('shop.example.org'), 'This console now serves shop.example.org\'s certificate. Keep it once this page has come back on it.');
});

test('each kind of row has the buttons it should, and a reader only looks', () => {
  const of = (certificateRow, more = {}) => model.actionsOf(certificateRow, { admin: true, serving: 'this-machine', ...more });
  assert.deepEqual(of(own()), { buttons: ['show', 'make-anew'], more: [], removeNote: null });
  assert.deepEqual(of(own(), { serving: 'shop.example.org' }).more, ['again']);
  assert.deepEqual(of(certificate()).buttons, ['renew']);
  assert.deepEqual(of(certificate()).more, ['show', 'use', 'revoke'], 'nginx uses it, so it has no Remove');
  assert.equal(of(certificate()).removeNote, 'Remove is offered when no service uses it: nginx does.');
  assert.equal(of(certificate({ used_by: ['nginx', 'blog'] })).removeNote, 'Remove is offered when no service uses it: nginx, blog do.');
  assert.deepEqual(of(certificate({ used_by: [] })).more, ['show', 'use', 'revoke', 'remove']);
  assert.deepEqual(of(certificate({ used_by: [] }), { byDns: new Set(['shop.example.org']) }).buttons, ['renew-dns']);
  assert.deepEqual(of(certificate({ kind: 'added', used_by: [] })), { buttons: ['replace'], more: ['show', 'use', 'remove'], removeNote: null });
  assert.deepEqual(of(certificate({ kind: 'added', used_by: ['nginx'] })).more, ['show', 'use']);
  assert.deepEqual(of(certificate({ revoked: true, used_by: [] })), { buttons: ['show'], more: ['remove'], removeNote: null });
  assert.deepEqual(of(certificate({ used_by: [] }), { serving: 'shop.example.org' }).more, ['show', 'revoke', 'remove']);
  assert.deepEqual(model.actionsOf(certificate(), { admin: false, serving: null }), { buttons: ['show'], more: [], removeNote: null });
  for (const id of ['show', 'make-anew', 'renew', 'renew-dns', 'replace', 'use', 'again', 'revoke', 'remove']) assert.ok(model.ACTION_WORDS[id], id);
  assert.equal(model.ACTION_WORDS.show, 'Show fingerprint');
  assert.deepEqual(model.REVOKE_REASONS.map((one) => one.id), ['unspecified', 'key-compromise', 'superseded', 'cessation-of-operation']);
});

test('the line under the head is certd\'s, and says ARI and 14 days', () => {
  assert.match(model.CERTD_WORDS, /renews each Let's Encrypt certificate on the day the authority suggests \(ARI\), or when two thirds of its life are gone, and warns you from 14 days before/);
  assert.equal(model.suggestedDomain(machineWithDomain() && { first: 'duke-nas', domain: 'example.org' }), 'duke-nas.example.org');
  assert.equal(model.suggestedDomain({ first: 'duke-nas', domain: null }), null);
});

test('the history is told newest first, and a failure and the success that followed are one line', () => {
  const lines = past.linesOf(history(), { exists: new Set(['shop.example.org', 'photos.example.org']), zone: 'UTC' });
  assert.deepEqual(lines.map((line) => [line.name, line.words]), [
    ['photos.example.org', 'renewal failed'],
    ['photos.example.org', 'renewed'],
    ['shop.example.org', 'renewal failed'],
    ['this-machine', 'made by this server'],
  ]);
  const shop = lines.find((line) => line.name === 'shop.example.org');
  assert.equal(shop.detail, 'Port 80 was closed');
  assert.equal(shop.id, 'CRT-103');
  assert.equal(shop.then, 'Tried again on its own and succeeded the next day.');
  assert.equal(shop.tone, 'bad');
  const photos = lines.find((line) => line.words === 'renewed');
  assert.equal(photos.detail, 'from Let\'s Encrypt, 4 seconds');
  assert.equal(photos.then, null);
});

test('a failure is not joined to a success a week later, nor over another failure, and a person\'s retry says so', () => {
  const at = (day, nameOf, kind, by = 'certd') => ({
    at_ms: Date.UTC(2026, 9, day), name: nameOf, event: kind, detail: 'x', took_ms: null, by,
  });
  const apart = past.linesOf([at(1, 'a', 'failed'), at(9, 'a', 'renewed')], { zone: 'UTC' });
  assert.equal(apart.length, 2);
  assert.equal(apart[1].then, null);
  const twice = past.linesOf([at(1, 'a', 'failed'), at(2, 'a', 'failed'), at(3, 'a', 'renewed')], { zone: 'UTC' });
  assert.deepEqual(twice.map((line) => line.then), ['Tried again on its own and succeeded the next day.', null]);
  const byHand = past.linesOf([at(1, 'a', 'failed'), { ...at(1, 'a', 'renewed', 'cert'), at_ms: Date.UTC(2026, 9, 1, 23) }], { zone: 'UTC' });
  assert.equal(byHand.length, 1);
  assert.equal(byHand[0].then, 'Renewed by hand the same day.');
  const later = past.linesOf([at(1, 'a', 'failed'), at(4, 'a', 'got')], { zone: 'UTC' });
  assert.equal(later[0].then, 'Tried again on its own and succeeded 3 days later.');
  assert.equal(past.linesOf([at(1, 'a', 'failed')], { zone: 'UTC' })[0].words, 'getting it failed');
  assert.deepEqual(past.linesOf(undefined), []);
  assert.deepEqual(past.linesOf([{ name: 5 }, null]), []);
});

test('the history shows 90 days until Show all', () => {
  const lines = past.linesOf(history(), { zone: 'UTC' });
  assert.equal(past.shownLines(lines, true, NOW).length, 4);
  assert.equal(past.shownLines(lines, false, NOW + 60 * 86_400_000).length, 3, 'the line made 100 days before is not shown');
  assert.equal(past.shownLines(lines, false, NOW).length, 4);
});

test('an ID at the end of a detail is split from its words, and a time is in words', () => {
  assert.deepEqual(past.splitId('Port 80 was closed (CRT-103)'), { words: 'Port 80 was closed', id: 'CRT-103' });
  assert.deepEqual(past.splitId('no id here'), { words: 'no id here', id: null });
  assert.deepEqual(past.splitId(undefined), { words: '', id: null });
  assert.equal(past.tookWords(4000), '4 seconds');
  assert.equal(past.tookWords(300), '1 second');
  assert.equal(past.tookWords(65_000), '1 minute 5 seconds');
  assert.equal(past.tookWords(120_000), '2 minutes');
  assert.equal(past.tookWords(null), null);
});

test('the newest failure of a certificate is its row\'s, until something else happens to it', () => {
  const failures = past.failuresOf(history());
  assert.deepEqual([...failures.keys()], ['photos.example.org']);
  assert.deepEqual(failures.get('photos.example.org'), { at: Date.UTC(2026, 10, 14, 6) / 1000, words: 'Let\'s Encrypt could not reach port 80', id: 'CRT-106' });
  assert.equal(past.failureWords(failures.get('photos.example.org'), time.when), 'renewal failed today 6:00 AM, certd tries again at its next look');
  assert.equal(past.failureWords({ at: Date.UTC(2026, 10, 13, 19, 40) / 1000 }, time.when), 'renewal failed yesterday 7:40 PM, certd tries again at its next look');
  assert.equal(past.failureWords({ at: Date.UTC(2026, 10, 9, 19, 40) / 1000 }, time.when), 'renewal failed Nov 9 7:40 PM, certd tries again at its next look');
  const mended = [...history(), { at_ms: NOW, name: 'photos.example.org', event: 'renewed', detail: '', took_ms: null, by: 'cert' }];
  assert.equal(past.failuresOf(mended).size, 0);
  const rowsWith = model.rowsOf(list(), time, { failures, now: NOW });
  assert.equal(rowsWith.find((one) => one.name === 'photos.example.org').failure.id, 'CRT-106');
  assert.equal(rowsWith.find((one) => one.name === 'shop.example.org').failure, null);
});

test('names are read from what is typed, in small letters and once each', () => {
  assert.deepEqual(get.namesOf('Shop.example.org  www.shop.example.org,shop.example.org\nx.example.org'), ['shop.example.org', 'www.shop.example.org', 'x.example.org']);
  assert.deepEqual(get.namesOf(''), []);
  assert.deepEqual(get.namesOf(undefined), []);
  assert.equal(get.certName(['shop.example.org', 'www.shop.example.org']), 'shop.example.org');
  assert.equal(get.certName(['*.shop.example.org']), 'shop.example.org');
  assert.equal(get.certName([]), null);
  assert.equal(get.certName([`${'a'.repeat(60)}.example.org`]), null);
});

test('a name that cannot be asked for is refused before anything is sent', () => {
  assert.equal(get.namesProblem(['shop.example.org', 'www.shop.example.org'], false), '');
  assert.match(get.namesProblem([], false), /^Give the name of the site/);
  assert.match(get.namesProblem(Array.from({ length: 21 }, (_, at) => `n${at}.example.org`), false), /1 to 20 names, not 21/);
  assert.match(get.namesProblem(['*.example.org'], false), /is a wildcard, which Let's Encrypt proves only through DNS/);
  assert.equal(get.namesProblem(['*.example.org'], true), '');
  assert.match(get.namesProblem(['192.168.1.10'], false), /is an IP address/);
  assert.match(get.namesProblem(['duke-nas'], false), /is not a name Let's Encrypt issues for/);
  assert.match(get.namesProblem(['-x.example.org'], false), /is not a name/);
  assert.match(get.namesProblem(['sh_op.example.org'], false), /is not a name/);
});

test('the checks are read from an address of the names, and the certificate\'s name when it is not the first', () => {
  assert.equal(get.checkPath(['shop.example.org', 'www.shop.example.org'], false), '/api/1/certificates/check/shop.example.org%2Cwww.shop.example.org');
  assert.equal(get.checkPath(['*.shop.example.org', 'shop.example.org'], true), '/api/1/certificates/check/*.shop.example.org%2Cshop.example.org?as=shop.example.org&dns=true');
  assert.deepEqual(get.checkArgs(['shop.example.org'], true), { names: 'shop.example.org', dns: true });
  assert.deepEqual(get.checkArgs(['*.shop.example.org'], false), { names: '*.shop.example.org', as: 'shop.example.org' });
  assert.equal(get.isReport(report()), true);
  assert.equal(get.isReport(null), false);
  assert.equal(get.isReport({ checks: 'x' }), false);
});

test('each check is told in the page\'s words, with the button it offers', () => {
  const words = (checkReport) => get.checkRows(checkReport, false).map((one) => [one.tone, one.id, one.words, one.button]);
  assert.deepEqual(words(report()), [
    ['ok', null, 'shop.example.org leads to 203.0.113.10', null],
    ['ok', null, 'www.shop.example.org leads to 203.0.113.10', null],
    ['ok', null, 'Port 80 is open (firewall rule 3)', null],
    ['ok', null, 'nginx holds port 80 and serves the answer', null],
  ]);
  // CRT-103: the firewall, with the button that opens port 80.
  assert.deepEqual(words(closed())[1], ['bad', 'CRT-103', 'The firewall closes port 80, which Let\'s Encrypt must reach', 'open-port']);
  assert.equal(words(closed())[2][2], 'Port 80 is free: the order answers Let\'s Encrypt itself');
  // CRT-101 and CRT-102: a name with no address, and one with a private one.
  assert.deepEqual(words(oneMissing())[1], ['bad', 'CRT-101', 'www.blog.example.org does not resolve to an address: give it an A record (and AAAA for IPv6) that leads to this server', null]);
  const privateName = report({ checks: [{ id: 'CRT-102', result: 'failed', name: 'nas.example.org', words: 'nas.example.org leads to 192.168.1.20, a private address the authority cannot reach: give it this machine\'s public address, or use --dns' }] });
  assert.deepEqual(words(privateName)[0], ['bad', 'CRT-102', 'nas.example.org leads to 192.168.1.20, a private address Let\'s Encrypt cannot reach: give it this server\'s public address, or prove it by DNS', null]);
  // CRT-104: a warning, in both of its forms.
  const other = report({ checks: [{
    id: 'CRT-104', result: 'warning', name: 'www.blog.example.org', words: 'www.blog.example.org\'s AAAA record points to 2001:db8::5, which is not this machine (it has 2001:db8:42::10, 2001:db8:42::11): the authority tries IPv6 first, so it would check that machine: remove the AAAA record or point it to 2001:db8:42::10',
  }] });
  assert.deepEqual(words(other)[0], ['warn', 'CRT-104', 'www.blog.example.org\'s AAAA record points to 2001:db8::5, which is not this server (it has 2001:db8:42::10, 2001:db8:42::11). Let\'s Encrypt tries IPv6 first, so it would check that machine. Remove the AAAA record or point it to 2001:db8:42::10', null]);
  const none = report({ checks: [{ id: 'CRT-104', result: 'warning', name: 'www.blog.example.org', words: 'www.blog.example.org has an IPv6 address and this machine has none: if 2001:db8::5 is not this machine, the authority, which tries IPv6 first, may not reach it' }] });
  assert.equal(words(none)[0][2], 'www.blog.example.org has an IPv6 address, 2001:db8::5, and this server has none: if it is not this server, Let\'s Encrypt, which tries IPv6 first, may not reach it');
  // CRT-105: the clock.
  const clock = report({ checks: [{ id: 'CRT-105', result: 'failed', name: null, words: 'the clock is wrong (1970-01-01): set it, then try again' }] });
  assert.deepEqual(words(clock)[0], ['bad', 'CRT-105', 'The clock is wrong (1970-01-01): it must be right first', null]);
  // CRT-106: a program holds port 80.
  const held = report({ checks: [{ id: 'CRT-106', result: 'failed', name: null, words: 'port 80 is held by apache2, which is not nginx or certd: stop it for the few minutes the order takes, or use --dns' }], port80_holder: 'apache2' });
  assert.deepEqual(words(held)[0], ['bad', 'CRT-106', 'Port 80 is held by apache2, which is not nginx or certd: stop it for the few minutes the order takes, or prove the names by DNS', null]);
  // Open with no rule, and the warnings that have no ID.
  assert.equal(words(report({ checks: [{ id: null, result: 'passed', name: null, words: 'port 80 is open (no firewall rule closes it)' }], firewall_rule: null }))[0][2], 'Port 80 is open (no firewall rule closes it)');
  const certd = report({ checks: [{ id: null, result: 'warning', name: null, words: 'certd holds port 80 while it renews a certificate: if the order fails, try again in a minute' }] });
  assert.deepEqual(words(certd)[0], ['warn', null, 'certd holds port 80 while it renews a certificate: if the order fails, try again in a minute', null]);
  assert.deepEqual(get.checkRows(null, false), []);
});

test('by DNS the checks of the names and the port are left out, and one line says what to do', () => {
  const rows = get.checkRows(byDns(), true);
  assert.deepEqual(rows.map((one) => [one.tone, one.words]), [['ok', get.DNS_LINE]]);
  assert.equal(get.DNS_LINE, 'You add a TXT record for each name at your DNS provider; this server waits up to 10 minutes for them to show.');
  assert.match(get.DNS_RENEWAL, /21 days and again 7 days/);
});

test('the button gets the certificate, or says which name it leaves out, or stays off', () => {
  const names = ['shop.example.org', 'www.blog.example.org'];
  const rowsOf = (checkReport, dns = false) => get.checkRows(checkReport, dns);
  assert.deepEqual(get.plan(rowsOf(report()), ['shop.example.org', 'www.shop.example.org']), {
    can: true, names: ['shop.example.org', 'www.shop.example.org'], dropped: [], label: 'Get the certificate', why: '',
  });
  const without = get.plan(rowsOf(oneMissing()), names);
  assert.equal(without.can, true);
  assert.equal(without.label, 'Get it without www.blog.example.org');
  assert.deepEqual(without.names, ['shop.example.org']);
  assert.deepEqual(without.dropped, ['www.blog.example.org']);
  const firewall = get.plan(rowsOf(closed()), ['shop.example.org']);
  assert.equal(firewall.can, false);
  assert.equal(firewall.why, 'Put right what failed first, then check again.');
  const all = get.plan(rowsOf(report({ checks: [{ id: 'CRT-101', result: 'failed', name: 'a.example.org', words: 'x' }] })), ['a.example.org']);
  assert.deepEqual([all.can, all.why], [false, 'No name passes the checks.']);
  assert.equal(get.plan(null, names).can, false);
  const two = get.plan(rowsOf(report({ checks: [
    { id: 'CRT-101', result: 'failed', name: 'a.example.org', words: 'x' }, { id: 'CRT-101', result: 'failed', name: 'b.example.org', words: 'x' },
    { id: null, result: 'passed', name: 'c.example.org', words: 'c.example.org leads to 203.0.113.10' },
  ] })), ['a.example.org', 'b.example.org', 'c.example.org']);
  assert.equal(two.label, 'Get it without a.example.org and b.example.org');
  assert.deepEqual(get.listWords(['a', 'b', 'c']), 'a, b and c');
  assert.deepEqual(get.plan(rowsOf(byDns(), true), ['shop.example.org']).can, true);
});

test('the order is made of the names a line each, the proof, and what the person agreed to', () => {
  assert.deepEqual(get.getArgs({ names: ['shop.example.org', 'www.shop.example.org'] }), {
    name: 'shop.example.org', names: 'shop.example.org\nwww.shop.example.org', agree_terms: true,
  });
  assert.deepEqual(get.getArgs({
    names: ['*.shop.example.org'], dns: true, dryRun: true, email: ' me@example.org ',
  }), {
    name: 'shop.example.org', names: '*.shop.example.org', dns: true, dry_run: true, email: 'me@example.org', agree_terms: true,
  });
  assert.equal('email' in get.getArgs({ names: ['a.example.org'], email: '  ' }), false);
  assert.deepEqual(get.openPortArgs(), { action: 'allow', protocol: 'tcp', ports: '80', try: 600 });
  assert.deepEqual(get.openPortArgs(30), { action: 'allow', protocol: 'tcp', ports: '80', try: 30 });
});

test('the terms link is one a link may go to', () => {
  assert.equal(get.termsLink(report()), 'https://letsencrypt.org/documents/LE-SA-v1.5-February-24-2025.pdf');
  assert.equal(get.termsLink(report({ terms_url: 'javascript:alert(1)' })), null);
  assert.equal(get.termsLink(report({ terms_url: null })), null);
  assert.equal(get.termsLink(null), null);
});

test('the records an order by DNS asks for are read from the lines it printed, each once', () => {
  const log = [
    'step 3 of 6: waiting for the TXT records to show',
    'add: _acme-challenge.shop.example.org TXT "abc-123_value"',
    'add: _acme-challenge.shop.example.org TXT "abc-123_value"',
    'add: _acme-challenge.www.shop.example.org TXT "def-456_value"',
    'Add a TXT record at _acme-challenge.other',
  ];
  assert.deepEqual(get.recordsOf(log), [
    { name: '_acme-challenge.shop.example.org', value: 'abc-123_value' },
    { name: '_acme-challenge.www.shop.example.org', value: 'def-456_value' },
  ]);
  assert.equal(get.recordLine({ name: '_acme-challenge.shop.example.org', value: 'v' }), '_acme-challenge.shop.example.org TXT "v"');
  assert.deepEqual(get.recordsOf(undefined), []);
  assert.equal(get.waitsForRecords({ words: 'waiting for the TXT records to show: looked 4 times, 1 of 2 found' }), true);
  assert.equal(get.waitsForRecords({ words: 'making a key' }), false);
});

test('how an order ended is said in words', () => {
  assert.deepEqual(get.endedWords({ state: 'done' }, { dryRun: false, name: 'shop.example.org' }), { ok: true, words: 'shop.example.org is got and installed.' });
  assert.equal(get.endedWords({ state: 'done' }, { dryRun: true, name: 'x' }).words, 'The staging authority accepted the order, so this order should work. Nothing was installed.');
  const failed = get.endedWords({ state: 'failed', note: 'the authority could not reach port 80 (CRT-106)' }, { dryRun: false, name: 'x' });
  assert.deepEqual(failed, { ok: false, words: 'The order did not finish. Failed: the authority could not reach port 80 (CRT-106).' });
  assert.equal(get.whereUsed('shop.example.org', list(), ['shop.example.org']), 'shop.example.org is used by nginx.');
  assert.equal(get.whereUsed('new.example.org', list(), ['new.example.org']), 'No nginx site answers for new.example.org yet. The certificate is kept until a site uses it.');
  assert.match(get.TRY_WORDS, /Try it first asks Let's Encrypt's staging authority, installs nothing and does not count against Let's Encrypt's limits/);
});

test('the machine is read from hostnamectl, and its full name is its name and domain', () => {
  assert.deepEqual(name.machineOf(machineWithDomain()), {
    name: 'duke-nas.example.org', first: 'duke-nas', domain: 'example.org', local: 'duke-nas.local',
  });
  assert.deepEqual(name.machineOf(machine()), {
    name: 'duke-nas', first: 'duke-nas', domain: null, local: 'duke-nas.local',
  });
  assert.equal(name.machineOf({}), null);
  assert.equal(name.machineOf(null), null);
  assert.equal(name.machineWords(name.machineOf(machineWithDomain())), 'Full name: duke-nas.example.org · On this network: duke-nas.local');
  assert.equal(name.fullNameOf('office-nas', 'example.org'), 'office-nas.example.org');
  assert.equal(name.fullNameOf('office-nas', ' .example.org. '), 'office-nas.example.org');
  assert.equal(name.fullNameOf('office-nas', ''), 'office-nas');
  assert.equal(name.localOf('Office-NAS.example.org'), 'office-nas.local');
});

test('a name follows the rules of hostname(5)', () => {
  assert.equal(name.nameProblem('office-nas', 'example.org'), '');
  assert.equal(name.nameProblem('Office1', ''), '');
  assert.equal(name.nameProblem('', ''), 'Give the machine a name.');
  assert.match(name.nameProblem('office.nas', ''), /one word: put the rest in Domain/);
  assert.match(name.nameProblem('-office', ''), /none starting or ending with -/);
  assert.match(name.nameProblem('office-', ''), /none starting or ending with -/);
  assert.match(name.nameProblem('of fice', ''), /Use letters, digits and -/);
  assert.match(name.nameProblem('office_nas', ''), /Use letters, digits and -/);
  assert.match(name.nameProblem('office', 'bad..domain'), /Use letters, digits and -/);
  assert.equal(name.nameProblem('a'.repeat(63), ''), '');
  assert.equal(name.nameProblem('a'.repeat(64), ''), 'A word of the name is 63 characters at most.');
  assert.equal(name.nameProblem('a'.repeat(30), `${'b'.repeat(30)}.com`), 'A name is 64 characters at most.');
  assert.equal(name.fullNameProblem('x'.repeat(64)), 'A word of the name is 63 characters at most.');
});

test('the dialog says four things for the address the browser comes by', () => {
  const now = name.machineOf(machineWithDomain());
  const lines = (host, more = {}) => name.renameLines(now, 'office-nas.example.org', {
    host, port: 9090, address: '192.168.1.10', ...more,
  });
  const common = [
    'The name on the local network becomes office-nas.local at once; duke-nas.local stops answering.',
    'This server makes its own certificate anew for the new name. Browsers and phones that trusted the old one warn again; its new fingerprint is shown when it is made.',
    'Certificates from Let\'s Encrypt do not change: they name the domain names you gave them.',
  ];
  // By the old local name.
  assert.deepEqual(lines('duke-nas.local'), [...common, 'You reach this console as duke-nas.local. After the change it is at https://office-nas.local:9090/; this page takes you there. If that does not answer, the console is still at https://192.168.1.10:9090/.']);
  // By the old full name: the new full name follows.
  assert.equal(lines('duke-nas.example.org')[3], 'You reach this console as duke-nas.example.org. After the change it is at https://office-nas.example.org:9090/; this page takes you there. If that does not answer, the console is still at https://192.168.1.10:9090/.');
  // By number: it keeps working.
  assert.deepEqual(lines('192.168.1.10').slice(0, 3), common);
  assert.equal(lines('192.168.1.10')[3], 'You reach this console as 192.168.1.10. That address keeps working.');
  assert.equal(lines('[2001:db8::5]')[3], 'You reach this console as [2001:db8::5]. That address keeps working.');
  // By another name.
  assert.equal(lines('nas')[3], 'You reach this console as nas. It may not answer after the change. The console is still at https://192.168.1.10:9090/.');
  // Without an address to fall back on.
  assert.equal(lines('duke-nas.local', { address: null })[3], 'You reach this console as duke-nas.local. After the change it is at https://office-nas.local:9090/; this page takes you there.');
  // The local name does not change when only the domain does.
  assert.equal(name.renameLines(now, 'duke-nas.example.com', { host: '192.168.1.10', port: 9090, address: null })[0], 'The name on the local network stays duke-nas.local.');
  // The standard port has no number.
  assert.match(name.renameLines(now, 'office-nas.example.org', { host: 'duke-nas.local', port: 443, address: '192.168.1.10' })[3], /is at https:\/\/office-nas\.local\/; this page/);
});

test('the link after the change goes to the new name, and not for a browser that came by number', () => {
  const now = name.machineOf(machineWithDomain());
  assert.deepEqual(name.followAddress('duke-nas.local', now, 'office-nas.example.org', 9090), { host: 'office-nas.local', url: 'https://office-nas.local:9090/' });
  assert.deepEqual(name.followAddress('duke-nas.example.org', now, 'office-nas.example.org', 9090), { host: 'office-nas.example.org', url: 'https://office-nas.example.org:9090/' });
  assert.equal(name.followAddress('192.168.1.10', now, 'office-nas.example.org', 9090), null);
  assert.equal(name.followAddress('nas', now, 'office-nas.example.org', 9090).host, 'office-nas.local');
  assert.equal(name.originOf('2001:db8::5', 9090), 'https://[2001:db8::5]:9090/');
  assert.equal(name.originOf('x.local', ''), 'https://x.local/');
});

test('the new fingerprint is read from what the command printed', () => {
  const lines = ['hostnamectl: the name is office-nas.example.org', `hostnamectl: this-machine's certificate now names office-nas.example.org: SHA256 ${SHOWN_FINGERPRINT.toLowerCase()}`];
  assert.equal(name.fingerprintOf(lines), SHOWN_FINGERPRINT);
  assert.equal(name.fingerprintOf(['hostnamectl: the name is x']), '');
  assert.equal(name.fingerprintOf(undefined), '');
  assert.equal(name.renamedWords('x', 'AB:CD'), 'The machine is called x now. The new fingerprint: AB:CD');
  assert.match(name.renamedWords('x', ''), /Its certificate is made anew/);
});

const KEY = '-----BEGIN PRIVATE KEY-----\nTUFSS0VS\n-----END PRIVATE KEY-----\n';
const CHAIN = '-----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nSU5URVI=\n-----END CERTIFICATE-----\n';

test('before it is sent only what the page can tell is checked: the name, a certificate, a key and the size', () => {
  assert.equal(upload.nameProblem('intranet.example.org'), '');
  assert.match(upload.nameProblem(''), /^Give the certificate a name/);
  assert.match(upload.nameProblem('Intranet'), /1 to 64 lowercase letters/);
  assert.match(upload.nameProblem('a..b'), /no two dots/);
  assert.match(upload.nameProblem('a'.repeat(65)), /1 to 64/);
  assert.equal(upload.pemProblem(CHAIN, KEY), null);
  assert.equal(upload.pemProblem(CHAIN, KEY.replace(/PRIVATE KEY/g, 'RSA PRIVATE KEY')), null);
  assert.equal(upload.pemProblem(CHAIN, KEY.replace(/PRIVATE KEY/g, 'EC PRIVATE KEY')), null);
  assert.equal(upload.pemProblem('nothing', KEY).field, 'chain');
  assert.equal(upload.pemProblem(`${CHAIN}${KEY}`, KEY).field, 'chain', 'a key in the chain box');
  assert.equal(upload.pemProblem(CHAIN, 'nothing').field, 'key');
  assert.equal(upload.pemProblem(CHAIN, CHAIN).field, 'key', 'a certificate in the key box');
  assert.equal(upload.pemProblem(`${CHAIN}\0`, KEY).field, 'chain');
  const big = `${CHAIN}${'A'.repeat(upload.PEM_MAX)}`;
  assert.match(upload.pemProblem(big, KEY).words, /64 KiB at most/);
  assert.equal(upload.PEM_MAX, 65_536);
});

test('the document is the key and then the chain, and the shown call never has it', () => {
  assert.equal(upload.documentOf(CHAIN.replace(/\n/g, '\r\n'), KEY), `${KEY}${CHAIN}`);
  assert.ok(upload.documentOf(CHAIN, KEY).startsWith('-----BEGIN PRIVATE KEY-----'));
  assert.deepEqual(upload.shownArgs({ name: 'intranet.example.org', replace: false, untrusted: false }), { name: 'intranet.example.org' });
  assert.deepEqual(upload.shownArgs({ name: 'x', replace: true, untrusted: true }), { name: 'x', replace: true, untrusted: true });
  assert.deepEqual(upload.addArgs({ name: 'x', replace: true }, 'DOC'), { name: 'x', replace: true, pem: 'DOC' });
  assert.equal('pem' in upload.shownArgs({ name: 'x' }), false);
});
