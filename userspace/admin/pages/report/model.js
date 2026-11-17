// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Report a problem as plain values: what can be reported, how a choice is asked
// of reportd, the words for what the draft holds, and what Send answers. No DOM.

/** The longest an answer is, in characters. */
export const ANSWER_MAX = 2000;

/** The longest a report's title is, in characters. */
const TITLE_MAX = 80;

/** The most crashes the choices list before Show more. */
const CRASHES_SHOWN = 5;

/** The kinds `reports.draft` takes, by the word the address uses for each. */
const ABOUT_WORDS = { failure: 'failure', crash: 'crash', service: 'service', hardware: 'hardware', update: 'update', updates: 'update', other: 'other' };

/** `count` and its noun, with an s when it is not one. */
const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** A size as the board writes it: "612 B", "19.2 KB". */
const sizeWords = (bytes) => (bytes < 1000 ? `${bytes} B` : `${(bytes / 1000).toFixed(1)} KB`);

const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);

/** `answer`'s list named `key`; the answer may be the list itself. */
export const listOf = (answer, key) => (Array.isArray(answer) ? answer : Array.isArray(answer?.[key]) ? answer[key] : []);

/** What a change answers as JSON: the answer itself, else the last output line that is a JSON object; null when none. */
export function jsonOf(result) {
  if (result && typeof result === 'object' && !Array.isArray(result.output) && result.done === undefined) return result;
  for (const line of [...(Array.isArray(result?.output) ? result.output : [])].reverse()) {
    try {
      const found = JSON.parse(line);
      if (found && typeof found === 'object' && !Array.isArray(found)) return found;
    } catch {
      // A line of words.
    }
  }
  return null;
}

// ---- reportd ------------------------------------------------------

/** Whether reportd is a service of `show` (the services' document) that is not running. */
export function reportdDown(show) {
  const one = listOf(show, 'services').find((service) => service?.name === 'reportd');
  return Boolean(one) && !['running', 'listening', 'starting'].includes(one.state);
}

/** Whether `error` is reportd not answering at all: the command says how to start it. */
export const reportdStopped = (error) => /elevate service reportd start|reportd is not running/i.test([error?.title, error?.detail, error?.fix, error?.command?.stderr].filter(Boolean).join(' '));

/** Whether `error` says the draft ended or is not the text that was read: drafting again is the answer. */
export const draftEnded = (error) => error?.id === 'RPT-101' || error?.id === 'RPT-102';

// ---- what can be reported ------------------------------------------------------

/** The key of a choice: `failure`, `crash:ID`, `service:NAME`, `hardware`, `update`, `other`. */
export const keyOf = ({ about, crash, name }) => (about === 'crash' ? `crash:${crash}` : about === 'service' ? `service:${name}` : about);

/** What `/report?about=…` asks for, `{about, crash, name, note}`, or null when it names nothing the page can choose. */
export function aboutOf(query) {
  const about = ABOUT_WORDS[query.get('about')];
  const crash = text(query.get('id'));
  const name = text(query.get('name'));
  if (!about || (about === 'crash' && !crash) || (about === 'service' && !name)) return null;
  return {
    about, crash: about === 'crash' ? crash : null, name: about === 'service' ? name : null, note: (query.get('note') ?? '').slice(0, ANSWER_MAX),
  };
}

/** "at 8:09 AM", "yesterday at 7:40 PM", "on Nov 12 at 8:09 AM". */
export function atWords(time, seconds) {
  const words = time.when(seconds);
  if (/^\d/.test(words)) return `at ${words}`;
  if (words.startsWith('Yesterday ')) return `yesterday at ${words.slice(10)}`;
  const found = /^(.+?) (\d{1,2}:\d{2} [AP]M)$/.exec(words);
  return found ? `on ${found[1]} at ${found[2]}` : `on ${words}`;
}

/** The failed start (`changes --failed --json`'s `failed`) as a choice. */
function failedChoice(failed, time) {
  const title = failed.result === 'fell-back' ? `The update to ${text(failed.release) ?? 'the new version'} was taken back` : `Start ${failed.start ?? ''} failed`.replace(/\s+/g, ' ');
  const found = Number.isFinite(failed.at_ms) ? ` ${atWords(time, failed.at_ms / 1000)}` : '';
  const check = text(failed.check);
  return {
    key: 'failure', about: 'failure', title, words: `Found by this server${found}${check ? `: it failed the check ${check}` : ''}.`, line: `${title}${found}.`, admin: true, questions: false,
  };
}

/** A crash reportd keeps as a choice. */
function crashChoice(crash, time) {
  const title = `${crash.program} stopped with an error`;
  const at = Number.isFinite(crash.at_ms) ? crash.at_ms / 1000 : null;
  const when = at === null ? 'Time not known' : time.when(at);
  return {
    key: keyOf({ about: 'crash', crash: crash.id }),
    about: 'crash',
    crash: crash.id,
    title,
    words: `${when}${text(crash.what) ? ` · ${crash.what}` : ''}`,
    line: `${title}${at === null ? '' : ` ${atWords(time, at)}`}.`,
    admin: false,
    questions: false,
  };
}

/** A failed service as a choice; `since` is when it failed, in seconds since 1970, or null. */
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

/** The choices of "What went wrong?": the failed start, the crashes not reported, the failed services (an administrator's), then the three always there. `now` is seconds since 1970; the services' times count from the machine's start. */
export function choicesOf({
  failed, crashes, show, now, time, admin,
}) {
  const choices = [];
  if (admin && failed?.failed) choices.push(failedChoice(failed.failed, time));
  choices.push(...listOf(crashes, 'crashes')
    .filter((crash) => typeof crash?.id === 'string' && typeof crash.program === 'string' && !crash.reported)
    .sort((a, b) => (b.at_ms ?? 0) - (a.at_ms ?? 0))
    .map((crash) => crashChoice(crash, time)));
  if (admin) {
    for (const one of listOf(show, 'services').filter((service) => service?.state === 'failed' && typeof service.name === 'string')) {
      const ago = Number.isFinite(show.now_ms) && Number.isFinite(one.since_ms) ? Math.max(0, show.now_ms - one.since_ms) / 1000 : null;
      choices.push(serviceChoice(one.name, ago === null ? null : Math.floor(now - ago), time));
    }
  }
  return [...choices, ...FIXED];
}

/** `{shown, more}`: the crashes past `CRASHES_SHOWN` are behind Show more, unless `all` or the chosen one (`keep`) is among them. */
export function shownChoices(choices, { all, keep }) {
  let crashes = 0;
  const shown = choices.filter((choice) => choice.about !== 'crash' || all || (crashes += 1) <= CRASHES_SHOWN || choice.key === keep);
  return { shown, more: choices.length - shown.length };
}

// ---- the answers and the draft ------------------------------------------------------

/** What the form holds: `note` for a choice that asks one question, the three answers for the others. */
export const emptyFields = () => ({
  note: '', happened: '', expected: '', again: '',
});

/** The first answer: the choice's own line and the note after it, or what the person wrote. */
export const happenedOf = (choice, fields) => (choice.questions ? fields.happened.trim() : [choice.line, fields.note.trim()].filter(Boolean).join(' '));

/** How long the note can be: the answer's limit less the choice's own line. */
export const noteMax = (choice) => Math.max(0, ANSWER_MAX - choice.line.length - 1);

/** Whether a draft can be made: a choice that asks questions needs the first answer. */
export const ready = (choice, fields) => choice !== null && (!choice.questions || fields.happened.trim() !== '');

/**
 * The arguments of `reports.draft` for `choice`, the form's `fields` and the
 * box that leaves the logs out. Whether it is a security report is said when
 * it is sent, and does not change the text.
 */
export function draftArgs(choice, fields, { withoutLogs = false } = {}) {
  const args = { about: choice.about, happened: happenedOf(choice, fields) };
  if (choice.crash) args.crash = choice.crash;
  if (choice.name) args.name = choice.name;
  for (const name of choice.questions ? ['expected', 'again'] : []) if (fields[name].trim() !== '') args[name] = fields[name].trim();
  if (withoutLogs) args.without_logs = true;
  return args;
}

/** The three answers as the person has them, which the form for writing them again starts with. */
export const answersOf = (choice, fields) => ({
  happened: happenedOf(choice, fields), expected: choice.questions ? fields.expected.trim() : '', again: choice.questions ? fields.again.trim() : '',
});

/** The title of the report: the first line of what happened, or the choice's own words, in at most `TITLE_MAX` characters. */
export function titleOf(choice, happened) {
  const first = choice.questions ? (happened.split('\n').find((line) => line.trim() !== '') ?? '').trim() : '';
  const chars = [...(first || choice.title)];
  return chars.length <= TITLE_MAX ? chars.join('') : `${chars.slice(0, TITLE_MAX - 1).join('').trimEnd()}…`;
}

/** `report-2026-11-16.txt`, by the day in the person's zone. */
export function fileNameOf(zone, now = Date.now()) {
  return `report-${new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(now)}.txt`;
}

// ---- what the report holds ------------------------------------------------------

/** What each mark of `cut_by` counts, in the order of the sentence: `[keys, one, many]`. */
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

const countOf = (cutBy, keys) => keys.reduce((sum, key) => sum + (Number.isFinite(cutBy?.[key]) ? cutBy[key] : 0), 0);

/** "3 addresses, 2 account names, 1 host name", the most first; "" when nothing was taken out. */
export function takenOutList(cutBy) {
  return MARKS
    .map(([keys, one, many]) => ({ count: countOf(cutBy, keys), one, many }))
    .filter((mark) => mark.count > 0)
    .sort((a, b) => b.count - a.count)
    .map((mark) => `${mark.count} ${mark.count === 1 ? mark.one : mark.many}`)
    .join(', ');
}

/** What was taken out before anything was shown. */
export function takenOutWords(cutBy) {
  const list = takenOutList(cutBy);
  return list === '' ? 'Nothing needed to be taken out.' : `Taken out before anything is shown: ${list}.`;
}

/** The parts of the report and their sizes: `[{title, size}]`. */
export const partsOf = (answer) => (Array.isArray(answer?.parts) ? answer.parts : []).map((part) => ({ title: part.title, size: sizeWords(part.bytes) }));

/** The size of the whole report in words. */
export const holdsWords = (answer) => sizeWords(answer?.bytes ?? 0);

/** What the marks in the text are, and how many words were cut, said before the text. */
export function marksWords(cutBy, you) {
  const marks = countOf(cutBy, MARKS.flatMap(([keys]) => keys));
  return `A mark such as [user] shows what was taken out: ${marks} in all.${you > 0 ? ` [cut] shows what you cut: ${you}.` : ''}`;
}

/** The name a screen reader gives the text. */
export const textLabel = (answer) => `Exactly what is sent, ${holdsWords(answer)}`;

/** Why Save to a file and Download by QR code are off for a security report. */
const SECURITY_WORDS = 'A security report is sent encrypted only';

/** Which buttons are on, for a report with a text (`has`) that is for the security team or not; `why` is said of the two that are off. */
export const offered = ({ has, security }) => ({
  send: has, save: has && !security, qr: has && !security, why: security ? SECURITY_WORDS : null,
});

/** "12 lines and 3 parts of lines", "1 line", "1 part of a line"; `you` is `cut_by_you`. */
function cutNouns(you) {
  const lines = you?.lines > 0 ? counted(you.lines, 'line') : null;
  const parts = you?.parts > 0 ? (you.parts === 1 ? '1 part of a line' : `${you.parts} parts of lines`) : null;
  return [lines, parts].filter(Boolean).join(' and ');
}

/** "You cut 12 lines and 3 parts of lines.", or "" when nothing is cut. */
export const youCutWords = (you) => (cutNouns(you) === '' ? '' : `You cut ${cutNouns(you)}.`);

/** The question before a new draft takes the cuts, and the answers written again, away. */
export function bringsBackWords(you, typed) {
  const cut = cutNouns(you);
  return `Drafting again brings back ${[cut !== '' && `the ${cut} you cut`, typed && 'your answers as you first wrote them'].filter(Boolean).join(' and ')}. Draft again?`;
}

// ---- Send ------------------------------------------------------

/** What Send says of a report sent. */
export const sentWords = (number) => (Number.isFinite(number) ? `Sent as report ${number}.` : 'Sent.');

/** What Send says of a report that waits. */
export const QUEUED_WORDS = 'Queued: it is sent when reports.dukovo.org can be reached. reportd tries again after 1, 5, 15 and 60 minutes, then every hour for 30 days.';

/** What `reports.send` answered, `{number, queued, why}`; null when the page cannot read it. */
export function sendOf(result) {
  const answer = jsonOf(result);
  if (!answer || (answer.state !== 'sent' && answer.state !== 'queued')) return null;
  return { number: Number.isFinite(answer.number) ? answer.number : null, queued: answer.state === 'queued', why: text(answer.why) };
}
