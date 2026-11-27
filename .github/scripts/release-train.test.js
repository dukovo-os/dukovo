// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const train = require('./release-train.js');
const { FakeRepo, fakeContext, fakeCore, sha } = require('./testing.js');

const CARGO_TOML = `# SPDX header

[workspace]
resolver = "3"
members = ["tools/xtask"]

[workspace.package]
# The Dukovo release version.
version = "0.15.0"
edition = "2024"

[workspace.lints.rust]
unsafe_code = "forbid"
`;

/** A repository with main at version 0.15.0, and the train as configured. */
function repoWith({ enabled } = {}) {
  const repo = new FakeRepo();
  repo.files['Cargo.toml'] = CARGO_TOML;
  if (enabled !== undefined) {
    repo.files['.github/release-train.json'] = JSON.stringify({ enabled });
  }
  return repo;
}

/** Runs the train on `day` (YYYY-MM-DD) and returns what it said. */
async function runOn(repo, day, env = {}) {
  const core = fakeCore();
  await train.run({
    github: repo.github,
    context: fakeContext(),
    core,
    env: { TRAIN_DATE: day, ...env },
    now: new Date('2020-01-01T00:00:00Z'),
  });
  return core;
}

test('only "enabled": true turns the train on', () => {
  assert.deepEqual(train.parseConfig(null), { enabled: false });
  assert.deepEqual(train.parseConfig('{"enabled": false}'), { enabled: false });
  assert.deepEqual(train.parseConfig('{"enabled": true}'), { enabled: true });
  assert.deepEqual(train.parseConfig('{"enabled": "true"}'), { enabled: false });
  assert.deepEqual(train.parseConfig('{}'), { enabled: false });
  assert.throws(() => train.parseConfig('{"enabled": tru'), /not valid JSON/);
  assert.throws(() => train.parseConfig('[true]'), /must hold a JSON object/);
  assert.throws(() => train.parseConfig('null'), /must hold a JSON object/);
});

test('the version is the one of the workspace package table', () => {
  assert.deepEqual(train.workspaceVersion(CARGO_TOML), { text: '0.15.0', major: 0, minor: 15 });
  const alpha = '[package]\nversion = "9.9.9"\n[workspace.package]\nversion = "1.0.0-alpha.1"\n';
  assert.deepEqual(train.workspaceVersion(alpha), { text: '1.0.0-alpha.1', major: 1, minor: 0 });
  // A version in another table, even one that comes first, is not the release's.
  const after = '[workspace.package]\nedition = "2024"\n[workspace.dependencies]\nversion = "7.7.7"\n';
  assert.throws(() => train.workspaceVersion(after), /no version/);
  assert.throws(() => train.workspaceVersion('[package]\nversion = "1.2.3"\n'), /no version/);
  const crlf = '[workspace.package]\r\nversion = "2.10.3"\r\n';
  assert.deepEqual(train.workspaceVersion(crlf), { text: '2.10.3', major: 2, minor: 10 });
});

test('the branch is named for the major and minor numbers', () => {
  assert.equal(train.branchFor({ major: 0, minor: 15 }), 'release/0.15');
  assert.equal(train.trackingTitle('release/0.15'), 'Release 0.15');
  assert.equal(train.nextMinor({ major: 0, minor: 15 }), '0.16.0');
  assert.equal(train.nextMinor({ major: 1, minor: 9 }), '1.10.0');
});

test('release branches are sorted by version, not by name', () => {
  const names = ['main', 'release/0.9', 'release/0.10', 'release/1.0', 'release/foo', 'release/1.2.3', 'releases/2.0', 'release/0.2'];
  assert.deepEqual(
    train.releaseBranches(names).map((branch) => branch.name),
    ['release/1.0', 'release/0.10', 'release/0.9', 'release/0.2'],
  );
  assert.deepEqual(train.releaseBranches([]), []);
});

test('the checklist names the commands of the release and ticks nothing', () => {
  const version = { text: '0.15.0', major: 0, minor: 15 };
  const body = train.issueBody({
    version,
    branch: 'release/0.15',
    head: sha('main-1'),
    cutDay: new Date('2026-11-24T00:00:00Z'),
    releaseDay: new Date('2026-12-08T00:00:00Z'),
  });
  const state = train.checklistState(body);
  assert.equal(state.total, 11);
  assert.equal(state.done, 0);
  for (const wanted of [
    './ob release canaries',
    './ob release sign 0.15.0',
    './ob release deploy out/release-sign/0.15.0',
    './ob release images 0.15.0',
    './ob release channel <repository> stable 0.15.0',
    './ob release deploy <repository>',
    './ob release check',
    './ob security check',
    './ob release advisories <repository>',
    '`v0.15.0` on `release/0.15`',
    '0.16.0',
    'CI passed',
  ]) {
    assert.ok(body.includes(wanted), `the checklist lacks ${wanted}`);
  }
  assert.ok(body.includes('Release day is 2026-12-08'));
  assert.ok(body.includes(sha('main-1').slice(0, 12)));
  assert.ok(body.startsWith('<!-- release-train:release/0.15 -->'));
});

test('ticked and open boxes are counted', () => {
  const body = 'Text\n- [x] one\n- [ ] two\n  * [X] three\n- [ ] four\n- not a box\n';
  assert.deepEqual(train.checklistState(body), { total: 4, done: 2, open: ['two', 'four'] });
  assert.deepEqual(train.checklistState(''), { total: 0, done: 0, open: [] });
  assert.deepEqual(train.checklistState(null), { total: 0, done: 0, open: [] });
});

test('the newest check run named CI passed decides', () => {
  const head = sha('head');
  const run = (extra) => ({ name: 'CI passed', status: 'completed', conclusion: 'success', started_at: '2026-12-01T10:00:00Z', ...extra });
  assert.equal(train.ciState([], head).ok, false);
  assert.match(train.ciState([], head).text, /has not run on/);
  assert.equal(train.ciState([run()], head).ok, true);
  assert.match(train.ciState([run({ conclusion: 'failure' })], head).text, /ended failure/);
  assert.match(train.ciState([run({ status: 'in_progress', conclusion: null })], head).text, /still running/);
  // A run again after a failure: the later one counts.
  const failed = run({ conclusion: 'failure' });
  const again = run({ started_at: '2026-12-02T10:00:00Z' });
  assert.equal(train.ciState([failed, again], head).ok, true);
  assert.equal(train.ciState([again, run({ conclusion: 'cancelled', started_at: '2026-12-03T10:00:00Z' })], head).ok, false);
  // Another check's success is not CI passed.
  assert.equal(train.ciState([{ name: 'Lint', status: 'completed', conclusion: 'success' }], head).ok, false);
});

test('a release that is not green says it skips the month and when the next release day is', () => {
  const report = train.releaseDayReport({
    branch: 'release/0.15',
    today: new Date('2026-12-08T00:00:00Z'),
    checklist: { total: 10, done: 8, open: ['The release notes are written.', 'The release is tagged.'] },
    ci: { ok: false, text: '`CI passed` ended failure on abc' },
    nextRelease: new Date('2027-01-12T00:00:00Z'),
    marker: '<!-- m -->',
  });
  assert.ok(report.startsWith('<!-- m -->'));
  assert.match(report, /Release 0\.15 is not green on its release day, 2026-12-08, so it skips this month/);
  assert.match(report, /The next release day is 2027-01-12/);
  assert.match(report, /8 of 10 checklist boxes are ticked/);
  assert.match(report, /The release notes are written\./);
  assert.match(report, /`CI passed` ended failure on abc/);
});

test('a release with every box ticked and CI passed is green', () => {
  const report = train.releaseDayReport({
    branch: 'release/0.15',
    today: new Date('2026-12-08T00:00:00Z'),
    checklist: { total: 10, done: 10, open: [] },
    ci: { ok: true, text: '`CI passed` succeeded on abc' },
    nextRelease: new Date('2027-01-12T00:00:00Z'),
    marker: '<!-- m -->',
  });
  assert.match(report, /is green on its release day/);
  assert.match(report, /all 10 checklist boxes are ticked/);
  assert.doesNotMatch(report, /skips/);
});

test('an issue with no checklist is not green, even when CI passed', () => {
  const report = train.releaseDayReport({
    branch: 'release/0.15',
    today: new Date('2026-12-08T00:00:00Z'),
    checklist: { total: 0, done: 0, open: [] },
    ci: { ok: true, text: '`CI passed` succeeded on abc' },
    nextRelease: new Date('2027-01-12T00:00:00Z'),
    marker: '<!-- m -->',
  });
  assert.match(report, /no checklist/);
  assert.match(report, /skips this month/);
});

test('on a day with no cut and no release, nothing is changed and the next cut day is said', async () => {
  const repo = repoWith({ enabled: true });
  const core = await runOn(repo, '2026-11-17');
  assert.deepEqual(repo.mutations(), []);
  assert.match(core.summaryText, /Nothing to do today/);
  assert.match(core.summaryText, /cut on 2026-11-24, for the release on 2026-12-08/);
});

test('a dry run on cut day says what it would do and changes nothing', async () => {
  for (const enabled of [false, undefined]) {
    const repo = repoWith({ enabled });
    const core = await runOn(repo, '2026-11-24');
    assert.deepEqual(repo.mutations(), []);
    assert.deepEqual(Object.keys(repo.branches), ['main']);
    assert.equal(repo.issues.length, 0);
    assert.match(core.summaryText, /Dry run/);
    assert.match(core.summaryText, /cut `release\/0\.15` from main at/);
    assert.match(core.summaryText, /open the issue "Release 0\.15"/);
    assert.match(core.summaryText, /\[ \] The release notes are written\./);
  }
});

test('on cut day the branch is cut from the head of main and the issue is opened once', async () => {
  const repo = repoWith({ enabled: true });
  const head = repo.branches.main;
  const core = await runOn(repo, '2026-11-24');
  assert.equal(repo.branches['release/0.15'], head);
  assert.equal(repo.issues.length, 1);
  const [issue] = repo.issues;
  assert.equal(issue.title, 'Release 0.15');
  assert.deepEqual(repo.labelNamesOf(issue.number), ['release']);
  assert.ok(repo.labels.includes('release'));
  assert.equal(train.checklistState(issue.body).total, 11);
  assert.match(core.summaryText, /cut `release\/0\.15`/);
  assert.match(core.summaryText, /opened issue #1/);
  // The version is read at the commit it cuts from, not at a branch name.
  const read = repo.calls.find((call) => call.name === 'repos.getContent' && call.params.path === 'Cargo.toml');
  assert.equal(read.params.ref, head);

  // A second run, the same day or by a dispatch, makes nothing.
  repo.calls.length = 0;
  const again = await runOn(repo, '2026-11-24');
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issues.length, 1);
  assert.match(again.summaryText, /exist: nothing to do/);
});

test('a branch that exists is left alone, and the issue it lacks is opened', async () => {
  const repo = repoWith({ enabled: true });
  repo.branches['release/0.15'] = sha('older');
  await runOn(repo, '2026-11-24');
  assert.equal(repo.branches['release/0.15'], sha('older'));
  assert.equal(repo.issues.length, 1);
  assert.ok(!repo.mutations().includes('git.createRef'));
});

test('an issue that exists, open or closed, is never made again', async () => {
  for (const state of ['open', 'closed']) {
    const repo = repoWith({ enabled: true });
    repo.addIssue({ title: 'Release 0.15', labels: ['release'], state });
    await runOn(repo, '2026-11-24');
    assert.equal(repo.issues.length, 1);
    assert.equal(repo.branches['release/0.15'], repo.branches.main);
    assert.ok(!repo.mutations().includes('issues.create'));
  }
  // A pull request with that title, or an issue without the label, is not the issue.
  const repo = repoWith({ enabled: true });
  repo.addIssue({ title: 'Release 0.15', labels: ['release'], pullRequest: true });
  repo.addIssue({ title: 'Release 0.15', labels: ['question'] });
  await runOn(repo, '2026-11-24');
  assert.equal(repo.issues.length, 3);
});

test('the version on main names the branch', async () => {
  const repo = repoWith({ enabled: true });
  repo.files['Cargo.toml'] = CARGO_TOML.replace('0.15.0', '1.0.0-alpha.1');
  await runOn(repo, '2026-11-24');
  assert.ok(repo.branches['release/1.0']);
  assert.equal(repo.issues[0].title, 'Release 1.0');
  assert.match(repo.issues[0].body, /release sign 1\.0\.0-alpha\.1/);
});

test('a missing version stops the run with a message', async () => {
  const repo = repoWith({ enabled: true });
  repo.files['Cargo.toml'] = '[package]\nname = "x"\n';
  await assert.rejects(runOn(repo, '2026-11-24'), /no version in \[workspace\.package\]/);
  assert.deepEqual(repo.mutations(), []);
});

test('a configuration that is not JSON stops the run before anything is changed', async () => {
  const repo = repoWith();
  repo.files['.github/release-train.json'] = '{"enabled": ';
  await assert.rejects(runOn(repo, '2026-11-24'), /not valid JSON/);
  assert.deepEqual(repo.mutations(), []);
});

test('a bad date from a dispatch is refused', async () => {
  const repo = repoWith({ enabled: true });
  await assert.rejects(runOn(repo, '2026-02-30'), /is not a date/);
  assert.equal(train.runDay(undefined, new Date('2026-11-24T23:59:00Z')).toISOString(), '2026-11-24T00:00:00.000Z');
  assert.equal(train.runDay('', new Date('2026-11-24T23:59:00Z')).toISOString(), '2026-11-24T00:00:00.000Z');
});

/** A repository with release/0.15 cut and its issue open. */
function repoWithTrain() {
  const repo = repoWith({ enabled: true });
  repo.branches['release/0.15'] = sha('release-head');
  const body = train.issueBody({
    version: { text: '0.15.0', major: 0, minor: 15 },
    branch: 'release/0.15',
    head: sha('main-1'),
    cutDay: new Date('2026-11-24T00:00:00Z'),
    releaseDay: new Date('2026-12-08T00:00:00Z'),
  });
  const issue = repo.addIssue({ title: 'Release 0.15', labels: ['release'], body });
  return { repo, issue };
}

const PASSED = { name: 'CI passed', status: 'completed', conclusion: 'success', started_at: '2026-12-07T10:00:00Z' };

test('on release day a release with open boxes is not green and skips the month', async () => {
  const { repo, issue } = repoWithTrain();
  repo.checkRuns[sha('release-head')] = [PASSED];
  const core = await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 1);
  assert.match(issue.comments[0].body, /not green on its release day/);
  assert.match(issue.comments[0].body, /0 of 11 checklist boxes are ticked/);
  assert.match(issue.comments[0].body, /The next release day is 2027-01-12/);
  assert.match(core.summaryText, /Commented on #1/);
  // The issue stays open: the bot comments and changes nothing else.
  assert.equal(issue.state, 'open');
});

test('on release day a release with every box ticked and CI passed is green', async () => {
  const { repo, issue } = repoWithTrain();
  issue.body = issue.body.replaceAll('- [ ]', '- [x]');
  repo.checkRuns[sha('release-head')] = [PASSED];
  await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 1);
  assert.match(issue.comments[0].body, /is green on its release day/);
  assert.doesNotMatch(issue.comments[0].body, /skips/);
});

test('every box ticked is not green while CI passed has failed or not run', async () => {
  for (const runs of [[], [{ ...PASSED, conclusion: 'failure' }]]) {
    const { repo, issue } = repoWithTrain();
    issue.body = issue.body.replaceAll('- [ ]', '- [x]');
    repo.checkRuns[sha('release-head')] = runs;
    await runOn(repo, '2026-12-08');
    assert.match(issue.comments[0].body, /not green/);
    assert.match(issue.comments[0].body, /`CI passed`/);
  }
});

test('the report is said once a release day, and again on the next', async () => {
  const { repo, issue } = repoWithTrain();
  await runOn(repo, '2026-12-08');
  await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 1);
  // The release skipped the month: next month's release day is a new report.
  await runOn(repo, '2027-01-12');
  assert.equal(issue.comments.length, 2);
  assert.match(issue.comments[1].body, /2027-01-12/);
});

test('release day reports on the newest branch, by version', async () => {
  const { repo, issue } = repoWithTrain();
  repo.branches['release/0.9'] = sha('old');
  repo.addIssue({ title: 'Release 0.9', labels: ['release'] });
  await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 1);
  assert.equal(repo.issue(2).comments.length, 0);
});

test('release day says nothing on a closed issue, a missing one or no branch', async () => {
  const { repo, issue } = repoWithTrain();
  issue.state = 'closed';
  const closed = await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 0);
  assert.match(closed.summaryText, /is closed/);

  const noIssue = repoWith({ enabled: true });
  noIssue.branches['release/0.15'] = sha('release-head');
  const missing = await runOn(noIssue, '2026-12-08');
  assert.deepEqual(noIssue.mutations(), []);
  assert.match(missing.summaryText, /no tracking issue/);

  const noBranch = repoWith({ enabled: true });
  const none = await runOn(noBranch, '2026-12-08');
  assert.deepEqual(noBranch.mutations(), []);
  assert.match(none.summaryText, /no release branch/);
});

test('a dry run on release day shows the comment and posts none', async () => {
  const { repo, issue } = repoWithTrain();
  repo.files['.github/release-train.json'] = '{"enabled": false}';
  const core = await runOn(repo, '2026-12-08');
  assert.equal(issue.comments.length, 0);
  assert.deepEqual(repo.mutations(), []);
  assert.match(core.summaryText, /Dry run/);
  assert.match(core.summaryText, /not green on its release day/);
});

test('the run takes its day from the clock when no date is given', async () => {
  const repo = repoWith({ enabled: true });
  const core = fakeCore();
  await train.run({
    github: repo.github,
    context: fakeContext(),
    core,
    env: {},
    now: new Date('2026-11-24T04:41:00Z'),
  });
  assert.ok(repo.branches['release/0.15']);
});
