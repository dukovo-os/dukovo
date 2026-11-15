// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const backport = require('./backport.js');
const { FakeRepo, fakeContext, fakeCore, sha } = require('./testing.js');

const branch = (name) => {
  const [, major, minor] = /^release\/(\d+)\.(\d+)$/.exec(name);
  return { name, major: Number(major), minor: Number(minor) };
};
const NEWEST = branch('release/0.15');
const PREVIOUS = branch('release/0.14');

test('backport is the newest release branch, by version, and backport-previous the one before', () => {
  const branches = [branch('release/0.15'), branch('release/0.14'), branch('release/0.9')];
  const choose = (labels) => backport.chooseTargets({ labels, branches }).targets.map((target) => target.name);
  assert.deepEqual(choose(['backport']), ['release/0.15']);
  assert.deepEqual(choose(['backport', 'severity:high', 'backport-previous']), ['release/0.15', 'release/0.14']);
  assert.deepEqual(choose(['backport-previous', 'severity:critical']), ['release/0.14']);
  assert.deepEqual(choose(['bug']), []);
});

test('backport-previous needs a Critical or High severity, and says so when it has none', () => {
  const branches = [NEWEST, PREVIOUS];
  for (const labels of [['backport', 'backport-previous'], ['backport', 'backport-previous', 'severity:medium'], ['backport', 'backport-previous', 'severity:low']]) {
    const { targets, notes } = backport.chooseTargets({ labels, branches });
    assert.deepEqual(targets.map((target) => target.name), ['release/0.15']);
    assert.equal(notes.length, 1);
    assert.match(notes[0], /Critical and High fixes only/);
  }
  const serious = backport.chooseTargets({ labels: ['backport', 'backport-previous', 'severity:high'], branches });
  assert.deepEqual(serious.notes, []);
});

test('with no release branch, or one, the note says so and nothing is chosen for the missing one', () => {
  const none = backport.chooseTargets({ labels: ['backport', 'backport-previous', 'severity:high'], branches: [] });
  assert.deepEqual(none.targets, []);
  assert.equal(none.notes.length, 1);
  assert.match(none.notes[0], /no release branch yet/);
  const one = backport.chooseTargets({ labels: ['backport', 'backport-previous', 'severity:critical'], branches: [NEWEST] });
  assert.deepEqual(one.targets.map((target) => target.name), ['release/0.15']);
  assert.match(one.notes[0], /no release branch before `release\/0\.15`/);
});

test('the branch and the title name the pull request and the release', () => {
  assert.equal(backport.backportBranch(412, NEWEST), 'backport/412-to-0.15');
  assert.equal(backport.backportTitle(NEWEST, 'Fix the thing (#412)'), '[release/0.15] Fix the thing (#412)');
  assert.equal(backport.backportTitle(NEWEST, 'Two\nlines'), '[release/0.15] Two lines');
  assert.ok(backport.backportTitle(NEWEST, 'x'.repeat(400)).length <= 256);
});

test('signoff lines are read as ob signoff reads them', () => {
  assert.deepEqual(backport.parseIdentity(' Ann Smith <ann@example.org> '), { name: 'Ann Smith', email: 'ann@example.org' });
  assert.equal(backport.parseIdentity('no email'), null);
  assert.equal(backport.parseIdentity('<only@email>'), null);
  assert.equal(backport.parseIdentity('Name <not-an-email>'), null);
});

test('a commit passes when its author signed it off with the same email', () => {
  const ann = { name: 'Ann', email: 'ann@example.org' };
  const check = (author, signoffs, tools = ['tool@example.org']) => backport.signoffProblem({ author, signoffs }, tools);
  assert.equal(check(ann, [ann]), null);
  assert.equal(check(ann, [{ name: 'ANN', email: 'Ann@Example.org' }]), null);
  assert.equal(check(ann, []), 'no Signed-off-by line');
  assert.equal(check(ann, [{ name: 'Bob', email: 'bob@example.org' }]), "no Signed-off-by line with the author's email address");
  // A tool's sign-off is no sign-off, and a tool's commit needs a person's.
  const tool = { name: 'Tool', email: 'tool@example.org' };
  assert.notEqual(check(ann, [tool]), null);
  assert.equal(check(tool, [ann]), null);
  assert.match(check(tool, [tool]), /written by a tool/);
  const bot = { name: 'dependabot[bot]', email: '1+dependabot[bot]@users.noreply.github.com' };
  assert.match(check(bot, [bot], []), /written by a tool/);
});

test('the list of tool identities is the one ob signoff has', () => {
  const sample = 'const X: u8 = 1;\n/// doc\nconst TOOL_EMAILS: &[&str] = &["Tool@Example.org", "other@example.org"];\n';
  assert.deepEqual(backport.toolEmails(sample), ['tool@example.org', 'other@example.org']);
  assert.deepEqual(backport.toolEmails('nothing here'), []);
  // When signoff.rs changes shape, this fails, so that the two do not drift apart.
  const real = fs.readFileSync(path.join(__dirname, '..', '..', 'tools', 'xtask', 'src', 'signoff.rs'), 'utf8');
  const emails = backport.toolEmails(real);
  assert.ok(emails.length > 0);
  assert.ok(emails.every((email) => email.includes('@')));
});

test('quoting and fencing keep a title from changing a command or a block', () => {
  assert.equal(backport.shellQuote("it's"), "'it'\\''s'");
  assert.equal(backport.shellQuote('plain'), "'plain'");
  assert.equal(backport.codeBlock(['a', 'b']), '```sh\na\nb\n```');
  assert.ok(backport.codeBlock(['x ``` y']).startsWith('````sh'));
  const commands = backport.manualCommands({
    pr: { number: 7, title: "Fix 'quotes' and $(rm -rf) `ticks`" },
    target: NEWEST,
    sha: 'a'.repeat(40),
    mainline: true,
    repository: 'dukovo-os/dukovo',
  });
  const text = commands.join('\n');
  assert.match(text, /git cherry-pick -x -m 1 a{40}/);
  assert.match(text, /git switch --no-track -c backport\/7-to-0\.15 origin\/release\/0\.15/);
  assert.match(text, /--title '\[release\/0\.15\] Fix '\\''quotes'\\'' and \$\(rm -rf\) `ticks`'/);
  assert.match(text, /gh workflow run ci\.yml --repo dukovo-os\/dukovo --ref backport\/7-to-0\.15/);
});

const IDENTITY = {
  GIT_AUTHOR_NAME: 'Alice',
  GIT_AUTHOR_EMAIL: 'alice@example.org',
  GIT_COMMITTER_NAME: 'Alice',
  GIT_COMMITTER_EMAIL: 'alice@example.org',
};

/**
 * A remote with `main`, `release/0.15` and `release/0.14`, a merged fix
 * on main, and a checkout of it as the runner has: all in a folder of
 * its own, which `remove` deletes.
 */
function world({ message, conflicting = false, mergeCommit = false } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'backport-test-'));
  const run = (cwd, ...args) => execFileSync('git', args, { cwd, env: { ...process.env, ...IDENTITY }, encoding: 'utf8' }).trim();
  const origin = path.join(dir, 'origin.git');
  const dev = path.join(dir, 'dev');
  const work = path.join(dir, 'work');
  run(dir, 'init', '--quiet', '--bare', '--initial-branch=main', origin);
  run(dir, 'clone', '--quiet', origin, dev);
  fs.writeFileSync(path.join(dev, 'a.txt'), 'one\ntwo\nthree\n');
  fs.writeFileSync(path.join(dev, 'b.txt'), 'x\n');
  run(dev, 'add', '.');
  run(dev, 'commit', '--quiet', '-m', 'Initial');
  run(dev, 'push', '--quiet', 'origin', 'HEAD:refs/heads/main');
  run(dev, 'push', '--quiet', 'origin', 'HEAD:refs/heads/release/0.14');
  run(dev, 'push', '--quiet', 'origin', 'HEAD:refs/heads/release/0.15');
  if (conflicting) {
    run(dev, 'switch', '--quiet', '-c', 'rel', 'origin/release/0.15');
    fs.writeFileSync(path.join(dev, 'a.txt'), 'one\ndos\nthree\n');
    run(dev, 'commit', '--quiet', '-am', 'Change line two on the release branch');
    run(dev, 'push', '--quiet', 'origin', 'HEAD:refs/heads/release/0.15');
    run(dev, 'switch', '--quiet', 'main');
  }
  fs.writeFileSync(path.join(dev, 'a.txt'), 'one\nTWO\nthree\n');
  run(dev, 'commit', '--quiet', '-am', message ?? 'Fix line two (#12)\n\nIt was wrong.\n\nSigned-off-by: Alice <alice@example.org>');
  let fix = run(dev, 'rev-parse', 'HEAD');
  if (mergeCommit) {
    run(dev, 'reset', '--quiet', '--hard', 'HEAD~1');
    run(dev, 'switch', '--quiet', '-c', 'topic');
    fs.writeFileSync(path.join(dev, 'a.txt'), 'one\nTWO\nthree\n');
    run(dev, 'commit', '--quiet', '-am', 'Topic commit');
    run(dev, 'switch', '--quiet', 'main');
    run(dev, 'merge', '--quiet', '--no-ff', 'topic', '-m', 'Merge pull request #12\n\nSigned-off-by: Alice <alice@example.org>');
    fix = run(dev, 'rev-parse', 'HEAD');
  }
  run(dev, 'push', '--quiet', 'origin', 'HEAD:refs/heads/main');
  run(dir, 'clone', '--quiet', origin, work);
  return {
    dir,
    origin,
    work,
    fix,
    run,
    git: (args, options = {}) => backport.realGit(args, { cwd: work, ...options }),
    remove: () => fs.rmSync(dir, { recursive: true, force: true }),
  };
}

/** The fake repository that goes with a world, with pull request `number`. */
function repoFor(w, { labels = ['backport'], merged = true, base = 'main', branches = ['release/0.15', 'release/0.14'] } = {}) {
  const repo = new FakeRepo();
  repo.branches = { main: sha('main') };
  for (const name of branches) {
    repo.branches[name] = sha(name);
  }
  const pull = repo.addPull({ title: 'Fix line two (#12)', labels, merged, base, mergeCommit: merged ? w.fix : null });
  return { repo, pull };
}

async function runBackport(w, repo, pull, options = {}) {
  const core = fakeCore();
  await backport.run({
    github: repo.github,
    context: fakeContext({ pull_request: { number: pull.number } }),
    core,
    git: w.git,
    tools: ['tool@example.org'],
    ...options,
  });
  return core;
}

/** Runs `body` on a world, and removes it after. */
async function inWorld(options, body) {
  const w = world(options);
  try {
    await body(w);
  } finally {
    w.remove();
  }
}

test('a merged pull request labelled backport is cherry-picked onto the newest release branch', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    const core = await runBackport(w, repo, pull);
    const name = `backport/${pull.number}-to-0.15`;

    // The branch is on the remote, one commit above the release branch.
    const base = w.run(w.origin, 'rev-parse', 'refs/heads/release/0.15');
    assert.equal(w.run(w.origin, 'rev-parse', `refs/heads/${name}~1`), base);
    assert.equal(w.run(w.origin, 'show', `refs/heads/${name}:a.txt`), 'one\nTWO\nthree');

    // The commit keeps its author and message, says where it came from,
    // is committed by the bot, and carries the sign-off that ob signoff reads.
    const format = '%an <%ae>|%cn <%ce>|%(trailers:key=Signed-off-by,valueonly,separator=;)';
    const [author, committer, signoff] = w.run(w.origin, 'log', '-1', `--format=${format}`, `refs/heads/${name}`).split('|');
    assert.equal(author, 'Alice <alice@example.org>');
    assert.equal(committer, 'github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com>');
    assert.equal(signoff.trim(), 'Alice <alice@example.org>');
    const message = w.run(w.origin, 'log', '-1', '--format=%B', `refs/heads/${name}`);
    assert.ok(message.startsWith('Fix line two (#12)'));
    assert.ok(message.includes(`(cherry picked from commit ${w.fix})`));

    // The pull request is to the release branch and links the original.
    const created = repo.issues.filter((issue) => issue.pull_request && issue.number !== pull.number);
    assert.equal(created.length, 1);
    assert.equal(created[0].title, '[release/0.15] Fix line two (#12)');
    assert.equal(created[0].base.ref, 'release/0.15');
    assert.equal(created[0].head.ref, name);
    assert.match(created[0].body, new RegExp(`Backport of #${pull.number} \\(${pull.html_url.replaceAll('/', '\\/')}\\)`));
    assert.doesNotMatch(created[0].body, /ob signoff/);

    // CI is started on the branch by a dispatch of ci.yml.
    assert.deepEqual(repo.dispatches, [{ owner: 'dukovo-os', repo: 'dukovo', workflow_id: 'ci.yml', ref: name }]);
    assert.match(core.summaryText, /Opened #\d+/);
    assert.match(core.summaryText, /CI started/);

    // The checkout is clean and back where it started.
    assert.equal(w.run(w.work, 'status', '--porcelain'), '');
    assert.equal(w.run(w.work, 'rev-parse', 'HEAD'), w.run(w.work, 'rev-parse', 'origin/main'));
    assert.ok(repo.labels.includes('backport-conflict'));
  });
});

test('a second run for the same pull request and branch does nothing', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    await runBackport(w, repo, pull);
    const before = w.run(w.origin, 'for-each-ref', '--format=%(refname) %(objectname)');
    repo.calls.length = 0;
    repo.dispatches.length = 0;
    const core = await runBackport(w, repo, pull);
    assert.equal(w.run(w.origin, 'for-each-ref', '--format=%(refname) %(objectname)'), before);
    assert.deepEqual(repo.mutations().filter((name) => !name.startsWith('issues.createLabel')), []);
    assert.deepEqual(repo.dispatches, []);
    assert.match(core.summaryText, /exists already/);
  });
});

test('a pull request that was closed unmerged is not backported again either', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    await runBackport(w, repo, pull);
    const made = repo.issues.find((issue) => issue.pull_request && issue.number !== pull.number);
    made.state = 'closed';
    repo.dispatches.length = 0;
    await runBackport(w, repo, pull);
    assert.equal(repo.issues.filter((issue) => issue.pull_request).length, 2);
    assert.deepEqual(repo.dispatches, []);
  });
});

test('a conflict is aborted, labelled on the original, and the commands are in the summary', async () => {
  await inWorld({ conflicting: true }, async (w) => {
    const { repo, pull } = repoFor(w);
    const core = await runBackport(w, repo, pull);
    const name = `backport/${pull.number}-to-0.15`;
    assert.equal(w.run(w.origin, 'branch', '--list', name), '');
    assert.equal(w.run(w.work, 'status', '--porcelain'), '');
    assert.equal(w.run(w.work, 'rev-parse', 'HEAD'), w.run(w.work, 'rev-parse', 'origin/main'));
    assert.ok(repo.labelNamesOf(pull.number).includes('backport-conflict'));
    assert.equal(repo.issues.filter((issue) => issue.pull_request).length, 1);
    assert.deepEqual(repo.dispatches, []);
    assert.match(core.summaryText, /conflicts in `a\.txt`/);
    assert.match(core.summaryText, new RegExp(`git cherry-pick -x ${w.fix}`));
    assert.match(core.summaryText, new RegExp(`git push origin ${name.replaceAll('/', '\\/')}`));
    assert.match(core.summaryText, /gh workflow run ci\.yml/);
    assert.equal(core.warnings.length, 1);
    // The original says it once, however often the bot runs.
    assert.equal(pull.comments.length, 1);
    assert.match(pull.comments[0].body, /do it by hand/);
    await runBackport(w, repo, pull);
    assert.equal(pull.comments.length, 1);
  });
});

test('a change the release branch has already is not backported', async () => {
  await inWorld({}, async (w) => {
    // The release branch gets the same change by another commit.
    w.run(w.work, 'switch', '--quiet', '-c', 'rel', 'origin/release/0.15');
    fs.writeFileSync(path.join(w.work, 'a.txt'), 'one\nTWO\nthree\n');
    w.run(w.work, 'commit', '--quiet', '-am', 'The same fix, by hand');
    w.run(w.work, 'push', '--quiet', 'origin', 'HEAD:refs/heads/release/0.15');
    w.run(w.work, 'switch', '--quiet', '--detach', 'origin/main');

    const { repo, pull } = repoFor(w);
    const core = await runBackport(w, repo, pull);
    assert.match(core.summaryText, /has the change already/);
    assert.equal(w.run(w.origin, 'branch', '--list', 'backport/*'), '');
    assert.ok(!repo.labelNamesOf(pull.number).includes('backport-conflict'));
    assert.equal(w.run(w.work, 'status', '--porcelain'), '');
  });
});

test('a merge commit is cherry-picked from its first parent', async () => {
  await inWorld({ mergeCommit: true }, async (w) => {
    const { repo, pull } = repoFor(w);
    await runBackport(w, repo, pull);
    const name = `backport/${pull.number}-to-0.15`;
    assert.equal(w.run(w.origin, 'show', `refs/heads/${name}:a.txt`), 'one\nTWO\nthree');
    assert.equal(w.run(w.origin, 'rev-list', '--parents', '-n', '1', `refs/heads/${name}`).split(' ').length, 2);
  });
});

test('a branch pushed by a run that stopped is given its pull request, not cherry-picked again', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    const name = `backport/${pull.number}-to-0.15`;
    w.run(w.work, 'push', '--quiet', 'origin', `origin/release/0.15:refs/heads/${name}`);
    await runBackport(w, repo, pull);
    assert.equal(w.run(w.origin, 'rev-parse', `refs/heads/${name}`), w.run(w.origin, 'rev-parse', 'refs/heads/release/0.15'));
    assert.equal(repo.issues.filter((issue) => issue.pull_request).length, 2);
    assert.equal(repo.dispatches.length, 1);
  });
});

test('a push the server refuses is reported with the commands, and no pull request is opened', async () => {
  await inWorld({}, async (w) => {
    const hook = path.join(w.origin, 'hooks', 'pre-receive');
    fs.writeFileSync(hook, '#!/bin/sh\necho "refusing to allow a GitHub App to update workflow" >&2\nexit 1\n', { mode: 0o755 });
    const { repo, pull } = repoFor(w);
    const core = await runBackport(w, repo, pull);
    assert.equal(repo.issues.filter((issue) => issue.pull_request).length, 1);
    assert.deepEqual(repo.dispatches, []);
    assert.match(core.summaryText, /pushing `backport\/\d+-to-0\.15` was refused/);
    assert.match(core.summaryText, /refusing to allow a GitHub App to update workflow/);
    assert.match(core.summaryText, /gh pr create/);
    assert.ok(repo.labelNamesOf(pull.number).includes('backport-conflict'));
    assert.equal(w.run(w.work, 'status', '--porcelain'), '');
  });
});

test('CI that cannot be started is said in the summary, and the pull request is still there', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    repo.failWith('actions.createWorkflowDispatch', 422, 'Workflow does not have workflow_dispatch trigger');
    const core = await runBackport(w, repo, pull);
    assert.equal(repo.issues.filter((issue) => issue.pull_request).length, 2);
    assert.match(core.summaryText, /CI could not be started \(Workflow does not have workflow_dispatch trigger\)/);
    assert.match(core.summaryText, /gh workflow run ci\.yml --ref backport\//);
  });
});

test('a commit that would fail the sign-off check is opened with a warning', async () => {
  await inWorld({ message: 'Fix line two (#12)\n\nNo sign-off here.' }, async (w) => {
    const { repo, pull } = repoFor(w);
    const core = await runBackport(w, repo, pull);
    const created = repo.issues.find((issue) => issue.pull_request && issue.number !== pull.number);
    assert.match(created.body, /would fail `\.\/ob signoff`: no Signed-off-by line/);
    assert.match(created.body, /git rebase --signoff origin\/release\/0\.15/);
    assert.match(core.summaryText, /would fail `\.\/ob signoff`/);
  });
});

test('backport-previous with a Critical or High severity also goes to the previous branch', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w, { labels: ['backport', 'backport-previous', 'severity:high'] });
    const core = await runBackport(w, repo, pull);
    const heads = repo.issues.filter((issue) => issue.pull_request && issue.number !== pull.number).map((issue) => issue.head.ref);
    assert.deepEqual(heads.sort(), [`backport/${pull.number}-to-0.14`, `backport/${pull.number}-to-0.15`]);
    assert.equal(repo.dispatches.length, 2);
    assert.match(core.summaryText, /`release\/0\.14`/);
    assert.equal(w.run(w.work, 'status', '--porcelain'), '');
  });
});

test('backport-previous without a serious severity says so and leaves the previous branch alone', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w, { labels: ['backport', 'backport-previous', 'severity:medium'] });
    const core = await runBackport(w, repo, pull);
    assert.equal(w.run(w.origin, 'branch', '--list', 'backport/*-to-0.14'), '');
    assert.equal(repo.dispatches.length, 1);
    assert.match(core.summaryText, /Critical and High fixes only/);
  });
});

test('a conflict on one branch does not stop the other', async () => {
  await inWorld({ conflicting: true }, async (w) => {
    const { repo, pull } = repoFor(w, { labels: ['backport', 'backport-previous', 'severity:critical'] });
    const core = await runBackport(w, repo, pull);
    assert.ok(repo.labelNamesOf(pull.number).includes('backport-conflict'));
    assert.equal(w.run(w.origin, 'branch', '--list', `backport/${pull.number}-to-0.14`).trim(), `backport/${pull.number}-to-0.14`);
    assert.equal(repo.dispatches.length, 1);
    assert.match(core.summaryText, /by itself/);
    assert.match(core.summaryText, /Opened #/);
  });
});

test('nothing is done for a pull request that is not merged, not labelled or not for the default branch', async () => {
  await inWorld({}, async (w) => {
    const cases = [
      [{ merged: false }, /not merged/],
      [{ labels: ['bug'] }, /no `backport` or `backport-previous` label/],
      [{ base: 'release/0.15' }, /not `main`/],
    ];
    for (const [options, wanted] of cases) {
      const { repo, pull } = repoFor(w, options);
      const core = await runBackport(w, repo, pull);
      assert.deepEqual(repo.mutations(), [], JSON.stringify(options));
      assert.match(core.summaryText, wanted);
    }
    assert.equal(w.run(w.origin, 'branch', '--list', 'backport/*'), '');
  });
});

test('with no release branch yet the summary says so and the run ends well', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w, { branches: [] });
    const core = await runBackport(w, repo, pull);
    assert.deepEqual(repo.mutations(), []);
    assert.match(core.summaryText, /no release branch yet/);
    assert.deepEqual(core.failures, []);
  });
});

test('a title with shell characters reaches the pull request as it is, and no shell sees it', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    pull.title = 'Fix `x`; $(touch /tmp/never-made) "quoted"';
    await runBackport(w, repo, pull);
    const created = repo.issues.find((issue) => issue.pull_request && issue.number !== pull.number);
    assert.equal(created.title, '[release/0.15] Fix `x`; $(touch /tmp/never-made) "quoted"');
    assert.ok(!fs.existsSync('/tmp/never-made'));
  });
});

test('a pull request with no merge commit is an error, not a guess', async () => {
  await inWorld({}, async (w) => {
    const { repo, pull } = repoFor(w);
    pull.merge_commit_sha = null;
    await assert.rejects(runBackport(w, repo, pull), /no merge commit/);
    assert.deepEqual(repo.mutations(), []);
  });
});
