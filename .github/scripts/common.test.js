// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const common = require('./common.js');
const { ApiError, FakeRepo, fakeContext, fakeCore } = require('./testing.js');

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

test('an at sign inside a word is no mention, as in a package URL', () => {
  assert.equal(common.inline('pkg:generic/openssl@3.5.4'), 'pkg:generic/openssl@3.5.4');
  assert.equal(common.inline('mail a@example.org, @maintainers'), 'mail a@example.org, @&#8203;maintainers');
  assert.equal(common.inline('(@maintainers) and x|@y'), '(@&#8203;maintainers) and x\\|@&#8203;y');
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

/** A writer that records its pauses in milliseconds instead of waiting. */
function recordedWriter(options = {}) {
  const sleeps = [];
  const write = common.pacedWriter({ sleep: async (ms) => sleeps.push(ms), ...options });
  return { sleeps, write };
}

const SECONDARY = 'You have exceeded a secondary rate limit and have been temporarily blocked from content creation.';

/** A request that fails with `errors`, one for each call, and then returns `result`. */
function failingRequest(errors, result = 'done') {
  const request = async () => {
    request.calls += 1;
    if (errors.length > 0) {
      throw errors.shift();
    }
    return result;
  };
  request.calls = 0;
  return request;
}

test('a writer leaves a second between its writes, and none before the first', async () => {
  const { sleeps, write } = recordedWriter();
  const order = [];
  const request = (name) => async () => {
    order.push(name);
    return `${name} written`;
  };
  assert.equal(await write(request('a')), 'a written');
  assert.deepEqual(sleeps, []);
  await write(request('b'));
  await write(request('c'));
  assert.deepEqual(order, ['a', 'b', 'c']);
  assert.deepEqual(sleeps, [1000, 1000]);
  const quick = recordedWriter({ gapMs: 250 });
  await quick.write(async () => {});
  await quick.write(async () => {});
  assert.deepEqual(quick.sleeps, [250]);
});

test('a secondary rate limit is waited out for the seconds of retry-after, and the write is tried again', async () => {
  const { sleeps, write } = recordedWriter();
  const request = failingRequest([new ApiError(403, SECONDARY, { 'retry-after': '37' })]);
  assert.equal(await write(request), 'done');
  assert.equal(request.calls, 2);
  assert.deepEqual(sleeps, [37000]);
  // The next write keeps its gap after the one that was tried again.
  await write(async () => {});
  assert.deepEqual(sleeps, [37000, 1000]);
});

test('a secondary rate limit with no retry-after is waited out for a minute', async () => {
  const { sleeps, write } = recordedWriter();
  await write(failingRequest([new ApiError(403, SECONDARY)]));
  assert.deepEqual(sleeps, [60000]);
  const slow = recordedWriter({ waitMs: 90000 });
  await slow.write(failingRequest([new ApiError(403, 'You have exceeded a SECONDARY RATE LIMIT.')]));
  assert.deepEqual(slow.sleeps, [90000]);
});

test('a 429 or a retry-after names a secondary rate limit even when the message does not', async () => {
  const { sleeps, write } = recordedWriter();
  await write(failingRequest([new ApiError(429, 'Too Many Requests', { 'retry-after': '5' })]));
  await write(failingRequest([new ApiError(403, 'Forbidden', { 'retry-after': '8' })]));
  assert.deepEqual(sleeps, [5000, 1000, 8000]);
});

test('a write that is limited every time is given up after its tries, and its error is thrown', async () => {
  const { sleeps, write } = recordedWriter();
  const error = new ApiError(403, SECONDARY, { 'retry-after': '2' });
  const request = failingRequest([error, error, error, error]);
  await assert.rejects(write(request), (thrown) => thrown === error);
  assert.equal(request.calls, 3);
  assert.deepEqual(sleeps, [2000, 2000]);

  const once = recordedWriter({ tries: 1 });
  const single = failingRequest([new ApiError(403, SECONDARY)]);
  await assert.rejects(once.write(single), /secondary rate limit/);
  assert.equal(single.calls, 1);
  assert.deepEqual(once.sleeps, []);
});

test('any other error is thrown at once, and so is a limit that is not the secondary one', async () => {
  for (const error of [
    new ApiError(403, 'Resource not accessible by integration'),
    new ApiError(403, 'API rate limit exceeded for installation', { 'x-ratelimit-remaining': '0' }),
    new ApiError(404, 'Not Found', { 'retry-after': '3' }),
    new ApiError(422, 'Validation Failed'),
    new ApiError(500, 'Server Error'),
    new Error('socket hang up'),
  ]) {
    const { sleeps, write } = recordedWriter();
    const request = failingRequest([error]);
    await assert.rejects(write(request), (thrown) => thrown === error);
    assert.equal(request.calls, 1);
    assert.deepEqual(sleeps, []);
  }
});

test('the labels are made through the writer when the caller has one', async () => {
  const repo = new FakeRepo();
  const { sleeps, write } = recordedWriter();
  const created = await common.ensureLabels({ github: repo.github, context: fakeContext(), write }, ['security', 'overdue', 'release']);
  assert.deepEqual(created, ['security', 'overdue', 'release']);
  assert.deepEqual(sleeps, [1000, 1000]);
  repo.failWith('issues.createLabel', 403, SECONDARY, { 'retry-after': '4' });
  assert.deepEqual(await common.ensureLabels({ github: repo.github, context: fakeContext(), write }, ['sbom-scan']), ['sbom-scan']);
  assert.deepEqual(sleeps.slice(2), [1000, 4000]);
});
