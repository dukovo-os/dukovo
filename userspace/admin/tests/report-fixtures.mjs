// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Report a problem page reads and what reportd answers it, in the
// shapes of the commands' JSON, and a small model of how reportd puts a
// draft together from its parts, cuts it and writes the answer, so that a
// test that presses Cut line gets the answer reportd would give. Not a test
// itself, and never installed.

import { createHash } from 'node:crypto';

export const NOW = Date.UTC(2026, 10, 16, 9, 30, 0);

/** `GET /api/1/reports/crashes`: two crashes kept, one of them reported already. */
export const crashes = () => ({
  format: 1,
  crashes: [
    {
      id: '20261113T100211-4012', kind: 'signal', program: 'php-fpm', version: '8.3.1', service: 'php', user: 0, at_ms: NOW - 3_600_000, what: 'SIGSEGV (address not mapped) in ngx_http_core_run_phases', signature: '3fa2c4d19e0b7a61', seen: 2, reported: null,
    },
    {
      id: '20261110T081500-377', kind: 'signal', program: 'nginx', version: '1.29.0', service: 'web', user: 0, at_ms: NOW - 5 * 86_400_000, what: 'SIGABRT', signature: '9a8b7c6d5e4f3a21', seen: 1, reported: 6,
    },
    {
      id: '20261112T200000-99', kind: 'signal', program: 'valkey', version: null, service: null, user: 1000, at_ms: NOW - 4 * 86_400_000, what: 'SIGSEGV', signature: '1111222233334444', seen: 1,
    },
  ],
});

/** `GET /api/1/updates/failed`: an update that was taken back. */
export const failed = () => ({
  failed: {
    start: 42, at_ms: NOW - 6 * 3_600_000, release: '1.0.0-alpha.2', slot: 'b', result: 'fell-back', check: 'BOOT-103', ready_ms: null, changes: [],
  },
  since_good: [],
  bisect: { running: false, try: null, of: null },
});

/** `GET /api/1/services`: one service has failed. Its times count from the machine's start. */
export const services = () => ({
  show_version: 1,
  now_ms: 20_000_000,
  services: [
    { name: 'nginx', description: 'Web server', state: 'running', since_ms: 1000 },
    { name: 'backup-sync', description: 'Copies backups', state: 'failed', since_ms: 20_000_000 - 7_200_000 },
  ],
});

/** The actions the machine offers, as `GET /api/1/actions` lists them. */
export const actions = (ids = ['reports.crashes', 'reports.draft', 'reports.redact', 'reports.send', 'reports.list', 'reports.status', 'reports.retry', 'reports.delete']) => ids.map((id) => ({
  id, title: id, method: 'POST', path: `/api/1/${id.replace('.', '/')}`, runs: 'person', changes: !['reports.crashes', 'reports.list', 'reports.status', 'reports.draft', 'reports.redact'].includes(id),
}));

// ---- a model of reportd's drafts ------------------------------------------------------

const ANSWER_HEADINGS = { happened: 'What happened', expected: 'What was expected', again: 'How to make it happen again' };

/** `text` redacted as the model does: an address and the name alice. */
const redact = (text) => text.replace(/\b\d{1,3}(?:\.\d{1,3}){3}\b/g, '[IPv4]').replace(/\balice\b/gi, '[user]');

/** The sections of a report for the person's `answers`: the head, the answers, then what was gathered. */
export function sectionsOf({ head, answers, gathered }) {
  const typed = Object.entries(ANSWER_HEADINGS)
    .map(([id, heading]) => ({ id, heading, lines: String(answers[id] ?? '').split('\n').map((line) => line.trimEnd()).filter((line, at, all) => line !== '' || (at > 0 && at < all.length - 1)).map((line) => (line.startsWith('==') ? ` ${line}` : line)) }))
    .filter((section) => section.lines.length > 0 && section.lines.some((line) => line !== ''));
  return [{ id: 'head', heading: null, lines: head }, ...typed, ...gathered];
}

/** What reportd answers for `sections`: the text and everything the page reads of it. */
function described(sections, { hasLogs, you }) {
  const lines = [];
  const places = [];
  const parts = [];
  let answersBytes = 0;
  for (const section of sections) {
    const start = lines.length;
    if (section.id === 'head') {
      lines.push(...section.lines, '');
      places.push({ id: 'head', title: 'What this computer says of itself', first_line: 0, lines: section.lines.length });
      parts.push({ title: 'What this computer says of itself', bytes: 0 });
    } else {
      lines.push(`== ${section.heading}`, ...section.lines);
      places.push({ id: section.id, title: section.heading, first_line: start + 1, lines: section.lines.length });
      parts.push({ title: section.heading, bytes: 0, id: section.id });
    }
  }
  const text = `${redact(lines.join('\n'))}\n`;
  const rows = text.split('\n');
  const bytesOf = (from, count) => rows.slice(from, from + count).reduce((sum, row) => sum + Buffer.byteLength(`${row}\n`), 0);
  const sized = places.map((place, at) => {
    const bytes = bytesOf(place.id === 'head' ? 0 : place.first_line - 1, place.lines + (place.id === 'head' ? 1 : 1));
    if (['happened', 'expected', 'again'].includes(place.id)) answersBytes += bytes;
    return { ...parts[at], bytes };
  });
  const shown = sized.filter((part) => !['happened', 'expected', 'again'].includes(part.id)).map(({ title, bytes }) => ({ title, bytes }));
  if (answersBytes > 0) shown.splice(1, 0, { title: 'Your answers', bytes: answersBytes });
  const count = (pattern) => (text.match(pattern) ?? []).length;
  return {
    format: 1,
    sha256: createHash('sha256').update(text).digest('hex'),
    bytes: Buffer.byteLength(text),
    text,
    cuts: count(/\[(?:user|IPv4)\]/g),
    has_logs: hasLogs,
    cut_by: {
      user: count(/\[user\]/g), host: 0, ipv4: count(/\[IPv4\]/g), ipv6: 0, mac: 0, email: 0, wifi: 0, serial: 0, machine: 0, you: you.lines + you.parts,
    },
    parts: shown,
    sections: places,
    typed: places.filter((place) => ['happened', 'expected', 'again'].includes(place.id)),
    cut_by_you: you,
  };
}

/**
 * The text the edits `{cuts, answers}` make of `base` (`{head, answers,
 * gathered, hasLogs}`), as reportd's `redact` answers it: whole lines gone,
 * runs of words `[cut]`, the answers replaced, and a head line that counts
 * what was cut. A cut outside a part that can be cut is refused as reportd
 * refuses it.
 */
export function apply(base, edits = { cuts: [] }) {
  const you = { lines: 0, parts: 0 };
  // What was gathered is redacted when the draft is made: the cuts' places are in those lines.
  const gathered = base.gathered.map((made) => ({ ...made, lines: made.lines.map(redact) })).map((section) => {
    const mine = edits.cuts.filter((cut) => cut.section === section.id);
    const gone = new Set();
    const words = new Map();
    for (const cut of mine) {
      if (cut.line >= section.lines.length || (cut.through ?? cut.line) >= section.lines.length) throw new Error(`This part has no such line: ${cut.section} line ${cut.line} (RPT-105)`);
      if (cut.from === undefined) for (let at = cut.line; at <= (cut.through ?? cut.line); at += 1) gone.add(at);
      else words.set(cut.line, [...(words.get(cut.line) ?? []), [cut.from, cut.to]]);
    }
    const lines = [];
    section.lines.forEach((line, at) => {
      if (gone.has(at)) {
        you.lines += 1;
        return;
      }
      const ranges = (words.get(at) ?? []).sort((a, b) => a[0] - b[0]);
      const merged = [];
      for (const range of ranges) {
        if (merged.length > 0 && range[0] <= merged.at(-1)[1]) merged.at(-1)[1] = Math.max(merged.at(-1)[1], range[1]);
        else merged.push([...range]);
      }
      you.parts += merged.length;
      const chars = [...line];
      let from = 0;
      let out = '';
      for (const [start, end] of merged) {
        out += `${chars.slice(from, start).join('')}[cut]`;
        from = end;
      }
      lines.push(out + chars.slice(from).join(''));
    });
    return { ...section, lines };
  });
  const head = [...base.head];
  if (you.lines + you.parts > 0) {
    const said = [you.lines > 0 && (you.lines === 1 ? '1 line' : `${you.lines} lines`), you.parts > 0 && (you.parts === 1 ? '1 part of a line' : `${you.parts} parts of lines`)].filter(Boolean);
    head.push(`Cut by the sender: ${said.join(', ')}`);
  }
  const answers = edits.answers ?? base.answers;
  return described(sectionsOf({ head, answers, gathered: gathered.filter((section) => section.lines.length > 0) }), { hasLogs: base.hasLogs, you });
}

/** The parts of a report about the crash of php-fpm, with a log of the service. */
export const crashBase = (answers = { happened: 'php-fpm stopped with an error at 8:09 AM.' }, { logs = true } = {}) => ({
  head: ['Dukovo problem report', 'Dukovo 0.16.0, channel stable', 'About: the crash of php-fpm', 'Machine: Dell OptiPlex 7040'],
  answers,
  hasLogs: true,
  gathered: [
    { id: 'crash', heading: 'Crash', lines: ['php-fpm stopped: SIGSEGV (address not mapped) at 0x0', 'in /users/alice/site from 10.1.2.3', 'Signature: 3fa2c4d19e0b7a61'] },
    ...(logs ? [{
      id: 'log',
      heading: 'Log of the service php (last 200 lines)',
      lines: ['php: listening on 9000', 'php: alice signed in', 'php: pool www reloaded', 'php: signal 11'],
    }] : []),
  ],
});

/** The parts of a report about the hardware, which has no log. */
export const hardwareBase = (answers) => ({
  head: ['Dukovo problem report', 'Dukovo 0.16.0, channel stable', 'About: the hardware', 'Machine: Dell OptiPlex 7040'],
  answers,
  hasLogs: false,
  gathered: [],
});

/** A fake reportd for a page test: the drafts it keeps, each with the parts it was made from. */
export function makeDesk({ base = crashBase } = {}) {
  const kept = new Map();
  let made = 0;
  const desk = {
    kept,
    asked: [],
    /** What `POST /api/1/reports/drafts` answers for `args`. */
    draft(args) {
      desk.asked.push(['draft', args]);
      const answers = { happened: args.happened, expected: args.expected ?? '', again: args.again ?? '' };
      const parts = args.about === 'hardware' || args.about === 'other' || args.about === 'update'
        ? hardwareBase(answers)
        : base(answers, { logs: !args.without_logs });
      made += 1;
      const id = made.toString(16).padStart(32, '0');
      kept.set(id, { parts, args });
      const answer = apply(parts);
      Object.assign(kept.get(id), { sha256: answer.sha256, text: answer.text });
      return { ...answer, draft: id };
    },
    /** What `POST /api/1/reports/drafts/{draft}/redact` answers. */
    redact(id, { sha256, edits }) {
      desk.asked.push(['redact', id, sha256, JSON.parse(edits)]);
      const one = kept.get(id);
      if (!one) throw Object.assign(new Error('That draft has ended'), { id: 'RPT-101' });
      if (one.sha256 !== sha256) throw Object.assign(new Error('The report is not the one you read'), { id: 'RPT-102' });
      const answer = apply(one.parts, JSON.parse(edits));
      Object.assign(one, { sha256: answer.sha256, text: answer.text });
      return { ...answer, draft: id };
    },
  };
  return desk;
}

// ---- the table ------------------------------------------------------

/** `GET /api/1/reports`: one report sent, one waiting, one that could not be sent. */
export const list = () => ({
  format: 1,
  reports: [
    {
      number: 7, title: 'valkey stopped with an error after a restore', state: 'sent', queued_ms: NOW - 14 * 86_400_000, sent_ms: NOW - 14 * 86_400_000, tries: 1, next_ms: null, receipt: 'r7',
    },
    {
      number: null, title: 'php-fpm stopped with an error', state: 'queued', queued_ms: NOW - 3_600_000, sent_ms: null, tries: 4, next_ms: NOW + 900_000, receipt: null,
    },
    {
      number: 5, title: 'The screen goes black', state: 'failed', queued_ms: NOW - 40 * 86_400_000, sent_ms: null, tries: 30, next_ms: null, receipt: null,
    },
  ],
});

/** `GET /api/1/reports/status`: the desk's words for the report sent. */
export const status = (words = 'Fixed in valkey 8.0.3') => ({
  format: 1, reports: [{ number: 7, receipt: 'r7', status: words, unreachable: false }],
});

/** `reports.send`'s answer as a change gives it: the command's JSON is an output line. */
export const sendAnswer = (more = {}) => ({
  done: true, changed: true, output: [JSON.stringify({ format: 1, number: 8, state: 'sent', why: null, ...more })],
});

/** `POST /api/1/reports/drafts/{draft}/phone`. */
export const phone = (more = {}) => ({
  url: 'https://192.168.1.20:8443/once/9f3c1a7e5b2d4c6a8e0f1b3d5a7c9e2b/report.txt',
  expires_ms: NOW + 600_000,
  fingerprint_sha256: Array.from({ length: 32 }, (_, at) => (at * 7 + 10).toString(16).padStart(2, '0').toUpperCase()).join(':'),
  ...more,
});
