// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Show command panel's parts as plain values (design 6): its four tabs,
// the lines of a file's diff with the words for a screen reader, and which
// change of the journal tells a file's story.

/** The tabs, in order, with the key of `POST /api/1/show`'s answer each shows. */
export const TABS = [
  { id: 'command', label: 'Command' },
  { id: 'api', label: 'API' },
  { id: 'ansible', label: 'Ansible' },
  { id: 'files', label: 'Files' },
];

/** The footnote under the panel. */
export const FOOTNOTE = 'The API and Ansible tabs are the same call, typed. What you see here is what runs.';

/** The words under Files when no change of the file is in the journal. */
export const NO_CHANGE = 'No change of this file is in the journal.';

/**
 * The lines of a unified diff, each `{kind, text, words}`: `kind` is
 * `head` (the two file lines), `hunk`, `add`, `del` or `ctx`; `words` is
 * what a screen reader says before the line, since the colour and the
 * sign are no help to it.
 */
export function diffLines(unified) {
  const lines = unified.replace(/\n$/, '').split('\n');
  return lines.map((text) => {
    if (text.startsWith('--- ') || text.startsWith('+++ ')) return { kind: 'head', text, words: '' };
    if (text.startsWith('@@')) return { kind: 'hunk', text, words: 'Lines: ' };
    if (text.startsWith('+')) return { kind: 'add', text, words: 'Added: ' };
    if (text.startsWith('-')) return { kind: 'del', text, words: 'Taken away: ' };
    return { kind: 'ctx', text, words: '' };
  });
}

/**
 * The newest change of `changes` (newest first, as `GET /api/1/changes`
 * gives them) that touched `path`, or null.
 */
export function changeFor(changes, path) {
  return changes.find((change) => (change.files ?? []).some((file) => file.path === path)) ?? null;
}

/** The diff text of `path` in `change` (`GET /api/1/changes/ID`), or why there is none. */
export function diffOf(change, path) {
  const file = (change.files ?? []).find((one) => one.path === path);
  if (!file) return { text: null, why: NO_CHANGE };
  if (typeof file.diff === 'string') return { text: file.diff, why: null, truncated: Boolean(file.diff_truncated) };
  return { text: null, why: file.no_diff ? `No diff is shown: ${file.no_diff}.` : NO_CHANGE };
}

/** The text a tab copies: Files copies the paths' diffs together, the others their own text. */
export const textOfTab = (shown, tab, diffs = []) => (tab === 'files' ? diffs.filter(Boolean).join('\n') : shown[tab] ?? '');
