// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Alerts page says as plain values: the channels' words, the kinds of
// alert each is told of, the active and recent alerts and the page each is
// about, a test's lines, and what the email and webhook forms accept.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  channelsWords, joinWords, kindRows, kindsOf, listOf, shortName,
} from '../pages/alerts/channels-model.js';
import {
  emailArgs, emailProblems, isAddress, isPrivateHost, roomProblem, takesToken, tokenProblem, urlProblem, webhookArgs,
} from '../pages/alerts/forms-model.js';
import {
  activeRows, alertdDown, channelRows, lastWords, pageFor, recentRows,
} from '../pages/alerts/model.js';
import {
  deliveriesOf, lastTestWords, NO_CHANNEL_WORDS, testRows,
} from '../pages/alerts/test-model.js';
import {
  aboutArg, channelArg, failing, toldWords, uncoveredWords,
} from '../pages/alerts/told-model.js';

const email = {
  channel: 'email to ops@example.org and 1 more', kind: 'email', host: 'mail.example.org', severity: 'warning', cleared: true, about: null, number: null, id: null, name: null, last_ok: 1000, last_error: null, error_id: null, waiting: 0,
};
const ntfy = {
  channel: 'ntfy (ntfy.sh)', kind: 'ntfy', host: 'ntfy.sh', severity: 'critical', cleared: false, about: ['data', 'stopped'], number: 2, id: null, name: null, last_ok: null, last_error: [2000, 'ntfy answered 403 Forbidden: forbidden'], error_id: 'ALR-102', waiting: 2,
};
const phone = {
  channel: 'push to Firefox on the kitchen tablet', kind: 'push', host: 'push.example.net', severity: 'warning', cleared: false, about: ['hardware'], number: null, id: '0123456789abcdef', name: 'Firefox on the kitchen tablet', last_ok: 3000, last_error: null, error_id: null, waiting: 0,
};
const time = { when: (seconds) => `at ${seconds}` };

test('a list is what alertd answers, and anything else is no lines', () => {
  assert.deepEqual(listOf([1, 2]), [1, 2]);
  assert.deepEqual(listOf(null), []);
  assert.deepEqual(listOf({ output: [] }), []);
});

test('words join with commas and "and"', () => {
  assert.equal(joinWords([]), '');
  assert.equal(joinWords(['email']), 'email');
  assert.equal(joinWords(['email', 'ntfy']), 'email and ntfy');
  assert.equal(joinWords(['a', 'b', 'c']), 'a, b and c');
});

test('a channel is told of every kind unless it names some, and a mistake widens', () => {
  assert.deepEqual(kindsOf(email), ['data', 'stopped', 'hardware']);
  assert.deepEqual(kindsOf(ntfy), ['data', 'stopped']);
  assert.deepEqual(kindsOf({ about: ['hardware', 'data'] }), ['data', 'hardware'], 'in the order of the kinds');
  assert.deepEqual(kindsOf({ about: [] }), ['data', 'stopped', 'hardware']);
  assert.deepEqual(kindsOf({ about: ['colour'] }), ['data', 'stopped', 'hardware']);
  assert.deepEqual(kindsOf({ about: 'data' }), ['data', 'stopped', 'hardware']);
});

test('what a channel is told reads as the page says it', () => {
  assert.equal(toldWords(email), 'Told of: every kind · every alert, and when one clears');
  assert.equal(toldWords(ntfy), 'Told of: data at risk, something stopped · critical only');
  assert.equal(toldWords(phone), 'Told of: hardware · every alert');
});

test('a channel goes by its short name in a sentence', () => {
  assert.equal(shortName(email), 'email');
  assert.equal(shortName(ntfy), 'ntfy (ntfy.sh)');
  assert.equal(channelsWords([email, ntfy]), 'email and ntfy (ntfy.sh)');
  assert.equal(channelsWords([]), null);
});

test('Security\'s rows name the channels told of each kind, and a kind no channel gets is a warning', () => {
  assert.deepEqual(kindRows([email, ntfy]).map((row) => [row.label, row.words, row.tone]), [
    ['Data at risk', 'email and ntfy (ntfy.sh) (critical only)', 'ok'],
    ['Something stopped', 'email and ntfy (ntfy.sh) (critical only)', 'ok'],
    ['Hardware', 'email', 'ok'],
  ]);
  assert.deepEqual(kindRows([ntfy]).map((row) => [row.id, row.words, row.tone]), [
    ['data', 'ntfy (ntfy.sh) (critical only)', 'ok'],
    ['stopped', 'ntfy (ntfy.sh) (critical only)', 'ok'],
    ['hardware', 'no channel', 'warn'],
  ]);
  assert.equal(kindRows([]).every((row) => row.words === 'no channel' && row.tone === 'warn'), true);
});

test('a kind that no channel is told of is one line, and none when no channel is set', () => {
  assert.equal(uncoveredWords([ntfy]), 'Hardware alerts reach no channel: only this server\'s screens.');
  assert.equal(uncoveredWords([{ ...ntfy, about: ['data'] }]), 'Something stopped and hardware alerts reach no channel: only this server\'s screens.');
  assert.equal(uncoveredWords([ntfy, phone]), null);
  assert.equal(uncoveredWords([email]), null);
  assert.equal(uncoveredWords([]), null);
});

test('the about argument is all or the kinds, and the channel argument names the row', () => {
  assert.equal(aboutArg(['data', 'stopped', 'hardware']), 'all');
  assert.equal(aboutArg(['hardware', 'data']), 'data,hardware');
  assert.equal(aboutArg(['data']), 'data');
  assert.equal(aboutArg([]), '');
  assert.equal(channelArg(email), 'email');
  assert.equal(channelArg(ntfy), 'webhook:2');
  assert.equal(channelArg(phone), 'push:0123456789abcdef');
  assert.equal(channelArg({ kind: 'push', id: null }), null);
  assert.equal(channelArg({ kind: 'slack', number: null }), null);
});

test('a channel fails when it has an error and no success after it', () => {
  assert.equal(failing(email), false);
  assert.equal(failing(ntfy), true);
  assert.equal(failing({ last_ok: 5, last_error: [4, 'x'] }), false);
  assert.equal(failing({ last_ok: 4, last_error: [5, 'x'] }), true);
});

test('a test gives a row for each channel, and the last result in one line', () => {
  const answer = {
    done: true,
    changed: true,
    output: ['alerts: test sent', JSON.stringify({
      format: 1,
      deliveries: [
        { channel: 'email', ok: true, error: null, error_id: null },
        { channel: 'ntfy (ntfy.sh)', ok: false, error: 'ntfy answered 403 Forbidden: forbidden', error_id: 'ALR-102' },
      ],
    })],
  };
  const rows = testRows(deliveriesOf(answer));
  assert.deepEqual(rows.map((row) => row.words), ['email: sent', 'ntfy (ntfy.sh): failed: ntfy answered 403 Forbidden: forbidden']);
  assert.equal(rows[1].id, 'ALR-102');
  assert.equal(lastTestWords(rows, '7:20 AM'), 'Last test reached email at 7:20 AM. It failed at ntfy (ntfy.sh).');
  assert.equal(lastTestWords([], '7:20 AM'), NO_CHANNEL_WORDS);
  assert.equal(deliveriesOf({ output: ['nothing'] }), null);
  assert.equal(deliveriesOf(null), null);
  assert.deepEqual(deliveriesOf({ output: [JSON.stringify({ format: 1, deliveries: [] })] }), []);
});

test('each alert opens the page it is about', () => {
  const at = (key) => pageFor(key)?.path ?? null;
  for (const key of ['smart:wd-1', 'smart-rising:wd-1', 'fs-full:/users', 'fs-filling:/shared', 'btrfs-missing:data', 'btrfs-behind:data']) assert.equal(at(key), '/storage', key);
  assert.equal(at('backup-failed:off_site'), '/snapshots/backups');
  assert.equal(at('backup-overdue:off_site'), '/snapshots/backups');
  assert.equal(at('service:web'), '/services/web');
  assert.equal(at('service:odd name/..'), '/services/odd%20name%2F..');
  assert.equal(at('certificate:duke-nas'), '/certificates');
  assert.equal(at('temp:coretemp-isa-0000/Core 0'), '/hardware');
  assert.equal(at('memory-test'), '/hardware');
  for (const key of ['update-failed', 'update-fell-back', 'update-check-wanted', 'start-failed']) assert.equal(at(key), '/updates#happened', key);
  for (const key of ['memory:low', 'kernel-panic', 'test', 'smart', '']) assert.equal(at(key), null, key);
  assert.equal(pageFor('service:web').label, 'web service');
});

test('active alerts come critical first, then the oldest, each with its page', () => {
  const line = (source, key, severity, since) => ({
    source, alert: { key, severity, title: `${key} title`, detail: `${key} detail` }, since, changed: since,
  });
  const rows = activeRows([
    line('metricsd', 'fs-full:/users', 'warning', 100),
    line('diskhealth', 'smart:wd-1', 'critical', 300),
    line('metricsd', 'memory:low', 'warning', 50),
    line('alertd', 'service:web', 'critical', 200),
    null,
    { source: 'x' },
  ], time);
  assert.deepEqual(rows.map((row) => row.id), ['alertd:service:web', 'diskhealth:smart:wd-1', 'metricsd:memory:low', 'metricsd:fs-full:/users']);
  assert.equal(rows[0].severity, 'Critical');
  assert.equal(rows[0].tone, 'bad');
  assert.equal(rows[0].since, 'since at 200');
  assert.equal(rows[0].page.path, '/services/web');
  assert.equal(rows[2].page, null);
  assert.equal(rows[2].tone, 'warn');
  assert.equal(rows[2].detail, 'memory:low detail');
  assert.deepEqual(activeRows([], time), []);
});

test('recent changes are the last twenty, newest first, with a word for each state', () => {
  const lines = Array.from({ length: 25 }, (_, at) => ({
    at: 1000 + at, source: 'metricsd', state: ['raised', 'cleared', 'event', 'test'][at % 4], alert: { key: 'k', severity: at % 2 ? 'critical' : 'warning', title: `title ${at}` },
  }));
  const rows = recentRows([...lines, { at: null, alert: {} }, { at: 5 }], time);
  assert.equal(rows.length, 20);
  assert.equal(rows[0].title, 'title 24');
  assert.equal(rows[0].when, 'at 1024');
  assert.deepEqual(rows.slice(0, 4).map((row) => row.state), ['Raised', 'Test', 'Told', 'Cleared']);
  assert.equal(rows[1].tone, 'bad');
});

test('a channel row has what it is told, how it last did and what waits', () => {
  const rows = channelRows([email, ntfy, phone], time, '0123456789abcdef');
  assert.deepEqual(rows.map((row) => row.name), ['email to ops@example.org and 1 more', 'ntfy (ntfy.sh)', 'push · Firefox on the kitchen tablet']);
  assert.equal(rows[0].sent, 'Last sent at 1000');
  assert.equal(rows[0].failed, null);
  assert.equal(rows[0].bad, false);
  assert.equal(rows[1].failed, 'Last failed at 2000: ntfy answered 403 Forbidden: forbidden');
  assert.equal(rows[1].bad, true);
  assert.equal(rows[1].id, 'ALR-102');
  assert.equal(rows[1].waiting, '2 waiting');
  assert.equal(rows[0].waiting, null);
  assert.deepEqual(rows.map((row) => row.here), [false, false, true]);
  assert.deepEqual(rows.map((row) => row.arg), ['email', 'webhook:2', 'push:0123456789abcdef']);
  assert.equal(channelRows([phone], time, null)[0].here, false);
  assert.equal(lastWords({ last_ok: null, last_error: null }, time).none, true);
});

test('alertd that is not running is told from its socket\'s words', () => {
  assert.equal(alertdDown({ command: { stderr: 'alerts: alertd is not running; try service alertd start\n' } }), true);
  assert.equal(alertdDown({ title: 'The command failed', detail: 'alertd is not running' }), true);
  assert.equal(alertdDown({ title: 'The command failed', detail: 'alertd did not answer within 5 seconds' }), false);
  assert.equal(alertdDown(null), false);
});

test('an address is local@domain in the plain form, as alertd takes it', () => {
  for (const good of ['ops@example.org', 'a.b+c@mail.example.org', 'x@10.0.0.1', 'o\'brien@example.org']) assert.equal(isAddress(good), true, good);
  for (const bad of ['nobody', '@example.org', 'ops@', 'ops@exa mple.org', 'ops@-example.org', 'ops@example.org-', '<ops>@example.org', 'a"b@example.org', 'a,b@example.org', `${'a'.repeat(65)}@example.org`, 'ops@exa_mple.org']) assert.equal(isAddress(bad), false, bad);
});

test('the email form says what is wrong at the field', () => {
  const values = {
    to: 'ops@example.org, admin@example.org\nops@example.org', server: 'mail.example.org', from: '', user: '', port: '587', severity: 'warning', cleared: true, about: ['data', 'stopped', 'hardware'],
  };
  assert.deepEqual(emailProblems(values), {});
  assert.match(emailProblems({ ...values, to: '' }).to, /at least one/);
  assert.match(emailProblems({ ...values, to: 'nobody' }).to, /nobody is not an email address/);
  assert.match(emailProblems({ ...values, to: Array.from({ length: 9 }, (_, at) => `a${at}@example.org`).join('\n') }).to, /at most 8/);
  assert.match(emailProblems({ ...values, server: '' }).server, /name of the mail server/);
  assert.match(emailProblems({ ...values, server: 'mail.example.org:587' }).server, /port goes in Port/);
  assert.match(emailProblems({ ...values, from: 'me' }).from, /not an email address/);
});

test('the email action gets the addresses one a line, and only what was filled in', () => {
  const values = {
    to: 'ops@example.org, admin@example.org\nops@example.org', server: ' mail.example.org ', from: '', user: ' ops ', port: '465', severity: 'critical', cleared: false, about: ['data', 'stopped', 'hardware'],
  };
  assert.deepEqual(emailArgs(values, 'secret'), {
    to: 'ops@example.org\nadmin@example.org', server: 'mail.example.org', port: '465', user: 'ops', severity: 'critical', cleared: false, password: 'secret',
  });
  assert.deepEqual(emailArgs({ ...values, user: '', from: 'a@example.org', about: ['hardware'] }, ''), {
    to: 'ops@example.org\nadmin@example.org', server: 'mail.example.org', port: '465', from: 'a@example.org', severity: 'critical', cleared: false, about: 'hardware',
  });
});

test('a webhook address is https, or http for json on the owner\'s own network', () => {
  assert.equal(urlProblem('ntfy', 'https://ntfy.sh/duke-nas'), null);
  assert.equal(urlProblem('json', 'http://10.0.2.2:8080/alerts'), null);
  assert.equal(urlProblem('json', 'http://localhost:8080/'), null);
  assert.equal(urlProblem('json', 'http://[::1]:8080/'), null);
  assert.match(urlProblem('json', 'http://example.org/alerts'), /on this network/);
  assert.match(urlProblem('slack', 'http://10.0.0.1/x'), /https only/);
  assert.match(urlProblem('ntfy', 'ntfy.sh/topic'), /whole address/);
  assert.match(urlProblem('ntfy', 'ftp://ntfy.sh/topic'), /https/);
  assert.match(urlProblem('ntfy', `https://ntfy.sh/${'a'.repeat(2048)}`), /2,048/);
  for (const host of ['127.0.0.1', '10.1.2.3', '192.168.1.5', '172.16.0.1', '172.31.255.1', '169.254.1.1', 'fd00::1', 'fe80::1', '[fc00::1]']) assert.equal(isPrivateHost(host), true, host);
  for (const host of ['172.32.0.1', '8.8.8.8', 'example.org', '2001:db8::1']) assert.equal(isPrivateHost(host), false, host);
});

test('a webhook token and room are asked for by the kinds that need them', () => {
  assert.equal(tokenProblem('ntfy', ''), null);
  assert.match(tokenProblem('matrix', ''), /needs an access token/);
  assert.match(tokenProblem('ntfy', 'a b'), /space/);
  assert.match(tokenProblem('ntfy', 'a\nb'), /space or a line break/);
  assert.match(tokenProblem('json', 'x'.repeat(1025)), /1,024/);
  assert.equal(roomProblem('matrix', '!abc:example.org'), null);
  assert.match(roomProblem('matrix', 'abc'), /room's ID/);
  assert.equal(roomProblem('ntfy', ''), null);
  assert.deepEqual(['ntfy', 'matrix', 'slack', 'discord', 'json'].map(takesToken), [true, true, false, false, true]);
});

test('the webhook action gets the secrets only in their own members', () => {
  const values = {
    kind: 'matrix', room: ' !abc:example.org ', severity: 'warning', cleared: true, about: ['data', 'stopped', 'hardware'],
  };
  assert.deepEqual(webhookArgs(values, 'https://matrix.example.org/hook', 'tok'), {
    kind: 'matrix', url: 'https://matrix.example.org/hook', token: 'tok', room: '!abc:example.org', severity: 'warning', cleared: true,
  });
  assert.deepEqual(webhookArgs({ ...values, kind: 'slack', room: '!x:y' }, 'https://hooks.example.org/x', 'ignored'), {
    kind: 'slack', url: 'https://hooks.example.org/x', severity: 'warning', cleared: true,
  });
  assert.deepEqual(webhookArgs({ ...values, kind: 'json', about: ['data'] }, '', ''), {
    kind: 'json', url: '', severity: 'warning', cleared: true, about: 'data',
  });
});
