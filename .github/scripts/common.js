// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the bots share (ADR 0534): the labels they use and create, the
// plain text they make of what others wrote, and the job summary.
//
// The text that reaches an issue, a pull request or a summary from outside
// (a pull request's title, an advisory's summary) goes through `oneLine`
// or `inline` first: it is one line, and `inline` is free of table and
// mention markup too.

'use strict';

/**
 * Every label the bots use. They create a label that is missing and leave
 * one that exists as it is, so that a maintainer can change its colour or
 * words. GitHub allows 100 characters in a description.
 */
const LABELS = {
  backport: {
    color: '0e8a16',
    description: 'Cherry-pick this merged pull request onto the newest release branch',
  },
  'backport-previous': {
    color: '1d76db',
    description: 'Cherry-pick it onto the previous release branch too: Critical and High fixes only',
  },
  'backport-conflict': {
    color: 'b60205',
    description: 'The backport did not apply by itself: do it by hand (docs/handbook/src/release-train.md)',
  },
  security: {
    color: 'd93f0b',
    description: 'A security issue: the security clock keeps its time',
  },
  'severity:critical': {
    color: 'b60205',
    description: 'Critical: the fix is on stable within 72 hours of its being available',
  },
  'severity:high': {
    color: 'd93f0b',
    description: 'High: the fix is on stable within 7 days of its being available',
  },
  'severity:medium': {
    color: 'fbca04',
    description: 'Medium: the fix is in the next monthly release',
  },
  'severity:low': {
    color: 'c5def5',
    description: 'Low: the fix is in the next monthly release',
  },
  'fix-available': {
    color: '0e8a16',
    description: 'A fix exists, upstream or Dukovo\'s own: the security clock starts when this is added',
  },
  overdue: {
    color: 'b60205',
    description: 'Late by the security clock (ADR 0458): set and removed by the bot',
  },
  'security-clock': {
    color: '5319e7',
    description: 'The pinned issue that lists the open security issues and when each is due',
  },
  release: {
    color: '1d76db',
    description: 'The tracking issue of a release, with its checklist',
  },
  'sbom-scan': {
    color: '5319e7',
    description: 'Opened by the daily scan of what the system is built from',
  },
  'needs-triage': {
    color: 'ededed',
    description: 'Its severity is not known yet: decide it and label it',
  },
};

/** The severity labels, most severe first. */
const SEVERITIES = ['critical', 'high', 'medium', 'low'];

/** The label of a severity name: `high` is `severity:high`. */
function severityLabel(severity) {
  return `severity:${severity}`;
}

/**
 * Creates each of the labels in `names` that the repository lacks. A name
 * that LABELS does not hold is a mistake in the bot, so it throws.
 *
 * @returns the names it created.
 */
async function ensureLabels({ github, context }, names) {
  const { owner, repo } = context.repo;
  const unknown = names.filter((name) => !LABELS[name]);
  if (unknown.length > 0) {
    throw new Error(`no label is defined for: ${unknown.join(', ')}`);
  }
  const existing = await github.paginate(github.rest.issues.listLabelsForRepo, {
    owner,
    repo,
    per_page: 100,
  });
  const have = new Set(existing.map((label) => label.name.toLowerCase()));
  const created = [];
  for (const name of names) {
    if (have.has(name.toLowerCase())) {
      continue;
    }
    try {
      await github.rest.issues.createLabel({ owner, repo, name, ...LABELS[name] });
      created.push(name);
    } catch (error) {
      // A label made by someone else between the list and the call is as
      // good as ours.
      if (error.status !== 422) {
        throw error;
      }
    }
  }
  return created;
}

/**
 * Text that came from outside, as one line of at most `max` characters:
 * no line breaks, and no more than a single space between words. It is
 * fit for a title.
 */
function oneLine(text, max = 200) {
  const line = String(text ?? '')
    .replace(/\s+/g, ' ')
    .trim();
  if (line.length <= max) {
    return line;
  }
  return `${line.slice(0, max - 3).trimEnd()}...`;
}

/**
 * Text that came from outside, as one line for a body or a table cell:
 * `oneLine`, with no table bar and no mention that would notify somebody.
 */
function inline(text, max = 200) {
  return oneLine(text, max).replace(/\|/g, '\\|').replace(/@/g, '@&#8203;');
}

/** The label names of an issue or pull request from the API. */
function labelNames(item) {
  return (item.labels ?? []).map((label) => (typeof label === 'string' ? label : label.name));
}

/**
 * Writes `markdown` to the job summary, and to the log for a reader of the
 * log.
 */
async function summarize(core, markdown) {
  core.info(markdown);
  if (core.summary) {
    await core.summary.addRaw(`${markdown}\n`).write();
  }
}

module.exports = {
  LABELS,
  SEVERITIES,
  ensureLabels,
  inline,
  labelNames,
  oneLine,
  severityLabel,
  summarize,
};
