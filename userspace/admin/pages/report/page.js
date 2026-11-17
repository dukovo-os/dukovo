// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Report a problem: /report. reportd drafts the report and keeps the draft; the
// page shows every byte, lets the person cut and write answers again (one edit
// list that reportd applies), and sends, saves or hands to a phone exactly the
// text read. A draft is made when the choice or a box changes, a field is left
// with other words, or on Draft again, never on each key.

import {
  asSentence, fill, h, icon,
} from '../../dom.js';
import { saveText } from '../../lib/save-file.js';
import { forget, keep, recall } from '../../lib/store.js';
import { makeChoices } from './choices.js';
import { editsOf } from './cuts-model.js';
import { makeHolds } from './holds.js';
import {
  aboutOf, answersOf, bringsBackWords, choicesOf, draftArgs, draftEnded, emptyFields, fileNameOf, happenedOf, keyOf, listOf, offered,
  QUEUED_WORDS, ready, reportdDown, reportdStopped, sendOf, sentWords, shownChoices, titleOf,
} from './model.js';
import { makePreview } from './preview.js';

const NOTE = 'report.note';
const DRAFTS = '/api/1/reports/drafts';
const CRASHES = '/api/1/reports/crashes';
const LEAD = 'Nothing leaves this server until you press Send, and you see every byte first. Reports go to reports.dukovo.org over an encrypted connection.';
const PRIVACY = 'A report holds your answers, facts about this machine (the Dukovo version, the maker and model, the processor, the memory, the device numbers of its PCI and USB devices, and the language) and what it is about. Before you see it, user names, home folders, the host name, addresses, email addresses, Wi-Fi network names, serial numbers and the machine\'s ID are taken out and marked, in your own words too. No registers, files, account or cookie go with it.';
const SEND_WORDS = {
  unlock: 'send the report', running: 'Sending the report', done: 'The report was given to reportd to send.', unchanged: 'The report was given to reportd to send.', busy: 'Sending…',
};

/** `promise`'s answer, or null when it could not be had: nothing the page reads besides the draft is needed to draft. */
const soft = (promise) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return null;
});

/** What was kept of what the person typed: the fields, and only text. */
const recalled = () => {
  const kept = recall(NOTE, {});
  return Object.fromEntries(Object.keys(emptyFields()).map((name) => [name, typeof kept?.[name] === 'string' ? kept[name] : '']));
};

/** A banner of `kind` with `words` and `buttons`. */
const banner = (kind, iconName, words, buttons = []) => h('div', { class: `banner ${kind}` }, icon(iconName), h('div', {}, words, buttons.length > 0 && h('div', { class: 'btns' }, ...buttons)));

export async function show(view) {
  view.head({ title: 'Report a problem', commands: true });
  const { admin } = view.session;
  const fields = recalled();
  const [crashes, failed, services, actions] = await Promise.all([
    soft(view.api.get(CRASHES)), admin ? soft(view.api.get('/api/1/updates/failed')) : null, soft(view.api.get('/api/1/services')), soft(view.api.get('/api/1/actions')),
  ]);
  const offers = (id) => actions === null || listOf(actions, 'actions').some((one) => one.id === id);
  const older = !offers('reports.draft');
  let crashList = crashes;
  let sentIds = new Set();

  const choicesNow = () => choicesOf({
    failed, crashes: listOf(crashList, 'crashes').filter((crash) => !sentIds.has(crash.id)), show: services, now: Math.floor(Date.now() / 1000), time: view.time, admin,
  });
  let all = choicesNow();
  const wanted = aboutOf(view.query);
  let key = wanted && all.some((choice) => choice.key === keyOf(wanted)) ? keyOf(wanted) : null;
  const adminNeeded = Boolean(wanted) && !key && ['failure', 'service'].includes(wanted.about) && !admin;
  if (key && wanted.note) fields[all.find((choice) => choice.key === key).questions ? 'happened' : 'note'] = wanted.note.trim();
  let showAll = false;
  let listed = shownChoices(all, { all: showAll, keep: key });
  let security = false;
  let withoutLogs = false;
  let first = null;
  let now = null;
  let cuts = [];
  let answers = null;
  let committed = null;
  let drafting = false;
  let generation = 0;
  let stopped = reportdDown(services);

  const chosen = () => all.find((choice) => choice.key === key) ?? null;
  const hasChanges = () => cuts.length > 0 || answers !== null;
  const argsOf = () => draftArgs(chosen(), fields, { withoutLogs });
  const dirty = () => key !== null || Object.values(fields).some((value) => value.trim() !== '');
  const sendArgs = () => ({
    draft: first.draft,
    sha256: now.sha256,
    title: titleOf(chosen(), answers?.happened ?? happenedOf(chosen(), fields)),
    ...(chosen().crash ? { crash: chosen().crash } : {}),
    ...(security ? { security: true } : {}),
  });

  // The table is read after the page has drawn.
  const sentPart = import('./sent.js').then(({ makeSent }) => makeSent(view, { listed: offers('reports.list') }));
  const sentSlot = h('div', {});
  sentPart.then((sent) => {
    sentSlot.append(sent.element);
    return sent.reload();
  }).catch((error) => view.problem(error, { slot: sentSlot }));

  const problem = h('div', { class: 'rp-problem' });
  const status = h('div', { class: 'rp-status', role: 'status' });
  const send = h('button', { class: 'btn default', type: 'button' }, 'Send');
  const save = h('button', { class: 'btn', type: 'button', 'aria-describedby': 'report-why' }, 'Save to a file…');
  const qr = h('button', { class: 'btn', type: 'button', 'aria-describedby': 'report-why' }, 'Download by QR code…');
  const why = h('p', { id: 'report-why', class: 'small muted' });
  const commandSlot = h('div', { class: 'rp-command' });

  const choices = makeChoices({
    fields,
    onChoose: (next) => change(() => { key = next; }, () => { key = committed.key; paint(); }),
    onMore: () => {
      showAll = true;
      listed = shownChoices(all, { all: showAll, keep: key });
      paint();
    },
    onType: () => {
      if (dirty()) keep(NOTE, fields);
      else forget(NOTE);
      paintChoices();
    },
    onField: () => afterChange(() => { Object.assign(fields, committed.fields); choices.setValues(); }),
    // Said when it is sent: the text, and the cuts made in it, stay.
    onSecurity: (on) => {
      security = on;
      paint();
    },
    onDiscard: () => reset(),
  });
  const holds = makeHolds({ onLogs: (left) => afterChange(() => { withoutLogs = committed.withoutLogs; paint(); }, () => { withoutLogs = left; }) });
  const preview = makePreview(view, {
    onCut: (next) => redact(next, answers),
    onAnswers: (written) => redact(cuts, written),
    onUndo: () => redact([], null),
    onAgain: () => draftAgain(),
  });

  // ---- drawing ----

  const mode = () => {
    if (older) return 'older';
    if (stopped) return 'stopped';
    if (now) return 'ready';
    if (drafting) return 'drafting';
    return adminNeeded ? 'admin' : 'choose';
  };

  /** What the preview says in place of a text. */
  function noneFor(state) {
    if (state === 'older') return banner('info', 'info', h('p', {}, 'This server\'s admind does not offer this yet; run ', h('code', {}, 'report'), '.'));
    if (state === 'admin') return banner('info', 'info', h('p', {}, 'A report about a failed start or a service is an administrator\'s to send. Ask an administrator, or choose what else went wrong.'));
    if (state !== 'stopped') return null;
    return banner('warn', 'warn', h('p', {}, 'reportd is not running, so no report can be drafted: ', h('code', {}, 'elevate service reportd start')), [
      h('button', { class: 'btn', type: 'button', onclick: () => { stopped = false; draft(); } }, 'Try again'),
      h('a', { class: 'btn', href: '/services/reportd' }, 'Open the page of reportd'),
    ]);
  }

  function paintChoices() {
    choices.paint({
      shown: listed.shown, more: listed.more, key, choice: chosen(), security, readerNote: !admin, dirty: dirty(),
    });
  }

  function paintButtons() {
    const state = mode();
    const on = offered({ has: state === 'ready' && !drafting, security });
    send.disabled = !on.send;
    save.disabled = !on.save;
    qr.disabled = !on.qr;
    why.textContent = on.why && state === 'ready' ? `Save to a file and Download by QR code are off. ${on.why}.` : '';
    fill(commandSlot, view.prefs.commands === 'shown' && state === 'ready' && [
      view.api.inline('reports.send', sendArgs()), h('span', { class: 'inline-cmd' }, 'report --save /volumes/USB/report.txt'),
    ]);
  }

  function paint() {
    const state = mode();
    paintChoices();
    holds.paint(state === 'ready' ? now : null, { withoutLogs, drafting });
    preview.paint({
      mode: state,
      drafting,
      first,
      now,
      cuts,
      edited: answers !== null,
      needsFirst: chosen()?.questions === true,
      answers: answers ?? (chosen() ? answersOf(chosen(), fields) : null),
    }, noneFor(state));
    paintButtons();
  }

  // ---- drafting, and what changes it ----

  /** What the controls held when the text was drafted. */
  const snapshot = () => ({
    key, fields: { ...fields }, withoutLogs, args: JSON.stringify(chosen() && ready(chosen(), fields) ? argsOf() : null),
  });

  /** Forgets the draft, and the cuts with it. */
  function clearDraft() {
    first = null;
    now = null;
    cuts = [];
    answers = null;
  }

  /** Shows an error where the page's go; a draft that ended is answered with Draft again. */
  function fail(error) {
    if (error?.name === 'AbortError') return;
    if (!draftEnded(error)) {
      view.problem(error, { slot: problem });
      return;
    }
    fill(problem, banner('warn', 'warn', h('p', {}, `${asSentence(error.title)} Draft the report again to go on.`), [
      h('button', { class: 'btn', type: 'button', onclick: () => draftAgain() }, 'Draft again'),
    ]));
  }

  /** Posts `body` to `path` for a text; `apply(answer)` takes it, `onFail(error)` what went wrong. */
  async function fetchText(path, body, apply, onFail) {
    fill(problem);
    const mine = (generation += 1);
    drafting = true;
    paint();
    try {
      const answer = await view.api.post(path, body);
      if (mine !== generation) return false;
      apply(answer);
      return true;
    } catch (error) {
      if (mine === generation && error?.name !== 'AbortError') onFail(error);
      return false;
    } finally {
      if (mine === generation) {
        drafting = false;
        paint();
      }
    }
  }

  /** Drafts the report from the choice and the fields, and forgets the cuts. */
  async function draft() {
    if (chosen() && ready(chosen(), fields) && !older && !stopped) {
      await fetchText(DRAFTS, argsOf(), (answer) => {
        clearDraft();
        first = answer;
        now = answer;
        stopped = false;
        committed = snapshot();
        fill(status);
      }, (error) => {
        clearDraft();
        committed = null;
        if (reportdStopped(error)) stopped = true;
        else fail(error);
      });
      return;
    }
    generation += 1;
    clearDraft();
    committed = snapshot();
    drafting = false;
    fill(problem);
    paint();
  }

  /** Asks before the cuts and the answers written again are lost to a new draft: true when it may go on. */
  async function mayDraftAgain() {
    if (!hasChanges()) return true;
    const { ask } = await import('../../shell/dialogs.js');
    const choice = await ask({
      title: 'Draft again?',
      words: bringsBackWords(now.cut_by_you, answers !== null),
      iconName: 'warn',
      choices: [{ label: 'Keep my cuts', value: 'keep', kind: 'default', safe: true }, { label: 'Draft again', value: 'again' }],
    });
    return choice === 'again';
  }

  /** Sets what changed with `commit` and drafts again, after the question when there are changes; `revert` puts the control back. */
  async function change(commit, revert) {
    if (!(await mayDraftAgain())) {
      revert();
      return;
    }
    commit();
    await draft();
  }

  /** Drafts again when the arguments are not those of the draft there is. */
  async function afterChange(revert, commit = () => {}) {
    commit();
    if (!chosen() || (committed && snapshot().args === committed.args)) {
      paint();
      return;
    }
    await change(() => {}, revert);
  }

  async function draftAgain() {
    if (await mayDraftAgain()) await draft();
  }

  /** Asks for the text with `nextCuts` cut and the answers `written` (null as drafted). */
  const redact = (nextCuts, written) => fetchText(`${DRAFTS}/${first.draft}/redact`, { sha256: now.sha256, edits: JSON.stringify(editsOf(nextCuts, written)) }, (answer) => {
    now = answer;
    cuts = nextCuts;
    answers = written;
  }, fail);

  /** Puts the page as it was before anything was chosen. */
  function reset() {
    generation += 1;
    key = null;
    Object.assign(fields, emptyFields());
    choices.setValues();
    forget(NOTE);
    security = false;
    withoutLogs = false;
    clearDraft();
    committed = null;
    drafting = false;
    fill(problem);
    paint();
  }

  // ---- Send, Save to a file and Download by QR code ----

  /** The words of a report that waits, with Retry now. */
  function queuedNote(reason) {
    const retry = h('button', { class: 'btn small', type: 'button' }, 'Retry now');
    retry.addEventListener('click', async () => {
      await (await sentPart).retry(retry);
      fill(status, h('p', { class: 'note' }, 'The reports that wait were tried again. The table below says where each stands.'));
    });
    return banner('warn', 'info', [h('p', {}, QUEUED_WORDS), reason && h('p', {}, `Why: ${reason}.`)], [retry]);
  }

  send.addEventListener('click', async () => {
    fill(problem);
    const choice = chosen();
    const result = await view.api.run('reports.send', sendArgs(), { button: send, words: SEND_WORDS, where: { slot: problem } });
    if (!result.ok) {
      if (draftEnded(result.error)) fail(result.error);
      return;
    }
    const answer = sendOf(result.result);
    if (choice.crash) {
      sentIds = new Set([...sentIds, choice.crash]);
      soft(view.api.get(CRASHES)).then((fresh) => {
        crashList = fresh ?? crashList;
        all = choicesNow();
        listed = shownChoices(all, { all: showAll, keep: null });
        paint();
      });
    }
    reset();
    fill(status, answer?.queued ? queuedNote(answer.why) : banner('ok', 'check', h('p', {}, sentWords(answer?.number))));
    (await sentPart).reload();
  });

  save.addEventListener('click', () => {
    const name = fileNameOf(view.time.zone);
    saveText(name, now.text);
    fill(status, h('p', { class: 'note' }, `Saved as ${name}. Nothing was sent.`));
  });

  qr.addEventListener('click', async () => {
    try {
      await (await import('./phone.js')).openPhone(view, { draft: first.draft, sha256: now.sha256, again: draftAgain });
    } catch (error) {
      view.problem(error, { slot: problem });
    }
    qr.focus();
  });

  view.onPrefs(() => paintButtons());

  view.main.append(
    h('p', { class: 'lead' }, LEAD),
    h('details', { class: 'rp-privacy' }, h('summary', {}, 'What you send when you report a problem'), h('p', {}, PRIVACY)),
    problem,
    h('div', { class: 'rp' },
      h('div', { class: 'rp-left' }, choices.element, holds.element,
        h('div', { class: 'rp-actions' }, h('div', { class: 'btns' }, send, save, qr), why, status, commandSlot)),
      preview.element),
    sentSlot);
  choices.setValues();
  paint();
  if (key && ready(chosen(), fields)) draft();
}
