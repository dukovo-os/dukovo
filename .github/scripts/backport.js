// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The backport bot (ADR 0534), run by .github/workflows/backport.yml when
// a pull request is merged or labelled.
//
// A merged pull request labelled `backport` is cherry-picked onto the
// newest release branch, and one labelled `backport-previous` onto the
// one before it, which takes Critical and High fixes only. For each
// branch the bot pushes `backport/<number>-to-<X.Y>`, opens a pull request
// to the release branch, and starts CI on it by a dispatch, since a push
// by the workflow's own token starts no workflow.
//
// The bot runs in the checkout of the default branch and never fetches or
// runs a pull request's own code: it cherry-picks the commit the merge
// made on the default branch, which review has accepted. The title of the
// pull request is text from outside, so it reaches the API and the
// summary through `oneLine` and `inline`, and no shell.
//
// When it misbehaves, read the job summary: it says for each branch what
// happened, and for a conflict or a refused push, the commands to do the
// backport by hand.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { execFile } = require('node:child_process');

const { ensureLabels, inline, labelNames, oneLine, summarize } = require('./common.js');
const { releaseBranches } = require('./release-train.js');

const COMMITTER = {
  name: 'github-actions[bot]',
  email: '41898282+github-actions[bot]@users.noreply.github.com',
};
const SERIOUS = ['severity:critical', 'severity:high'];
const SIGNOFF_SOURCE = path.join(__dirname, '..', '..', 'tools', 'xtask', 'src', 'signoff.rs');

/** Runs git with `args`; the answer is its exit code and output, never a throw. */
function realGit(args, { env = {}, cwd = process.cwd() } = {}) {
  return new Promise((resolve) => {
    execFile('git', args, { cwd, env: { ...process.env, ...env }, maxBuffer: 256 * 1024 * 1024 }, (error, stdout, stderr) => {
      if (!error) {
        resolve({ code: 0, stdout, stderr });
      } else if (typeof error.code === 'number') {
        resolve({ code: error.code, stdout, stderr });
      } else {
        resolve({ code: 1, stdout, stderr: `${stderr}${error.message}` });
      }
    });
  });
}

/** Runs git and throws, with what git said, when it fails. */
async function must(git, args, options) {
  const result = await git(args, options);
  if (result.code !== 0) {
    throw new Error(`git ${args.join(' ')} failed (${result.code}): ${result.stderr.trim() || result.stdout.trim()}`);
  }
  return result;
}

/** The name of the branch that carries a backport. */
function backportBranch(number, target) {
  return `backport/${number}-to-${target.major}.${target.minor}`;
}

/** The title of a backport's pull request. */
function backportTitle(target, title) {
  return oneLine(`[${target.name}] ${title}`, 250);
}

/**
 * The release branches a merged pull request is backported to, and what to
 * say of the ones it is not. `backport` is the newest branch and
 * `backport-previous` the one before it, which takes only a pull request
 * of Critical or High severity. `branches` are newest first.
 */
function chooseTargets({ labels, branches }) {
  const targets = [];
  const notes = [];
  if (labels.includes('backport')) {
    if (branches.length === 0) {
      notes.push('There is no release branch yet: there is nothing to backport to.');
    } else {
      targets.push(branches[0]);
    }
  }
  if (labels.includes('backport-previous')) {
    if (!labels.some((name) => SERIOUS.includes(name))) {
      notes.push('`backport-previous` takes Critical and High fixes only, and this pull request has neither `severity:critical` nor `severity:high`: nothing is done for the previous release branch.');
    } else if (branches.length < 2) {
      notes.push(
        branches.length === 0
          ? 'There is no release branch yet: there is nothing to backport to.'
          : `There is no release branch before \`${branches[0].name}\`: nothing is done for the previous one.`,
      );
    } else {
      targets.push(branches[1]);
    }
  }
  return { targets, notes: [...new Set(notes)] };
}

/** `Name <email>` as a Signed-off-by line has it, or null. */
function parseIdentity(text) {
  const match = /^([^<]*)<([^>]*)>$/.exec(text.trim());
  if (!match) {
    return null;
  }
  const name = match[1].trim();
  const email = match[2].trim();
  return name && email.includes('@') ? { name, email } : null;
}

/**
 * The identities of tools that write commits but cannot certify the
 * Developer Certificate of Origin, from `ob signoff`'s own list, so that
 * the two do not differ.
 */
function toolEmails(signoffSource) {
  const list = /TOOL_EMAILS\s*:\s*&\[&str\]\s*=\s*&\[([^\]]*)\]/.exec(signoffSource);
  return list ? [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1].toLowerCase()) : [];
}

/**
 * Why a commit would fail `./ob signoff` (tools/xtask/src/signoff.rs),
 * or null when it would pass: its author, or a person who submits it for
 * a tool, signs it off with the author's email address.
 */
function signoffProblem(commit, tools) {
  const isTool = (identity) => tools.includes(identity.email.toLowerCase()) || identity.name.endsWith('[bot]');
  const people = commit.signoffs.filter((signoff) => !isTool(signoff));
  if (isTool(commit.author)) {
    return people.length === 0 ? 'written by a tool, so it needs a Signed-off-by line from the person submitting it' : null;
  }
  if (people.some((signoff) => signoff.email.toLowerCase() === commit.author.email.toLowerCase())) {
    return null;
  }
  return commit.signoffs.length === 0 ? 'no Signed-off-by line' : "no Signed-off-by line with the author's email address";
}

/** `text` as one word for a POSIX shell. */
function shellQuote(text) {
  return `'${text.replace(/'/g, "'\\''")}'`;
}

/**
 * A code block of the job summary: the fence is longer than any run of
 * backticks in the text, so that the text cannot end it.
 */
function codeBlock(lines) {
  const longest = Math.max(0, ...(lines.join('\n').match(/`+/g) ?? []).map((run) => run.length));
  const fence = '`'.repeat(Math.max(3, longest + 1));
  return `${fence}sh\n${lines.join('\n')}\n${fence}`;
}

/** The commands that do a backport by hand, for the summary. */
function manualCommands({ pr, target, sha, mainline, repository }) {
  const branch = backportBranch(pr.number, target);
  return [
    `git fetch origin ${target.name} ${sha}`,
    `git switch --no-track -c ${branch} origin/${target.name}`,
    `git cherry-pick -x ${mainline ? '-m 1 ' : ''}${sha}`,
    '# resolve the conflicts, `git add` the files, then:',
    'git cherry-pick --continue',
    `git push origin ${branch}`,
    `gh pr create --repo ${repository} --base ${target.name} --head ${branch} \\`,
    `  --title ${shellQuote(backportTitle(target, pr.title))} \\`,
    `  --body ${shellQuote(`Backport of #${pr.number} to ${target.name}.`)}`,
    `gh workflow run ci.yml --repo ${repository} --ref ${branch}`,
  ];
}

/** The body of a backport's pull request. */
function backportBody({ pr, target, sha, problem }) {
  const lines = [
    `Backport of #${pr.number} (${pr.html_url}) to \`${target.name}\`, cherry-picked from the default branch with \`git cherry-pick -x ${sha}\`.`,
  ];
  if (problem) {
    lines.push(
      '',
      `**The commit would fail \`./ob signoff\`: ${problem}.** Add the sign-off of its author with \`git rebase --signoff origin/${target.name}\` and push again.`,
    );
  }
  return lines.join('\n');
}

/** The commit HEAD is: its author and the people who signed it off. */
async function headCommit(git) {
  const { stdout } = await must(git, ['log', '-1', '--format=%an%x1f%ae%x1f%(trailers:key=Signed-off-by,valueonly,separator=%x1d)']);
  const [name, email, trailers = ''] = stdout.replace(/\n$/, '').split('\x1f');
  return {
    author: { name, email },
    signoffs: trailers.split('\x1d').map(parseIdentity).filter(Boolean),
  };
}

/** Makes `sha` known to the checkout: the merge commit is on the default branch. */
async function ensureCommit(git, sha) {
  if ((await git(['cat-file', '-e', `${sha}^{commit}`])).code !== 0) {
    await must(git, ['fetch', '--no-tags', '--quiet', 'origin', sha]);
  }
}

/**
 * Cherry-picks `sha` onto a new local branch of the release branch
 * `target`, as `git cherry-pick -x`. A true merge commit is taken from its
 * first parent. The author of the commit is kept and the committer is
 * the bot's, set here since the runner has no identity of its own.
 *
 * @returns `{ status: 'picked', mainline }`, `{ status: 'conflict', files,
 * mainline }` or `{ status: 'empty', mainline }`, with the working tree
 * clean for a conflict or an empty pick.
 */
async function cherryPick(git, { target, branch, sha }) {
  const base = `refs/remotes/origin/${target.name}`;
  await must(git, ['fetch', '--no-tags', '--quiet', 'origin', `+refs/heads/${target.name}:${base}`]);
  await ensureCommit(git, sha);
  const parents = (await must(git, ['rev-list', '--parents', '-n', '1', sha])).stdout.trim().split(/\s+/);
  const mainline = parents.length > 2;
  await must(git, ['switch', '--quiet', '--no-track', '-C', branch, base]);
  const env = { GIT_COMMITTER_NAME: COMMITTER.name, GIT_COMMITTER_EMAIL: COMMITTER.email };
  const identity = ['-c', `user.name=${COMMITTER.name}`, '-c', `user.email=${COMMITTER.email}`];
  const pick = await git([...identity, 'cherry-pick', '-x', ...(mainline ? ['-m', '1'] : []), sha], { env });
  if (pick.code === 0) {
    return { status: 'picked', mainline };
  }
  const unmerged = (await git(['diff', '--name-only', '--diff-filter=U'])).stdout.split('\n').filter(Boolean);
  await git(['cherry-pick', '--abort']);
  if (unmerged.length > 0) {
    return { status: 'conflict', files: unmerged, mainline };
  }
  if (/now empty|nothing to commit/i.test(`${pick.stdout}${pick.stderr}`)) {
    return { status: 'empty', mainline };
  }
  throw new Error(`git cherry-pick ${sha} failed: ${pick.stderr.trim() || pick.stdout.trim()}`);
}

/** Puts the checkout back on the default branch's commit, between targets. */
async function leave(git, sha) {
  await git(['switch', '--quiet', '--detach', sha]);
}

/** Whether `branch` is on the remote already. */
async function remoteHas(git, branch) {
  return (await git(['ls-remote', '--exit-code', '--heads', 'origin', branch])).code === 0;
}

/**
 * Opens the pull request of a pushed backport branch and starts CI on it.
 *
 * @returns the pull request's number and URL, and what could not be done.
 */
async function openPull({ github, context }, { pr, target, branch, sha, problem }) {
  const { owner, repo } = context.repo;
  const { data: created } = await github.rest.pulls.create({
    owner,
    repo,
    title: backportTitle(target, pr.title),
    head: branch,
    base: target.name,
    body: backportBody({ pr, target, sha, problem }),
  });
  let ci = 'started';
  try {
    await github.rest.actions.createWorkflowDispatch({ owner, repo, workflow_id: 'ci.yml', ref: branch });
  } catch (error) {
    ci = `could not be started (${error.message}): run \`gh workflow run ci.yml --ref ${branch}\``;
  }
  return { number: created.number, url: created.html_url, ci };
}

/**
 * Labels the original pull request `backport-conflict`, and says on it,
 * once for each branch, what the summary says: the label alone is easy
 * to miss.
 */
async function reportByHand({ github, context }, pr, target, lines) {
  const { owner, repo } = context.repo;
  await github.rest.issues.addLabels({ owner, repo, issue_number: pr.number, labels: ['backport-conflict'] });
  const marker = `<!-- backport:by-hand:${target.name} -->`;
  const comments = await github.paginate(github.rest.issues.listComments, { owner, repo, issue_number: pr.number, per_page: 100 });
  if (!comments.some((comment) => (comment.body ?? '').includes(marker))) {
    await github.rest.issues.createComment({ owner, repo, issue_number: pr.number, body: `${marker}\n${lines.join('\n')}` });
  }
}

/**
 * Cherry-picks and pushes the backport branch of one release branch, and
 * leaves the checkout on the default branch's commit.
 *
 * @returns what happened: `{ status, lines }`, and for a pushed branch
 * `{ status: 'pushed', problem }`.
 */
async function pickAndPush(api, { pr, target, branch, sha, home, tools }) {
  const { git, context } = api;
  const { owner, repo } = context.repo;
  const heading = `### \`${target.name}\``;
  const byHand = (picked, why) => [
    heading,
    why,
    '',
    codeBlock(manualCommands({ pr, target, sha, mainline: picked.mainline, repository: `${owner}/${repo}` })),
  ];
  try {
    const picked = await cherryPick(git, { target, branch, sha });
    if (picked.status === 'empty') {
      return { status: 'empty', lines: [heading, `\`${target.name}\` has the change already: there is nothing to backport.`] };
    }
    if (picked.status === 'conflict') {
      const files = picked.files.map((file) => `\`${inline(file)}\``).join(', ');
      const lines = byHand(picked, `The change does not apply to \`${target.name}\` by itself (conflicts in ${files}). The pull request is labelled \`backport-conflict\`. To do it by hand:`);
      await reportByHand(api, pr, target, lines.slice(1));
      return { status: 'conflict', lines };
    }
    const problem = signoffProblem(await headCommit(git), tools);
    const push = await git(['push', 'origin', `${branch}:refs/heads/${branch}`]);
    if (push.code !== 0) {
      const lines = byHand(picked, `The cherry-pick applied, but pushing \`${branch}\` was refused: ${inline(push.stderr, 400)}. A workflow's token cannot push a change to a workflow file. To do it by hand:`);
      await reportByHand(api, pr, target, lines.slice(1));
      return { status: 'push-failed', lines };
    }
    return { status: 'pushed', problem };
  } finally {
    await leave(git, home.sha);
  }
}

/**
 * Backports `pr` to one release branch.
 *
 * @returns `{ status, lines }`: what happened, as lines of the summary.
 */
async function backportOne(api, { pr, target, sha, home, tools }) {
  const { github, context, git, core } = api;
  const { owner, repo } = context.repo;
  const branch = backportBranch(pr.number, target);
  const heading = `### \`${target.name}\``;

  const existing = await github.paginate(github.rest.pulls.list, { owner, repo, head: `${owner}:${branch}`, state: 'all', per_page: 100 });
  if (existing.length > 0) {
    return { status: 'exists', lines: [heading, `The backport pull request #${existing[0].number} exists already: nothing to do.`] };
  }

  let problem = null;
  if (await remoteHas(git, branch)) {
    // An earlier run pushed the branch and stopped before the pull request.
    core.info(`${branch} is on the remote already: opening its pull request.`);
  } else {
    const done = await pickAndPush(api, { pr, target, branch, sha, home, tools });
    if (done.status !== 'pushed') {
      return done;
    }
    ({ problem } = done);
  }
  const opened = await openPull(api, { pr, target, branch, sha, problem });
  const lines = [heading, `Opened #${opened.number} (${opened.url}) from \`${branch}\`; CI ${opened.ci}.`];
  if (problem) {
    lines.push(`The commit would fail \`./ob signoff\`: ${problem}. The pull request says so.`);
  }
  return { status: 'opened', lines };
}

/**
 * The entry of the workflow. `git` and `options.tools` are there for tests,
 * which run it on repositories made for them.
 */
async function run({ github, context, core, git = realGit, ...options }) {
  const api = { github, context, core, git };
  const { owner, repo } = context.repo;
  const number = context.payload.pull_request.number;
  const heading = `## Backport of #${number}`;

  const { data: pr } = await github.rest.pulls.get({ owner, repo, pull_number: number });
  const { data: repository } = await github.rest.repos.get({ owner, repo });
  const labels = labelNames(pr);
  if (!pr.merged) {
    await summarize(core, `${heading}\n\nThe pull request is not merged: nothing to do.`);
    return;
  }
  if (pr.base.ref !== repository.default_branch) {
    await summarize(core, `${heading}\n\nThe pull request was merged into \`${inline(pr.base.ref, 100)}\`, not \`${repository.default_branch}\`: nothing to do.`);
    return;
  }
  if (!labels.includes('backport') && !labels.includes('backport-previous')) {
    await summarize(core, `${heading}\n\nThe pull request has no \`backport\` or \`backport-previous\` label: nothing to do.`);
    return;
  }
  if (!pr.merge_commit_sha) {
    throw new Error('the pull request has no merge commit to cherry-pick');
  }

  const refs = await github.paginate(github.rest.git.listMatchingRefs, { owner, repo, ref: 'heads/release/', per_page: 100 });
  const branches = releaseBranches(refs.map((ref) => ref.ref.replace(/^refs\/heads\//, '')));
  const { targets, notes } = chooseTargets({ labels, branches });
  const lines = [heading, '', `${inline(pr.title, 120)}`, '', ...notes];
  if (targets.length === 0) {
    await summarize(core, lines.join('\n'));
    return;
  }

  await ensureLabels(api, ['backport', 'backport-previous', 'backport-conflict']);
  const tools = options.tools ?? toolEmails(fs.existsSync(SIGNOFF_SOURCE) ? fs.readFileSync(SIGNOFF_SOURCE, 'utf8') : '');
  const home = { sha: (await must(git, ['rev-parse', 'HEAD'])).stdout.trim() };
  const results = [];
  for (const target of targets) {
    results.push(await backportOne(api, { pr, target, sha: pr.merge_commit_sha, home, tools }));
  }
  for (const result of results) {
    lines.push('', ...result.lines);
  }
  await summarize(core, lines.join('\n'));
  if (results.some((result) => result.status === 'conflict' || result.status === 'push-failed')) {
    core.warning(`The backport of #${number} needs doing by hand: the job summary has the commands.`);
  }
}

module.exports = {
  backportBody,
  backportBranch,
  backportTitle,
  chooseTargets,
  codeBlock,
  manualCommands,
  parseIdentity,
  realGit,
  run,
  shellQuote,
  signoffProblem,
  toolEmails,
};
