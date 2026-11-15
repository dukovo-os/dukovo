// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the daily scan of the system's SBOM does with its findings (ADR
// 0534), run by the `sbom-scan` job of .github/workflows/upstream.yml after
// `./ob security scan --json > findings.json`.
//
// Each advisory ID the scan finds has one issue, labelled `security` and
// `sbom-scan` and with its severity, or `needs-triage` when the advisory
// gives none. An ID never has a second issue, open or closed: the issue
// of an ID that matches again is reopened, unless it was closed as not
// planned, which is a decision to live with it. An open issue whose ID no
// longer matches is closed with a comment that says so.
//
// The job runs this only when the scan itself ran to its end, so a scan
// that could not reach an advisory source closes nothing.

'use strict';

const fs = require('node:fs');

const { SEVERITIES, ensureLabels, inline, labelNames, oneLine, severityLabel, summarize } = require('./common.js');

const MARKER = /<!-- sbom-scan:(\S+) -->/;
const FILE = 'findings.json';
const RUNBOOK = 'docs/handbook/src/release-train.md';

/**
 * Reads the document `ob security scan --json` prints, and checks that it
 * has the parts this script reads.
 */
function parseScan(text) {
  let scan;
  try {
    scan = JSON.parse(text);
  } catch (error) {
    throw new Error(`the scan's output is not JSON: ${error.message}`);
  }
  if (!Array.isArray(scan?.findings) || !Array.isArray(scan?.not_covered)) {
    throw new Error('the scan\'s output has no "findings" and "not_covered" lists');
  }
  for (const finding of scan.findings) {
    for (const key of ['id', 'component', 'source']) {
      if (typeof finding[key] !== 'string' || finding[key] === '') {
        throw new Error(`a finding has no "${key}": ${JSON.stringify(finding)}`);
      }
    }
  }
  return scan;
}

/**
 * The most severe of two severities, where null is none: a finding of
 * the same ID that one source rates and another does not takes the rating.
 */
function moreSevere(a, b) {
  const rank = (severity) => (severity === null || severity === undefined ? SEVERITIES.length : SEVERITIES.indexOf(severity));
  return rank(b) < rank(a) ? b : a;
}

/**
 * The findings of one advisory ID, together: the places it was found
 * and what is known of it. The result has an entry for each ID, in the
 * order the IDs first appear.
 */
function groupById(findings) {
  const groups = new Map();
  for (const finding of findings) {
    let group = groups.get(finding.id);
    if (!group) {
      group = { id: finding.id, aliases: new Set(), severity: null, summary: '', places: [] };
      groups.set(finding.id, group);
    }
    for (const alias of finding.aliases ?? []) {
      group.aliases.add(alias);
    }
    group.severity = moreSevere(group.severity, finding.severity ?? null);
    group.summary = group.summary || finding.summary || '';
    group.places.push({
      component: finding.component,
      location: finding.location ?? '',
      source: finding.source,
      url: finding.url ?? '',
    });
  }
  return groups;
}

/** The title of an ID's issue: it starts with the ID. */
function issueTitle(group) {
  const summary = oneLine(group.summary, 120);
  return summary ? `${group.id}: ${summary}` : group.id;
}

/** The body of an ID's issue. */
function issueBody(group) {
  const aliases = [...group.aliases].filter((alias) => alias !== group.id);
  const rows = group.places.map(
    (place) => `| ${inline(place.component)} | ${inline(place.location)} | ${inline(place.source)} | ${place.url ? `<${place.url}>` : ''} |`,
  );
  return [
    `<!-- sbom-scan:${group.id} -->`,
    `The daily scan of what the system is built from found **${inline(group.id)}**${aliases.length > 0 ? ` (also ${aliases.map((alias) => inline(alias)).join(', ')})` : ''}.`,
    '',
    group.summary ? `> ${inline(group.summary, 500)}` : '',
    '',
    group.severity
      ? `Severity, from the advisory: **${group.severity}**.`
      : 'The advisory gives no severity, so this issue is labelled `needs-triage`: decide the severity and label it `severity:critical`, `severity:high`, `severity:medium` or `severity:low`.',
    '',
    '| Component | Where | Found by | Advisory |',
    '| --- | --- | --- | --- |',
    ...rows,
    '',
    `When a fix exists, upstream or Dukovo's own, add \`fix-available\`: the clock of ADR 0458 starts then, and the security clock issue keeps its time. The fix goes to main first. Label its pull request \`backport\` once a release branch exists, and \`backport-previous\` too for a Critical or High fix (\`${RUNBOOK}\`).`,
    '',
    'The scan closes this issue when the finding no longer matches, and reopens it if it matches again. Close it as not planned to live with the finding: the scan leaves it closed then.',
    '',
  ].join('\n');
}

/** The ID an issue made by the scan is for, from its marker or its title. */
function idOfIssue(issue) {
  const marked = MARKER.exec(issue.body ?? '');
  if (marked) {
    return marked[1];
  }
  const titled = /^([^\s:]+)/.exec(issue.title ?? '');
  return titled ? titled[1] : null;
}

/** Whether a label name is one of the severities'. */
function isSeverityLabel(name) {
  return SEVERITIES.some((severity) => severityLabel(severity) === name);
}

/** The labels of a new issue, or of one that is opened again. */
function labelsFor(group) {
  return ['security', 'sbom-scan', group.severity ? severityLabel(group.severity) : 'needs-triage'];
}

/**
 * An issue that was triaged keeps its severity label; one that has none
 * gets the advisory's, once the advisory gives one.
 */
async function updateSeverity({ github, context }, issue, group) {
  const { owner, repo } = context.repo;
  const names = labelNames(issue);
  if (!group.severity || names.some(isSeverityLabel)) {
    return false;
  }
  await github.rest.issues.addLabels({ owner, repo, issue_number: issue.number, labels: [severityLabel(group.severity)] });
  if (names.includes('needs-triage')) {
    await github.rest.issues.removeLabel({ owner, repo, issue_number: issue.number, name: 'needs-triage' });
  }
  return true;
}

/**
 * Opens a closed issue again, with the labels of an issue of the scan:
 * a severity it had before is kept.
 */
async function reopenIssue({ github, context }, issue, group, today) {
  const { owner, repo } = context.repo;
  await github.rest.issues.update({ owner, repo, issue_number: issue.number, state: 'open', title: issueTitle(group), body: issueBody(group) });
  await github.rest.issues.createComment({
    owner,
    repo,
    issue_number: issue.number,
    body: `The scan of ${today} matches ${inline(group.id)} again, so this issue is open again.`,
  });
  const names = labelNames(issue);
  const wanted = names.some(isSeverityLabel) ? ['security', 'sbom-scan'] : labelsFor(group);
  const missing = wanted.filter((name) => !names.includes(name));
  if (missing.length > 0) {
    await github.rest.issues.addLabels({ owner, repo, issue_number: issue.number, labels: missing });
  }
}

/**
 * Makes an ID's issue, or brings the one it has up to date.
 *
 * @returns what it did, and the number of the issue.
 */
async function keepIssue(api, group, existing, today) {
  const { github, context } = api;
  const { owner, repo } = context.repo;
  if (!existing) {
    const { data } = await github.rest.issues.create({
      owner,
      repo,
      title: issueTitle(group),
      body: issueBody(group),
      labels: labelsFor(group),
    });
    return { action: 'opened', number: data.number };
  }
  if (existing.state === 'closed') {
    if (existing.state_reason === 'not_planned') {
      return { action: 'left closed as not planned', number: existing.number };
    }
    await reopenIssue(api, existing, group, today);
    return { action: 'reopened', number: existing.number };
  }
  let changed = false;
  if (existing.title !== issueTitle(group) || existing.body !== issueBody(group)) {
    await github.rest.issues.update({ owner, repo, issue_number: existing.number, title: issueTitle(group), body: issueBody(group) });
    changed = true;
  }
  if (await updateSeverity(api, existing, group)) {
    changed = true;
  }
  return { action: changed ? 'updated' : 'unchanged', number: existing.number };
}

/** Closes the issue of an ID that no longer matches, with a comment that says so. */
async function closeIssue({ github, context }, issue, id, today) {
  const { owner, repo } = context.repo;
  await github.rest.issues.createComment({
    owner,
    repo,
    issue_number: issue.number,
    body: `The scan of ${today} does not match ${inline(id)} any more: the part that had it has moved to a version without it, or the advisory was withdrawn. Closing this issue. It is opened again if the scan matches ${inline(id)} again.`,
  });
  await github.rest.issues.update({ owner, repo, issue_number: issue.number, state: 'closed', state_reason: 'completed' });
}

/**
 * The issue of each ID the scan made, as a map from the ID: an open one
 * where there is one, else the oldest.
 */
function issuesById(issues) {
  const byId = new Map();
  for (const issue of [...issues].sort((a, b) => a.number - b.number)) {
    const id = idOfIssue(issue);
    if (id === null) {
      continue;
    }
    const kept = byId.get(id);
    if (!kept || (kept.state !== 'open' && issue.state === 'open')) {
      byId.set(id, issue);
    }
  }
  return byId;
}

/** The entry of the workflow. `read` and `now` are there for tests. */
async function run({ github, context, core, env = process.env, now = new Date(), read = fs.readFileSync }) {
  const api = { github, context, core };
  const { owner, repo } = context.repo;
  const scan = parseScan(read(env.FINDINGS_FILE || FILE, 'utf8'));
  const today = now.toISOString().slice(0, 10);
  const groups = groupById(scan.findings);

  await ensureLabels(api, ['security', 'sbom-scan', ...SEVERITIES.map(severityLabel), 'needs-triage']);
  const issues = await github.paginate(github.rest.issues.listForRepo, {
    owner,
    repo,
    labels: 'sbom-scan',
    state: 'all',
    per_page: 100,
  });
  const existing = issuesById(issues.filter((issue) => !issue.pull_request));

  const lines = [];
  for (const group of groups.values()) {
    const done = await keepIssue(api, group, existing.get(group.id), today);
    if (done.action !== 'unchanged') {
      lines.push(`- ${group.id}: #${done.number} ${done.action}`);
    }
  }
  for (const [id, issue] of existing) {
    if (issue.state === 'open' && !groups.has(id)) {
      await closeIssue(api, issue, id, today);
      lines.push(`- ${id}: #${issue.number} closed, no longer matches`);
    }
  }

  const notCovered = scan.not_covered.map((port) => `${port.port} ${port.version ?? ''}`.trim());
  await summarize(
    core,
    [
      `## SBOM scan, ${today}`,
      '',
      `${scan.findings.length} findings of ${groups.size} advisories.`,
      ...(lines.length > 0 ? lines : ['No issue changed.']),
      '',
      notCovered.length > 0
        ? `Not covered, pinned by a release archive that OSV cannot be asked about by commit: ${notCovered.join(', ')}.`
        : 'Every port with a source is covered.',
    ].join('\n'),
  );
}

module.exports = {
  groupById,
  idOfIssue,
  issueBody,
  issueTitle,
  issuesById,
  labelsFor,
  moreSevere,
  parseScan,
  run,
};
