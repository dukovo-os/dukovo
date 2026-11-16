// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Certificates page on a fake document: the list and what each button
// asks for, the form that gets a certificate with its checks, progress and
// records, the upload and the secret it holds, revoking and removing, the
// history, the certificate the console serves, and the machine's name with
// the one change after which the page asks for nothing.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import {
  byDns, certificate, closed, dnsLog, history, list, machineWithDomain, oneMissing, own, report, SHOWN_FINGERPRINT, signIn, task,
} from './certificates-fixtures.mjs';
import {
  holdings, openCertificates, refusal, settle, text,
} from './certificates-view.mjs';
import { sparkIdle, sparkWith } from './security-fixtures.mjs';

const realObserver = globalThis.MutationObserver;
beforeEach(() => {
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
});
afterEach(() => {
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

const NAMES = 'shop.example.org www.shop.example.org';
const form = (page) => page.card('get');
const names = (page) => form(page).querySelector('#get-names');
const proof = (page, id) => form(page).querySelectorAll('input').find((one) => one.getAttribute('type') === 'radio' && one.getAttribute('value') === id);
const link = (root, words) => root.querySelectorAll('a').find((one) => one.textContent === words);
const buttons = (root) => root.querySelectorAll('button').map((one) => one.textContent.trim());

/** Types the names of the form and leaves the field, so that the checks are read. */
async function typeNames(page, words = NAMES) {
  await page.type(names(page), words);
}

async function chooseDns(page) {
  page.choose(form(page), 'dns');
  await settle();
}

test('the page reads everything it shows and draws the four cards under the head', async (context) => {
  const page = await openCertificates(context);
  assert.equal(page.view.headed.title, 'Certificates');
  assert.equal(page.view.headed.commands, true);
  assert.deepEqual(page.view.headed.buttons.map((one) => one.textContent), ['Upload your own…', 'Get one from Let\'s Encrypt']);
  for (const id of ['certificates', 'get', 'renewals', 'name']) assert.ok(page.card(id), id);
  for (const path of ['/api/1/certificates', '/api/1/certificates/history/all', '/api/1/security/sign-in', '/api/1/console/about', '/api/1/machine/name', '/api/1/tasks']) {
    assert.ok(page.calls.includes(path), path);
  }
  assert.match(text(page.view.main), /certd renews each Let's Encrypt certificate on the day the authority suggests \(ARI\)/);
  assert.deepEqual(page.problems, []);
});

test('each row says who made it, when it ends, what uses it, and has the buttons its kind has', async (context) => {
  const page = await openCertificates(context);
  assert.match(text(page.row('this-machine')), /^duke-nas this consolemade by this serverThis serverJan 13, 2029admind/);
  assert.deepEqual(buttons(page.row('this-machine')).slice(0, 2), ['Show fingerprint', 'Make it anew…']);
  assert.match(text(page.row('shop.example.org')), /shop\.example\.organd 1 moreLet's EncryptJan 28, 2027renews Dec 30 by itselfnginxRenew now/);
  assert.match(text(page.row('photos.example.org')), /Nov 2511 days left, cannot renew by itselfno service uses it yet/);
  assert.match(text(page.row('intranet.example.org')), /Example CorpNot from an authority this server trustsJun 2, 2027200 days left, cannot renew by itself/);
  assert.match(text(page.row('blog.example.org')), /revoked/);
  assert.ok(buttons(page.row('intranet.example.org')).includes('Replace…'));
  assert.equal(buttons(page.row('shop.example.org')).includes('Remove…'), false, 'nginx uses it');
  assert.match(text(page.row('shop.example.org')), /Remove is offered when no service uses it: nginx does\./);
  assert.ok(buttons(page.row('photos.example.org')).includes('Remove…'));
  const link = page.row('shop.example.org').querySelectorAll('a').find((one) => one.textContent === 'nginx');
  assert.equal(link.getAttribute('href'), '/services/nginx');
  assert.match(text(page.card('certificates')), /Let's Encrypt account: made on Nov 2, with no contact email/);
  assert.match(text(page.card('certificates')), /Warnings go to the alerts \(email, webhooks and this console\)\. Open the alerts/);
});

test('a certificate whose last renewal failed says so under its row, with Check again and Retry', async (context) => {
  const page = await openCertificates(context);
  const failure = page.view.main.querySelector('tr.certs-failure');
  assert.match(text(failure), /renewal failed today 6:00 AM, certd tries again at its next lookLet's Encrypt could not reach port 80 CRT-106Check againRetry/);
  assert.equal(page.row('shop.example.org').parent.querySelectorAll('tr.certs-failure').length, 1, 'only one certificate has one');
  await page.press(failure, 'Retry');
  assert.deepEqual(page.ran('certificate.renew'), [{ name: 'photos.example.org' }]);
  await page.press(failure, 'Check again');
  assert.ok(page.calls.includes('/api/1/certificates/check/photos.example.org'));
  assert.equal(names(page).value, 'photos.example.org');
});

test('Renew now runs the renewal, reads again, and says when the certificate is good until', async (context) => {
  const state = { list: list() };
  const page = await openCertificates(context, {
    world: {
      reads: { '/api/1/certificates': () => state.list },
      answers: {
        'certificate.renew': () => {
          state.list = list().map((one) => (one.name === 'shop.example.org' ? { ...one, days_left: 90, not_after: '2027-02-12T00:00:00Z' } : one));
          return { ok: true, result: { done: true, changed: true, output: [] } };
        },
      },
    },
  });
  const reads = page.calls.filter((path) => path === '/api/1/certificates').length;
  await page.press(page.row('shop.example.org'), 'Renew now');
  assert.deepEqual(page.ran('certificate.renew'), [{ name: 'shop.example.org' }]);
  assert.equal(page.ranWith('certificate.renew')[0][2].words.done, 'shop.example.org is renewed.');
  assert.equal(page.calls.filter((path) => path === '/api/1/certificates').length, reads + 1);
  assert.match(text(page.card('certificates')), /Renewed: good until Feb 12, 2027/);
});

test('a renewal that is refused because the certificate was proved by DNS makes its row say Renew by DNS, which opens the form with DNS', async (context) => {
  const dns = refusal('CRT-121', 'shop.example.org was proved by DNS by hand, which certd cannot repeat', 409, { detail: 'renew it with `elevate cert get shop.example.org --dns`' });
  const page = await openCertificates(context, { world: { answers: { 'certificate.renew': dns } } });
  await page.press(page.row('shop.example.org'), 'Renew now');
  assert.equal(buttons(page.row('shop.example.org')).includes('Renew by DNS…'), true);
  assert.equal(buttons(page.row('shop.example.org')).includes('Renew now'), false);
  assert.match(text(page.row('shop.example.org')), /cannot renew by itself/);
  await page.press(page.row('shop.example.org'), 'Renew by DNS…');
  assert.equal(names(page).value, 'shop.example.org www.shop.example.org');
  assert.equal(proof(page, 'dns').checked, true);
  assert.ok(page.calls.includes('/api/1/certificates/check/shop.example.org%2Cwww.shop.example.org?dns=true'));
});

test('a reader sees the list and the history and no button that changes anything', async (context) => {
  const page = await openCertificates(context, { session: { admin: false } });
  assert.deepEqual(page.view.headed.buttons, []);
  assert.match(text(page.view.main), /An administrator can get, upload, renew, revoke and remove certificates/);
  for (const row of page.view.main.querySelectorAll('tbody tr')) {
    for (const one of buttons(row)) assert.ok(['Show fingerprint', ''].includes(one), one);
  }
  assert.deepEqual(buttons(page.card('get')), ['Check again']);
  assert.equal(buttons(page.card('name')).includes('Change…'), false);
  assert.match(text(page.card('renewals')), /renewal failed/);
});

test('Show fingerprint opens the panel with the fingerprint browsers show, and saves the certificate', async (context) => {
  const page = await openCertificates(context, { host: 'duke-nas.local' });
  await page.press(page.row('shop.example.org'), 'Show fingerprint');
  assert.ok(page.calls.includes('/api/1/certificates/shop.example.org'));
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'shop.example.org');
  assert.match(text(dialog), new RegExp(`SHA-256 fingerprint${SHOWN_FINGERPRINT}`));
  assert.match(text(dialog), /Namesshop\.example\.org, www\.shop\.example\.orgMade byLet's EncryptGoodfrom Oct 30 until Jan 28, 2027Used bynginx/);
  await page.submit();
  assert.deepEqual(page.saved, ['shop.example.org.crt']);
  assert.ok(page.dialog().open, 'the panel stays open');
  await page.press(dialog, 'Close');
});

test('a link to a certificate opens its panel, and a name that is not one says so', async (context) => {
  const page = await openCertificates(context, { rest: 'shop.example.org' });
  assert.equal(page.dialog().querySelector('h2').textContent, 'shop.example.org');
  const none = await openCertificates(context, { rest: 'nothing.example.org' });
  assert.match(text(none.card('certificates')), /There is no certificate called nothing\.example\.org\./);
});

test('the link the palette makes to Get or Name scrolls to that card, and the one to Upload opens the dialog', async (context) => {
  const page = await openCertificates(context, { hash: '#name' });
  assert.equal(page.dialog(), null);
  assert.equal(page.document.activeElement, page.card('name').querySelector('h2'), 'focus goes to the card\'s heading');
  const upload = await openCertificates(context, { hash: '#upload' });
  assert.equal(upload.dialog().querySelector('h2').textContent, 'Upload your own certificate');
  const reader = await openCertificates(context, { hash: '#upload', session: { admin: false } });
  assert.equal(reader.dialog(), null, 'a reader has no upload');
});

test('reads that fail say so in the card, and the others are still drawn', async (context) => {
  const down = refusal('ADM-135', 'admind could not answer', 500);
  const page = await openCertificates(context, { world: { reads: { '/api/1/certificates': down, '/api/1/certificates/history/all': down, '/api/1/machine/name': down } } });
  assert.match(text(page.card('certificates')), /The certificates could not be read\./);
  assert.match(text(page.card('renewals')), /What happened to the certificates could not be read\./);
  assert.match(text(page.card('name')), /The machine's name could not be read\./);
  assert.ok(page.card('get'));
  assert.equal(page.problems.length, 3);
});

test('the list is read again every minute and when the server comes back', async (context) => {
  const page = await openCertificates(context);
  const before = page.calls.filter((path) => path === '/api/1/certificates').length;
  await page.tick(60_000);
  assert.equal(page.calls.filter((path) => path === '/api/1/certificates').length, before + 1);
  await page.emit('online');
  assert.equal(page.calls.filter((path) => path === '/api/1/certificates').length, before + 2);
});

test('the Renewals card shows 90 days, and Show all shows the rest', async (context) => {
  const old = { at_ms: Date.UTC(2026, 3, 2, 6, 0, 0), name: 'shop.example.org', event: 'renewed', detail: 'from Let\'s Encrypt', took_ms: 3000, by: 'certd' };
  const page = await openCertificates(context, { world: { reads: { '/api/1/certificates/history/all': () => [old, ...history()] } } });
  assert.doesNotMatch(text(page.card('renewals')), /Apr 2/);
  await page.press(page.card('renewals'), 'Show all');
  assert.match(text(page.card('renewals')), /Apr 2 6:00 AM · shop\.example\.org renewed/);
  assert.equal(buttons(page.card('renewals')).includes('Show all'), false);
});

test('a failure and the success that followed it are one line', async (context) => {
  const page = await openCertificates(context);
  assert.match(text(page.card('renewals')), /Oct 23 6:00 AM · shop\.example\.org: renewal failed · Port 80 was closed \(CRT-103\) Tried again on its own and succeeded the next day\./);
  assert.equal(page.card('renewals').querySelectorAll('li').length, 4, 'five events, four lines');
});

// ---- Get one from Let's Encrypt ----------------------------------------------------------------------------------

test('the names are checked when the field is left, and the checks are drawn with their marks', async (context) => {
  const page = await openCertificates(context);
  assert.equal(page.calls.some((path) => path.startsWith('/api/1/certificates/check/')), false, 'nothing is asked before a name is typed');
  await typeNames(page);
  assert.ok(page.calls.includes('/api/1/certificates/check/shop.example.org%2Cwww.shop.example.org'));
  const checks = text(form(page).querySelector('.certs-checks'));
  assert.match(checks, /Passed shop\.example\.org leads to 203\.0\.113\.10/);
  assert.match(checks, /Passed Port 80 is open \(firewall rule 3\)/);
  assert.match(checks, /Passed nginx holds port 80 and serves the answer/);
  assert.match(text(form(page)), /Getting it agrees to Let's Encrypt's terms of service\./);
  assert.match(link(form(page), 'Let\'s Encrypt\'s terms of service').getAttribute('href'), /^https:\/\/letsencrypt\.org\//);
  assert.match(text(form(page)), /shop\.example\.org is used by nginx\./);
  assert.deepEqual(page.panels.at(-1), ['certificate.get', { name: 'shop.example.org', names: 'shop.example.org\nwww.shop.example.org', agree_terms: true }]);
  assert.match(text(form(page)), /Whether port 80 reaches this server from outside can only be told by asking\. Try it first asks/);
});

test('a name that cannot be asked for is told under the field and nothing is read', async (context) => {
  const page = await openCertificates(context);
  await typeNames(page, '*.example.org');
  assert.match(text(form(page).querySelector('#get-names-error')), /is a wildcard, which Let's Encrypt proves only through DNS/);
  assert.equal(page.calls.some((path) => path.startsWith('/api/1/certificates/check/')), false);
  await chooseDns(page);
  assert.equal(text(form(page).querySelector('#get-names-error')), '');
  assert.ok(page.calls.some((path) => path.startsWith('/api/1/certificates/check/*.example.org')));
});

test('the machine\'s domain is suggested in the field', async (context) => {
  const page = await openCertificates(context, { world: { reads: { '/api/1/machine/name': machineWithDomain() } } });
  assert.equal(names(page).getAttribute('placeholder'), 'duke-nas.example.org');
  await page.press(form(page), 'Use duke-nas.example.org');
  assert.equal(names(page).value, 'duke-nas.example.org');
  assert.ok(page.calls.includes('/api/1/certificates/check/duke-nas.example.org'));
});

test('the firewall that closes port 80 stops the order and offers to open it for a while', async (context) => {
  const state = { closed: true };
  const page = await openCertificates(context, {
    world: {
      reads: {
        '/api/1/certificates/check/*': () => (state.closed ? closed() : report()),
        '/api/1/services': () => (state.closed ? sparkIdle() : sparkWith('firewall', 590_000)),
      },
      answers: { 'firewall.add': () => { state.closed = false; return { ok: true, result: { done: true, changed: true, output: [] } }; } },
    },
  });
  await typeNames(page, 'shop.example.org');
  assert.match(text(form(page).querySelector('.certs-checks')), /Failed The firewall closes port 80, which Let's Encrypt must reach CRT-103 Open port 80/);
  assert.equal(buttons(form(page)).includes('Open port 80'), true);
  const get = form(page).querySelectorAll('button').find((one) => one.textContent === 'Get the certificate');
  assert.equal(get.getAttribute('aria-disabled'), 'true');
  assert.match(text(form(page)), /Put right what failed first, then check again\./);
  await page.press(form(page), 'Open port 80');
  assert.deepEqual(page.ran('firewall.add'), [{ action: 'allow', protocol: 'tcp', ports: '80', try: 600 }]);
  assert.match(text(page.view.main.querySelector('.certs-banners')), /Port 80 is open for a while so that Let's Encrypt can reach this server/);
  assert.match(text(form(page).querySelector('.certs-checks')), /Passed Port 80 is open \(firewall rule 3\)/);
  assert.equal(get.getAttribute('aria-disabled'), null);
});

test('a name that fails alone is left out, and the button says so', async (context) => {
  const page = await openCertificates(context, { world: { reads: { '/api/1/certificates/check/*': () => oneMissing() } } });
  await typeNames(page, 'shop.example.org www.blog.example.org');
  assert.match(text(form(page).querySelector('.certs-checks')), /Failed www\.blog\.example\.org does not resolve to an address: give it an A record \(and AAAA for IPv6\) that leads to this server CRT-101/);
  await page.press(form(page), 'Get it without www.blog.example.org');
  assert.deepEqual(page.ran('certificate.get'), [{ name: 'shop.example.org', names: 'shop.example.org', agree_terms: true }]);
});

test('Get the certificate starts the order, follows its steps, and says when it is got', async (context) => {
  const state = { list: list() };
  const page = await openCertificates(context, {
    world: {
      reads: { '/api/1/certificates': () => state.list, '/api/1/tasks/41': () => task({ state: 'done', step: 5, steps: 5, log: [] }) },
      answers: { 'certificate.get': { ok: true, task: 41, result: { task: 41 } } },
    },
  });
  await typeNames(page);
  await page.type(form(page).querySelector('#get-email'), 'me@example.org');
  await page.press(form(page), 'Get the certificate');
  assert.deepEqual(page.ran('certificate.get'), [{
    name: 'shop.example.org', names: 'shop.example.org\nwww.shop.example.org', email: 'me@example.org', agree_terms: true,
  }]);
  assert.match(text(form(page).querySelector('.certs-progress')), /Getting the certificate: Running/);
  const get = form(page).querySelectorAll('button').find((one) => one.textContent === 'Get the certificate');
  assert.equal(get.getAttribute('aria-disabled'), 'true', 'a get runs: the button is off');
  await page.emit('task', { id: 41, action: 'certificate.get', state: 'running', step: 3, steps: 5, words: 'the authority checks port 80' });
  assert.match(text(form(page).querySelector('.certs-progress')), /Getting the certificate: Step 3 of 5: the authority checks port 80/);
  state.list = [...list(), certificate({ name: 'new.example.org', names: ['new.example.org'] })];
  const reads = page.calls.filter((path) => path === '/api/1/certificates').length;
  await page.emit('task', { id: 41, action: 'certificate.get', state: 'done', step: 5, steps: 5 });
  assert.match(text(form(page).querySelector('.certs-progress')), /shop\.example\.org is got and installed\./);
  assert.equal(page.calls.filter((path) => path === '/api/1/certificates').length, reads + 1, 'the list is read again');
  assert.equal(get.getAttribute('aria-disabled'), null);
});

test('Try it first asks the staging authority and installs nothing', async (context) => {
  const page = await openCertificates(context, { world: { answers: { 'certificate.get': { ok: true, task: 41, result: { task: 41 } } } } });
  await typeNames(page, 'shop.example.org');
  await page.press(form(page), 'Try it first');
  assert.deepEqual(page.ran('certificate.get'), [{
    name: 'shop.example.org', names: 'shop.example.org', dry_run: true, agree_terms: true,
  }]);
  assert.match(text(form(page).querySelector('.certs-progress')), /Trying the order:/);
  await page.emit('task', { id: 41, action: 'certificate.get', state: 'done' });
  assert.match(text(form(page).querySelector('.certs-progress')), /The staging authority accepted the order, so this order should work\. Nothing was installed\./);
});

test('an order that fails says why, and the history is read again', async (context) => {
  const page = await openCertificates(context, {
    world: {
      reads: { '/api/1/tasks/41': () => task({ state: 'failed', note: 'the authority could not reach port 80 (CRT-106)' }) },
      answers: { 'certificate.get': { ok: true, task: 41, result: { task: 41 } } },
    },
  });
  await typeNames(page, 'shop.example.org');
  await page.press(form(page), 'Get the certificate');
  const reads = page.calls.filter((path) => path === '/api/1/certificates/history/all').length;
  await page.emit('task', { id: 41, action: 'certificate.get', state: 'failed' });
  assert.match(text(form(page).querySelector('.certs-progress')), /The order did not finish\. Failed: the authority could not reach port 80 \(CRT-106\)\./);
  assert.equal(page.calls.filter((path) => path === '/api/1/certificates/history/all').length, reads + 1);
});

test('by DNS the checks are left out, the records the order asks for are shown with Copy, and certd\'s part is said', async (context) => {
  const page = await openCertificates(context, {
    world: {
      reads: {
        '/api/1/certificates/check/*': () => byDns(),
        '/api/1/tasks/41': () => task({ words: 'waiting for the TXT records to show', log: dnsLog() }),
      },
      answers: { 'certificate.get': { ok: true, task: 41, result: { task: 41 } } },
    },
  });
  await typeNames(page, 'shop.example.org www.shop.example.org');
  await chooseDns(page);
  assert.ok(page.calls.includes('/api/1/certificates/check/shop.example.org%2Cwww.shop.example.org?dns=true'));
  assert.match(text(form(page).querySelector('.certs-checks')), /You add a TXT record for each name at your DNS provider; this server waits up to 10 minutes for them to show\./);
  assert.doesNotMatch(text(form(page).querySelector('.certs-checks')), /not looked up/);
  assert.match(text(form(page)), /certd does not renew a certificate proved by DNS\. It warns you 21 days and again 7 days before it runs out/);
  await page.press(form(page), 'Get the certificate');
  assert.equal(page.ran('certificate.get')[0].dns, true);
  await page.emit('task', {
    id: 41, action: 'certificate.get', state: 'running', step: 3, steps: 6, words: 'waiting for the TXT records to show: looked 4 times, 1 of 2 found',
  });
  const progress = text(form(page).querySelector('.certs-progress'));
  assert.match(progress, /Step 3 of 6: waiting for the TXT records to show: looked 4 times, 1 of 2 found/);
  assert.match(progress, /_acme-challenge\.shop\.example\.org TXT "abc-123_value"/);
  assert.match(progress, /_acme-challenge\.www\.shop\.example\.org TXT "def-456_value"/);
  await page.press(form(page).querySelector('.certs-progress'), 'Copy');
  assert.deepEqual(page.copied, ['_acme-challenge.shop.example.org TXT "abc-123_value"']);
});

test('an order is not started by a reader, nor twice, nor before the checks are in', async (context) => {
  const page = await openCertificates(context, { world: { answers: { 'certificate.get': { ok: true, task: 41, result: { task: 41 } } } } });
  const get = () => form(page).querySelectorAll('button').find((one) => one.textContent === 'Get the certificate');
  assert.equal(get().getAttribute('aria-disabled'), 'true', 'no names yet');
  get().click();
  await settle();
  assert.deepEqual(page.ran('certificate.get'), []);
  await typeNames(page, 'shop.example.org');
  get().click();
  await settle();
  get().click();
  await settle();
  assert.equal(page.ran('certificate.get').length, 1, 'the order that runs is the only one');
});

test('an order that runs when the page opens is followed', async (context) => {
  const page = await openCertificates(context, {
    world: { reads: { '/api/1/tasks': { tasks: [task({ id: 7, words: 'the authority checks port 80' })] } } },
  });
  assert.match(text(form(page).querySelector('.certs-progress')), /Getting the certificate: Step 3 of 5: the authority checks port 80/);
});

// ---- Revoke, remove, make anew -----------------------------------------------------------------------------------

test('Revoke asks why, says what it does not do, and revokes with the reason', async (context) => {
  const page = await openCertificates(context);
  await page.press(page.row('shop.example.org'), 'Revoke…');
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Revoke shop.example.org');
  assert.match(text(dialog), /Services keep using the certificate until a new one replaces it/);
  for (const words of ['No reason given', 'Its key may have been seen by someone else', 'A new certificate replaces it', 'The site is gone']) assert.match(text(dialog), new RegExp(words));
  assert.deepEqual(page.shown(), ['certificate.revoke', { name: 'shop.example.org', reason: 'unspecified' }]);
  page.choose(dialog, 'key-compromise');
  assert.deepEqual(page.shown(), ['certificate.revoke', { name: 'shop.example.org', reason: 'key-compromise' }]);
  await page.submit();
  assert.deepEqual(page.ran('certificate.revoke'), [{ name: 'shop.example.org', reason: 'key-compromise' }]);
  assert.equal(page.dialog(), null);
});

test('Remove is offered where no service uses the certificate, and removes it', async (context) => {
  const page = await openCertificates(context);
  await page.press(page.row('photos.example.org'), 'Remove…');
  const dialog = page.dialog();
  assert.match(text(dialog), /This server forgets photos\.example\.org and its key\. No service uses it\./);
  await page.submit();
  assert.deepEqual(page.ran('certificate.remove'), [{ name: 'photos.example.org' }]);
  assert.equal(buttons(page.row('this-machine')).includes('Remove…'), false, 'this server\'s own certificate has no Remove');
});

test('Make it anew says that browsers warn again, and shows the new fingerprint when it is made', async (context) => {
  const state = { list: list() };
  const page = await openCertificates(context, {
    world: {
      reads: { '/api/1/certificates': () => state.list },
      answers: {
        'certificate.renew': () => {
          state.list = [own({ fingerprint_sha256: 'cd'.repeat(32) }), ...list().slice(1)];
          return { ok: true, result: { done: true, changed: true, output: [] } };
        },
      },
    },
  });
  await page.press(page.row('this-machine'), 'Make it anew…');
  assert.match(text(page.dialog()), /Browsers and phones that trusted the old one warn again/);
  await page.submit();
  assert.deepEqual(page.ran('certificate.renew'), [{ name: 'this-machine' }]);
  assert.match(text(page.card('certificates')), new RegExp(`Made anew\\. Its fingerprint is ${'CD:'.repeat(31)}CD\\.`));
});

// ---- The certificate the console serves --------------------------------------------------------------------------

test('a certificate that names the browser\'s address is used for the console, as a trial the person keeps once the page is back', async (context) => {
  const state = { serving: 'this-machine', spark: null };
  const named = certificate({ name: 'shop.example.org', names: ['192.168.1.10', 'shop.example.org'], used_by: [] });
  const page = await openCertificates(context, {
    world: {
      reads: {
        '/api/1/certificates': [own(), named],
        '/api/1/security/sign-in': () => signIn(state.serving),
      },
      answers: { 'console.certificate': (args) => { state.serving = args.name; return { ok: true, result: { done: true, changed: true, output: [] } }; } },
    },
  });
  assert.match(text(page.row('shop.example.org')), /Use for this console/);
  await page.press(page.row('shop.example.org'), 'Use for this console');
  assert.deepEqual(page.ran('console.certificate'), [{ name: 'shop.example.org', try: 300 }]);
  assert.match(text(page.row('shop.example.org')), /this console/);
  assert.equal(buttons(page.row('shop.example.org')).includes('Use for this console'), false);
  assert.ok(buttons(page.row('this-machine')).includes('Use this server\'s own certificate again'));
});

test('Keep stays off until the page has come back on the new certificate, and then keeps it', async (context) => {
  const state = { serving: 'this-machine', fresh: false };
  const named = certificate({ name: 'shop.example.org', names: ['192.168.1.10', 'shop.example.org'], used_by: [] });
  const trial = () => ({
    show_version: 1, now_ms: 1_000_000, services: [], undone: [], trials: state.serving === 'this-machine' ? [] : [{ id: 'console', files: [], services: [], user: 1000, left_ms: 290_000 }],
  });
  const page = await openCertificates(context, {
    world: {
      reads: {
        '/api/1/certificates': [own(), named],
        '/api/1/security/sign-in': () => signIn(state.serving),
        '/api/1/console/fresh/console': () => ({ fresh: state.fresh }),
        '/api/1/services': trial,
      },
      answers: { 'console.certificate': (args) => { state.serving = args.name; return { ok: true, result: { done: true, changed: true, output: [] } }; } },
    },
  });
  await page.press(page.row('shop.example.org'), 'Use for this console');
  const banners = page.view.main.querySelector('.certs-banners');
  assert.match(text(banners), /This console now serves shop\.example\.org's certificate\. Keep it once this page has come back on it\./);
  const keep = banners.querySelectorAll('button').find((one) => one.textContent === 'Keep this change');
  assert.equal(keep.getAttribute('aria-disabled'), 'true', 'the page has not come back on it yet');
  keep.click();
  await settle();
  assert.deepEqual(page.ran('console.keep'), []);
  state.fresh = true;
  for (const ms of [1000, 2000, 5000, 10_000]) {
    page.context.mock.timers.tick(ms);
    await settle();
  }
  assert.equal(keep.getAttribute('aria-disabled'), 'false');
  keep.click();
  await settle();
  assert.equal(page.ran('console.keep').length, 1);
});

test('a certificate that names only the machine\'s full name says to open the console there first', async (context) => {
  const page = await openCertificates(context, {
    world: { reads: { '/api/1/machine/name': machineWithDomain(), '/api/1/certificates': [own(), certificate({ names: ['duke-nas.example.org'], used_by: [] })] } },
  });
  assert.match(text(page.row('shop.example.org')), /Open the console at https:\/\/duke-nas\.example\.org:9090\/ and choose it there/);
  assert.equal(page.row('shop.example.org').querySelectorAll('a').find((one) => one.getAttribute('href') === 'https://duke-nas.example.org:9090/').textContent, 'https://duke-nas.example.org:9090/');
  assert.equal(buttons(page.row('shop.example.org')).includes('Use for this console'), false);
});

test('a trial of the console\'s certificate that runs when the page opens shows its banner', async (context) => {
  const page = await openCertificates(context, { world: { reads: { '/api/1/services': sparkWith('console') } } });
  assert.ok(page.calls.includes('/api/1/services'), 'the banners read Spark\'s document');
  assert.match(text(page.view.main.querySelector('.certs-banners')), /Keep this change/);
});

// ---- The machine's name and domain -------------------------------------------------------------------------------

test('the Name and domain card says the name, the domain and the full name', async (context) => {
  const page = await openCertificates(context, { world: { reads: { '/api/1/machine/name': machineWithDomain() } } });
  const card = text(page.card('name'));
  assert.match(card, /Nameduke-nasDomainexample\.org/);
  assert.match(card, /Full name: duke-nas\.example\.org · On this network: duke-nas\.local/);
  assert.match(card, /Letters, digits and -, none starting or ending a word with -/);
  assert.ok(buttons(page.card('name')).includes('Change…'));
});

test('Change the name says what changes for the address the browser comes by, and runs the rename', async (context) => {
  const page = await openCertificates(context, { host: 'duke-nas.local', world: { reads: { '/api/1/machine/name': machineWithDomain() } } });
  await page.press(page.card('name'), 'Change…');
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('#rename-first').value, 'duke-nas', 'the box starts with the name it has');
  await page.type(dialog.querySelector('#rename-first'), 'office-nas');
  const lead = text(dialog);
  assert.match(lead, /The name on the local network becomes office-nas\.local at once; duke-nas\.local stops answering\./);
  assert.match(lead, /This server makes its own certificate anew for the new name\./);
  assert.match(lead, /Certificates from Let's Encrypt do not change: they name the domain names you gave them\./);
  assert.match(lead, /You reach this console as duke-nas\.local\. After the change it is at https:\/\/office-nas\.local:9090\/; this page takes you there\. If that does not answer, the console is still at https:\/\/192\.168\.1\.10:9090\/\./);
  assert.deepEqual(page.shown(), ['machine.rename', { name: 'office-nas.example.org' }]);
});

test('a name that is not one is told under the field and nothing is run', async (context) => {
  const page = await openCertificates(context);
  await page.press(page.card('name'), 'Change…');
  const dialog = page.dialog();
  await page.type(dialog.querySelector('#rename-first'), '-bad');
  await page.submit();
  assert.match(text(dialog.querySelector('#rename-first-error')), /none starting or ending with -/);
  await page.type(dialog.querySelector('#rename-first'), 'duke-nas');
  await page.submit();
  assert.match(text(dialog.querySelector('#rename-first-error')), /This is the name it has\./);
  assert.deepEqual(page.ran('machine.rename'), []);
});

test('after the rename answers the page asks for nothing but the link to the new address', async (context) => {
  const page = await openCertificates(context, {
    host: 'duke-nas.local',
    world: {
      reads: { '/api/1/machine/name': machineWithDomain() },
      answers: {
        'machine.rename': {
          ok: true,
          result: {
            done: true,
            changed: true,
            output: ['hostnamectl: the name is office-nas.example.org', `hostnamectl: this-machine's certificate now names office-nas.example.org: SHA256 ${SHOWN_FINGERPRINT}`],
          },
        },
      },
    },
  });
  await page.press(page.card('name'), 'Change…');
  await page.type(page.dialog().querySelector('#rename-first'), 'office-nas');
  await page.submit();
  assert.deepEqual(page.ran('machine.rename'), [{ name: 'office-nas.example.org' }]);
  const asked = page.calls.length;
  const run = page.allRan().length;
  const card = page.card('name');
  assert.match(text(card), new RegExp(`The machine is called office-nas\\.example\\.org now\\. The new fingerprint: ${SHOWN_FINGERPRINT}`));
  const link = card.querySelectorAll('a').find((one) => one.textContent === 'Open the console at office-nas.local');
  assert.equal(link.getAttribute('href'), 'https://office-nas.local:9090/');
  for (const id of ['certificates', 'get', 'renewals']) assert.equal(page.card(id).hasAttribute('hidden'), true, `${id} is put away`);
  // Nothing the page does from here asks for anything.
  await page.tick(60_000);
  await page.emit('online');
  await page.emit('task', { id: 3, action: 'certificate.get', state: 'running' });
  assert.equal(page.calls.length, asked, 'no read');
  assert.equal(page.allRan().length, run, 'no change');
});

test('a browser that came by number keeps working at that address, and is not sent to another', async (context) => {
  const page = await openCertificates(context, {
    host: '192.168.1.10',
    world: { answers: { 'machine.rename': { ok: true, result: { done: true, changed: true, output: [] } } } },
  });
  await page.press(page.card('name'), 'Change…');
  assert.equal(page.dialog().querySelector('#rename-first').value, 'duke-nas');
  await page.type(page.dialog().querySelector('#rename-first'), 'office-nas');
  assert.match(text(page.dialog()), /You reach this console as 192\.168\.1\.10\. That address keeps working\./);
  await page.submit();
  const card = page.card('name');
  assert.match(text(card), /This page keeps working at 192\.168\.1\.10, and asks for nothing more\./);
  assert.equal(card.querySelectorAll('a').find((one) => one.textContent === 'Reload this page').getAttribute('href'), 'https://192.168.1.10:9090/certificates');
  assert.match(text(card), /Its certificate is made anew: look at it in the list\./);
});

test('a rename that is refused changes nothing on the page', async (context) => {
  const page = await openCertificates(context, { world: { answers: { 'machine.rename': refusal('ADM-122', 'This change needs an administrator', 403) } } });
  await page.press(page.card('name'), 'Change…');
  await page.type(page.dialog().querySelector('#rename-first'), 'office-nas');
  await page.submit();
  assert.ok(page.dialog(), 'the dialog stays open');
  assert.equal(page.card('certificates').hasAttribute('hidden'), false);
  assert.match(text(page.dialog()), /This change needs an administrator/);
});

// ---- Upload your own ---------------------------------------------------------------------------------------------

const MARK = 'U0VDUkVULUtFWQ';
const KEY = `-----BEGIN PRIVATE KEY-----\n${MARK}==\n-----END PRIVATE KEY-----\n`;
const CHAIN = '-----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n';
const file = (words) => ({ text: async () => words });

/** The boxes of the upload dialog: the certificate's name, the chain, the key. */
const boxes = (dialog) => ({
  name: dialog.querySelector('#upload-name'),
  chain: dialog.querySelectorAll('textarea')[0],
  key: dialog.querySelectorAll('textarea')[1],
  chainFile: dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'file')[0],
  keyFile: dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'file')[1],
});

async function openUpload(page) {
  page.view.headed.buttons.find((one) => one.textContent === 'Upload your own…').click();
  await settle();
  return page.dialog();
}

test('the upload sends the key and then the chain as one document, and the key is nowhere the page keeps words', async (context) => {
  const stored = [];
  globalThis.localStorage = { setItem: (key, value) => stored.push([key, value]), getItem: () => null, removeItem() {} };
  globalThis.sessionStorage = globalThis.localStorage;
  context.after(() => {
    delete globalThis.localStorage;
    delete globalThis.sessionStorage;
  });
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  assert.equal(dialog.querySelector('h2').textContent, 'Upload your own certificate');
  const one = boxes(dialog);
  await page.type(one.name, 'intranet2.example.org');
  await page.type(one.chain, CHAIN);
  await page.type(one.key, KEY);
  assert.deepEqual(page.shown(), ['certificate.add', { name: 'intranet2.example.org' }], 'the command shown has no document');
  assert.equal(holdings(page.document.body).some((words) => words.includes(MARK)), false, 'not in an attribute or a text while it is typed');
  assert.ok(one.key.getAttribute('name') === null && one.chain.getAttribute('name') === null, 'the boxes have no name, so no form would send them');
  await page.submit();
  const sent = page.ran('certificate.add');
  assert.equal(sent.length, 1);
  assert.deepEqual(Object.keys(sent[0]).sort(), ['name', 'pem']);
  assert.equal(sent[0].pem, `${KEY}${CHAIN}`, 'the key comes first');
  assert.equal(page.live('certificate.add')[0].pem, '', 'the string is let go of once the call is over');
  assert.equal(one.chain.value, '');
  assert.equal(one.key.value, '');
  assert.equal(page.dialog(), null);
  assert.equal(holdings(page.document.body).some((words) => words.includes(MARK)), false);
  const elsewhere = JSON.stringify([page.notices, page.panels, page.problems.map((error) => error[0]?.message), page.calls, stored, page.view.prefs, globalThis.location.href]);
  assert.equal(elsewhere.includes(MARK), false, 'not in a notice, a command shown, a log, a read, storage or an address');
  assert.deepEqual(stored, []);
});

test('a file is read when the button is pressed, and the name of the file is all the page shows of it', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.name, 'intranet2.example.org');
  one.chainFile.files = [file(CHAIN)];
  one.chainFile.fire('change');
  one.keyFile.files = [file(KEY)];
  one.keyFile.fire('change');
  assert.equal(one.key.value, '');
  assert.equal(one.key.hasAttribute('disabled'), true, 'a file takes the place of what is typed');
  assert.equal(text(dialog).includes('U0VDUkVULUtFWQ'), false);
  await page.submit();
  assert.equal(page.ran('certificate.add')[0].pem, `${KEY}${CHAIN}`);
  assert.equal(one.keyFile.value, '');
  assert.equal(one.key.hasAttribute('disabled'), false);
});

test('before it is sent the page checks only what it can tell: the name, a certificate, a key', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.submit();
  assert.match(text(dialog), /Give the certificate a name/);
  await page.type(one.name, 'intranet2.example.org');
  await page.type(one.chain, 'nothing');
  await page.type(one.key, KEY);
  await page.submit();
  assert.match(text(dialog), /This does not hold a certificate/);
  assert.equal(one.key.value, KEY, 'nothing is taken out of a box that is not sent');
  await page.type(one.chain, CHAIN);
  await page.type(one.key, `${KEY}${CHAIN}`);
  await page.submit();
  assert.match(text(dialog), /This holds a certificate\. Give only the private key here\./);
  assert.deepEqual(page.ran('certificate.add'), []);
});

test('a refusal of the upload shows its words, empties the key and leaves the chain', async (context) => {
  const refused = refusal('CRT-112', 'The key is not the certificate\'s', 422, { detail: 'It does not match.' });
  const page = await openCertificates(context, { world: { answers: { 'certificate.add': refused } } });
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.name, 'intranet2.example.org');
  one.chain.value = CHAIN;
  one.key.value = KEY;
  await page.submit();
  assert.match(text(dialog), /The key is not the certificate's It does not match\. CRT-112/);
  assert.equal(one.key.value, '', 'the key does not wait in its box after it was sent');
  assert.equal(one.chain.value, CHAIN);
  assert.equal(page.live('certificate.add')[0].pem, '');
  assert.ok(page.dialog(), 'the dialog stays open for another try');
  await page.press(dialog, 'Cancel');
  assert.equal(one.chain.value, '', 'cancelling empties the chain too');
  assert.equal(page.dialog(), null);
});

test('closing the dialog with Escape empties what was typed', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.chain, CHAIN);
  await page.type(one.key, KEY);
  dialog.close('close');
  await settle();
  assert.equal(one.key.value, '');
  assert.equal(one.chain.value, '');
  assert.deepEqual(page.ran('certificate.add'), []);
});

test('leaving the page with the dialog open empties what was typed', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.chain, CHAIN);
  await page.type(one.key, KEY);
  one.keyFile.files = [file(KEY)];
  one.keyFile.fire('change');
  page.leave();
  await settle();
  assert.equal(one.key.value, '');
  assert.equal(one.chain.value, '');
  assert.equal(one.key.hasAttribute('disabled'), false, 'the file is let go of too');
  assert.deepEqual(page.ran('certificate.add'), []);
});

test('a locked administrator is asked to unlock before the key is read, and nothing is sent until then', async (context) => {
  const page = await openCertificates(context, { unlocked: false });
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  const readings = [];
  await page.type(one.name, 'intranet2.example.org');
  await page.type(one.chain, CHAIN);
  one.keyFile.files = [{ text: async () => { readings.push('read'); return KEY; } }];
  one.keyFile.fire('change');
  // The unlock dialog is the console's own and nobody answers it here.
  await page.submit();
  assert.deepEqual(readings, [], 'the file is not read while the console is locked');
  assert.deepEqual(page.ran('certificate.add'), []);
  assert.equal(page.document.body.querySelectorAll('dialog').length, 2, 'the unlock dialog is asked');
});

test('Replace… on a row that was added by hand opens the upload with its name and replace', async (context) => {
  const page = await openCertificates(context);
  await page.press(page.row('intranet.example.org'), 'Replace…');
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Replace intranet.example.org');
  const one = boxes(dialog);
  assert.equal(one.name.value, 'intranet.example.org');
  assert.equal(one.name.hasAttribute('readonly'), true);
  one.chain.value = CHAIN;
  one.key.value = KEY;
  await page.submit();
  assert.deepEqual(page.ran('certificate.add')[0].replace, true);
  assert.equal(page.ran('certificate.add')[0].name, 'intranet.example.org');
});

test('an upload with the name of a certificate from an authority is turned back', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.name, 'shop.example.org');
  one.chain.value = CHAIN;
  one.key.value = KEY;
  await page.submit();
  assert.match(text(dialog), /shop\.example\.org came from an authority: remove it first, or choose another name\./);
  assert.deepEqual(page.ran('certificate.add'), []);
});

test('typing the name of a certificate added by hand says that the upload replaces it', async (context) => {
  const page = await openCertificates(context);
  const dialog = await openUpload(page);
  const one = boxes(dialog);
  await page.type(one.name, 'intranet.example.org');
  assert.match(text(dialog), /intranet\.example\.org was added by hand: this replaces it\./);
  one.chain.value = CHAIN;
  one.key.value = KEY;
  await page.submit();
  assert.equal(page.ran('certificate.add')[0].replace, true);
});
