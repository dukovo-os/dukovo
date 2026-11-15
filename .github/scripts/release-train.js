// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The release train (ADR 0534), run each day by
// .github/workflows/release-train.yml.
//
// Releases are monthly, on the second Tuesday (UTC). On the day 14 days
// before, `release/X.Y` is cut from the head of main, where X.Y is the
// version in the root Cargo.toml, and the release gets a tracking issue
// with its checklist. On release day the bot says on that issue whether
// the release is green, and when it is not, that it skips the month.
//
// Until .github/release-train.json says `"enabled": true`, the train is a
// dry run: it writes what it would do to the job summary and changes
// nothing. When it misbehaves, read the job summary of the day first; a
// dispatch of the workflow with a date acts as on that day.

'use strict';

const calendar = require('./calendar.js');
const { ensureLabels, inline, summarize } = require('./common.js');

const CONFIG_PATH = '.github/release-train.json';
const VERSION_FILE = 'Cargo.toml';
const BRANCH_PATTERN = /^release\/(\d+)\.(\d+)$/;
const CI_CHECK = 'CI passed';

/**
 * The settings of .github/release-train.json. Only `"enabled": true`
 * turns the train on; anything else, and a missing file, is a dry run.
 * A file that is not JSON is an error, not a dry run, so that nobody
 * thinks the train is on or off by a mistake.
 */
function parseConfig(text) {
  if (text === null) {
    return { enabled: false };
  }
  let config;
  try {
    config = JSON.parse(text);
  } catch (error) {
    throw new Error(`${CONFIG_PATH} is not valid JSON: ${error.message}`);
  }
  if (config === null || typeof config !== 'object' || Array.isArray(config)) {
    throw new Error(`${CONFIG_PATH} must hold a JSON object`);
  }
  return { enabled: config.enabled === true };
}

/**
 * The release version in the `[workspace.package]` table of a Cargo.toml:
 * its text, and the major and minor numbers the release branch is named
 * for. A pre-release such as 1.0.0-alpha.1 is a version of 1.0.
 */
function workspaceVersion(cargoToml) {
  let inTable = false;
  for (const line of cargoToml.split(/\r?\n/)) {
    const header = /^\s*\[+\s*([^\]]+?)\s*\]+\s*(#.*)?$/.exec(line);
    if (header) {
      inTable = header[1] === 'workspace.package';
      continue;
    }
    const match = inTable && /^\s*version\s*=\s*"((\d+)\.(\d+)\.(\d+)[^"]*)"/.exec(line);
    if (match) {
      return { text: match[1], major: Number(match[2]), minor: Number(match[3]) };
    }
  }
  throw new Error(`${VERSION_FILE} has no version in [workspace.package]`);
}

/** The branch name of the release of `version`: `release/0.15`. */
function branchFor(version) {
  return `release/${version.major}.${version.minor}`;
}

/** The title of a release's tracking issue, from its branch name. */
function trackingTitle(branch) {
  return `Release ${branch.slice('release/'.length)}`;
}

/**
 * The release branches among `names`, newest first by version number
 * (release/0.10 is newer than release/0.9). Other names are not release
 * branches.
 */
function releaseBranches(names) {
  const found = [];
  for (const name of names) {
    const match = BRANCH_PATTERN.exec(name);
    if (match) {
      found.push({ name, major: Number(match[1]), minor: Number(match[2]) });
    }
  }
  return found.sort((a, b) => b.major - a.major || b.minor - a.minor);
}

/** The version that follows a minor release on main: 0.15.0 is 0.16.0. */
function nextMinor(version) {
  return `${version.major}.${version.minor + 1}.0`;
}

/**
 * The checklist of a release, as the lines of an issue body. The
 * commands are those of ob(1) and the handbook chapter "The release
 * train".
 */
function checklistItems({ version, branch }) {
  const repository = '<repository>';
  return [
    `Only fixes are merged into \`${branch}\`: each reaches main first and is brought over by the \`backport\` label.`,
    `\`${CI_CHECK}\` is successful on the head of \`${branch}\` (CI runs on each push to it; \`gh workflow run ci.yml --ref ${branch}\` runs it again).`,
    `The release is in the update repository (\`system-release add\`) and the beta channel offers it: \`./ob release channel ${repository} beta ${version.text}\`, then \`./ob release deploy ${repository}\`.`,
    'The betas have soaked on the canaries and the canaries are green: `./ob release canaries`.',
    `The security advisories of this release are checked, \`./ob security check\`, and signed with the channel, or alone: \`./ob release advisories ${repository}\`.`,
    'The release notes are written.',
    `The stable channel offers it: \`./ob release channel ${repository} stable ${version.text}\`, \`./ob release deploy ${repository}\`, then \`./ob release check\`.`,
    `The waves are watched with \`./ob release canaries\`, and a bad release is paused: \`./ob release rollout pause ${version.text} <reason>\`.`,
    `The release is tagged \`v${version.text}\` on \`${branch}\`.`,
    `The version on main is moved on to ${nextMinor(version)}, in Cargo.toml and the files that mirror it.`,
  ];
}

/** The body of a release's tracking issue. */
function issueBody({ version, branch, head, cutDay, releaseDay }) {
  const items = checklistItems({ version, branch })
    .map((item) => `- [ ] ${item}`)
    .join('\n');
  return [
    `<!-- release-train:${branch} -->`,
    `Release day is ${calendar.formatDay(releaseDay)} (UTC). \`${branch}\` was cut from main at ${head.slice(0, 12)} on ${calendar.formatDay(cutDay)}, and only fixes enter it while the canaries soak the betas.`,
    '',
    `On release day the release-train bot says here whether the release is green: every box below ticked, and \`${CI_CHECK}\` successful on the head of \`${branch}\`. A release that is not green skips the month (ADR 0458). The handbook chapter "The release train" says how each step is done.`,
    '',
    items,
    '',
  ].join('\n');
}

/**
 * How many boxes of a checklist are ticked, and the text of those that
 * are not.
 */
function checklistState(body) {
  const open = [];
  let total = 0;
  for (const line of (body ?? '').split(/\r?\n/)) {
    const match = /^\s*[-*]\s+\[([ xX])\]\s+(.*)$/.exec(line);
    if (!match) {
      continue;
    }
    total += 1;
    if (match[1] === ' ') {
      open.push(match[2].trim());
    }
  }
  return { total, done: total - open.length, open };
}

/**
 * What a list of check runs named `CI passed` says of a commit: the
 * newest one decides.
 */
function ciState(checkRuns, sha) {
  const startedAt = (run) => (run.started_at ? Date.parse(run.started_at) : 0);
  const runs = checkRuns.filter((run) => run.name === CI_CHECK).sort((a, b) => startedAt(b) - startedAt(a));
  const short = sha.slice(0, 12);
  if (runs.length === 0) {
    return { ok: false, text: `\`${CI_CHECK}\` has not run on ${short}` };
  }
  const [latest] = runs;
  if (latest.status !== 'completed') {
    return { ok: false, text: `\`${CI_CHECK}\` is still running on ${short}` };
  }
  if (latest.conclusion !== 'success') {
    return { ok: false, text: `\`${CI_CHECK}\` ended ${latest.conclusion} on ${short}` };
  }
  return { ok: true, text: `\`${CI_CHECK}\` succeeded on ${short}` };
}

/**
 * What the bot says on a release day: whether the release is green, and
 * when it is not, why, and that it skips the month. `marker` makes the
 * comment findable, so that it is said once.
 */
function releaseDayReport({ branch, today, checklist, ci, nextRelease, marker }) {
  const day = calendar.formatDay(today);
  const name = trackingTitle(branch);
  const state = `${checklist.done} of ${checklist.total} checklist boxes are ticked`;
  if (checklist.total > 0 && checklist.open.length === 0 && ci.ok) {
    return `${marker}\n${name} is green on its release day, ${day}: all ${checklist.total} checklist boxes are ticked, and ${ci.text}. It can ship today.`;
  }
  const lines = [marker, `${name} is not green on its release day, ${day}, so it skips this month (ADR 0458). The next release day is ${calendar.formatDay(nextRelease)}.`, ''];
  if (checklist.total === 0) {
    lines.push('- The issue has no checklist: a release with no boxes cannot be green.');
  } else if (checklist.open.length > 0) {
    lines.push(`- ${state}. Not ticked:`);
    for (const item of checklist.open) {
      lines.push(`  - ${item}`);
    }
  }
  if (!ci.ok) {
    lines.push(`- ${ci.text}.`);
  }
  lines.push('', `\`${branch}\` stays open, and fixes keep entering it.`);
  return lines.join('\n');
}

/** Reads a file of the repository at `ref`, or null when it is not there. */
async function readFile({ github, context }, path, ref) {
  const { owner, repo } = context.repo;
  try {
    const { data } = await github.rest.repos.getContent({ owner, repo, path, ref });
    return Buffer.from(data.content, data.encoding ?? 'base64').toString('utf8');
  } catch (error) {
    if (error.status === 404) {
      return null;
    }
    throw error;
  }
}

/**
 * The tracking issue of a release, open or closed, found by its label and
 * title; there is one for each release.
 */
async function findTrackingIssue({ github, context }, branch) {
  const { owner, repo } = context.repo;
  const issues = await github.paginate(github.rest.issues.listForRepo, {
    owner,
    repo,
    labels: 'release',
    state: 'all',
    per_page: 100,
  });
  const title = trackingTitle(branch);
  return issues.find((issue) => !issue.pull_request && issue.title === title) ?? null;
}

async function listReleaseBranches({ github, context }) {
  const { owner, repo } = context.repo;
  const refs = await github.paginate(github.rest.git.listMatchingRefs, {
    owner,
    repo,
    ref: 'heads/release/',
    per_page: 100,
  });
  return releaseBranches(refs.map((ref) => ref.ref.replace(/^refs\/heads\//, ''))).map((branch) => ({
    ...branch,
    sha: refs.find((ref) => ref.ref === `refs/heads/${branch.name}`).object.sha,
  }));
}

/**
 * Cuts the branch of the release whose day is `releaseDay`, from the head
 * of main, and opens its tracking issue. The branch and the issue are
 * each made only when they are missing, so that a run after a failure
 * finishes the work and a second run changes nothing.
 */
async function cut(api, { enabled, defaultBranch, today, releaseDay }) {
  const { github, context, core } = api;
  const { owner, repo } = context.repo;
  const { data: head } = await github.rest.repos.getBranch({ owner, repo, branch: defaultBranch });
  const text = await readFile(api, VERSION_FILE, head.commit.sha);
  if (text === null) {
    throw new Error(`${VERSION_FILE} is not in ${defaultBranch} at ${head.commit.sha}`);
  }
  const version = workspaceVersion(text);
  const branch = branchFor(version);
  const existing = (await listReleaseBranches(api)).find((candidate) => candidate.name === branch);
  const issue = await findTrackingIssue(api, branch);
  const body = issueBody({ version, branch, head: head.commit.sha, cutDay: today, releaseDay });
  const heading = `## Cut day, ${calendar.formatDay(today)}: ${branch} for the release on ${calendar.formatDay(releaseDay)}`;

  if (existing && issue) {
    await summarize(core, `${heading}\n\n\`${branch}\` and its issue #${issue.number} exist: nothing to do.`);
    return;
  }
  const todo = [];
  if (!existing) {
    todo.push(`cut \`${branch}\` from ${defaultBranch} at ${head.commit.sha.slice(0, 12)} (version ${version.text})`);
  }
  if (!issue) {
    todo.push(`open the issue "${trackingTitle(branch)}", labelled \`release\`, with this checklist:\n\n${body}`);
  }
  if (!enabled) {
    await summarize(
      core,
      `${heading}\n\nDry run (\`${CONFIG_PATH}\` does not say \`"enabled": true\`): nothing is changed. It would:\n\n${todo.map((line) => `- ${line}`).join('\n')}`,
    );
    return;
  }
  const done = [];
  if (!existing) {
    await github.rest.git.createRef({ owner, repo, ref: `refs/heads/${branch}`, sha: head.commit.sha });
    done.push(`cut \`${branch}\` from ${defaultBranch} at ${head.commit.sha.slice(0, 12)}`);
  }
  if (!issue) {
    await ensureLabels(api, ['release']);
    const { data: created } = await github.rest.issues.create({
      owner,
      repo,
      title: trackingTitle(branch),
      body,
      labels: ['release'],
    });
    done.push(`opened issue #${created.number}, "${created.title}"`);
  }
  await summarize(core, `${heading}\n\n${done.map((line) => `- ${line}`).join('\n')}`);
}

/**
 * Says on the newest release's issue whether it is green on its release
 * day, once for the day.
 */
async function reportOnReleaseDay(api, { enabled, today }) {
  const { github, context, core } = api;
  const { owner, repo } = context.repo;
  const heading = `## Release day, ${calendar.formatDay(today)}`;
  const [newest] = await listReleaseBranches(api);
  if (!newest) {
    await summarize(core, `${heading}\n\nThere is no release branch: nothing to report.`);
    return;
  }
  const issue = await findTrackingIssue(api, newest.name);
  if (!issue) {
    await summarize(core, `${heading}\n\n\`${newest.name}\` has no tracking issue to report on.`);
    return;
  }
  if (issue.state === 'closed') {
    await summarize(core, `${heading}\n\n#${issue.number} is closed: ${trackingTitle(newest.name)} is released.`);
    return;
  }
  const marker = `<!-- release-train:release-day:${newest.name}:${calendar.formatDay(today)} -->`;
  const comments = await github.paginate(github.rest.issues.listComments, {
    owner,
    repo,
    issue_number: issue.number,
    per_page: 100,
  });
  if (comments.some((comment) => (comment.body ?? '').includes(marker))) {
    await summarize(core, `${heading}\n\n#${issue.number} has today's report already.`);
    return;
  }
  const { data: checks } = await github.rest.checks.listForRef({
    owner,
    repo,
    ref: newest.sha,
    check_name: CI_CHECK,
    per_page: 100,
  });
  const text = releaseDayReport({
    branch: newest.name,
    today,
    checklist: checklistState(issue.body),
    ci: ciState(checks.check_runs, newest.sha),
    nextRelease: calendar.nextReleaseDay(today),
    marker,
  });
  if (!enabled) {
    await summarize(
      core,
      `${heading}\n\nDry run (\`${CONFIG_PATH}\` does not say \`"enabled": true\`): nothing is changed. It would comment on #${issue.number}:\n\n${text.replace(marker, '').trim()}`,
    );
    return;
  }
  await github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body: text });
  await summarize(core, `${heading}\n\nCommented on #${issue.number}:\n\n${text.replace(marker, '').trim()}`);
}

/**
 * The day the run acts for: the UTC day of `now`, or `override`, a
 * YYYY-MM-DD from a dispatch of the workflow.
 */
function runDay(override, now) {
  if (!override) {
    return calendar.utcDay(now);
  }
  const day = calendar.parseDay(override);
  if (!day) {
    throw new Error(`"${inline(override, 40)}" is not a date: write it as YYYY-MM-DD`);
  }
  return day;
}

/**
 * The entry of the workflow. `env.TRAIN_DATE` and `now` are the day it
 * acts for, and are there for tests and for a dispatch with a date.
 */
async function run({ github, context, core, env = process.env, now = new Date() }) {
  const api = { github, context, core };
  const { owner, repo } = context.repo;
  const today = runDay(env.TRAIN_DATE, now);
  const { data: repository } = await github.rest.repos.get({ owner, repo });
  const defaultBranch = repository.default_branch;
  const configText = await readFile(api, CONFIG_PATH, defaultBranch);
  const { enabled } = parseConfig(configText);

  const releaseDay = calendar.releaseCutOn(today);
  const isReleaseDay = calendar.isReleaseDay(today);
  if (!releaseDay && !isReleaseDay) {
    const nextCut = calendar.nextCutDay(today);
    const nextRelease = calendar.addDays(nextCut, calendar.CUT_DAYS_BEFORE);
    await summarize(
      core,
      `## ${calendar.formatDay(today)}\n\nNothing to do today. The next branch is cut on ${calendar.formatDay(nextCut)}, for the release on ${calendar.formatDay(nextRelease)}.`,
    );
    return;
  }
  if (releaseDay) {
    await cut(api, { enabled, defaultBranch, today, releaseDay });
  }
  if (isReleaseDay) {
    await reportOnReleaseDay(api, { enabled, today });
  }
}

module.exports = {
  branchFor,
  checklistItems,
  checklistState,
  ciState,
  issueBody,
  nextMinor,
  parseConfig,
  releaseBranches,
  releaseDayReport,
  run,
  runDay,
  trackingTitle,
  workspaceVersion,
};
