// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Report a problem page, drawn on a page of the test's own with a view
// that answers as admind does and a fake reportd that keeps and cuts drafts:
// what is offered, what each choice drafts, what the text shows, the cuts and
// the answers written again as the edit list reportd takes, the question before
// a new draft loses them, and what Send, Save to a file and Download by QR
// code give out. What the page looks like, and the selection the browser
// makes, are the lab's web-console/report test.

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';

import { makeDesk, sendAnswer, services } from './report-fixtures.mjs';
import {
  openReport, refusal, settle, words,
} from './report-view.mjs';

const PHP = 'crash:20261113T100211-4012';
const VALKEY = 'crash:20261112T200000-99';
const SERVICE_DOWN = { ...services(), services: [...services().services, { name: 'reportd', description: 'Reports', state: 'stopped', since_ms: 100 }] };

/** The titles of the radio cards, in order. */
const titles = (app) => app.main.querySelectorAll('.radio-card b').map((one) => one.textContent);

/** The row of the text that reads `text`. */
const row = (app, text) => app.rows().find((one) => one.text === text);

/** The Cut line button of the row that reads `text`. */
const cutButton = (app, text) => row(app, text).node.querySelector('.rl-cut');

/** Presses Cut line of the row that reads `text`: the text listens for the click of its buttons, as the browser lets it. */
async function cutLine(app, text) {
  app.main.querySelector('pre').fire('click', { target: cutButton(app, text) });
  await settle();
}

/** The page after the php-fpm crash was chosen and drafted. */
const drafted = async (context, options) => {
  const app = await openReport(context, options);
  await app.choose(PHP);
  return app;
};

/** The Delete button of report `number`. */
const deleteOf = (app, number) => app.main.querySelectorAll('button').find((one) => one.getAttribute('aria-label') === `Delete report ${number}`);

const sha = (text) => createHash('sha256').update(text).digest('hex');

// ---- what is offered ----

test('an administrator is offered the failed start, the crashes not reported (the newest first), the failed service, and the three that are always there', async (context) => {
  const app = await openReport(context);
  assert.equal(app.view.headed.title, 'Report a problem');
  assert.deepEqual(titles(app), [
    'The update to 1.0.0-alpha.2 was taken back', 'php-fpm stopped with an error', 'valkey stopped with an error', 'backup-sync failed',
    'The hardware', 'Installing or updating Dukovo', 'Something else',
  ]);
  assert.deepEqual(app.drafts(), [], 'nothing is drafted before a choice');
  assert.equal(app.main.querySelector('.rp-lead').textContent, 'Choose what went wrong to see the report.');
  assert.equal(app.button('Send').disabled, true);
  assert.equal(app.button('Save to a file…').disabled, true);
  assert.equal(app.button('Download by QR code…').disabled, true);
});

test('a reader is offered neither the failed start nor a service, and is told why', async (context) => {
  const app = await openReport(context, { session: { admin: false } });
  assert.deepEqual(titles(app), ['php-fpm stopped with an error', 'valkey stopped with an error', 'The hardware', 'Installing or updating Dukovo', 'Something else']);
  assert.equal(app.main.querySelector('p.note').hidden, false);
  assert.ok(app.main.textContent.includes('A report about a failed start or about a service is made by an administrator.'));
  assert.equal(app.calls.includes('/api/1/updates/failed'), false, 'a reader does not ask for the failed start');
});

test('the crashes past the fifth are behind Show more, and the chosen one is among those shown', async (context) => {
  const many = Array.from({ length: 7 }, (_, at) => ({
    id: `2026111${at}T000000-${at}`, kind: 'signal', program: `prog${at}`, at: 1_700_000_000 + at, what: 'SIGSEGV', reported: null,
  }));
  const app = await openReport(context, { world: { reads: { '/api/1/reports/crashes': { crashes: many } } } });
  assert.equal(titles(app).filter((one) => one.startsWith('prog')).length, 5);
  await app.press(app.button('Show 2 more'));
  assert.equal(titles(app).filter((one) => one.startsWith('prog')).length, 7);
  assert.equal(app.button('Show 2 more'), null);
});

// ---- drafting ----

test('choosing a crash drafts it with the choice\'s own line as the first answer, and shows the text row by row with the marks in it', async (context) => {
  const app = await drafted(context);
  assert.deepEqual(app.drafts(), [{ about: 'crash', happened: 'php-fpm stopped with an error at 8:30 AM.', crash: '20261113T100211-4012' }]);
  assert.equal(sha(`${app.shown()}\n`), app.desk.kept.values().next().value.sha256, 'the text on the screen is the text reportd holds');
  assert.ok(app.shown().includes('in /users/[user]/site from [IPv4]'));
  const marks = app.main.querySelectorAll('mark').map((one) => [one.textContent, one.classes.has('taken')]);
  assert.deepEqual(marks.slice(0, 2), [['[user]', true], ['[IPv4]', true]]);
  assert.equal(app.button('Send').disabled, false);
  assert.equal(app.button('Save to a file…').disabled, false);
  assert.equal(app.button('Download by QR code…').disabled, false);
});

test('What the report holds names its parts and sizes and what was taken out, and says what the marks are', async (context) => {
  const app = await drafted(context);
  const card = app.main.querySelectorAll('section.card').find((one) => one.querySelector('h2')?.textContent.startsWith('What the report holds'));
  assert.equal(card.hidden, false);
  assert.deepEqual(words(card.querySelectorAll('.rp-parts li')).map((one) => one.replace(/ · \d+ B$/, '')), [
    'What this computer says of itself', 'Your answers', 'Crash', 'Log of the service php (last 200 lines)',
  ]);
  assert.ok(card.textContent.includes('Taken out before anything is shown: 2 account names, 1 address.'));
  assert.ok(app.main.textContent.includes('A mark such as [user] shows what was taken out: 3 in all.'));
  assert.equal(app.main.querySelector('pre').getAttribute('aria-label').startsWith('Exactly what is sent, '), true);
});

test('only what the system gathered has Cut line; what the person typed says so', async (context) => {
  const app = await drafted(context);
  const gathered = app.rows().filter((one) => one.node.classes.has('gathered'));
  assert.equal(gathered.length, 7, 'the crash and the log');
  assert.ok(gathered.every((one) => one.node.querySelector('.rl-cut')));
  const typed = app.main.querySelectorAll('.rl.typed');
  assert.equal(app.main.querySelector('.rl.heading .rl-tag').textContent, 'Typed by you');
  assert.ok(typed.every((one) => one.querySelector('.rl-cut') === null));
  assert.equal(row(app, 'Dukovo 0.16.0, channel stable').node.querySelector('.rl-cut'), null, 'the head is not cut');
  assert.equal(cutButton(app, 'Signature: 3fa2c4d19e0b7a61').getAttribute('aria-label'), 'Cut line 3 of Crash');
});

test('a choice that asks questions drafts when the first answer is left, never on a key, with the answers it was given', async (context) => {
  const app = await openReport(context);
  await app.choose('hardware');
  assert.deepEqual(app.drafts(), [], 'the first answer is needed');
  await app.write('happened', 'The screen goes black', { leave: false });
  assert.deepEqual(app.drafts(), [], 'a key drafts nothing');
  app.main.querySelectorAll('textarea').find((one) => one.id.endsWith('-happened')).fire('change');
  await settle();
  await app.write('expected', 'It stays on');
  assert.deepEqual(app.drafts(), [
    { about: 'hardware', happened: 'The screen goes black' },
    { about: 'hardware', happened: 'The screen goes black', expected: 'It stays on' },
  ]);
  assert.equal(app.rows().filter((one) => one.node.classes.has('typed')).length, 2);
});

test('/report?about=crash&id= chooses that crash and drafts it at once; a note goes after the choice\'s own line', async (context) => {
  const app = await openReport(context, { query: `about=crash&id=${PHP.slice(6)}&note=It%20was%20the%20upload` });
  assert.deepEqual(app.drafts(), [{
    about: 'crash', happened: 'php-fpm stopped with an error at 8:30 AM. It was the upload', crash: '20261113T100211-4012',
  }]);
  assert.equal(app.main.querySelector('.radio-card.on b').textContent, 'php-fpm stopped with an error');
});

test('/report?about=hardware names the choice but waits for the first answer; ?about=service names a failed service', async (context) => {
  const hardware = await openReport(context, { query: 'about=hardware' });
  assert.equal(hardware.main.querySelector('.radio-card.on b').textContent, 'The hardware');
  assert.deepEqual(hardware.drafts(), []);
  const service = await openReport(context, { query: 'about=service&name=backup-sync' });
  assert.deepEqual(service.drafts()[0], { about: 'service', happened: 'backup-sync failed at 7:30 AM.', name: 'backup-sync' });
});

test('/report?about=failure for a reader chooses nothing and says whose it is to send', async (context) => {
  const app = await openReport(context, { query: 'about=failure', session: { admin: false } });
  assert.deepEqual(app.drafts(), []);
  assert.equal(app.main.querySelector('.radio-card.on'), null);
  assert.ok(app.main.querySelector('.rp-lead').textContent.includes('an administrator\'s to send'));
});

test('a failed start is drafted for an administrator with only its own line', async (context) => {
  const app = await openReport(context, { query: 'about=failure' });
  assert.equal(app.drafts()[0].about, 'failure');
  assert.equal(app.drafts()[0].happened, 'The update to 1.0.0-alpha.2 was taken back at 3:30 AM.');
});

test('the security box asks for a security report, turns Save to a file and Download by QR code off and says why, and Send stays on', async (context) => {
  const app = await drafted(context);
  const box = app.main.querySelectorAll('input').find((one) => one.getAttribute('type') === 'checkbox' && one.closest('label').textContent.includes('security problem'));
  box.checked = true;
  box.fire('change');
  await settle();
  assert.equal(app.drafts().at(-1).security, true);
  assert.equal(app.button('Send').disabled, false);
  assert.equal(app.button('Save to a file…').disabled, true);
  assert.equal(app.button('Download by QR code…').disabled, true);
  assert.ok(app.main.querySelector('#report-why').textContent.includes('A security report is sent encrypted only'));
  box.checked = false;
  box.fire('change');
  await settle();
  assert.equal(app.button('Save to a file…').disabled, false);
  assert.equal(app.main.querySelector('#report-why').textContent, '');
});

test('Leave out the logs is offered when the report has a log, and drafts again without it', async (context) => {
  const app = await drafted(context);
  const box = app.main.querySelectorAll('input').find((one) => one.id.endsWith('-logs'));
  assert.equal(box.closest('label').hidden, false);
  box.checked = true;
  box.fire('change');
  await settle();
  assert.equal(app.drafts().at(-1).without_logs, true);
  assert.equal(app.shown().includes('== Log of the service php'), false);
  assert.equal(box.closest('label').hidden, false, 'it stays, to take the logs back');
});

test('the privacy words are in a details element, and the lead says nothing is sent before Send', async (context) => {
  const app = await openReport(context);
  assert.ok(app.main.querySelector('.lead').textContent.startsWith('Nothing leaves this server until you press Send'));
  assert.equal(app.main.querySelector('details summary').textContent, 'What you send when you report a problem');
});

// ---- the cuts ----

test('Cut line asks for the text without that line, as an edit list against the lines as drafted and the hash of the text read', async (context) => {
  const app = await drafted(context);
  const was = app.desk.kept.values().next().value.sha256;
  await cutLine(app, 'in /users/[user]/site from [IPv4]');
  assert.deepEqual(app.redacts(), [{ cuts: [{ section: 'crash', line: 1 }] }]);
  assert.equal(app.posted.at(-1)[1].sha256, was);
  assert.equal(row(app, 'in /users/[user]/site from [IPv4]'), undefined, 'the line is gone');
  assert.ok(app.shown().includes('Cut by the sender: 1 line'), 'the head says so');
  assert.ok(app.main.textContent.includes('You cut 1 line.'));
});

test('the hash that goes with a cut is the hash of the text on the screen', async (context) => {
  const app = await drafted(context);
  const text = `${app.shown()}\n`;
  await cutLine(app, 'Signature: 3fa2c4d19e0b7a61');
  assert.equal(app.posted.at(-1)[1].sha256, sha(text));
});

test('a line cut after another is numbered as the first draft had it, and lines next to each other become one run', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php-fpm stopped: SIGSEGV (address not mapped) at 0x0');
  await cutLine(app, 'in /users/[user]/site from [IPv4]');
  assert.deepEqual(app.redacts().at(-1), { cuts: [{ section: 'crash', line: 0, through: 1 }] });
  await cutLine(app, 'php: signal 11');
  assert.deepEqual(app.redacts().at(-1), { cuts: [{ section: 'crash', line: 0, through: 1 }, { section: 'log', line: 3 }] });
  assert.ok(app.main.textContent.includes('You cut 3 lines.'));
});

test('Undo all my changes asks for the text with no cuts and brings every line back, and is there only when something was changed', async (context) => {
  const app = await drafted(context);
  const first = app.shown();
  assert.equal(app.button('Undo all my changes').hidden, true);
  await cutLine(app, 'php: signal 11');
  assert.equal(app.button('Undo all my changes').hidden, false);
  await app.press(app.button('Undo all my changes'));
  assert.deepEqual(app.redacts().at(-1), { cuts: [] });
  assert.equal(app.shown(), first);
  assert.equal(app.button('Undo all my changes').hidden, true);
});

test('a cut that reportd refuses is shown as its error where the page puts them, and the text stays', async (context) => {
  const app = await drafted(context);
  const was = app.shown();
  app.view.api.post = async () => { throw refusal('RPT-105', 'This part has no such line', 422); };
  await cutLine(app, 'php: signal 11');
  assert.equal(app.problems.at(-1)[0].id, 'RPT-105');
  assert.equal(app.shown(), was);
});

test('Edit your answers shows the three as they are, Done asks for the text with them written again, and Cancel leaves the text', async (context) => {
  const app = await drafted(context);
  await app.press(app.button('Edit your answers'));
  const form = app.main.querySelector('form.rp-editor');
  assert.equal(form.hidden, false);
  assert.equal(app.main.querySelector('pre').hidden, true);
  const [happened, expected, again] = form.querySelectorAll('textarea');
  assert.equal(happened.value, 'php-fpm stopped with an error at 8:30 AM.');
  assert.equal(app.button('Edit your answers').disabled, true);
  await app.press(app.button('Cancel', form));
  assert.equal(form.hidden, true);
  assert.equal(app.main.querySelector('pre').hidden, false);
  assert.deepEqual(app.redacts(), []);
  await app.press(app.button('Edit your answers'));
  happened.value = 'It stopped when alice uploaded a file';
  expected.value = 'It keeps going';
  again.value = '';
  form.fire('submit');
  await settle();
  assert.deepEqual(app.redacts(), [{ cuts: [], answers: { happened: 'It stopped when alice uploaded a file', expected: 'It keeps going', again: '' } }]);
  assert.ok(app.shown().includes('It stopped when [user] uploaded a file'), 'what is typed is redacted too');
  assert.equal(form.hidden, true);
  assert.equal(app.button('Undo all my changes').hidden, false);
});

test('an answer written again keeps the cuts, and the cuts keep the answers', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php: signal 11');
  await app.press(app.button('Edit your answers'));
  const form = app.main.querySelector('form.rp-editor');
  form.querySelectorAll('textarea')[0].value = 'It stopped';
  form.fire('submit');
  await settle();
  assert.deepEqual(app.redacts().at(-1), { cuts: [{ section: 'log', line: 3 }], answers: { happened: 'It stopped', expected: '', again: '' } });
  await cutLine(app, 'php: pool www reloaded');
  assert.deepEqual(app.redacts().at(-1), {
    cuts: [{ section: 'log', line: 2, through: 3 }], answers: { happened: 'It stopped', expected: '', again: '' },
  });
});

test('the first answer cannot be written empty when the choice asks for it, and nothing is asked of reportd', async (context) => {
  const app = await openReport(context);
  await app.choose('hardware');
  await app.write('happened', 'The screen goes black');
  await app.press(app.button('Edit your answers'));
  const form = app.main.querySelector('form.rp-editor');
  form.querySelectorAll('textarea')[0].value = '  ';
  form.fire('submit');
  await settle();
  assert.deepEqual(app.redacts(), []);
  assert.equal(form.hidden, false);
  assert.ok(form.textContent.includes('Say what happened'));
});

// ---- Draft again ----

test('Draft again with cuts asks first, in words that count them, and Keep my cuts leaves the text and the cuts as they are', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php: signal 11');
  const shown = app.shown();
  await app.press(app.button('Draft again'));
  const dialog = app.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Draft again?');
  assert.equal(dialog.querySelector('p').textContent, 'Drafting again brings back the 1 line you cut. Draft again?');
  assert.equal(app.drafts().length, 1);
  await app.press(app.button('Keep my cuts', dialog));
  assert.equal(app.dialog(), null);
  assert.equal(app.drafts().length, 1, 'nothing was drafted');
  assert.equal(app.shown(), shown);
});

test('Draft again after the question drafts the report anew without the cuts', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php: signal 11');
  await app.press(app.button('Draft again'));
  await app.press(app.button('Draft again', app.dialog()));
  assert.equal(app.drafts().length, 2);
  assert.ok(app.shown().includes('php: signal 11'));
  assert.equal(app.button('Undo all my changes').hidden, true);
  assert.equal(app.shown().includes('Cut by the sender'), false);
});

test('Draft again with nothing cut drafts at once, with no question', async (context) => {
  const app = await drafted(context);
  await app.press(app.button('Draft again'));
  assert.equal(app.dialog(), null);
  assert.equal(app.drafts().length, 2);
});

test('choosing another thing after cuts asks the same question, and keeping the cuts puts the choice back', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php: signal 11');
  await app.choose(VALKEY);
  assert.ok(app.dialog(), 'the question is asked');
  await app.press(app.button('Keep my cuts', app.dialog()));
  assert.equal(app.main.querySelector('.radio-card.on b').textContent, 'php-fpm stopped with an error');
  assert.equal(app.drafts().length, 1);
  await app.choose(VALKEY);
  await app.press(app.button('Draft again', app.dialog()));
  assert.equal(app.drafts().length, 2);
  assert.equal(app.drafts().at(-1).crash, '20261112T200000-99');
});

test('the security box after cuts asks first, and keeping the cuts puts the box back', async (context) => {
  const app = await drafted(context);
  await cutLine(app, 'php: signal 11');
  const box = app.main.querySelectorAll('input').find((one) => one.getAttribute('type') === 'checkbox' && one.closest('label').textContent.includes('security problem'));
  box.checked = true;
  box.fire('change');
  await settle();
  assert.ok(app.dialog());
  await app.press(app.button('Keep my cuts', app.dialog()));
  assert.equal(box.checked, false);
  assert.equal(app.drafts().length, 1);
});

test('a first answer left with other words after the answers were written again asks, and the question says the answers come back as first written', async (context) => {
  const app = await openReport(context);
  await app.choose('hardware');
  await app.write('happened', 'The screen goes black');
  await app.press(app.button('Edit your answers'));
  const form = app.main.querySelector('form.rp-editor');
  form.querySelectorAll('textarea')[0].value = 'It goes black';
  form.fire('submit');
  await settle();
  await app.write('expected', 'It stays on');
  assert.equal(app.dialog().querySelector('p').textContent, 'Drafting again brings back your answers as you first wrote them. Draft again?');
  await app.press(app.button('Keep my cuts', app.dialog()));
  assert.equal(app.drafts().length, 1);
  assert.equal(app.main.querySelectorAll('textarea').find((one) => one.id.endsWith('-expected')).value, '');
});

test('Discard puts the page as it was before anything was chosen', async (context) => {
  const app = await drafted(context);
  await app.press(app.button('Discard'));
  assert.equal(app.main.querySelector('.radio-card.on'), null);
  assert.equal(app.main.querySelector('.rp-lead').textContent, 'Choose what went wrong to see the report.');
  assert.equal(app.button('Send').disabled, true);
});

// ---- what a draft that ended does ----

test('a draft that ended is answered with Draft again, and the button drafts the report anew', async (context) => {
  const app = await drafted(context);
  app.desk.kept.clear();
  await cutLine(app, 'php: signal 11');
  const banner = app.main.querySelector('.rp-problem .banner');
  assert.equal(app.problems.length, 0, 'it is not an error page');
  assert.ok(banner.textContent.includes('Draft the report again to go on.'));
  await app.press(app.button('Draft again', banner));
  assert.equal(app.drafts().length, 2);
  assert.equal(app.main.querySelector('.rp-problem').textContent, '');
});

test('reportd not running: no draft is asked, the command that starts it is shown, and Try again asks', async (context) => {
  const app = await openReport(context, { world: { reads: { '/api/1/services': SERVICE_DOWN } } });
  await app.choose(PHP);
  assert.deepEqual(app.drafts(), []);
  const lead = app.main.querySelector('.rp-lead');
  assert.ok(lead.textContent.includes('reportd is not running'));
  assert.equal(lead.querySelector('code').textContent, 'elevate service reportd start');
  assert.equal(lead.querySelector('a').getAttribute('href'), '/services/reportd');
  assert.equal(app.button('Send').disabled, true);
  await app.press(app.button('Try again', lead));
  assert.equal(app.drafts().length, 1);
  assert.equal(app.main.querySelector('.rp-lead').textContent.includes('not running'), false);
});

test('an error that says reportd is not running is the same banner', async (context) => {
  const down = refusal('ADM-123', 'The command failed', 422, { command: { status: 1, stderr: 'report: reportd is not running: elevate service reportd start' } });
  const app = await openReport(context, { world: { posts: { '/api/1/reports/drafts': down } } });
  await app.choose(PHP);
  assert.ok(app.main.querySelector('.rp-lead').textContent.includes('reportd is not running'));
  assert.equal(app.problems.length, 0);
});

test('an admind that does not offer reports.draft says to run report, and drafts nothing', async (context) => {
  const app = await openReport(context, { world: { reads: { '/api/1/actions': { actions: [{ id: 'overview.read' }] } } } });
  await app.choose(PHP);
  assert.deepEqual(app.drafts(), []);
  assert.ok(app.main.querySelector('.rp-lead').textContent.includes('does not offer this yet'));
});

test('another error from reportd is shown in its own words where the page puts them, and the text is cleared', async (context) => {
  const app = await openReport(context, { world: { posts: { '/api/1/reports/drafts': refusal('RPT-110', 'Too many drafts', 429) } } });
  await app.choose(PHP);
  assert.equal(app.problems.at(-1)[0].id, 'RPT-110');
  assert.equal(app.main.querySelector('.rp-lead').textContent, 'Choose what went wrong to see the report.');
});

// ---- Send ----

test('Send gives reportd the draft and the hash of the text read, with the title and the crash, and then says the number and starts over', async (context) => {
  const app = await drafted(context);
  const text = `${app.shown()}\n`;
  await app.press(app.button('Send'));
  const [sent] = app.ran('reports.send');
  assert.equal(sent.sha256, sha(text));
  assert.equal(sent.title, 'php-fpm stopped with an error');
  assert.equal(sent.crash, '20261113T100211-4012');
  assert.match(sent.draft, /^[0-9a-f]{32}$/);
  assert.equal(app.main.querySelector('.rp-status').textContent, 'Sent as report 8.');
  assert.equal(app.main.querySelector('.radio-card.on'), null, 'the page starts over');
  assert.equal(app.main.querySelector('.rp-lead').textContent, 'Choose what went wrong to see the report.');
});

test('the title of a report of a question is the first line of what happened, in at most 80 characters', async (context) => {
  const app = await openReport(context);
  await app.choose('hardware');
  await app.write('happened', `${'It is a long sentence about the screen '.repeat(4)}\nand more`);
  await app.press(app.button('Send'));
  const { title } = app.ran('reports.send')[0];
  assert.equal([...title].length, 80);
  assert.ok(title.endsWith('…'));
  assert.equal(app.ran('reports.send')[0].crash, undefined);
});

test('a crash sent is no longer offered, and the crashes are read again', async (context) => {
  const app = await drafted(context);
  const reads = app.calls.filter((path) => path === '/api/1/reports/crashes').length;
  await app.press(app.button('Send'));
  assert.equal(app.calls.filter((path) => path === '/api/1/reports/crashes').length, reads + 1);
  assert.equal(titles(app).includes('php-fpm stopped with an error'), false);
});

test('a report that waits is said to wait, with why, and Retry now tries the queue', async (context) => {
  const waits = { ok: true, result: sendAnswer({ number: null, state: 'queued', why: 'no route to host' }) };
  const app = await drafted(context, { world: { answers: { 'reports.send': waits } } });
  await app.press(app.button('Send'));
  const banner = app.main.querySelector('.rp-status .banner');
  assert.ok(banner.textContent.includes('Queued: it is sent when reports.dukovo.org can be reached.'));
  assert.ok(banner.textContent.includes('Why: no route to host.'));
  await app.press(app.button('Retry now', banner));
  assert.deepEqual(app.ran('reports.retry'), [{}]);
  assert.ok(app.main.querySelector('.rp-status').textContent.includes('were tried again'));
});

test('a refusal of Send is shown where the page puts it and the text stays; a draft that ended is answered with Draft again', async (context) => {
  const refused = refusal('RPT-103', 'The reports server cannot be reached', 502);
  const app = await drafted(context, { world: { answers: { 'reports.send': refused } } });
  const shown = app.shown();
  await app.press(app.button('Send'));
  assert.equal(app.problems.at(-1)[0], refused);
  assert.equal(app.shown(), shown);
  const ended = refusal('RPT-102', 'The report is not the text that was read', 409);
  app.view.api.run = async () => ({ ok: false, error: ended });
  await app.press(app.button('Send'));
  assert.ok(app.main.querySelector('.rp-problem').textContent.includes('Draft the report again to go on.'));
});

test('the command line of Send is shown with the command of Save only when commands are shown', async (context) => {
  const off = await drafted(context);
  assert.equal(off.main.querySelector('.rp-command').textContent, '');
  const on = await drafted(context, { prefs: { commands: 'shown' } });
  assert.deepEqual(on.inlines.at(-1)[0], 'reports.send');
  assert.equal(on.inlines.at(-1)[1].crash, '20261113T100211-4012');
  assert.ok(on.main.querySelector('.rp-command').textContent.includes('report --save /volumes/USB/report.txt'));
});

// ---- Save to a file ----

test('Save to a file saves the text on the screen under the day\'s name, and says that nothing was sent', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const blobs = [];
  context.mock.method(URL, 'createObjectURL', (blob) => { blobs.push(blob); return 'blob:report'; });
  context.mock.method(URL, 'revokeObjectURL', () => {});
  const app = await drafted(context);
  const clicked = [];
  const link = app.document.body;
  const append = link.append.bind(link);
  link.append = (...nodes) => {
    nodes.forEach((node) => node.addEventListener?.('click', () => clicked.push(node.getAttribute('download'))));
    append(...nodes);
  };
  const text = `${app.shown()}\n`;
  await app.press(app.button('Save to a file…'));
  assert.deepEqual(clicked, ['report-2026-11-16.txt']);
  assert.equal(await blobs[0].text(), text);
  assert.equal(app.ran('reports.send').length, 0);
  assert.equal(app.main.querySelector('.rp-status').textContent, 'Saved as report-2026-11-16.txt. Nothing was sent.');
});

// ---- Download by QR code ----

test('Download by QR code asks for a link for the draft and the hash of the text read, and shows the address, the code, the fingerprint and the time', async (context) => {
  const app = await drafted(context);
  const text = `${app.shown()}\n`;
  app.button('Download by QR code…').click();
  await settle();
  const [path, body] = app.posted.at(-1);
  assert.match(path, /^\/api\/1\/reports\/drafts\/[0-9a-f]{32}\/phone$/);
  assert.deepEqual(body, { sha256: sha(text) });
  const dialog = app.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Download by QR code');
  assert.ok(dialog.textContent.includes('https://192.168.1.20:8443/once/3q2-7wHq9Zk0aB1cD2eF3g/report.txt'));
  assert.ok(dialog.querySelector('svg'), 'the code is drawn');
  assert.ok(dialog.textContent.includes('0A:11:18:1F:26:2D:34:3B'), 'the fingerprint is in groups');
  assert.ok(dialog.querySelector('[role="timer"]'));
  await app.press(app.button('Close', dialog));
  assert.equal(app.dialog(), null);
});

test('a draft that ended in the dialog is answered with Draft again, which closes it and drafts the report anew', async (context) => {
  const app = await drafted(context, { world: { posts: { phone: refusal('RPT-101', 'The draft has ended', 410) } } });
  app.button('Download by QR code…').click();
  await settle();
  const dialog = app.dialog();
  assert.ok(dialog.textContent.includes('Draft the report again to make a link.'));
  await app.press(app.button('Draft again', dialog));
  assert.equal(app.dialog(), null);
  assert.equal(app.drafts().length, 2);
});

test('a refusal of the link is shown in the dialog, with a button that asks again', async (context) => {
  const app = await drafted(context, { world: { posts: { phone: refusal('ADM-105', 'Too many links', 429, { retryAfter: 30 }) } } });
  app.button('Download by QR code…').click();
  await settle();
  assert.equal(app.problems.at(-1)[0].id, 'ADM-105');
  assert.equal(app.button('Try again', app.dialog()).hidden, false);
  await app.press(app.button('Close', app.dialog()));
});

// ---- Reports from this server ----

test('Reports from this server lists the reports that wait first, then those sent, and says where a failed one stands', async (context) => {
  const app = await openReport(context);
  const card = app.main.querySelectorAll('section.card').find((one) => one.querySelector('h2')?.textContent === 'Reports from this server');
  const rows = card.querySelectorAll('tbody tr').map((one) => one.querySelectorAll('td').map((cell) => cell.textContent));
  assert.equal(rows.length, 3);
  assert.equal(rows[0][1], 'php-fpm stopped with an error');
  assert.equal(rows[1][1], 'valkey stopped with an error after a restore');
  assert.equal(rows[2][0], 'Not sent');
  assert.ok(rows[2][2].startsWith('Could not be sent'));
  assert.equal(app.button('Retry now', card).hidden, false);
  assert.equal(app.button('Check the status', card).hidden, false);
});

test('Check the status asks the reports server and shows what it says; when it cannot be asked the table says why', async (context) => {
  const app = await openReport(context);
  await app.press(app.button('Check the status'));
  assert.ok(app.main.querySelector('tbody').textContent.includes('Fixed in valkey 8.0.3'));
  const down = await openReport(context, { world: { reads: { '/api/1/reports/status': refusal('RPT-103', 'The reports server cannot be reached', 502) } } });
  await down.press(down.button('Check the status'));
  assert.ok(down.main.querySelector('tbody').textContent.includes('Could not ask'));
});

test('Delete asks first, and then deletes the report by its number and reads the list again', async (context) => {
  const app = await openReport(context);
  const reads = app.calls.filter((path) => path === '/api/1/reports').length;
  await app.press(deleteOf(app, 7));
  const dialog = app.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Delete report 7?');
  assert.equal(app.ran('reports.delete').length, 0);
  await app.press(app.button('Keep it', dialog));
  assert.equal(app.ran('reports.delete').length, 0);
  await app.press(deleteOf(app, 7));
  await app.press(app.button('Delete', app.dialog()));
  assert.deepEqual(app.ran('reports.delete'), [{ number: 7 }]);
  assert.equal(app.calls.filter((path) => path === '/api/1/reports').length, reads + 1);
});

test('with no report the table says so, and a server without the list says to run report list', async (context) => {
  const none = await openReport(context, { world: { reads: { '/api/1/reports': { format: 1, reports: [] } } } });
  assert.ok(none.main.textContent.includes('No report has been sent from this server.'));
  const older = await openReport(context, { world: { reads: { '/api/1/actions': { actions: [{ id: 'reports.draft' }] } } } });
  assert.ok(older.main.textContent.includes('run report list'));
});

test('a table that cannot be read is an error in its own place, and the rest of the page works', async (context) => {
  const app = await openReport(context, { world: { reads: { '/api/1/reports': refusal('ADM-123', 'The command failed', 422) } } });
  assert.equal(app.problems.length, 1);
  await app.choose(PHP);
  assert.equal(app.drafts().length, 1);
});

// ---- what is kept ----

test('what is typed is kept in this browser until Send or Discard, and read back', async (context) => {
  const store = new Map();
  Object.defineProperty(globalThis, 'localStorage', {
    value: { getItem: (name) => store.get(name) ?? null, setItem: (name, value) => store.set(name, value), removeItem: (name) => store.delete(name) },
    configurable: true,
  });
  context.after(() => { delete globalThis.localStorage; });
  const first = await openReport(context);
  await first.choose('other');
  await first.write('happened', 'The fan is loud', { leave: false });
  assert.equal(JSON.parse(store.get('dukovo-admin.report.note')).happened, 'The fan is loud');
  const back = await openReport(context);
  assert.equal(back.main.querySelectorAll('textarea').find((one) => one.id.endsWith('-happened')).value, 'The fan is loud');
  await back.press(back.button('Discard'));
  assert.equal(store.has('dukovo-admin.report.note'), false);
});

test('while a draft is made the page says so and keeps Send off, and the text comes when reportd answers', async (context) => {
  const late = Promise.withResolvers();
  const app = await openReport(context, { world: { posts: { '/api/1/reports/drafts': () => late.promise } } });
  await app.choose(PHP);
  assert.equal(app.main.querySelector('.rp-lead').textContent, 'Drafting…');
  assert.equal(app.button('Send').disabled, true);
  late.resolve(makeDesk().draft({ happened: 'x', about: 'other' }));
  await settle();
  assert.equal(app.button('Send').disabled, false);
});
