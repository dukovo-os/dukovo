// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Report a problem as plain values: what can be reported and how a choice is
// asked of reportd, the words for what the draft holds and what was taken
// out of it, what Send answers, and the table of the reports from this
// server. No DOM.

import { bytesWords, counted } from '../../lib/size-words.js';

/** The longest an answer is, in characters (the argument's own limit). */
export const ANSWER_MAX = 2000;

/** The longest a report's title is, in characters. */
export const TITLE_MAX = 80;

/** The most crashes the choices list before "Show more". */
export const CRASHES_SHOWN = 5;

/** The kinds `reports.draft` takes for `about`, by the word the address uses for each. */
const ABOUT_WORDS = { failure: 'failure', crash: 'crash', service: 'service', hardware: 'hardware', update: 'update', updates: 'update', other: 'other' };

const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);

/** `answer`'s list named `key`: the answer may be the list or an object that holds it. */
export const listOf = (answer, key) => (Array.isArray(answer) ? answer : Array.isArray(answer?.[key]) ? answer[key] : []);

/**
 * What an action that changes something answers, as its JSON: the answer
 * itself when it is that, else the last line of its output that is a JSON
 * object, which is how a command's JSON comes back. Null when there is none.
 */
export function jsonOf(result) {
  if (result && typeof result === 'object' && !Array.isArray(result.output) && result.done === undefined) return result;
  const lines = Array.isArray(result?.output) ? result.output : [];
  for (const line of [...lines].reverse()) {
    try {
      const found = JSON.parse(line);
      if (found && typeof found === 'object' && !Array.isArray(found)) return found;
    } catch {
      // A line that is words, not JSON.
    }
  }
  return null;
}

// ---- what can be reported ------------------------------------------------------

/** The key of a choice: `failure`, `crash:ID`, `service:NAME`, `hardware`, `update`, `other`. */
export const keyOf = ({ about, crash, name }) => (about === 'crash' ? `crash:${crash}` : about === 'service' ? `service:${name}` : about);

/**
 * What `/report?about=…` asks for, from the address's query: `{about, crash,
 * name, note}`, or null when it names nothing the page can choose. `about`
 * is `failure`, `crash` with `id`, `service` with `name`, `hardware`,
 * `update` (or `updates`) and `other`; `note` is the person's first words.
 */
export function aboutOf(query) {
  const about = ABOUT_WORDS[query.get('about')];
  if (!about) return null;
  const crash = text(query.get('id'));
  const name = text(query.get('name'));
  if ((about === 'crash' && !crash) || (about === 'service' && !name)) return null;
  const note = (query.get('note') ?? '').slice(0, ANSWER_MAX);
  return { about, crash: about === 'crash' ? crash : null, name: about === 'service' ? name : null, note };
}

/** "at 8:09 AM", "yesterday at 7:40 PM", "on Nov 12 at 8:09 AM": the words after "stopped with an error". */
export function atWords(time, seconds) {
  const words = time.when(seconds);
  if (/^\d/.test(words)) return `at ${words}`;
  if (words.startsWith('Yesterday ')) return `yesterday at ${words.slice(10)}`;
  const found = /^(.+?) (\d{1,2}:\d{2} [AP]M)$/.exec(words);
  return found ? `on ${found[1]} at ${found[2]}` : `on ${words}`;
}

/**
 * The failed start as a choice: "The update to alpha.2 was taken back", or
 * a start that failed, with when this server found it and the check it
 * failed. `failed` is `changes --failed --json`'s `failed`.
 */
function failedChoice(failed, time) {
  const seconds = Number.isFinite(failed.at_ms) ? failed.at_ms / 1000 : null;
  const title = failed.result === 'fell-back'
    ? `The update to ${text(failed.release) ?? 'the new version'} was taken back`
    : `Start ${failed.start ?? ''} failed`.replace(/\s+/g, ' ');
  const found = seconds === null ? '' : ` ${atWords(time, seconds)}`;
  const check = text(failed.check);
  return {
    key: 'failure',
    about: 'failure',
    title,
    words: `Found by this server${found}${check ? `: it failed the check ${check}` : ''}.`,
    line: `${title}${found}.`,
    admin: true,
    questions: false,
  };
}

/** A crash reportd keeps as a choice. */
function crashChoice(crash, time) {
  const title = `${crash.program} stopped with an error`;
  return {
    key: keyOf({ about: 'crash', crash: crash.id }),
    about: 'crash',
    crash: crash.id,
    title,
    words: `${time.when(crash.at)}${text(crash.what) ? ` · ${crash.what}` : ''}`,
    line: `${title} ${atWords(time, crash.at)}.`,
    admin: false,
    questions: false,
  };
}

/** A failed service as a choice. `since` is when it failed, in seconds since 1970, or null. */
function serviceChoice(name, since, time) {
  const title = `${name} failed`;
  return {
    key: keyOf({ about: 'service', name }),
    about: 'service',
    name,
    title,
    words: since === null ? 'Its log goes with the report.' : `${time.when(since)} · its log goes with the report`,
    line: `${title}${since === null ? '' : ` ${atWords(time, since)}`}.`,
    admin: true,
    questions: false,
  };
}

const FIXED = [
  { key: 'hardware', about: 'hardware', title: 'The hardware', words: 'A part or a device that does not work as it should.' },
  { key: 'update', about: 'update', title: 'Installing or updating Dukovo', words: 'An install or an update that went wrong.' },
  { key: 'other', about: 'other', title: 'Something else', words: 'Describe it below.' },
].map((choice) => ({ ...choice, line: '', admin: false, questions: true }));

/**
 * The choices of "What went wrong?", the newest first: the failed start,
 * each crash not reported (`crashes`, reportd's list), each failed service,
 * then the hardware, installing or updating, and something else.
 * `failed` is `/api/1/updates/failed`'s answer, `show` the services'
 * document (its times count from the machine's start, so `now` is the
 * browser's time in seconds). A reader is offered neither the failed start
 * nor a service: an administrator reports those.
 */
export function choicesOf({
  failed, crashes, show, now, time, admin,
}) {
  const choices = [];
  if (admin && failed?.failed) choices.push(failedChoice(failed.failed, time));
  const crashed = listOf(crashes, 'crashes')
    .filter((crash) => crash && typeof crash.id === 'string' && typeof crash.program === 'string' && !crash.reported)
    .sort((a, b) => b.at - a.at);
  choices.push(...crashed.map((crash) => crashChoice(crash, time)));
  if (admin) {
    const services = listOf(show, 'services').filter((one) => one?.state === 'failed' && typeof one.name === 'string');
    choices.push(...services.map((one) => {
      const since = Number.isFinite(show?.now_ms) && Number.isFinite(one.since_ms) ? Math.floor(now - Math.max(0, show.now_ms - one.since_ms) / 1000) : null;
      return serviceChoice(one.name, since, time);
    }));
  }
  return [...choices, ...FIXED];
}

/** The choices that are shown: the crashes beyond `CRASHES_SHOWN` stay behind "Show more" unless `all` or `keep` (the chosen one) is among them. Gives `{shown, more}`. */
export function shownChoices(choices, { all, keep }) {
  let crashes = 0;
  const shown = choices.filter((choice) => {
    if (choice.about !== 'crash') return true;
    crashes += 1;
    return all || crashes <= CRASHES_SHOWN || choice.key === keep;
  });
  return { shown, more: choices.length - shown.length };
}

// ---- the answers and the draft ------------------------------------------------------

/** What the form holds: `note` for a choice that asks one question, the three answers for the others. */
export const emptyFields = () => ({
  note: '', happened: '', expected: '', again: '',
});

/** The first answer: the choice's own line and the note after it, or what the person wrote. */
export function happenedOf(choice, fields) {
  if (choice.questions) return fields.happened.trim();
  return [choice.line, fields.note.trim()].filter(Boolean).join(' ');
}

/** How long the note can be: the answer's limit less the choice's own line. */
export const noteMax = (choice) => Math.max(0, ANSWER_MAX - choice.line.length - 1);

/** Whether a draft can be made: a choice that asks questions needs the first answer. */
export const ready = (choice, fields) => choice !== null && (!choice.questions || fields.happened.trim() !== '');

/**
 * The arguments of `reports.draft`, for `choice`, the form's `fields` and the
 * boxes `{security, withoutLogs}`.
 */
export function draftArgs(choice, fields, { security = false, withoutLogs = false } = {}) {
  const args = { about: choice.about, happened: happenedOf(choice, fields) };
  if (choice.crash) args.crash = choice.crash;
  if (choice.name) args.name = choice.name;
  if (choice.questions) {
    if (fields.expected.trim() !== '') args.expected = fields.expected.trim();
    if (fields.again.trim() !== '') args.again = fields.again.trim();
  }
  if (withoutLogs) args.without_logs = true;
  if (security) args.security = true;
  return args;
}

/** The three answers as the person has them: for a choice with one question, its line and note are the first. */
export const answersOf = (choice, fields) => ({
  happened: happenedOf(choice, fields), expected: choice.questions ? fields.expected.trim() : '', again: choice.questions ? fields.again.trim() : '',
});

/** The title the report goes by in the table: what happened, to its first line, at most `TITLE_MAX` characters. */
export function titleOf(choice, happened) {
  const first = choice.questions ? (happened.split('\n').find((line) => line.trim() !== '') ?? '').trim() : choice.title;
  const title = first === '' ? choice.title : first;
  const chars = [...title];
  return chars.length <= TITLE_MAX ? title : `${chars.slice(0, TITLE_MAX - 1).join('').trimEnd()}…`;
}

/** The name of the file that Save to a file writes: `report-2026-11-16.txt`, by the day in the person's zone. */
export function fileNameOf(zone, now = Date.now()) {
  const day = new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(now);
  return `report-${day}.txt`;
}

// ---- what the report holds ------------------------------------------------------

/** What each mark of `cut_by` counts, in the order the sentence has them: `[keys, one, many]`. */
const MARKS = [
  [['user'], 'account name', 'account names'],
  [['host'], 'host name', 'host names'],
  [['ipv4', 'ipv6'], 'address', 'addresses'],
  [['mac'], 'hardware address', 'hardware addresses'],
  [['email'], 'email address', 'email addresses'],
  [['wifi'], 'Wi-Fi network name', 'Wi-Fi network names'],
  [['serial'], 'serial number', 'serial numbers'],
  [['machine'], 'machine ID', 'machine IDs'],
];

/** "3 addresses, 2 account names, 1 host name", most first; "" when nothing was taken out. `cut_by` is the draft's. */
export function takenOutList(cutBy) {
  return MARKS
    .map(([keys, one, many]) => ({ count: keys.reduce((sum, key) => sum + (Number.isFinite(cutBy?.[key]) ? cutBy[key] : 0), 0), one, many }))
    .filter((mark) => mark.count > 0)
    .sort((a, b) => b.count - a.count)
    .map((mark) => `${mark.count} ${mark.count === 1 ? mark.one : mark.many}`)
    .join(', ');
}

/** The sentence under the parts: what was taken out before anything was shown. */
export function takenOutWords(cutBy) {
  const list = takenOutList(cutBy);
  return list === '' ? 'Nothing needed to be taken out.' : `Taken out before anything is shown: ${list}.`;
}

/** The parts of the report and their sizes, as rows `{title, size}`. */
export const partsOf = (answer) => (Array.isArray(answer?.parts) ? answer.parts : []).map((part) => ({ title: part.title, size: bytesWords(part.bytes) }));

/** "What the report holds" and its size. */
export const holdsWords = (answer) => bytesWords(answer?.bytes);

/** What a person is told of the marks in the text: what they are, how many were taken out and how many words they cut. */
export function marksWords(cutBy, you) {
  const marks = MARKS.reduce((sum, [keys]) => sum + keys.reduce((inner, key) => inner + (cutBy?.[key] ?? 0), 0), 0);
  const cut = you > 0 ? ` [cut] shows what you cut: ${you}.` : '';
  return `A mark such as [user] shows what was taken out: ${marks} in all.${cut}`;
}

/** The card's name for the text, as a screen reader says it. */
export const textLabel = (answer) => `Exactly what is sent, ${holdsWords(answer)}`;

/** What is off while a report is for the security team: it goes encrypted and only so, so the file and the code are not offered. */
export const SECURITY_WORDS = 'A security report is sent encrypted only';

/**
 * Which of the buttons under the report are on, for a report with a text
 * (`has`) that goes to the security team or not: Send always, and Save to a
 * file and Download by QR code unless the report is a security report, which
 * is sent encrypted only. Gives `{send, save, qr, why}`; `why` is what is
 * said of the two that are off, or null.
 */
export const offered = ({ has, security }) => ({
  send: has, save: has && !security, qr: has && !security, why: security ? SECURITY_WORDS : null,
});

/** "You cut 12 lines and 3 parts of lines.", or "" when nothing is cut. `you` is `cut_by_you`. */
export function youCutWords(you) {
  const parts = cutNouns(you);
  return parts === '' ? '' : `You cut ${parts}.`;
}

/** "12 lines and 3 parts of lines", "1 line", "1 part of a line". */
function cutNouns(you) {
  const lines = you?.lines > 0 ? counted(you.lines, 'line') : null;
  const parts = you?.parts > 0 ? (you.parts === 1 ? '1 part of a line' : `${you.parts} parts of lines`) : null;
  return [lines, parts].filter(Boolean).join(' and ');
}

/** The question before a new draft takes the changes away: `you` is `cut_by_you`, `typed` whether the answers were edited. */
export function bringsBackWords(you, typed) {
  const cut = cutNouns(you);
  const said = [cut !== '' && `the ${cut} you cut`, typed && 'your answers as you first wrote them'].filter(Boolean).join(' and ');
  return `Drafting again brings back ${said}. Draft again?`;
}

// ---- Send ------------------------------------------------------

/** What Send says when the report was sent: its number. */
export const sentWords = (number) => (Number.isFinite(number) ? `Sent as report ${number}.` : 'Sent.');

/** What Send says when the report waits: when reports.dukovo.org cannot be reached, and when reportd tries again. */
export const QUEUED_WORDS = 'Queued: it is sent when reports.dukovo.org can be reached. reportd tries again after 1, 5, 15 and 60 minutes, then every hour for 30 days.';

/** What `reports.send` answered, as `{number, queued, why}`; null when it answered nothing the page can read. */
export function sendOf(result) {
  const answer = jsonOf(result);
  if (!answer || (answer.state !== 'sent' && answer.state !== 'queued')) return null;
  return { number: Number.isFinite(answer.number) ? answer.number : null, queued: answer.state === 'queued', why: text(answer.why) };
}

// ---- Reports from this server ------------------------------------------------------

/** The kind a status gets: a report fixed is good, one that will not be is plain, the others wait. */
const statusKind = (words) => (/^Fixed/.test(words) ? 'ok' : /^Will not/.test(words) ? 'warn' : 'off');

/** Words for a time given in milliseconds since 1970, or "" when it is none. */
const whenWords = (time, ms) => (Number.isFinite(ms) ? time.when(ms / 1000) : '');

/**
 * The rows of the table: the reports that wait first, then those sent, the
 * newest first. `status` is what `reports.status` answered, a Map of the
 * number to its row, or null when it was not asked; `asked` is `{why}` when
 * asking failed. Each row is `{number, sent, title, status, note, kind,
 * canDelete, queued}`.
 */
export function tableRows(list, { status = null, asked = null, time }) {
  const reports = listOf(list, 'reports').filter((report) => Number.isFinite(report?.number) || report?.state === 'queued');
  const order = (report) => (report.state === 'queued' ? Number.MAX_SAFE_INTEGER : (report.sent_ms ?? report.queued_ms ?? 0));
  return [...reports].sort((a, b) => order(b) - order(a)).map((report) => {
    const row = {
      number: report.number ?? null,
      title: text(report.title) ?? 'A report',
      sent: report.state === 'sent' ? whenWords(time, report.sent_ms) : report.state === 'queued' ? 'Waiting' : 'Not sent',
      canDelete: report.state === 'sent',
      queued: report.state === 'queued',
      note: null,
    };
    if (report.state === 'queued') {
      const tried = report.tries > 0 ? ` · tried ${counted(report.tries, 'time')}` : '';
      const next = Number.isFinite(report.next_ms) ? `, next at ${time.when(report.next_ms / 1000)}` : '';
      return { ...row, kind: 'off', status: 'Queued', note: `Sends when reports.dukovo.org can be reached${tried}${next}${text(report.why) ? `, last: ${report.why}` : ''}` };
    }
    if (report.state === 'failed') {
      return { ...row, kind: 'bad', status: 'Could not be sent', note: text(report.why) ?? 'It was tried for 30 days.' };
    }
    if (asked) return { ...row, kind: 'warn', status: `Could not ask: ${asked.why}`, note: null };
    const got = status?.get(report.number);
    if (got?.unreachable) return { ...row, kind: 'warn', status: 'Could not ask: reports.dukovo.org did not answer', note: null };
    if (got) {
      const words = text(got.status) ?? 'Received';
      return { ...row, kind: statusKind(words), status: words, note: null };
    }
    return { ...row, kind: 'off', status: 'Not asked yet', note: 'Check the status to ask the server.' };
  });
}

/** The status answers of `reports.status` as a Map of number to row. */
export const statusMap = (answer) => new Map(listOf(answer, 'reports').filter((one) => Number.isFinite(one?.number)).map((one) => [one.number, one]));

/** The words of an error, to say why the server could not be asked. */
export const whyWords = (error) => [error?.title, error?.detail].filter(Boolean).join(': ') || 'no answer';

/** What the delete question says. */
export const DELETE_WORDS = 'Deletes it on the reports server and here.';
