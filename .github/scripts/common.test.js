// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const common = require('./common.js');
const { FakeRepo, fakeContext, fakeCore } = require('./testing.js');

test('every label the bots use has a colour and a description GitHub accepts', () => {
  const wanted = [
    'backport',
    'backport-previous',
    'backport-conflict',
    'security',
    'severity:critical',
    'severity:high',
    'severity:medium',
    'severity:low',
    'fix-available',
    'overdue',
    'security-clock',
    'release',
    'sbom-scan',
    'needs-triage',
  ];
  assert.deepEqual(Object.keys(common.LABELS).sort(), [...wanted].sort());
  for (const [name, label] of Object.entries(common.LABELS)) {
    assert.match(label.color, /^[0-9a-f]{6}$/, name);
    assert.ok(label.description.length > 0 && label.description.length <= 100, `${name}: ${label.description.length} characters`);
  }
});

test('the severity labels are named for the severities, most severe first', () => {
  assert.deepEqual(common.SEVERITIES, ['critical', 'high', 'medium', 'low']);
  assert.equal(common.severityLabel('high'), 'severity:high');
});

test('only the labels that are missing are made, and the others are left as they are', async () => {
  const repo = new FakeRepo();
  repo.labels.push('security', 'Overdue');
  const created = await common.ensureLabels({ github: repo.github, context: fakeContext() }, ['security', 'overdue', 'sbom-scan']);
  assert.deepEqual(created, ['sbom-scan']);
  assert.deepEqual(repo.labels, ['security', 'Overdue', 'sbom-scan']);
  const call = repo.calls.find((candidate) => candidate.name === 'issues.createLabel');
  assert.equal(call.params.color, common.LABELS['sbom-scan'].color);
  assert.equal(call.params.description, common.LABELS['sbom-scan'].description);
  assert.deepEqual(await common.ensureLabels({ github: repo.github, context: fakeContext() }, ['sbom-scan']), []);
});

test('a label made by someone else in the meantime is not an error, and another error is', async () => {
  const repo = new FakeRepo();
  repo.failWith('issues.createLabel', 422, 'already_exists');
  assert.deepEqual(await common.ensureLabels({ github: repo.github, context: fakeContext() }, ['release']), []);
  repo.failWith('issues.createLabel', 403, 'Resource not accessible by integration');
  await assert.rejects(common.ensureLabels({ github: repo.github, context: fakeContext() }, ['release']), /not accessible/);
});

test('a label the bots do not define is a mistake in the bot', async () => {
  const repo = new FakeRepo();
  await assert.rejects(common.ensureLabels({ github: repo.github, context: fakeContext() }, ['no-such-label']), /no label is defined for: no-such-label/);
});

test('text from outside is one line, and is cut to its length', () => {
  assert.equal(common.oneLine('  two\n\nlines\twith   gaps  '), 'two lines with gaps');
  assert.equal(common.oneLine(null), '');
  assert.equal(common.oneLine(undefined), '');
  assert.equal(common.oneLine('abcdefghij', 8), 'abcde...');
  assert.equal(common.oneLine('abcdefgh', 8), 'abcdefgh');
});

test('text for a body or a table has no table bar and no mention', () => {
  assert.equal(common.inline('a | b'), 'a \\| b');
  assert.equal(common.inline('thanks @maintainers and @org/team'), 'thanks @&#8203;maintainers and @&#8203;org/team');
  assert.equal(common.inline('x\ny'), 'x y');
});

test('labels are read from names and from objects', () => {
  assert.deepEqual(common.labelNames({ labels: [{ name: 'a' }, 'b'] }), ['a', 'b']);
  assert.deepEqual(common.labelNames({}), []);
});

test('the summary goes to the log and to the job summary', async () => {
  const core = fakeCore();
  await common.summarize(core, 'Hello');
  assert.deepEqual(core.infos, ['Hello']);
  assert.equal(core.summaryText, 'Hello\n');
  const bare = { infos: [], info: (text) => bare.infos.push(text) };
  await common.summarize(bare, 'Only the log');
  assert.deepEqual(bare.infos, ['Only the log']);
});
