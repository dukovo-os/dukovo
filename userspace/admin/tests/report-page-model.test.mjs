// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Report a problem page says and asks, as plain values (the Firewall
// page's report-model.test.mjs is another file): the choices and their
// arguments, the counts in words, what Send answered and the table of the
// reports, and the text as rows that tell their part and number, the list of
// cuts reportd takes, and the way a selection goes back to the drafted line.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  CUT, editsOf, gatheredOf, drafted, linesPick, normalize, piecesOf, rangesOf, rowsOf, shownLine, withLines, withWords, wordsPick,
} from '../pages/report/cuts-model.js';
import {
  aboutOf, answersOf, bringsBackWords, choicesOf, draftArgs, emptyFields, fileNameOf, happenedOf, holdsWords, jsonOf, keyOf, listOf,
  marksWords, noteMax, offered, partsOf, QUEUED_WORDS, ready, sendOf, sentWords, shownChoices, statusMap, tableRows, takenOutList,
  takenOutWords, titleOf, whyWords, youCutWords,
} from '../pages/report/model.js';
import {
  apply, crashBase, crashes, failed, list, makeDesk, NOW, services, status,
} from './report-fixtures.mjs';

const time = makeTime('UTC', () => NOW);
const NOW_SECONDS = Math.floor(NOW / 1000);
const choices = (more = {}) => choicesOf({
  failed: failed(), crashes: crashes(), show: services(), now: NOW_SECONDS, time, admin: true, ...more,
});

// ---- the address ------------------------------------------------------

test('/report?about= names a choice, with what it needs', () => {
  const read = (search) => aboutOf(new URLSearchParams(search));
  assert.deepEqual(read('about=failure'), {
    about: 'failure', crash: null, name: null, note: '',
  });
  assert.deepEqual(read('about=crash&id=20261113T100211-4012'), {
    about: 'crash', crash: '20261113T100211-4012', name: null, note: '',
  });
  assert.deepEqual(read('about=service&name=nginx'), {
    about: 'service', crash: null, name: 'nginx', note: '',
  });
  assert.equal(read('about=hardware').about, 'hardware');
  assert.deepEqual(read('about=other&note=It%20does%20not%20boot'), {
    about: 'other', crash: null, name: null, note: 'It does not boot',
  });
});

test('the address says updates, and the action says update', () => {
  assert.equal(aboutOf(new URLSearchParams('about=updates')).about, 'update');
  assert.equal(aboutOf(new URLSearchParams('about=update')).about, 'update');
});

test('a crash with no id, a service with no name and a word that is no kind choose nothing', () => {
  for (const search of ['', 'about=crash', 'about=service', 'about=service&name=%20', 'about=bug', 'id=1']) {
    assert.equal(aboutOf(new URLSearchParams(search)), null, search);
  }
});

test('a note from the address is held to the answer\'s limit', () => {
  assert.equal(aboutOf(new URLSearchParams(`about=other&note=${'x'.repeat(3000)}`)).note.length, 2000);
});

// ---- what can be reported ------------------------------------------------------

test('the choices are the failed start, the crashes not reported, the failed services, then the three that are always there', () => {
  assert.deepEqual(choices().map((choice) => choice.key), [
    'failure', 'crash:20261113T100211-4012', 'crash:20261112T200000-99', 'service:backup-sync', 'hardware', 'update', 'other',
  ]);
});

test('each choice has its words', () => {
  const [failure, crash, , service, hardware, update, other] = choices();
  assert.equal(failure.title, 'The update to 1.0.0-alpha.2 was taken back');
  assert.equal(failure.words, 'Found by this server at 3:30 AM: it failed the check BOOT-103.');
  assert.equal(failure.line, 'The update to 1.0.0-alpha.2 was taken back at 3:30 AM.');
  assert.equal(crash.title, 'php-fpm stopped with an error');
  assert.equal(crash.words, '8:30 AM · SIGSEGV (address not mapped) in ngx_http_core_run_phases');
  assert.equal(crash.line, 'php-fpm stopped with an error at 8:30 AM.');
  assert.equal(service.title, 'backup-sync failed');
  assert.equal(service.words, '7:30 AM · its log goes with the report');
  assert.equal(service.line, 'backup-sync failed at 7:30 AM.');
  assert.deepEqual([hardware.title, update.title, other.title], ['The hardware', 'Installing or updating Dukovo', 'Something else']);
  assert.deepEqual([hardware.questions, update.questions, other.questions, crash.questions], [true, true, true, false]);
});

test('a crash of another day is told by its day', () => {
  const [, , older] = choices();
  assert.equal(older.line, 'valkey stopped with an error on Nov 12 at 9:30 AM.');
});

test('a reader is offered neither the failed start nor a service', () => {
  const keys = choices({ admin: false }).map((choice) => choice.key);
  assert.deepEqual(keys, ['crash:20261113T100211-4012', 'crash:20261112T200000-99', 'hardware', 'update', 'other']);
  assert.deepEqual(choices({ admin: true }).filter((choice) => choice.admin).map((choice) => choice.about), ['failure', 'service']);
});

test('a start that failed without going back is told by its number, and one with no failed start is no choice', () => {
  const other = failed();
  other.failed.result = 'failed';
  other.failed.release = null;
  assert.equal(choices({ failed: other })[0].title, 'Start 42 failed');
  assert.equal(choices({ failed: { failed: null } })[0].key, 'crash:20261113T100211-4012');
  assert.equal(choices({ failed: null })[0].key, 'crash:20261113T100211-4012');
});

test('what cannot be read is no choice, and the three that are always there stay', () => {
  const keys = choices({ crashes: null, show: null, failed: null }).map((choice) => choice.key);
  assert.deepEqual(keys, ['hardware', 'update', 'other']);
  assert.deepEqual(choices({ crashes: [{ id: 1 }, { program: 'x' }, null], failed: null, show: null }).map((choice) => choice.key), ['hardware', 'update', 'other']);
});

test('a list of crashes may be the answer itself', () => {
  assert.deepEqual(listOf([1, 2], 'crashes'), [1, 2]);
  assert.deepEqual(listOf({ crashes: [3] }, 'crashes'), [3]);
  assert.deepEqual(listOf({ crashes: 'no' }, 'crashes'), []);
  assert.deepEqual(listOf(null, 'crashes'), []);
});

test('only the five newest crashes are shown, and the rest behind Show more, unless one of them is chosen', () => {
  const many = Array.from({ length: 8 }, (_, at) => ({
    id: `c${at}`, program: `p${at}`, at: 1000 - at, what: 'SIGSEGV', reported: null,
  }));
  const all = choicesOf({ crashes: many, now: NOW_SECONDS, time, admin: false });
  const keys = (shown) => shown.shown.map((choice) => choice.key);
  assert.deepEqual(keys(shownChoices(all, { all: false, keep: null })), ['crash:c0', 'crash:c1', 'crash:c2', 'crash:c3', 'crash:c4', 'hardware', 'update', 'other']);
  assert.equal(shownChoices(all, { all: false, keep: null }).more, 3);
  assert.ok(keys(shownChoices(all, { all: false, keep: 'crash:c7' })).includes('crash:c7'));
  assert.equal(shownChoices(all, { all: true, keep: null }).more, 0);
  assert.equal(keys(shownChoices(all, { all: true, keep: null })).length, 11);
});

test('a choice has a key that the address can find again', () => {
  assert.equal(keyOf({ about: 'crash', crash: 'a1' }), 'crash:a1');
  assert.equal(keyOf({ about: 'service', name: 'web' }), 'service:web');
  assert.equal(keyOf({ about: 'hardware' }), 'hardware');
});

// ---- the answers ------------------------------------------------------

const crashChoice = () => choices().find((choice) => choice.about === 'crash');
const otherChoice = () => choices().find((choice) => choice.about === 'other');
const fields = (more) => ({ ...emptyFields(), ...more });

test('for a crash the first answer is the choice\'s own line and the note after it', () => {
  assert.equal(happenedOf(crashChoice(), fields({ note: 'The photos site uses a custom PHP pool.' })), 'php-fpm stopped with an error at 8:30 AM. The photos site uses a custom PHP pool.');
  assert.equal(happenedOf(crashChoice(), fields()), 'php-fpm stopped with an error at 8:30 AM.');
  assert.equal(happenedOf(otherChoice(), fields({ happened: '  The screen goes black.  ' })), 'The screen goes black.');
});

test('a draft can be made for a crash at once and for the others once they say what happened', () => {
  assert.equal(ready(crashChoice(), fields()), true);
  assert.equal(ready(otherChoice(), fields()), false);
  assert.equal(ready(otherChoice(), fields({ happened: '  ' })), false);
  assert.equal(ready(otherChoice(), fields({ happened: 'It froze' })), true);
  assert.equal(ready(null, fields({ happened: 'It froze' })), false);
});

test('the note leaves room for the choice\'s line in the answer\'s 2,000 characters', () => {
  assert.equal(noteMax(crashChoice()), 2000 - 'php-fpm stopped with an error at 8:30 AM.'.length - 1);
  assert.equal(noteMax(otherChoice()), 2000 - 1);
});

test('the draft\'s arguments are the choice\'s, with each answer that was written and each box that is checked', () => {
  assert.deepEqual(draftArgs(crashChoice(), fields({ note: 'More.' })), { about: 'crash', crash: '20261113T100211-4012', happened: 'php-fpm stopped with an error at 8:30 AM. More.' });
  assert.deepEqual(draftArgs(otherChoice(), fields({ happened: 'It froze', expected: ' It stays on ', again: '' }), { security: true, withoutLogs: true }), {
    about: 'other', happened: 'It froze', expected: 'It stays on', without_logs: true, security: true,
  });
  const service = choices().find((choice) => choice.about === 'service');
  assert.deepEqual(draftArgs(service, fields(), {}), { about: 'service', name: 'backup-sync', happened: 'backup-sync failed at 7:30 AM.' });
  assert.deepEqual(draftArgs(choices()[0], fields(), {}), { about: 'failure', happened: 'The update to 1.0.0-alpha.2 was taken back at 3:30 AM.' });
});

test('a note typed for a choice that asks one question is not sent as an answer to the others', () => {
  const args = draftArgs(crashChoice(), fields({ expected: 'x', again: 'y', note: 'n' }));
  assert.equal(args.expected, undefined);
  assert.equal(args.again, undefined);
});

test('the answers as the person has them are the three the edit form starts with', () => {
  assert.deepEqual(answersOf(otherChoice(), fields({ happened: 'a', expected: ' b ', again: 'c' })), { happened: 'a', expected: 'b', again: 'c' });
  assert.deepEqual(answersOf(crashChoice(), fields({ note: 'n' })), { happened: 'php-fpm stopped with an error at 8:30 AM. n', expected: '', again: '' });
});

test('the title is the choice\'s line, or the first line of what happened, in at most 80 characters', () => {
  assert.equal(titleOf(crashChoice(), 'x'), 'php-fpm stopped with an error');
  assert.equal(titleOf(otherChoice(), '\nThe screen goes black.\nSecond line'), 'The screen goes black.');
  assert.equal(titleOf(otherChoice(), ''), 'Something else');
  const long = titleOf(otherChoice(), 'é'.repeat(100));
  assert.equal([...long].length, 80);
  assert.ok(long.endsWith('…'));
});

test('the file is named by the day in the person\'s zone', () => {
  assert.equal(fileNameOf('UTC', Date.UTC(2026, 10, 16, 23, 30)), 'report-2026-11-16.txt');
  assert.equal(fileNameOf('Asia/Tokyo', Date.UTC(2026, 10, 16, 23, 30)), 'report-2026-11-17.txt');
});

// ---- what the report holds ------------------------------------------------------

test('what was taken out is counted in words, the most first, the two kinds of address as one', () => {
  assert.equal(takenOutList({ ipv4: 2, ipv6: 1, user: 2, host: 1 }), '3 addresses, 2 account names, 1 host name');
  assert.equal(takenOutList({ mac: 1, email: 2, wifi: 1, serial: 3, machine: 1 }), '3 serial numbers, 2 email addresses, 1 hardware address, 1 Wi-Fi network name, 1 machine ID');
  assert.equal(takenOutList({ you: 4 }), '');
  assert.equal(takenOutList(undefined), '');
});

test('the sentence says what was taken out, or that nothing was', () => {
  assert.equal(takenOutWords({ ipv4: 3, user: 2, host: 1 }), 'Taken out before anything is shown: 3 addresses, 2 account names, 1 host name.');
  assert.equal(takenOutWords({}), 'Nothing needed to be taken out.');
});

test('the parts are listed with their sizes in words', () => {
  const answer = apply(crashBase());
  assert.deepEqual(partsOf(answer).map((part) => part.title), [
    'What this computer says of itself', 'Your answers', 'Crash', 'Log of the service php (last 200 lines)',
  ]);
  assert.deepEqual(partsOf(answer).map((part) => part.size), ['109 B', '59 B', '124 B', '126 B']);
  assert.equal(holdsWords(answer), '418 B');
  assert.deepEqual(partsOf(null), []);
});

test('the marks are explained before the text', () => {
  assert.equal(marksWords({ user: 2, ipv4: 1 }, 0), 'A mark such as [user] shows what was taken out: 3 in all.');
  assert.equal(marksWords({ user: 2 }, 3), 'A mark such as [user] shows what was taken out: 2 in all. [cut] shows what you cut: 3.');
});

test('what the person cut is in words', () => {
  assert.equal(youCutWords({ lines: 12, parts: 3 }), 'You cut 12 lines and 3 parts of lines.');
  assert.equal(youCutWords({ lines: 1, parts: 0 }), 'You cut 1 line.');
  assert.equal(youCutWords({ lines: 0, parts: 1 }), 'You cut 1 part of a line.');
  assert.equal(youCutWords({ lines: 0, parts: 0 }), '');
  assert.equal(youCutWords(undefined), '');
});

test('the question before drafting again says what comes back', () => {
  assert.equal(bringsBackWords({ lines: 12, parts: 0 }, false), 'Drafting again brings back the 12 lines you cut. Draft again?');
  assert.equal(bringsBackWords({ lines: 2, parts: 1 }, false), 'Drafting again brings back the 2 lines and 1 part of a line you cut. Draft again?');
  assert.equal(bringsBackWords({ lines: 0, parts: 0 }, true), 'Drafting again brings back your answers as you first wrote them. Draft again?');
  assert.equal(bringsBackWords({ lines: 1, parts: 0 }, true), 'Drafting again brings back the 1 line you cut and your answers as you first wrote them. Draft again?');
});

test('a security report turns Save to a file and Download by QR code off, and says why', () => {
  assert.deepEqual(offered({ has: true, security: false }), {
    send: true, save: true, qr: true, why: null,
  });
  assert.deepEqual(offered({ has: true, security: true }), {
    send: true, save: false, qr: false, why: 'A security report is sent encrypted only',
  });
  assert.deepEqual(offered({ has: false, security: false }), {
    send: false, save: false, qr: false, why: null,
  });
});

// ---- Send ------------------------------------------------------

test('what Send answers is the report\'s number, or that it waits', () => {
  const sent = { done: true, changed: true, output: ['sent', '{"format":1,"number":8,"state":"sent","why":null}'] };
  assert.deepEqual(sendOf(sent), { number: 8, queued: false, why: null });
  assert.deepEqual(sendOf({ format: 1, number: null, state: 'queued', why: 'the report desk did not answer' }), { number: null, queued: true, why: 'the report desk did not answer' });
  assert.equal(sendOf({ done: true, output: ['words only'] }), null);
  assert.equal(sendOf({ done: true, output: ['{"state":"lost"}'] }), null);
  assert.equal(sendOf(null), null);
});

test('the JSON of a change is the last line of its output that is an object', () => {
  assert.deepEqual(jsonOf({ done: true, output: ['{"a":1}', 'words', '{"b":2}', '[1]'] }), { b: 2 });
  assert.deepEqual(jsonOf({ format: 1, x: 1 }), { format: 1, x: 1 });
  assert.equal(jsonOf({ done: true, output: [] }), null);
  assert.equal(jsonOf(undefined), null);
});

test('the words for a report sent and for one that waits', () => {
  assert.equal(sentWords(7), 'Sent as report 7.');
  assert.equal(sentWords(null), 'Sent.');
  assert.match(QUEUED_WORDS, /^Queued: it is sent when reports\.dukovo\.org can be reached\. reportd tries again after 1, 5, 15 and 60 minutes, then every hour for 30 days\.$/);
});

// ---- Reports from this server ------------------------------------------------------

test('the table has the reports that wait first, then those sent, the newest first', () => {
  const rows = tableRows(list(), { time });
  assert.deepEqual(rows.map((row) => [row.number, row.sent, row.title]), [
    [null, 'Waiting', 'php-fpm stopped with an error'],
    [7, 'Nov 2 9:30 AM', 'valkey stopped with an error after a restore'],
    [5, 'Not sent', 'The screen goes black'],
  ]);
});

test('a report that waits says when it is sent and how often it was tried', () => {
  const [queued] = tableRows(list(), { time });
  assert.equal(queued.status, 'Queued');
  assert.equal(queued.kind, 'off');
  assert.equal(queued.note, 'Sends when reports.dukovo.org can be reached · tried 4 times, next at 9:45 AM');
  assert.equal(queued.canDelete, false);
  const why = tableRows({ reports: [{ ...list().reports[1], why: 'no route to host', tries: 1, next_ms: null }] }, { time })[0];
  assert.equal(why.note, 'Sends when reports.dukovo.org can be reached · tried 1 time, last: no route to host');
});

test('a report sent that was not asked about says so, and Check the status fills the column', () => {
  const sent = (options) => tableRows(list(), { time, ...options }).find((row) => row.number === 7);
  assert.equal(sent({}).status, 'Not asked yet');
  assert.equal(sent({}).canDelete, true);
  const asked = sent({ status: statusMap(status('Fixed in valkey 8.0.3')) });
  assert.deepEqual([asked.status, asked.kind], ['Fixed in valkey 8.0.3', 'ok']);
  assert.deepEqual([sent({ status: statusMap(status('Will not be fixed: it works as designed')) }).kind], ['warn']);
  assert.deepEqual([sent({ status: statusMap(status('A duplicate of report 3')) }).kind], ['off']);
  assert.equal(sent({ status: statusMap(status(null)) }).status, 'Received');
  assert.equal(sent({ status: statusMap({ reports: [{ number: 7, unreachable: true, status: null }] }) }).status, 'Could not ask: reports.dukovo.org did not answer');
});

test('when the server could not be asked each report sent says why', () => {
  const rows = tableRows(list(), { time, asked: { why: 'The console cannot reach the server' } });
  assert.equal(rows.find((row) => row.number === 7).status, 'Could not ask: The console cannot reach the server');
  assert.equal(rows.find((row) => row.number === 7).kind, 'warn');
  assert.equal(rows.find((row) => row.queued).status, 'Queued');
});

test('a report that could not be sent in 30 days says so', () => {
  const failedRow = tableRows(list(), { time }).find((row) => row.number === 5);
  assert.deepEqual([failedRow.status, failedRow.kind, failedRow.note, failedRow.canDelete], ['Could not be sent', 'bad', 'It was tried for 30 days.', false]);
});

test('no reports is no rows, and a list that cannot be read is none', () => {
  assert.deepEqual(tableRows({ format: 1, reports: [] }, { time }), []);
  assert.deepEqual(tableRows(null, { time }), []);
});

test('why the server could not be asked is the error\'s words', () => {
  assert.equal(whyWords({ title: 'The console cannot reach the server', detail: 'The browser got no answer.' }), 'The console cannot reach the server: The browser got no answer.');
  assert.equal(whyWords({ title: 'Nope' }), 'Nope');
  assert.equal(whyWords(null), 'no answer');
});

// ---- the text as rows ------------------------------------------------------

const drafted1 = () => apply(crashBase());

test('each line of the text is a row that tells its part and its number in the part', () => {
  const rows = rowsOf(drafted1());
  assert.equal(rows.length, 16);
  const of = (n) => [rows[n].kind, rows[n].section, rows[n].at];
  assert.deepEqual(of(0), ['head', 'head', 0]);
  assert.deepEqual(of(3), ['head', 'head', 3]);
  assert.deepEqual(of(4), ['plain', null, null]);
  assert.deepEqual(of(5), ['heading', 'happened', null]);
  assert.deepEqual(of(6), ['typed', 'happened', 0]);
  assert.deepEqual(of(7), ['heading', 'crash', null]);
  assert.deepEqual(of(8), ['gathered', 'crash', 0]);
  assert.deepEqual(of(10), ['gathered', 'crash', 2]);
  assert.deepEqual(of(11), ['heading', 'log', null]);
  assert.deepEqual(of(15), ['gathered', 'log', 3]);
  assert.equal(rows[5].typed, true);
  assert.equal(rows[7].typed, false);
  assert.equal(rows[8].text, 'php-fpm stopped: SIGSEGV (address not mapped) at 0x0');
});

test('the rows of a text with no answer or no log are as the sections say', () => {
  const rows = rowsOf(apply(crashBase({ happened: '' }, { logs: false })));
  assert.deepEqual(rows.filter((row) => row.kind === 'typed'), []);
  assert.equal(rows.filter((row) => row.kind === 'gathered').length, 3);
  assert.deepEqual(rowsOf(null), []);
});

test('the lines the system gathered are kept as the first draft has them', () => {
  const original = gatheredOf(drafted1());
  assert.deepEqual([...original.keys()], ['crash', 'log']);
  assert.equal(original.get('log').length, 4);
  assert.equal(original.get('crash')[1], 'in /users/[user]/site from [IPv4]');
});

test('a line is pieces: plain words, what was taken out, and what was cut', () => {
  assert.deepEqual(piecesOf('in /users/[user]/site from [IPv4]'), [
    { text: 'in /users/', mark: null }, { text: '[user]', mark: 'taken' }, { text: '/site from ', mark: null }, { text: '[IPv4]', mark: 'taken' },
  ]);
  assert.deepEqual(piecesOf('[cut]pm stopped'), [{ text: '[cut]', mark: 'cut' }, { text: 'pm stopped', mark: null }]);
  assert.deepEqual(piecesOf('no marks [here]'), [{ text: 'no marks [here]', mark: null }]);
  assert.deepEqual(piecesOf(''), []);
});

// ---- the list of cuts ------------------------------------------------------

test('lines cut next to each other are one run, and a line alone has no through', () => {
  let cuts = [];
  cuts = withLines(cuts, 'log', 2);
  cuts = withLines(cuts, 'log', 0);
  cuts = withLines(cuts, 'log', 1);
  assert.deepEqual(cuts, [{ section: 'log', line: 0, through: 2 }]);
  cuts = withLines(cuts, 'log', 3, 3);
  assert.deepEqual(cuts, [{ section: 'log', line: 0, through: 3 }]);
  assert.deepEqual(withLines([], 'crash', 1), [{ section: 'crash', line: 1 }]);
  assert.deepEqual(withLines(withLines([], 'log', 5), 'log', 1), [{ section: 'log', line: 1 }, { section: 'log', line: 5 }]);
});

test('words cut that touch or overlap are one cut, as reportd merges them', () => {
  let cuts = withWords([], 'crash', 0, 12, 20);
  cuts = withWords(cuts, 'crash', 0, 3, 5);
  cuts = withWords(cuts, 'crash', 0, 5, 7);
  cuts = withWords(cuts, 'crash', 0, 18, 30);
  assert.deepEqual(cuts, [
    { section: 'crash', line: 0, from: 3, to: 7 }, { section: 'crash', line: 0, from: 12, to: 30 },
  ]);
});

test('words in a line that is cut whole are not kept, and the cuts are in the order of the parts', () => {
  let cuts = withWords([], 'log', 1, 0, 3);
  cuts = withWords(cuts, 'crash', 2, 4, 8);
  cuts = withLines(cuts, 'log', 1);
  assert.deepEqual(cuts, [{ section: 'crash', line: 2, from: 4, to: 8 }, { section: 'log', line: 1 }]);
});

test('the edit list is the cuts, and the answers only when they were written anew', () => {
  const cuts = withLines([], 'log', 0, 1);
  assert.deepEqual(editsOf(cuts), { cuts: [{ section: 'log', line: 0, through: 1 }] });
  assert.deepEqual(editsOf([]), { cuts: [] });
  assert.deepEqual(editsOf(cuts, { happened: 'a', expected: '', again: '' }), { cuts, answers: { happened: 'a', expected: '', again: '' } });
});

test('a line of the text is told back to the line that was drafted, past the lines cut whole', () => {
  const cuts = withLines(withLines([], 'log', 0, 1), 'log', 3);
  assert.equal(drafted(cuts, 'log', 0), 2);
  assert.equal(drafted([], 'log', 3), 3);
  assert.equal(drafted(cuts, 'crash', 1), 1);
  assert.equal(drafted(cuts, 'log', 1), 4);
});

test('a line reads with [cut] in place of the words cut', () => {
  assert.equal(shownLine('nginx stopped', [[0, 5]]), '[cut] stopped');
  assert.equal(shownLine('nginx stopped', [[0, 2], [6, 13]]), `${CUT}inx ${CUT}`);
  assert.equal(shownLine('é😀x', [[1, 2]]), `é${CUT}x`);
  assert.equal(shownLine('abc', []), 'abc');
  assert.deepEqual(rangesOf(withWords([], 'crash', 1, 2, 3), 'crash', 1), [[2, 3]]);
});

// ---- what was selected ------------------------------------------------------

const place = (cuts = []) => ({ cuts, original: gatheredOf(drafted1()) });
const rowsAfter = (cuts) => {
  const desk = makeDesk();
  const first = desk.draft({ about: 'crash', happened: 'php-fpm stopped with an error at 8:09 AM.' });
  const answer = desk.redact(first.draft, { sha256: first.sha256, edits: JSON.stringify(editsOf(cuts)) });
  return rowsOf(answer);
};

test('words selected in a line are a cut of the drafted line, by code points', () => {
  const rows = rowsAfter([]);
  const row = rows.find((one) => one.section === 'crash' && one.at === 1);
  assert.deepEqual(wordsPick(place(), row, 3, 9), { section: 'crash', line: 1, from: 3, to: 9 });
  assert.equal(wordsPick(place(), row, 4, 4), null);
  assert.equal(wordsPick(place(), row, 9, 3), null);
});

test('a selection counts a letter outside the first plane once', () => {
  const original = new Map([['log', ['a😀b c', 'd']]]);
  const row = {
    kind: 'gathered', section: 'log', at: 0, text: 'a😀b c',
  };
  assert.deepEqual(wordsPick({ cuts: [], original }, row, 3, 4), {
    section: 'log', line: 0, from: 2, to: 3,
  });
});

test('words selected after an earlier cut are placed in the drafted line', () => {
  const cuts = withWords([], 'crash', 1, 3, 9);
  const row = rowsAfter(cuts).find((one) => one.section === 'crash' && one.at === 1);
  assert.equal(row.text, 'in [cut]/[user]/site from [IPv4]');
  // "site" is at 15 to 19 as shown, 20 to 24 as drafted.
  assert.deepEqual(wordsPick(place(cuts), row, 15, 19), { section: 'crash', line: 1, from: 16, to: 20 });
  // Words before the cut are where they were.
  assert.deepEqual(wordsPick(place(cuts), row, 0, 2), { section: 'crash', line: 1, from: 0, to: 2 });
});

test('a selection that begins or ends inside [cut] takes the words it stands for whole', () => {
  const cuts = withWords([], 'crash', 1, 3, 9);
  const row = rowsAfter(cuts).find((one) => one.section === 'crash' && one.at === 1);
  assert.deepEqual(wordsPick(place(cuts), row, 5, 12), { section: 'crash', line: 1, from: 3, to: 13 });
  assert.deepEqual(wordsPick(place(cuts), row, 0, 6), { section: 'crash', line: 1, from: 0, to: 9 });
});

test('a row whose text is not what the cuts make of the drafted line is not cut by words', () => {
  const row = {
    kind: 'gathered', section: 'crash', at: 1, text: 'something else',
  };
  assert.equal(wordsPick(place(), row, 0, 4), null);
});

test('only what the system gathered is cut by words or by lines', () => {
  const rows = rowsAfter([]);
  assert.equal(wordsPick(place(), rows[0], 0, 4), null);
  assert.equal(wordsPick(place(), rows[6], 0, 4), null);
  assert.equal(linesPick(place(), [rows[6]]), null);
  assert.equal(linesPick(place(), [rows[7]]), null);
  assert.equal(linesPick(place(), []), null);
});

test('rows selected are a run of lines of one part, past the lines already cut', () => {
  const rows = rowsAfter([]);
  const log = rows.filter((row) => row.section === 'log' && row.kind === 'gathered');
  assert.deepEqual(linesPick(place(), [log[1], log[2]]), { section: 'log', line: 1, through: 2 });
  assert.deepEqual(linesPick(place(), [log[2], log[1]]), { section: 'log', line: 1, through: 2 });
  assert.deepEqual(linesPick(place(), [log[0]]), { section: 'log', line: 0, through: 0 });
  assert.equal(linesPick(place(), [log[0], rows.find((row) => row.section === 'crash' && row.kind === 'gathered')]), null);
  const cuts = withLines([], 'log', 0);
  const after = rowsAfter(cuts).filter((row) => row.section === 'log' && row.kind === 'gathered');
  assert.deepEqual(linesPick(place(cuts), [after[0]]), { section: 'log', line: 1, through: 1 });
});
