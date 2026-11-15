// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The security clock (ADR 0534, ADR 0458), run each day and on each change
// of a label of an issue by .github/workflows/security-clock.yml.
//
// Each open issue labelled `security` has a clock that starts when the
// label `fix-available` was last added to it, and runs by its
// `severity:` label: Critical 72 hours, High 7 days, and Medium and Low
// until the end of the next release day (the second Tuesday, UTC). One
// pinned issue, "Security clock", lists them with the time left, and a late
// issue is labelled `overdue`.
//
// When the table is wrong, read the issue's events (`fix-available` added
// or removed) and its `severity:` labels: they are all the clock reads.

'use strict';

const calendar = require('./calendar.js');
const { SEVERITIES, ensureLabels, inline, labelNames, severityLabel, summarize } = require('./common.js');

const CLOCK_TITLE = 'Security clock';
const CLOCK_MARKER = '<!-- security-clock -->';
const FIX_LABEL = 'fix-available';
const CRITICAL_HOURS = 72;
const HIGH_DAYS = 7;

const PIN_ISSUE = 'mutation($id: ID!) { pinIssue(input: { issueId: $id }) { issue { id } } }';

/**
 * The severity an issue's labels give, the most severe when it has more
 * than one, or null when it has none. `several` says that it had more
 * than one.
 */
function severityOf(names) {
  const found = SEVERITIES.filter((severity) => names.includes(severityLabel(severity)));
  return { severity: found[0] ?? null, several: found.length > 1 };
}

/**
 * When the clock of an issue started: the last time `fix-available` was
 * added, as a Date, or null when it was never added or was taken off
 * again after the last time. `events` are the events of the issue.
 */
function clockStart(events) {
  const changes = events
    .filter((event) => (event.event === 'labeled' || event.event === 'unlabeled') && event.label?.name === FIX_LABEL)
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
  let start = null;
  for (const change of changes) {
    start = change.event === 'labeled' ? new Date(change.created_at) : null;
  }
  return start;
}

/**
 * When an issue of `severity` is due, when its clock started at `start`.
 * Critical and High are 72 hours and 7 days later. Medium and Low are
 * due at the end of the first release day after the day the clock
 * started, since the release of the day it started on is cut already.
 */
function dueAt(severity, start) {
  switch (severity) {
    case 'critical':
      return new Date(start.getTime() + CRITICAL_HOURS * calendar.HOUR);
    case 'high':
      return new Date(start.getTime() + HIGH_DAYS * calendar.DAY);
    case 'medium':
    case 'low':
      return calendar.endOfDay(calendar.nextReleaseDay(start));
    default:
      return null;
  }
}

/** A span of milliseconds as "3 days", "5 hours" or "less than an hour". */
function span(milliseconds) {
  const hours = Math.floor(milliseconds / calendar.HOUR);
  if (hours >= 48) {
    return `${Math.floor(hours / 24)} days`;
  }
  if (hours >= 1) {
    return hours === 1 ? '1 hour' : `${hours} hours`;
  }
  return 'less than an hour';
}

/**
 * What the clock says of one issue at `now`: its severity, when the clock
 * started and when it is due, and a state: `overdue`, `due`,
 * `not-started` or `no-severity`.
 */
function clockOf({ severity, start, now }) {
  if (!severity) {
    return { state: 'no-severity', text: 'no severity label', start, due: null };
  }
  if (!start) {
    return { state: 'not-started', text: 'clock not started', start: null, due: null };
  }
  const due = dueAt(severity, start);
  if (now.getTime() > due.getTime()) {
    return { state: 'overdue', text: `overdue by ${span(now.getTime() - due.getTime())}`, start, due };
  }
  return { state: 'due', text: `due in ${span(due.getTime() - now.getTime())}`, start, due };
}

const STATE_ORDER = { overdue: 0, due: 1, 'not-started': 2, 'no-severity': 3 };

/**
 * Entries in the order of the table: overdue first, then by the time due,
 * then those whose clock has not started, then those with no severity;
 * issues with no time due go by their number.
 */
function sortEntries(entries) {
  return [...entries].sort((a, b) => {
    const byState = STATE_ORDER[a.state] - STATE_ORDER[b.state];
    if (byState !== 0) {
      return byState;
    }
    if (a.due && b.due && a.due.getTime() !== b.due.getTime()) {
      return a.due.getTime() - b.due.getTime();
    }
    return a.number - b.number;
  });
}

/** The body of the Security clock issue for `entries` at `now`. */
function clockBody(entries, now) {
  const rows = sortEntries(entries).map((entry) => {
    const severity = entry.severity ? `${entry.severity}${entry.several ? ' (several labels)' : ''}` : 'none';
    return [
      `#${entry.number} ${inline(entry.title, 80)}`,
      severity,
      entry.start ? calendar.formatTime(entry.start) : '',
      entry.due ? calendar.formatTime(entry.due) : '',
      entry.text,
    ];
  });
  const table =
    rows.length === 0
      ? 'No security issue is open.'
      : [
          '| Issue | Severity | Clock started | Due | State |',
          '| --- | --- | --- | --- | --- |',
          ...rows.map((cells) => `| ${cells.join(' | ')} |`),
        ].join('\n');
  return [
    CLOCK_MARKER,
    'Every open issue labelled `security`, and when its fix is due (ADR 0458). The clock of an issue starts when `fix-available` is added to it, and runs by its `severity:` label: Critical 72 hours, High 7 days, Medium and Low until the end of the next release day, the second Tuesday of a month (UTC). A late issue is labelled `overdue`.',
    '',
    table,
    '',
    `Last changed ${calendar.formatTime(now)}. The security-clock workflow keeps this issue: its text is written again when an issue is labelled, closed or reopened, and each day.`,
    '',
  ].join('\n');
}

/** The body without its "Last changed" line, to see whether the table moved. */
function withoutStamp(body) {
  return (body ?? '').replace(/^Last changed .*$/m, '');
}

/** Whether `error` is the API saying a thing is not there. */
function isMissing(error) {
  return error.status === 404;
}

async function openSecurityIssues({ github, context }) {
  const { owner, repo } = context.repo;
  const issues = await github.paginate(github.rest.issues.listForRepo, {
    owner,
    repo,
    labels: 'security',
    state: 'open',
    per_page: 100,
  });
  return issues.filter((issue) => !issue.pull_request && !labelNames(issue).includes('security-clock'));
}

/** The entry for an open issue: what its labels and events say. */
async function entryFor({ github, context }, issue, now) {
  const { owner, repo } = context.repo;
  const names = labelNames(issue);
  const { severity, several } = severityOf(names);
  let start = null;
  if (names.includes(FIX_LABEL)) {
    const events = await github.paginate(github.rest.issues.listEvents, {
      owner,
      repo,
      issue_number: issue.number,
      per_page: 100,
    });
    // The label is on the issue, so its clock runs; an issue whose events
    // do not show the label being added started it when it was opened.
    start = clockStart(events) ?? new Date(issue.created_at);
  }
  return { number: issue.number, title: issue.title, severity, several, ...clockOf({ severity, start, now }) };
}

/**
 * The Security clock issue, made once and pinned, found again by its
 * label whether it is open or closed, and written again when the table
 * changes.
 */
async function keepClockIssue({ github, context, core }, body) {
  const { owner, repo } = context.repo;
  const found = await github.paginate(github.rest.issues.listForRepo, {
    owner,
    repo,
    labels: 'security-clock',
    state: 'all',
    per_page: 100,
  });
  const existing = found.filter((issue) => !issue.pull_request).sort((a, b) => a.number - b.number);
  const clock = existing.find((issue) => issue.state === 'open') ?? existing[0];
  if (!clock) {
    const { data: created } = await github.rest.issues.create({
      owner,
      repo,
      title: CLOCK_TITLE,
      body,
      labels: ['security-clock'],
    });
    try {
      await github.graphql(PIN_ISSUE, { id: created.node_id });
    } catch (error) {
      core.warning(`The Security clock issue #${created.number} is not pinned (${error.message}): pin it by hand.`);
    }
    return { number: created.number, action: 'made and pinned' };
  }
  const reopen = clock.state !== 'open';
  if (!reopen && withoutStamp(clock.body) === withoutStamp(body)) {
    return { number: clock.number, action: 'unchanged' };
  }
  await github.rest.issues.update({ owner, repo, issue_number: clock.number, body, ...(reopen ? { state: 'open' } : {}) });
  return { number: clock.number, action: reopen ? 'reopened and written' : 'written' };
}

async function removeLabel({ github, context }, number, name) {
  const { owner, repo } = context.repo;
  try {
    await github.rest.issues.removeLabel({ owner, repo, issue_number: number, name });
  } catch (error) {
    if (!isMissing(error)) {
      throw error;
    }
  }
}

/**
 * Labels the late issues `overdue` and takes the label off issues that
 * are not late any more, or are closed. Only issues labelled `security`
 * are touched: the label is the bot's there.
 */
async function keepOverdueLabels(api, open, entries) {
  const { github, context } = api;
  const { owner, repo } = context.repo;
  const late = new Set(entries.filter((entry) => entry.state === 'overdue').map((entry) => entry.number));
  const changes = [];
  for (const issue of open) {
    const has = labelNames(issue).includes('overdue');
    if (late.has(issue.number) && !has) {
      await github.rest.issues.addLabels({ owner, repo, issue_number: issue.number, labels: ['overdue'] });
      changes.push(`#${issue.number} is now \`overdue\``);
    } else if (!late.has(issue.number) && has) {
      await removeLabel(api, issue.number, 'overdue');
      changes.push(`#${issue.number} is not late: \`overdue\` removed`);
    }
  }
  const closed = await github.paginate(github.rest.issues.listForRepo, {
    owner,
    repo,
    labels: 'overdue,security',
    state: 'closed',
    per_page: 100,
  });
  for (const issue of closed.filter((candidate) => !candidate.pull_request)) {
    await removeLabel(api, issue.number, 'overdue');
    changes.push(`#${issue.number} is closed: \`overdue\` removed`);
  }
  return changes;
}

/** The entry of the workflow. `now` is there for tests. */
async function run({ github, context, core, now = new Date() }) {
  const api = { github, context, core };
  await ensureLabels(api, ['security', ...SEVERITIES.map(severityLabel), FIX_LABEL, 'overdue', 'security-clock', 'needs-triage']);
  const open = await openSecurityIssues(api);
  const entries = [];
  for (const issue of open) {
    entries.push(await entryFor(api, issue, now));
  }
  const body = clockBody(entries, now);
  const clock = await keepClockIssue(api, body);
  const changes = await keepOverdueLabels(api, open, entries);
  const late = entries.filter((entry) => entry.state === 'overdue').length;
  const lines = [
    `## Security clock, ${calendar.formatTime(now)}`,
    '',
    `${open.length} open security issues, ${late} overdue. Issue #${clock.number}: ${clock.action}.`,
    ...changes.map((change) => `- ${change}`),
    '',
    body.replace(CLOCK_MARKER, '').trim(),
  ];
  await summarize(core, lines.join('\n'));
}

module.exports = {
  clockBody,
  clockOf,
  clockStart,
  dueAt,
  run,
  severityOf,
  sortEntries,
  span,
};
