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
// longer matches is closed with a comment that says so, and, when
// `security/vex.json` states that Dukovo is not affected (ADR 0537), a
// comment that quotes the statement.
//
// The job runs this only when the scan itself ran to its end, so a scan
// that could not reach an advisory source closes nothing.
//
// Every write goes through one paced writer (common.js): a second apart,
// and after GitHub's secondary rate limit it waits and tries again.

'use strict';

const fs = require('node:fs');

const { SEVERITIES, ensureLabels, inline, labelNames, oneLine, pacedWriter, severityLabel, summarize } = require('./common.js');

const MARKER = /<!-- sbom-scan:(\S+) -->/;
const FILE = 'findings.json';
const RUNBOOK = 'docs/handbook/src/release-train.md';

/** The statuses of a statement that leave a finding out, in words. */
const STATUS_WORDS = {
  fixed: 'fixed',
  not_affected: 'not affected',
};

/** The justifications of a `not_affected` statement, in words. */
const JUSTIFICATION_WORDS = {
  component_not_present: 'the component is not present',
  vulnerable_code_not_present: 'the vulnerable code is not present',
  vulnerable_code_not_in_execute_path: 'the vulnerable code is not in the execute path',
  vulnerable_code_cannot_be_controlled_by_adversary: 'the vulnerable code cannot be controlled by an adversary',
  inline_mitigations_already_exist: 'mitigations already exist',
};

/**
 * Checks that each entry of the list `name` of the scan has the string
 * `keys`.
 */
function checkEntries(scan, name, keys) {
  for (const entry of scan[name]) {
    for (const key of keys) {
      if (typeof entry?.[key] !== 'string' || entry[key] === '') {
        throw new Error(`an entry of "${name}" has no "${key}": ${JSON.stringify(entry)}`);
      }
    }
  }
}

/**
 * Reads the document `ob security scan --json` prints, and checks that it
 * has the parts this script reads. The lists `not_affected` and
 * `unused_statements` are empty when the scan has none.
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
  for (const name of ['not_affected', 'unused_statements']) {
    if (scan[name] === undefined) {
      scan[name] = [];
    }
    if (!Array.isArray(scan[name])) {
      throw new Error(`the scan's "${name}" is not a list`);
    }
  }
  for (const finding of scan.findings) {
    for (const key of ['id', 'component', 'source']) {
      if (typeof finding[key] !== 'string' || finding[key] === '') {
        throw new Error(`a finding has no "${key}": ${JSON.stringify(finding)}`);
      }
    }
  }
  checkEntries(scan, 'not_affected', ['id', 'product', 'status']);
  checkEntries(scan, 'unused_statements', ['id', 'product']);
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
      product: finding.product ?? '',
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
    (place) =>
      `| ${inline(place.component)} | ${inline(place.product)} | ${inline(place.location)} | ${inline(place.source)} | ${place.url ? `<${place.url}>` : ''} |`,
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
    '| Component | Product | Where | Found by | Advisory |',
    '| --- | --- | --- | --- | --- |',
    ...rows,
    '',
    `When a fix exists, upstream or Dukovo's own, add \`fix-available\`: the clock of ADR 0458 starts then, and the security clock issue keeps its time. The fix goes to main first. Label its pull request \`backport\` once a release branch exists, and \`backport-previous\` too for a Critical or High fix (\`${RUNBOOK}\`).`,
    '',
    'The scan closes this issue when the finding no longer matches, and reopens it if it matches again. Close it as not planned to live with the finding: the scan leaves it closed then.',
    '',
    `A finding that does not affect Dukovo is stated in \`security/vex.json\`, with this ID and the product above, in a pull request: the scan closes this issue when the statement is in (\`${RUNBOOK}\`).`,
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
async function updateSeverity({ github, context, write }, issue, group) {
  const { owner, repo } = context.repo;
  const names = labelNames(issue);
  if (!group.severity || names.some(isSeverityLabel)) {
    return false;
  }
  await write(() => github.rest.issues.addLabels({ owner, repo, issue_number: issue.number, labels: [severityLabel(group.severity)] }));
  if (names.includes('needs-triage')) {
    await write(() => github.rest.issues.removeLabel({ owner, repo, issue_number: issue.number, name: 'needs-triage' }));
  }
  return true;
}

/**
 * Opens a closed issue again, with the labels of an issue of the scan:
 * a severity it had before is kept.
 */
async function reopenIssue({ github, context, write }, issue, group, today) {
  const { owner, repo } = context.repo;
  await write(() =>
    github.rest.issues.update({ owner, repo, issue_number: issue.number, state: 'open', title: issueTitle(group), body: issueBody(group) }),
  );
  await write(() =>
    github.rest.issues.createComment({
      owner,
      repo,
      issue_number: issue.number,
      body: `The scan of ${today} matches ${inline(group.id)} again, so this issue is open again.`,
    }),
  );
  const names = labelNames(issue);
  const wanted = names.some(isSeverityLabel) ? ['security', 'sbom-scan'] : labelsFor(group);
  const missing = wanted.filter((name) => !names.includes(name));
  if (missing.length > 0) {
    await write(() => github.rest.issues.addLabels({ owner, repo, issue_number: issue.number, labels: missing }));
  }
}

/**
 * Makes an ID's issue, or brings the one it has up to date.
 *
 * @returns what it did, and the number of the issue.
 */
async function keepIssue(api, group, existing, today) {
  const { github, context, write } = api;
  const { owner, repo } = context.repo;
  if (!existing) {
    const { data } = await write(() =>
      github.rest.issues.create({
        owner,
        repo,
        title: issueTitle(group),
        body: issueBody(group),
        labels: labelsFor(group),
      }),
    );
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
    await write(() =>
      github.rest.issues.update({ owner, repo, issue_number: existing.number, title: issueTitle(group), body: issueBody(group) }),
    );
    changed = true;
  }
  if (await updateSeverity(api, existing, group)) {
    changed = true;
  }
  return { action: changed ? 'updated' : 'unchanged', number: existing.number };
}

/** The words of a statement's status: `fixed`, or `not affected`. */
function statusWord(status) {
  return STATUS_WORDS[status] ?? inline(status);
}

/** The words of a statement's justification. */
function justificationWords(justification) {
  return JUSTIFICATION_WORDS[justification] ?? inline(justification);
}

/** Text from the scan as a sentence: it ends with a full stop unless it ends with a mark. */
function sentence(text) {
  return /[.!?]$/.test(text) ? text : `${text}.`;
}

/**
 * What one statement says, for the comment of a closed issue: the
 * product, the status and its justification, and the statement.
 */
function statementText(entry) {
  const why = entry.justification ? `, ${justificationWords(entry.justification)}` : '';
  const said = entry.statement ? ` ${sentence(inline(entry.statement, 1000))}` : '';
  return `${inline(entry.product)}, ${statusWord(entry.status)}${why}.${said}`;
}

/**
 * The comment that closes the issue of an ID the scan lists as not
 * affecting Dukovo: it quotes each statement, and names its product.
 */
function statedComment(id, entries, today) {
  const start = `The scan of ${today} matches ${inline(id)} only where \`security/vex.json\` states that Dukovo is not affected:`;
  if (entries.length === 1) {
    return `${start} ${statementText(entries[0])} Closing this issue. It is opened again if the statement is withdrawn or the scan matches ${inline(id)} elsewhere.`;
  }
  const list = entries.map((entry) => `- ${statementText(entry)}`).join('\n');
  return `${start}\n\n${list}\n\nClosing this issue. It is opened again if a statement is withdrawn or the scan matches ${inline(id)} elsewhere.`;
}

/** The comment that closes the issue of an ID that no longer matches and is not stated. */
function goneComment(id, today) {
  return `The scan of ${today} does not match ${inline(id)} any more: the part that had it has moved to a version without it, the advisory was withdrawn, or the scan no longer asks about the part (the run's summary lists those and why). Closing this issue. It is opened again if the scan matches ${inline(id)} again.`;
}

/**
 * Closes the issue of an ID that no longer matches, with a comment that
 * says why: `entries` are the statements that leave the ID out, when there
 * are any. The issue is closed as completed, so that it is opened again
 * when a statement is withdrawn.
 */
async function closeIssue({ github, context, write }, issue, id, today, entries = []) {
  const { owner, repo } = context.repo;
  const body = entries.length > 0 ? statedComment(id, entries, today) : goneComment(id, today);
  await write(() => github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body }));
  await write(() => github.rest.issues.update({ owner, repo, issue_number: issue.number, state: 'closed', state_reason: 'completed' }));
}

/** The statements that leave an ID out of the findings, as a map from the ID. */
function statementsById(notAffected) {
  const byId = new Map();
  for (const entry of notAffected) {
    byId.set(entry.id, [...(byId.get(entry.id) ?? []), entry]);
  }
  return byId;
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

/**
 * The text of the run's summary: the issues that changed, the findings
 * that are stated not affected, the statements that match nothing, and
 * the ports the scan did not ask about.
 */
function summaryText({ today, scan, groups, lines }) {
  const stated = scan.not_affected.map((entry) => `- ${inline(entry.id)}: ${inline(entry.product)}, ${statusWord(entry.status)}`);
  const unused = scan.unused_statements.map((entry) => `${inline(entry.id)} (${inline(entry.product)})`);
  const notAsked = scan.not_covered.map((port) => {
    const name = inline(`${port.port} ${port.version ?? ''}`);
    return port.reason ? `${name} (${inline(port.reason)})` : name;
  });
  return [
    `## SBOM scan, ${today}`,
    '',
    `${scan.findings.length} findings of ${groups.size} advisories.`,
    ...(lines.length > 0 ? lines : ['No issue changed.']),
    ...(stated.length > 0 ? ['', `Stated not affected in \`security/vex.json\`, so left out of the findings (${stated.length}):`, ...stated] : []),
    ...(unused.length > 0 ? ['', `Statements in \`security/vex.json\` that match no finding, to remove or fix: ${unused.join('; ')}.`] : []),
    '',
    notAsked.length > 0 ? `Not asked about: ${notAsked.join('; ')}.` : 'Every port with a source is covered.',
  ].join('\n');
}

/** The entry of the workflow. `read`, `now` and `sleep` are there for tests. */
async function run({ github, context, core, env = process.env, now = new Date(), read = fs.readFileSync, sleep }) {
  const api = { github, context, core, write: pacedWriter({ sleep }) };
  const { owner, repo } = context.repo;
  const scan = parseScan(read(env.FINDINGS_FILE || FILE, 'utf8'));
  const today = now.toISOString().slice(0, 10);
  const groups = groupById(scan.findings);
  const stated = statementsById(scan.not_affected);

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
      const entries = stated.get(id) ?? [];
      await closeIssue(api, issue, id, today, entries);
      lines.push(`- ${id}: #${issue.number} closed, ${entries.length > 0 ? 'stated not affected' : 'no longer matches'}`);
    }
  }

  await summarize(core, summaryText({ today, scan, groups, lines }));
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
  statementsById,
};
