// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A site's page (pages/websites/site.js and site-dialogs.js): its names, what
// it serves, its HTTPS and the changes to it, its database, its file with a
// hand edit shown and kept, Check the sites and Remove, for an administrator
// and for a reader.

import assert from 'node:assert/strict';
import test from 'node:test';

import { certificate, own } from './certificates-fixtures.mjs';
import {
  checkBroken, checkNoLogs, checkOk, failedTask, list, off, phpOff, problem, site, visits, wantedFor,
} from './websites-fixtures.mjs';
import {
  buttonOf, dialogsOf, fieldOf, linkOf, openWebsites, refusal, settle, text, tick, type,
} from './websites-view.mjs';

const clocked = new WeakSet();
const open = (context, name, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openWebsites(context, { rest: name, ...more });
};

/** A change that starts the task `id`. */
const starts = (id) => ({ ok: true, task: id, result: { done: true, changed: true, output: [] } });

/** The value of a row of the site's card, by its key. */
const row = (world, key) => world.view.main.querySelector(`dd[data-row="${key}"]`);

const titles = (world) => world.view.main.querySelectorAll('.flow-step b').map((one) => one.textContent);
const states = (world) => world.view.main.querySelectorAll('.flow-step').map((one) => one.getAttribute('data-state'));

/** A server with a site on HTTP whose name a certificate that certd keeps covers. */
const withPhotos = () => list({
  websites: [site('photos.example.org'), site('shop.example.org', { https: true, certificate: 'shop.example.org', redirects_http: true })],
});
const certificatesWithPhotos = () => [own(), certificate(), certificate({ name: 'photos.example.org', names: ['photos.example.org'], used_by: [] })];

// ---- what it says ----

test('the page names the site by its first name, with Apps and Websites above it, its state and Check the sites', async (context) => {
  const world = await open(context, 'shop.example.org');
  const head = world.view.headed;
  assert.equal(head.title, 'shop.example.org');
  assert.deepEqual(head.crumbs, [{ title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }]);
  assert.equal(head.commands, true);
  assert.deepEqual(head.pill, { words: 'Answers', kind: 'ok' });
  assert.deepEqual(head.buttons.map((one) => text(one)), ['Check the sites']);
  assert.deepEqual(world.calls, ['/api/1/websites', '/api/1/certificates', '/api/1/certificates/wanted/all']);
});

test('the rows are its names as links, what it serves, its HTTPS, its database and its file', async (context) => {
  const world = await open(context, 'shop.example.org');
  assert.deepEqual(row(world, 'names').querySelectorAll('a').map((one) => [one.textContent, one.getAttribute('href')]), [
    ['shop.example.org', 'https://shop.example.org/'], ['www.shop.example.org', 'https://www.shop.example.org/'],
  ]);
  assert.match(text(row(world, 'serves')), /^PHP pages/);
  assert.match(text(row(world, 'serves')), /Open the Terminal/);
  assert.equal(linkOf(row(world, 'serves'), 'Open the Terminal').getAttribute('href'), '/terminal');
  assert.match(text(row(world, 'https')), /Let's Encrypt · renews Dec 30/);
  assert.equal(linkOf(row(world, 'https'), 'shop.example.org').getAttribute('href'), '/certificates/shop.example.org');
  assert.match(text(row(world, 'database')), /None that this page knows of/);
  assert.equal(linkOf(row(world, 'database'), 'Make its database').getAttribute('href'), '/databases/add?site=shop.example.org');
  assert.match(text(row(world, 'file')), /\/system\/settings\/websites\/shop\.example\.org\.conf/);
  assert.match(text(row(world, 'file')), /As website wrote it/);
});

test('a site on HTTP says so in the warning colour and offers the certificate it can have, but no other', async (context) => {
  const world = await open(context, 'blog.example.org');
  assert.match(text(row(world, 'https')), /^HTTP only/);
  assert.equal(row(world, 'https').querySelector('span').classes.has('web-tone-warn'), true);
  assert.deepEqual(row(world, 'https').querySelectorAll('button').map((one) => text(one)), ['Get a certificate']);
});

test('a name of this machine can be served with this machine\'s certificate, and a site that certd has a certificate for can use it', async (context) => {
  const hand = list({ websites: [site('duke-nas.local'), site('photos.example.org')] });
  const world = await open(context, 'duke-nas.local', { world: { list: hand, reads: { '/api/1/certificates': certificatesWithPhotos() } } });
  assert.deepEqual(row(world, 'https').querySelectorAll('button').map((one) => text(one)), ['Get a certificate', 'Use this machine\'s certificate']);
  const photos = await open(context, 'photos.example.org', { world: { list: hand, reads: { '/api/1/certificates': certificatesWithPhotos() } } });
  assert.deepEqual(row(photos, 'https').querySelectorAll('button').map((one) => text(one)), ['Get a certificate', 'Use one that is here…']);
});

test('a site served over HTTPS has Turn HTTPS off…, and says when its certificate ends', async (context) => {
  const world = await open(context, 'shop.example.org');
  assert.deepEqual(row(world, 'https').querySelectorAll('button').map((one) => text(one)), ['Turn HTTPS off…']);
  assert.match(text(row(world, 'https')), /Ends /);
});

test('a hand edit is said in words, kept, and the file can be shown; it is also a row of What changed', async (context) => {
  const hand = list({ websites: [site('blog.example.org', { by_hand: true, text: 'server { listen 80; # mine\n}\n' })] });
  const world = await open(context, 'blog.example.org', { world: { list: hand } });
  const file = row(world, 'file');
  assert.match(text(file), /Changed by hand · kept as you wrote it/);
  assert.match(text(row(world, 'https')), /The file was changed by hand, so website does not write it: add the HTTPS server yourself, as website\(8\) shows\./);
  assert.equal(row(world, 'https').querySelectorAll('button').length, 0, 'no button writes HTTPS into a file that was changed by hand');
  assert.equal(linkOf(file, 'What changed').getAttribute('href'), '/reliability');
  const pre = file.querySelector('pre');
  assert.equal(pre.hidden, true);
  await world.press(buttonOf(file, 'Show the file'));
  assert.equal(pre.hidden, false);
  assert.equal(pre.textContent, 'server { listen 80; # mine\n}\n');
  assert.ok(buttonOf(file, 'Hide the file'));
});

test('a site that was added by hand has no HTTPS button and its file is not written by website', async (context) => {
  const world = await open(context, 'legacy');
  assert.match(text(row(world, 'file')), /Added by hand · website does not write it, and keeps it as you wrote it/);
  assert.match(text(row(world, 'https')), /nginx serves this site by a file made by hand, which website does not change: add HTTPS to it yourself\./);
  assert.equal(row(world, 'https').querySelectorAll('button').length, 0);
  assert.match(text(row(world, 'serves')), /Set up by hand/);
});

test('a problem anyone can see is in a banner with its ID, and the head says the state', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { list: problem() } });
  assert.match(text(world.view.main.querySelector('.banner.warn')), /Its folder \/shared\/www\/sites\/shop\.example\.org is not there/);
  assert.equal(world.view.headed.pill.kind, 'warn');
});

test('nginx that is off is a banner of its own, and the pill says so', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { list: off() } });
  assert.match(text(world.view.main.querySelector('.banner.bad')), /nginx is off/);
  assert.deepEqual(world.view.headed.pill, { words: 'nginx is off', kind: 'bad' });
  const php = await open(context, 'shop.example.org', { world: { list: phpOff() } });
  assert.match(php.view.headed.pill.words, /PHP-FPM is off/);
});

test('a site that is not there says so, with the way back, and the head still has its name', async (context) => {
  const world = await open(context, 'nothing.example.org');
  assert.match(text(world.view.main), /There is no website called nothing\.example\.org\./);
  assert.equal(linkOf(world.view.main, 'Back to the websites').getAttribute('href'), '/websites');
  assert.equal(world.view.headed.title, 'nothing.example.org');
});

test('a site called add has its own address, apart from Add a website', async (context) => {
  const hand = list({ websites: [site('add')] });
  const world = await open(context, '%61dd', { world: { list: hand } });
  assert.equal(world.view.headed.title, 'add');
});

// ---- a reader ----

test('a reader sees the site in words and the command, and has no button that changes it', async (context) => {
  const world = await open(context, 'blog.example.org', { session: { admin: false } });
  assert.match(text(world.view.main), /Only an administrator can add, change or remove a website\./);
  assert.deepEqual(world.view.headed.buttons, []);
  assert.equal(buttonOf(world.view.main, 'Remove…'), undefined);
  assert.equal(row(world, 'https').querySelectorAll('button').length, 0);
  assert.equal(linkOf(world.view.main, 'Make its database'), undefined);
  assert.equal(linkOf(world.view.main, 'Open the Terminal'), undefined);
  assert.deepEqual(world.inlines.map(([action]) => action), ['websites.list'], 'the command that lists the sites is there to copy');
  assert.equal(world.ran.length, 0);
});

// ---- Check the sites ----

test('Check the sites asks as an administrator and puts a site\'s problem on its page', async (context) => {
  const world = await open(context, 'blog.example.org', { world: { runs: { 'websites.check': { ok: true, result: checkBroken() } } } });
  await world.press(world.view.headed.buttons[0]);
  assert.equal(world.ranOf('websites.check').length, 1);
  assert.match(text(world.view.main), /nginx refuses its file: unknown directive "listn"/);
  assert.match(text(world.view.main), /WEB-103/);
});

test('a check that finds nothing says so', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { runs: { 'websites.check': { ok: true, result: checkOk() } } } });
  await world.press(world.view.headed.buttons[0]);
  assert.match(text(world.view.main), /nginx's check of the sites passed\./);
});

// ---- Get a certificate ----

test('Get a certificate checks the names, asks for the terms, and gets the certificate and turns HTTPS on, in that order', async (context) => {
  const world = await open(context, 'blog.example.org', { world: { runs: { 'certificate.get': starts(12), 'websites.https': starts(13) } } });
  await world.press(buttonOf(row(world, 'https'), 'Get a certificate'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Get a certificate for blog.example.org');
  assert.ok(world.calls.includes('/api/1/certificates/check/blog.example.org'));
  assert.match(text(dialog), /shop\.example\.org leads to 203\.0\.113\.10/);
  await world.submit(dialog);
  assert.equal(dialogsOf(world.document).length, 1, 'the terms are not agreed, so the dialog stays');
  assert.match(text(dialog), /Agree to Let's Encrypt's terms of service to get a certificate\./);
  tick(fieldOf(dialog, 'I agree to'), true);
  type(fieldOf(dialog, 'Contact email'), 'me@example.org');
  await world.submit(dialog);
  assert.deepEqual(world.ran.filter(([action]) => action === 'certificate.get' || action === 'websites.https').map(([action, args]) => [action, args]), [
    ['certificate.get', { name: 'blog.example.org', names: 'blog.example.org', agree_terms: true, email: 'me@example.org' }],
    ['websites.https', { name: 'blog.example.org', certificate: 'blog.example.org' }],
  ]);
  assert.equal(world.view.main.querySelector('h2#web-flow-h').textContent, 'Getting a certificate for blog.example.org');
  assert.deepEqual(titles(world), ['Get the certificate for blog.example.org', 'Serve blog.example.org over HTTPS']);
  assert.deepEqual(states(world), ['done', 'done']);
});

test('a certificate that is not got leaves the site as it was, with no Try again here: the button on the page is the way', async (context) => {
  const world = await open(context, 'blog.example.org', {
    world: { runs: { 'certificate.get': starts(12) }, reads: { '/api/1/tasks/12': failedTask(12, 'cert: the authority could not reach blog.example.org (CRT-102)') } },
  });
  await world.press(buttonOf(row(world, 'https'), 'Get a certificate'));
  tick(fieldOf(world.dialog(), 'I agree to'), true);
  await world.submit();
  assert.deepEqual(states(world), ['failed', 'skipped']);
  assert.equal(world.ranOf('websites.https').length, 0);
  assert.match(text(world.view.main.querySelector('.web-flow')), /blog\.example\.org is live over HTTP\. The certificate was not got/);
  assert.equal(buttonOf(world.view.main.querySelector('.web-flow'), 'Try again'), undefined);
  assert.ok(buttonOf(row(world, 'https'), 'Get a certificate'), 'the page still has the way to try again');
});

test('while certd tries to get the certificate the row says so in place of Get a certificate, for an administrator and a reader', async (context) => {
  const reads = { '/api/1/certificates/wanted/all': wantedFor('blog.example.org') };
  const world = await open(context, 'blog.example.org', { world: { reads } });
  assert.equal(buttonOf(row(world, 'https'), 'Get a certificate'), undefined);
  assert.match(text(row(world, 'https')), /HTTP onlyGetting a certificate: certd tries again every hour$/);
  assert.equal(row(world, 'https').querySelector('[data-row="getting"]').textContent, 'Getting a certificate: certd tries again every hour');
  const reader = await open(context, 'blog.example.org', { session: { admin: false }, world: { reads } });
  assert.match(text(row(reader, 'https')), /Getting a certificate: certd tries again every hour/);
  assert.equal(row(reader, 'https').querySelectorAll('button').length, 0);
});

test('a record certd has given up, or one of another name, leaves the button where it was', async (context) => {
  for (const wanted of [wantedFor('blog.example.org', true), wantedFor('other.example.org')]) {
    const world = await open(context, 'blog.example.org', { world: { reads: { '/api/1/certificates/wanted/all': wanted } } });
    assert.ok(buttonOf(row(world, 'https'), 'Get a certificate'));
    assert.doesNotMatch(text(row(world, 'https')), /Getting a certificate/);
  }
});

test('a site that has HTTPS does not say a certificate is being got, and the others\' buttons stay', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { reads: { '/api/1/certificates/wanted/all': wantedFor('shop.example.org') } } });
  assert.doesNotMatch(text(row(world, 'https')), /Getting a certificate/);
  assert.ok(buttonOf(row(world, 'https'), 'Turn HTTPS off…'));
  const hand = list({ websites: [site('duke-nas.local')] });
  const own = await open(context, 'duke-nas.local', { world: { list: hand, reads: { '/api/1/certificates/wanted/all': wantedFor('duke-nas.local') } } });
  assert.ok(buttonOf(row(own, 'https'), 'Use this machine\'s certificate'), 'the machine\'s own certificate is still the other way');
});

test('records that cannot be read leave the row as it was, and no problem is shown for them', async (context) => {
  const world = await open(context, 'blog.example.org', { world: { reads: { '/api/1/certificates/wanted/all': refusal('ADM-123', 'no', 422) } } });
  assert.ok(buttonOf(row(world, 'https'), 'Get a certificate'));
  assert.equal(world.problems.length, 0);
});

test('a certificate that certd keeps trying for after the step failed is the row the page reads again', async (context) => {
  const reads = {
    '/api/1/tasks/12': failedTask(12, 'cert: the authority could not reach blog.example.org (CRT-102)'),
    '/api/1/certificates/wanted/all': () => (world.ranOf('certificate.get').length > 0 ? wantedFor('blog.example.org') : { format: 1, wanted: [] }),
  };
  const world = await open(context, 'blog.example.org', { world: { runs: { 'certificate.get': starts(12) }, reads } });
  await world.press(buttonOf(row(world, 'https'), 'Get a certificate'));
  tick(fieldOf(world.dialog(), 'I agree to'), true);
  await world.submit();
  assert.equal(buttonOf(row(world, 'https'), 'Get a certificate'), undefined, 'certd has the order now');
  assert.match(text(row(world, 'https')), /Getting a certificate: certd tries again every hour/);
});

test('the command of the certificate shows in the dialog while commands are shown', async (context) => {
  const world = await open(context, 'blog.example.org', { prefs: { commands: 'shown' } });
  await world.press(buttonOf(row(world, 'https'), 'Get a certificate'));
  assert.deepEqual(world.panels.at(-1), ['certificate.get', { name: 'blog.example.org', names: 'blog.example.org', agree_terms: true }]);
});

// ---- Use one ----

test('Use this machine\'s certificate serves the site with it, once asked', async (context) => {
  const hand = list({ websites: [site('duke-nas.local')] });
  const world = await open(context, 'duke-nas.local', { world: { list: hand, runs: { 'websites.https': starts(13) } } });
  await world.press(buttonOf(row(world, 'https'), 'Use this machine\'s certificate'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Serve duke-nas.local over HTTPS');
  assert.match(text(dialog), /Browsers warn until they trust this machine's certificate; Certificates says how\./);
  assert.match(text(dialog), /nginx restarts once, to give it the certificate and port 443/);
  await world.submit(dialog);
  assert.deepEqual(world.ranOf('websites.https')[0][0], { name: 'duke-nas.local', certificate: 'this-machine' });
});

test('Use one that is here offers the certificates that name every name of the site', async (context) => {
  const hand = list({ websites: [site('photos.example.org')] });
  const world = await open(context, 'photos.example.org', { world: { list: hand, reads: { '/api/1/certificates': certificatesWithPhotos() }, runs: { 'websites.https': starts(13) } } });
  await world.press(buttonOf(row(world, 'https'), 'Use one that is here…'));
  const which = fieldOf(world.dialog(), 'The certificate');
  assert.deepEqual(which.querySelectorAll('option').map((one) => one.getAttribute('value')), ['photos.example.org']);
  await world.submit();
  assert.deepEqual(world.ranOf('websites.https')[0][0], { name: 'photos.example.org', certificate: 'photos.example.org' });
});

// ---- Turn HTTPS off ----

test('Turn HTTPS off… says what it costs, keeps the certificate in certd, and serves the site over HTTP only', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { runs: { 'websites.https': starts(13) } } });
  await world.press(buttonOf(row(world, 'https'), 'Turn HTTPS off…'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Turn HTTPS off for shop.example.org?');
  assert.match(text(dialog), /pages and passwords travel unencrypted\. The certificate stays in certd\./);
  assert.match(text(dialog), /nginx restarts once, since no other site uses this certificate/);
  await world.submit(dialog);
  assert.deepEqual(world.ranOf('websites.https')[0][0], { name: 'shop.example.org', off: true });
  assert.deepEqual(titles(world), ['Serve shop.example.org over HTTP only']);
});

test('another site that uses the same certificate means nginx is not restarted, and the words say so', async (context) => {
  const both = list({
    websites: [
      site('shop.example.org', { https: true, certificate: 'shared', redirects_http: true }),
      site('blog.example.org', { https: true, certificate: 'shared', redirects_http: true }),
    ],
  });
  const world = await open(context, 'shop.example.org', { world: { list: both } });
  await world.press(buttonOf(row(world, 'https'), 'Turn HTTPS off…'));
  assert.match(text(world.dialog()), /nginx reads its settings again, and no visitor is turned away\./);
});

test('HTTPS that could not be turned off says what stands and has Try again, which runs only that step', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { runs: { 'websites.https': refusal('WEB-103', 'nginx refuses its file') } } });
  await world.press(buttonOf(row(world, 'https'), 'Turn HTTPS off…'));
  await world.submit();
  assert.match(text(world.view.main.querySelector('.web-flow')), /shop\.example\.org is still served over HTTPS: nothing was changed\./);
  world.ran.length = 0;
  await world.press(buttonOf(world.view.main.querySelector('.web-flow'), 'Try again'));
  assert.deepEqual(world.ran.map(([action]) => action), ['websites.https']);
});

// ---- Remove ----

test('Remove… asks for the name typed, and says what stays; it removes the site and keeps its folder', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { runs: { 'websites.remove': starts(14) } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Remove shop.example.org?');
  assert.match(text(dialog), /Removes nginx's site for shop\.example\.org\. Its folder is kept unless you tick the box below\. Its certificate stays in certd\./);
  const confirm = buttonOf(dialog, 'Remove the website');
  assert.equal(confirm.disabled, true);
  type(fieldOf(dialog, 'Type shop.example.org to go on'), 'shop');
  assert.equal(confirm.disabled, true);
  type(fieldOf(dialog, 'Type shop.example.org to go on'), 'shop.example.org');
  assert.equal(confirm.disabled, false);
  await world.submit(dialog);
  assert.deepEqual(world.ranOf('websites.remove')[0][0], { name: 'shop.example.org' });
  assert.match(text(world.view.main), /shop\.example\.org is removed\. Its folder is kept\. Its certificate stays in certd\./);
  assert.equal(linkOf(world.view.main, 'Back to the websites').getAttribute('href'), '/websites');
});

test('the folder is removed with the site only when its box is ticked, and the box says which folder', async (context) => {
  const world = await open(context, 'blog.example.org', { world: { runs: { 'websites.remove': starts(14) } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  const dialog = world.dialog();
  const box = fieldOf(dialog, 'Also remove its folder /shared/www/sites/blog.example.org and everything in it');
  assert.equal(box.checked, false, 'the folder stays unless it is asked for');
  tick(box, true);
  type(fieldOf(dialog, 'Type blog.example.org to go on'), 'blog.example.org');
  await world.submit(dialog);
  assert.deepEqual(world.ranOf('websites.remove')[0][0], { name: 'blog.example.org', files: true });
  assert.match(text(world.view.main), /blog\.example\.org is removed\. Its folder is removed too\./);
});

test('a site with no folder of its own has no box for it', async (context) => {
  const world = await open(context, 'legacy', { world: { runs: { 'websites.remove': starts(14) } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  assert.equal(world.dialog().querySelector('input[type="checkbox"]'), null);
  type(fieldOf(world.dialog(), 'Type legacy to go on'), 'legacy');
  await world.submit();
  assert.deepEqual(world.ranOf('websites.remove')[0][0], { name: 'legacy' });
});

test('a site that could not be removed stays on the page, with what stands and Try again', async (context) => {
  const world = await open(context, 'blog.example.org', { world: { runs: { 'websites.remove': refusal('WEB-103', 'nginx refuses') } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  type(fieldOf(world.dialog(), 'Type blog.example.org to go on'), 'blog.example.org');
  await world.submit();
  assert.match(text(world.view.main.querySelector('.web-flow')), /blog\.example\.org is still there: nothing was changed\./);
  assert.ok(buttonOf(world.view.main.querySelector('.web-flow'), 'Try again'));
  assert.equal(world.view.headed.title, 'blog.example.org');
});

test('cancelling a question changes nothing', async (context) => {
  const world = await open(context, 'blog.example.org');
  await world.press(buttonOf(world.view.main, 'Remove…'));
  await world.press(buttonOf(world.dialog(), 'Cancel'));
  assert.equal(world.ran.length, 0);
  assert.equal(dialogsOf(world.document).length, 0);
  await settle();
});

// ---- its visits ----

const card = (world) => world.view.main.querySelector('section[data-card="visits"]');

test('a site that has been counted has the chart of its 30 days, with where the numbers come from', async (context) => {
  const world = await open(context, 'shop.example.org');
  await settle();
  const visited = card(world);
  assert.equal(text(visited.querySelector('h2')), 'Visits');
  const chart = visited.querySelector('.usage-chart[data-chart="visits"]');
  assert.ok(chart, 'the chart is drawn once its module is read');
  assert.match(text(chart.querySelector('.stat')), /^1,243Visits and errors on the last day counted · peak 1,\d{3} visits at /);
  assert.deepEqual(chart.querySelectorAll('.legend li').map((one) => text(one)), ['Visits', 'Errors (dashed)']);
  assert.match(chart.querySelector('svg').getAttribute('aria-label'), /^Visits and errors over the last 30 days: visits: average /);
  assert.equal(chart.querySelectorAll('polyline.line').length, 2);
  assert.equal(chart.querySelectorAll('polyline.line.second').length, 1, 'the errors are the dashed line');
  assert.match(text(visited), /Counted once a day from nginx's log; the log holds no addresses\./);
  assert.doesNotMatch(text(visited), /Not counted yet/);
  assert.doesNotMatch(text(visited), /limits went/);
});

test('the numbers of the chart are a table of days, newest first, in UTC', async (context) => {
  const world = await open(context, 'shop.example.org');
  await settle();
  const chart = card(world).querySelector('.usage-chart');
  const toggle = buttonOf(chart, 'Show the numbers');
  assert.ok(toggle);
  await world.press(toggle);
  const rows = chart.querySelectorAll('tbody tr').map((one) => [...one.querySelectorAll('th'), ...one.querySelectorAll('td')].map((cell) => text(cell)));
  assert.equal(rows.length, 30);
  assert.deepEqual(rows[0], ['Nov 13', '1,240', '3']);
  assert.deepEqual(chart.querySelectorAll('thead th').map((one) => text(one)), ['Day (UTC)', 'Visits', 'Errors']);
});

test('a site that is not counted yet says so, and loads no chart', async (context) => {
  const world = await open(context, 'blog.example.org');
  await settle();
  assert.match(text(card(world)), /Not counted yet: visits are counted once a day\./);
  assert.equal(card(world).querySelector('.usage-chart'), null);
  assert.doesNotMatch(text(card(world)), /Counted once a day from nginx's log/);
});

test('days counted only as far as the limits went say their numbers are at least what is shown', async (context) => {
  const counted = list({ websites: [site('shop.example.org', { visits: visits({ complete: false }) })] });
  const world = await open(context, 'shop.example.org', { world: { list: counted } });
  await settle();
  assert.match(text(card(world)), /Some days were counted only as far as the limits went, so their numbers are at least what is shown\./);
});

test('a reader has the chart too, and the card is drawn again with the page', async (context) => {
  const world = await open(context, 'shop.example.org', { session: { admin: false } });
  await settle();
  assert.ok(card(world).querySelector('.usage-chart'));
  await world.press(world.view.main.querySelector('[data-inline]') ?? card(world));
  assert.equal(card(world).querySelectorAll('.usage-chart').length, 1, 'one chart, not one for each time the page is drawn');
});

test('Check the sites finds settings that lack the log format, and says it on the site\'s page too', async (context) => {
  const world = await open(context, 'shop.example.org', { world: { runs: { 'websites.check': { ok: true, result: checkNoLogs() } } } });
  await world.press(world.view.headed.buttons[0]);
  assert.match(text(world.view.main.querySelector('.web-check')), /do not define the log format of the sites' own logs/);
  assert.equal(world.view.main.querySelectorAll('section[data-card="visits"] .usage-chart').length, 1, 'the chart is drawn again, once');
});
