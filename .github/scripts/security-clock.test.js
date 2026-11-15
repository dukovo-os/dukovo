// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const clock = require('./security-clock.js');
const { FakeRepo, fakeContext, fakeCore } = require('./testing.js');

const at = (text) => new Date(text);
const labeled = (name, created_at) => ({ event: 'labeled', label: { name }, created_at });
const unlabeled = (name, created_at) => ({ event: 'unlabeled', label: { name }, created_at });

test('the severity is the most severe label, and several labels are said', () => {
  assert.deepEqual(clock.severityOf([]), { severity: null, several: false });
  assert.deepEqual(clock.severityOf(['security', 'bug']), { severity: null, several: false });
  assert.deepEqual(clock.severityOf(['severity:medium']), { severity: 'medium', several: false });
  assert.deepEqual(clock.severityOf(['severity:low', 'severity:high']), { severity: 'high', several: true });
  assert.deepEqual(clock.severityOf(['severity:critical', 'severity:low', 'severity:medium']), {
    severity: 'critical',
    several: true,
  });
});

test('the clock starts when fix-available was last added, not when the issue was opened', () => {
  assert.equal(clock.clockStart([]), null);
  assert.equal(clock.clockStart([labeled('security', '2026-11-01T00:00:00Z')]), null);
  const added = [labeled('security', '2026-11-01T00:00:00Z'), labeled('fix-available', '2026-11-05T09:30:00Z')];
  assert.equal(clock.clockStart(added).toISOString(), '2026-11-05T09:30:00.000Z');
  // Added, taken off and added again: the last time counts.
  const again = [...added, unlabeled('fix-available', '2026-11-06T00:00:00Z'), labeled('fix-available', '2026-11-09T12:00:00Z')];
  assert.equal(clock.clockStart(again).toISOString(), '2026-11-09T12:00:00.000Z');
  // Taken off after the last time it was added: no clock.
  assert.equal(clock.clockStart([...added, unlabeled('fix-available', '2026-11-06T00:00:00Z')]), null);
  // Other labels' events do not stop the clock, and the order of the list is not trusted.
  const mixed = [unlabeled('needs-triage', '2026-11-20T00:00:00Z'), labeled('fix-available', '2026-11-05T09:30:00Z'), { event: 'closed', created_at: '2026-11-21T00:00:00Z' }];
  assert.equal(clock.clockStart(mixed).toISOString(), '2026-11-05T09:30:00.000Z');
  assert.equal(clock.clockStart([labeled('fix-available', '2026-11-09T12:00:00Z'), ...added.slice(1), unlabeled('fix-available', '2026-11-06T00:00:00Z')]).toISOString(), '2026-11-09T12:00:00.000Z');
});

test('Critical is due 72 hours after the clock starts and High 7 days after', () => {
  const start = at('2026-11-12T08:15:00Z');
  assert.equal(clock.dueAt('critical', start).toISOString(), '2026-11-15T08:15:00.000Z');
  assert.equal(clock.dueAt('high', start).toISOString(), '2026-11-19T08:15:00.000Z');
  // Across a month, a year and a leap day.
  assert.equal(clock.dueAt('critical', at('2026-12-30T20:00:00Z')).toISOString(), '2027-01-02T20:00:00.000Z');
  assert.equal(clock.dueAt('high', at('2026-12-28T00:00:00Z')).toISOString(), '2027-01-04T00:00:00.000Z');
  assert.equal(clock.dueAt('critical', at('2028-02-27T12:00:00Z')).toISOString(), '2028-03-01T12:00:00.000Z');
  assert.equal(clock.dueAt('high', at('2028-02-25T12:00:00Z')).toISOString(), '2028-03-03T12:00:00.000Z');
});

test('Medium and Low are due at the end of the next release day', () => {
  for (const severity of ['medium', 'low']) {
    // The second Tuesday of November 2026 is the 10th; of December, the 8th.
    assert.equal(clock.dueAt(severity, at('2026-11-03T10:00:00Z')).toISOString(), '2026-11-10T23:59:59.999Z');
    assert.equal(clock.dueAt(severity, at('2026-11-09T23:59:59Z')).toISOString(), '2026-11-10T23:59:59.999Z');
    // Started on release day itself: that day's release is cut, so the next one counts.
    assert.equal(clock.dueAt(severity, at('2026-11-10T00:00:00Z')).toISOString(), '2026-12-08T23:59:59.999Z');
    assert.equal(clock.dueAt(severity, at('2026-11-10T15:00:00Z')).toISOString(), '2026-12-08T23:59:59.999Z');
    // Across the end of a year, and a leap day.
    assert.equal(clock.dueAt(severity, at('2026-12-31T23:00:00Z')).toISOString(), '2027-01-12T23:59:59.999Z');
    assert.equal(clock.dueAt(severity, at('2028-02-29T09:00:00Z')).toISOString(), '2028-03-14T23:59:59.999Z');
  }
  assert.equal(clock.dueAt(null, at('2026-11-03T10:00:00Z')), null);
});

test('spans are written in days from two days and in hours below', () => {
  const hours = (count) => count * 3600 * 1000;
  assert.equal(clock.span(0), 'less than an hour');
  assert.equal(clock.span(hours(1) - 1), 'less than an hour');
  assert.equal(clock.span(hours(1)), '1 hour');
  assert.equal(clock.span(hours(47) + 59 * 60 * 1000), '47 hours');
  assert.equal(clock.span(hours(48)), '2 days');
  assert.equal(clock.span(hours(100)), '4 days');
});

test('the state of a clock is overdue, due, not started or without severity', () => {
  const start = at('2026-11-12T08:00:00Z');
  const state = (severity, startedAt, now) => clock.clockOf({ severity, start: startedAt, now: at(now) });

  assert.equal(state('critical', start, '2026-11-13T08:00:00Z').text, 'due in 48 hours'.replace('48 hours', '2 days'));
  assert.equal(state('critical', start, '2026-11-14T20:00:00Z').text, 'due in 12 hours');
  assert.equal(state('critical', start, '2026-11-15T08:00:00Z').state, 'due');
  assert.equal(state('critical', start, '2026-11-15T08:00:00Z').text, 'due in less than an hour');
  assert.equal(state('critical', start, '2026-11-15T08:00:00.001Z').state, 'overdue');
  assert.equal(state('critical', start, '2026-11-15T08:00:00.001Z').text, 'overdue by less than an hour');
  assert.equal(state('critical', start, '2026-11-15T11:00:00Z').text, 'overdue by 3 hours');
  assert.equal(state('high', start, '2026-11-25T08:00:00Z').text, 'overdue by 6 days');
  assert.deepEqual(
    { ...state('low', null, '2026-11-15T00:00:00Z') },
    { state: 'not-started', text: 'clock not started', start: null, due: null },
  );
  assert.equal(state(null, start, '2026-11-15T00:00:00Z').state, 'no-severity');
  assert.equal(state(null, null, '2026-11-15T00:00:00Z').text, 'no severity label');
});

test('the table puts overdue first, then by the time due, then the rest', () => {
  const entry = (number, state, due) => ({ number, state, due: due ? at(due) : null });
  const sorted = clock.sortEntries([
    entry(7, 'no-severity'),
    entry(6, 'not-started'),
    entry(5, 'due', '2026-11-20T00:00:00Z'),
    entry(4, 'due', '2026-11-18T00:00:00Z'),
    entry(3, 'overdue', '2026-11-14T00:00:00Z'),
    entry(2, 'overdue', '2026-11-10T00:00:00Z'),
    entry(1, 'not-started'),
    entry(0, 'no-severity'),
  ]);
  assert.deepEqual(sorted.map((e) => e.number), [2, 3, 4, 5, 1, 6, 0, 7]);
});

test('the issue lists each security issue with its severity, start, due and state', () => {
  const now = at('2026-11-15T12:00:00Z');
  const body = clock.clockBody(
    [
      { number: 12, title: 'Remote code | execution @someone', severity: 'critical', several: false, state: 'overdue', text: 'overdue by 28 hours', start: at('2026-11-11T08:00:00Z'), due: at('2026-11-14T08:00:00Z') },
      { number: 15, title: 'Something', severity: null, several: false, state: 'no-severity', text: 'no severity label', start: null, due: null },
      { number: 16, title: 'Both', severity: 'high', several: true, state: 'not-started', text: 'clock not started', start: null, due: null },
    ],
    now,
  );
  assert.ok(body.startsWith('<!-- security-clock -->'));
  assert.match(body, /\| Issue \| Severity \| Clock started \| Due \| State \|/);
  assert.match(body, /\| #12 Remote code \\\| execution @&#8203;someone \| critical \| 2026-11-11 08:00 UTC \| 2026-11-14 08:00 UTC \| overdue by 28 hours \|/);
  assert.match(body, /\| #15 Something \| none \|  \|  \| no severity label \|/);
  assert.match(body, /\| high \(several labels\) \|/);
  assert.match(body, /Last changed 2026-11-15 12:00 UTC/);
  assert.ok(body.indexOf('#12') < body.indexOf('#16') && body.indexOf('#16') < body.indexOf('#15'));
  assert.match(clock.clockBody([], now), /No security issue is open\./);
});

const NOW = at('2026-11-15T12:00:00Z');

async function runClock(repo, core = fakeCore(), now = NOW) {
  await clock.run({ github: repo.github, context: fakeContext(), core, now });
  return core;
}

/** The issues of the table scenario, numbered 1 to 6. */
function scenario(repo) {
  repo.addIssue({
    title: 'Medium, overdue',
    labels: ['security', 'severity:medium', 'fix-available'],
    events: [labeled('fix-available', '2026-11-03T10:00:00Z')],
  });
  repo.addIssue({
    title: 'Critical, overdue',
    labels: ['security', 'severity:critical', 'fix-available'],
    createdAt: '2026-10-01T00:00:00Z',
    events: [labeled('security', '2026-10-01T00:00:00Z'), labeled('fix-available', '2026-11-11T08:00:00Z')],
  });
  repo.addIssue({
    title: 'High, due',
    labels: ['security', 'severity:high', 'fix-available'],
    events: [labeled('fix-available', '2026-11-12T12:00:00Z')],
  });
  repo.addIssue({ title: 'Low, not started', labels: ['security', 'severity:low'] });
  repo.addIssue({
    title: 'No severity',
    labels: ['security', 'fix-available'],
    events: [labeled('fix-available', '2026-11-14T00:00:00Z')],
  });
  repo.addIssue({
    title: 'High, fix withdrawn',
    labels: ['security', 'severity:high'],
    events: [labeled('fix-available', '2026-11-01T00:00:00Z'), unlabeled('fix-available', '2026-11-02T00:00:00Z')],
  });
}

test('the bot makes the clock issue once, pins it, and labels the late issues', async () => {
  const repo = new FakeRepo();
  scenario(repo);
  const core = await runClock(repo);

  const clockIssue = repo.issues.find((issue) => issue.title === 'Security clock');
  assert.ok(clockIssue);
  assert.deepEqual(repo.labelNamesOf(clockIssue.number), ['security-clock']);
  assert.deepEqual(repo.pinned, [clockIssue.node_id]);
  const rows = clockIssue.body.split('\n').filter((line) => /^\| #/.test(line));
  assert.deepEqual(rows.map((line) => line.match(/^\| #(\d+)/)[1]), ['1', '2', '3', '4', '6', '5']);
  assert.match(rows[0], /overdue by 4 days/);
  assert.match(rows[1], /overdue by 28 hours/);
  assert.match(rows[2], /due in 4 days/);
  assert.match(rows[3], /clock not started/);
  assert.match(rows[5], /no severity label/);

  assert.ok(repo.labelNamesOf(1).includes('overdue'));
  assert.ok(repo.labelNamesOf(2).includes('overdue'));
  assert.ok(!repo.labelNamesOf(3).includes('overdue'));
  assert.match(core.summaryText, /6 open security issues, 2 overdue/);
  for (const name of ['security', 'severity:critical', 'severity:high', 'severity:medium', 'severity:low', 'fix-available', 'overdue', 'security-clock', 'needs-triage']) {
    assert.ok(repo.labels.includes(name), `the label ${name} is made`);
  }
});

test('a second run changes nothing, and never makes a second clock issue', async () => {
  const repo = new FakeRepo();
  scenario(repo);
  await runClock(repo);
  repo.calls.length = 0;
  const core = await runClock(repo);
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issues.filter((issue) => issue.title === 'Security clock').length, 1);
  assert.equal(repo.pinned.length, 1);
  assert.match(core.summaryText, /unchanged/);
});

test('a later run whose table reads the same does not write the issue for its date alone', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'Not started', labels: ['security', 'severity:low'] });
  await runClock(repo);
  repo.calls.length = 0;
  await runClock(repo, fakeCore(), at('2026-11-16T12:00:00Z'));
  assert.deepEqual(repo.mutations(), []);
  assert.match(repo.issue(2).body, /Last changed 2026-11-15 12:00 UTC/);
});

test('the clock issue is written again when the table changes', async () => {
  const repo = new FakeRepo();
  scenario(repo);
  await runClock(repo);
  repo.issue(3).labels.push({ name: 'severity:critical' }, { name: 'security' });
  repo.calls.length = 0;
  await runClock(repo, fakeCore(), at('2026-11-16T12:00:00Z'));
  assert.ok(repo.mutations().includes('issues.update'));
  const clockIssue = repo.issues.find((issue) => issue.title === 'Security clock');
  assert.match(clockIssue.body, /Last changed 2026-11-16 12:00 UTC/);
});

test('a closed clock issue is reopened, not made again', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'Security clock', labels: ['security-clock'], state: 'closed', body: 'old' });
  await runClock(repo);
  assert.equal(repo.issues.length, 1);
  assert.equal(repo.issue(1).state, 'open');
  assert.match(repo.issue(1).body, /No security issue is open/);
  assert.deepEqual(repo.pinned, []);
});

test('the label overdue comes off an issue that is not late any more', async () => {
  const repo = new FakeRepo();
  scenario(repo);
  await runClock(repo);
  assert.ok(repo.labelNamesOf(2).includes('overdue'));
  // Its severity changed from Critical to Low: due at the end of the next release day.
  repo.issue(2).labels = repo.issue(2).labels.filter((label) => label.name !== 'severity:critical');
  repo.issue(2).labels.push({ name: 'severity:low' });
  await runClock(repo, fakeCore(), at('2026-11-15T13:00:00Z'));
  // fix-available was added on 2026-11-11, so the next release day is 2026-12-08.
  assert.ok(!repo.labelNamesOf(2).includes('overdue'));
  // Issue 1 is still late.
  assert.ok(repo.labelNamesOf(1).includes('overdue'));
});

test('the label overdue comes off a closed security issue, and off none that is not one', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'Fixed', labels: ['security', 'overdue'], state: 'closed' });
  repo.addIssue({ title: 'Another kind of overdue', labels: ['overdue'] });
  await runClock(repo);
  assert.ok(!repo.labelNamesOf(1).includes('overdue'));
  assert.ok(repo.labelNamesOf(2).includes('overdue'));
});

test('an issue with fix-available and no event for it counts from when it was opened', async () => {
  const repo = new FakeRepo();
  repo.addIssue({
    title: 'Odd',
    labels: ['security', 'severity:critical', 'fix-available'],
    createdAt: '2026-11-14T12:00:00Z',
  });
  await runClock(repo);
  assert.match(repo.issues.find((issue) => issue.title === 'Security clock').body, /\| #1 Odd \| critical \| 2026-11-14 12:00 UTC \| 2026-11-17 12:00 UTC \| due in 2 days \|/);
});

test('pull requests and the clock issue itself are not on the clock', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'A pull request', labels: ['security', 'severity:high'], pullRequest: true });
  repo.addIssue({ title: 'Security clock', labels: ['security', 'security-clock'] });
  await runClock(repo);
  assert.match(repo.issue(2).body, /No security issue is open/);
});

test('a pin that is refused is a warning, and the issue still exists', async () => {
  const repo = new FakeRepo();
  repo.github.graphql = async () => {
    throw new Error('pinned issues are at their limit');
  };
  const core = await runClock(repo);
  assert.equal(repo.issues.length, 1);
  assert.match(core.warnings[0], /pinned issues are at their limit/);
  assert.match(core.warnings[0], /pin it by hand/);
});

test('with no security issue the table says so', async () => {
  const repo = new FakeRepo();
  const core = await runClock(repo);
  assert.match(repo.issue(1).body, /No security issue is open/);
  assert.match(core.summaryText, /0 open security issues, 0 overdue/);
});
