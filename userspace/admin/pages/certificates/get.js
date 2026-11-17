// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The form that gets a certificate from Let's Encrypt: the names and how they
// are proved, the checks that are made before an order and what to do about
// each, the order itself as a task with its steps, and, for names proved by
// DNS, the TXT records to add while it waits. The words and the arguments are
// get-model.js's.

import { clearError, errorSlot, fill, h, icon, showError } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { stepWords } from '../../lib/tasks-words.js';
import { button, makeCard, setOff } from './cards.js';
import {
  certName, checkPath, checkRows, DNS_RENEWAL, EMAIL_HINT, endedWords, getArgs, isReport, namesOf, namesProblem, openPortArgs,
  PORT_TRY_SECONDS, plan, PROOFS, recordLine, recordsOf, TRY_DNS_WORDS, TRY_WORDS, termsLink, waitsForRecords, whereUsedLine,
} from './get-model.js';
import { suggestedDomain } from './model.js';

const ACTION = 'certificate.get';

/** The least time between two reads of a task's log, in milliseconds. */
const LOG_EVERY_MS = 2000;

const running = (task) => task !== null && (task.state === 'running' || task.state === 'waiting');

/** The mark of a check: its word, which says what the colour does. */
const MARKS = Object.freeze({
  ok: ['ok', 'Passed'], warn: ['warn', 'Warning'], bad: ['bad', 'Failed'], skip: ['off', 'Left out'],
});

/** The line of where a certificate is used, as nodes: the words, the websites that use it as links, and for an administrator the way to add one when none does. */
function usedWords(line, admin) {
  const links = line.sites.map((one) => h('a', { href: one.href }, one.first));
  const joined = links.flatMap((link, at) => [at > 0 && (at === links.length - 1 ? ' and ' : ', '), link]);
  return [
    line.words,
    links.length > 0 && [` ${links.length === 1 ? 'The website' : 'The websites'} that ${links.length === 1 ? 'uses' : 'use'} it: `, joined, '.'],
    admin && line.add && [' ', h('a', { href: line.add }, 'Add a website')],
  ];
}

/** The card. `ctx` is the page's (see page.js). Gives `{element, errors, draw, fill, adopt, running}`. */
export function makeGet(ctx) {
  const { view, admin } = ctx;
  const card = makeCard('get', 'Get a certificate from Let\'s Encrypt');
  let checks = null;
  let report = null;
  let latest = 0;
  let task = null;
  let ended = null;
  let order = null;
  let records = [];
  let logAt = 0;
  let shown = '';

  const namesInput = h('input', {
    id: 'get-names', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': 'get-names-hint get-names-error',
  });
  const namesError = errorSlot('get-names-error');
  const suggestion = h('span', { class: 'hint', hidden: true });
  const proofs = PROOFS.map((one) => {
    const input = h('input', { type: 'radio', name: 'get-proof', value: one.id, checked: one.id === 'http' });
    return { id: one.id, input, label: h('label', { class: 'radio-card' }, input, h('span', {}, h('b', {}, one.label), h('small', { class: 'hint' }, ` ${one.hint}`))) };
  });
  const email = h('input', {
    id: 'get-email', class: 'input', type: 'email', autocomplete: 'email', spellcheck: 'false', 'aria-describedby': 'get-email-hint',
  });
  const checkList = h('ul', { class: 'certs-checks', 'aria-label': 'The checks' });
  const checkNote = h('p', { class: 'small muted', role: 'status' });
  const used = h('p', { class: 'small' });
  const proof = h('p', { class: 'small muted' });
  const terms = h('p', { class: 'small muted' });
  const why = h('p', { id: 'get-why', class: 'hint' });
  const command = h('div', { class: 'certs-command' });
  const progress = h('div', { class: 'certs-progress', role: 'status', hidden: true });
  const note = h('p', { class: 'small muted', role: 'status' });
  const getButton = button('Get the certificate', () => startOrder(false), 'default');
  const tryButton = button('Try it first', () => startOrder(true));
  const againButton = button('Check again', () => check());

  const dns = () => proofs.find((one) => one.id === 'dns').input.checked === true;
  const names = () => namesOf(namesInput.value);

  const form = h('form', { class: 'certs-form', novalidate: true },
    h('div', { class: 'field' },
      h('label', { for: namesInput.id }, 'Domain'), namesInput,
      h('span', { id: 'get-names-hint', class: 'hint' }, 'The names of the site, separated by spaces: shop.example.org www.shop.example.org. The first is the certificate\'s name.'),
      suggestion, namesError),
    h('fieldset', { class: 'choices' }, h('legend', {}, 'Prove the name'), h('div', { class: 'cards-row' }, proofs.map((one) => one.label))),
    admin && h('div', { class: 'field' }, h('label', { for: email.id }, 'Contact email (optional)'), email, h('span', { id: 'get-email-hint', class: 'hint' }, EMAIL_HINT)),
    h('div', { class: 'certs-checks-box' }, h('h3', {}, 'The checks'), checkNote, checkList),
    used,
    command,
    terms,
    h('p', { class: 'small muted' }, TRY_WORDS),
    proof,
    h('div', { class: 'btns' }, admin && getButton, admin && tryButton, againButton),
    why);
  fill(card.body, form, progress, note);

  form.addEventListener('submit', (event) => event.preventDefault());
  view.signal.addEventListener('abort', () => { latest += 1; }, { once: true });

  /** Draws the progress of the order that runs or ended, and the records it asks for. */
  function drawProgress() {
    progress.hidden = task === null && ended === null;
    if (progress.hidden) return;
    const copy = (record) => {
      const text = h('span', { class: 'mono' }, recordLine(record));
      const press = button('Copy', async () => tell(await copyText(recordLine(record), text) ? COPIED : PRESS_CTRL_C), 'small');
      press.setAttribute('aria-label', `Copy the record for ${record.name}`);
      return h('li', {}, text, ' ', press);
    };
    fill(progress,
      task !== null && h('p', {}, h('b', {}, order?.dryRun ? 'Trying the order: ' : 'Getting the certificate: '), stepWords(task)),
      task !== null && records.length > 0 && [
        h('p', {}, 'Add these TXT records at your DNS provider. This server looks for them until they show.'),
        h('ul', { class: 'certs-records' }, records.map(copy)),
        h('p', { class: 'small muted' }, DNS_RENEWAL),
      ],
      ended !== null && h('p', { class: `msg certs-ended${ended.ok ? ' certs-tone-ok' : ''}` }, icon(ended.ok ? 'check' : 'warn'), h('span', {}, ended.words)));
  }

  function tell(words) {
    note.textContent = words;
  }

  /** Draws what depends on the checks and the task: the rows, the buttons and the command. */
  function draw() {
    const typed = names();
    const rows = checks;
    const p = plan(rows, typed);
    const busy = running(task);
    fill(checkList, (rows ?? []).map((row) => {
      const [tone, word] = MARKS[row.tone] ?? MARKS.warn;
      const open = row.button === 'open-port' && admin
        && [' ', button('Open port 80', (one) => openPort(one), 'small')];
      return h('li', { class: 'certs-check', 'data-id': row.id }, h('span', { class: `st ${tone}` }, word), ' ', h('span', {}, row.words, row.id && h('span', { class: 'mono muted' }, ` ${row.id}`)), open);
    }));
    checkNote.textContent = rows === null && typed.length > 0 && namesProblem(typed, dns()) === '' ? 'Checking…' : '';
    getButton.textContent = p.label;
    setOff(getButton, !p.can || busy);
    setOff(tryButton, !p.can || busy);
    setOff(againButton, typed.length === 0);
    const reason = !p.can && rows !== null ? p.why : '';
    why.textContent = reason;
    for (const one of [getButton, tryButton]) {
      if (reason) one.setAttribute('aria-describedby', why.id);
      else one.removeAttribute('aria-describedby');
    }
    fill(terms, 'Getting it agrees to ', termsLink(report) ? h('a', { href: termsLink(report), rel: 'noopener', target: '_blank' }, 'Let\'s Encrypt\'s terms of service') : 'Let\'s Encrypt\'s terms of service', '.');
    fill(used, typed.length > 0 && rows !== null && usedWords(whereUsedLine(certName(typed) ?? typed[0], ctx.data.list, typed, ctx.data.sites), admin));
    drawCommand(p);
    fill(proof, dns() && `${DNS_RENEWAL} ${TRY_DNS_WORDS}`);
    drawProgress();
  }

  /** The command of the order under the checks: the server shows it once the names are right. */
  function drawCommand(p) {
    const args = p.can ? getArgs({ names: p.names, dns: dns(), email: email.value }) : null;
    const key = JSON.stringify(args);
    if (key === shown) return;
    shown = key;
    fill(command, args && [h('h3', {}, 'This runs'), view.api.panel(ACTION, args)]);
  }

  /** Asks the checks again for the names that are typed. */
  async function check() {
    const typed = names();
    latest += 1;
    const mine = latest;
    clearError(namesError, namesInput);
    checks = null;
    report = null;
    const problem = namesProblem(typed, dns());
    if (typed.length === 0 || problem !== '') {
      if (typed.length > 0) showError(namesError, { title: problem, detail: '', fix: '' }, namesInput);
      draw();
      return;
    }
    draw();
    try {
      const answer = await view.api.get(checkPath(typed, dns()));
      if (mine !== latest || view.signal.aborted) return;
      if (!isReport(answer)) throw new Error('The checks gave no answer.');
      report = answer;
      checks = checkRows(answer, dns());
    } catch (error) {
      if (mine !== latest || view.signal.aborted) return;
      view.problem(error, { slot: card.errors });
    }
    draw();
  }

  /** Opens port 80 for a while, and checks again. */
  async function openPort(one) {
    const result = await view.api.run('firewall.add', openPortArgs(), {
      button: one,
      where: { slot: card.errors },
      words: {
        unlock: 'open port 80', running: 'Opening port 80', busy: 'Opening…', done: 'Port 80 is open for a while.',
      },
    });
    if (!result.ok) return;
    const trials = await ctx.trials();
    await trials?.firewall.begin({ seconds: PORT_TRY_SECONDS });
    await check();
  }

  /** Starts the order, or the test of it at the staging authority. */
  async function startOrder(dryRun) {
    const p = plan(checks, names());
    if (!p.can || running(task)) return;
    clearError(namesError, namesInput);
    fill(card.errors);
    const args = getArgs({
      names: p.names, dns: dns(), dryRun, email: email.value,
    });
    const result = await view.api.run(ACTION, args, {
      button: dryRun ? tryButton : getButton,
      where: { slot: card.errors, fields: { names: namesInput, email } },
      words: {
        unlock: dryRun ? 'try an order at the staging authority' : 'get a certificate', running: dryRun ? 'Trying the order' : `Getting ${args.name}`, busy: 'Starting…', done: 'The order is done.',
      },
    });
    if (typeof result.task === 'number') adopt({ id: result.task, action: ACTION, state: 'running' }, { name: args.name, dryRun, dns: dns() });
  }

  /** Follows the task `next`, which `made` says what it is for (the page's own, or one started elsewhere). */
  function adopt(next, made = {}) {
    task = { step: 0, steps: 0, ...next };
    order = { name: made.name ?? next.args?.name ?? '', dryRun: made.dryRun === true || next.args?.dry_run === true, dns: made.dns === true || next.args?.dns === true };
    ended = null;
    records = [];
    tell('');
    draw();
  }

  /** Reads the log of the task for the records, no more than once in a while. */
  async function readLog(final = false) {
    if (task === null || (!final && Date.now() - logAt < LOG_EVERY_MS)) return null;
    logAt = Date.now();
    try {
      const full = await view.api.get(`/api/1/tasks/${task.id}`);
      if (task !== null && full.id === task.id) records = recordsOf(full.log);
      return full;
    } catch {
      return null;
    }
  }

  view.on('task', async (event) => {
    if (event.action !== ACTION && event.id !== task?.id) return;
    if (task === null || event.id !== task.id) {
      if (!admin || !running(event)) return;
      adopt(event);
    }
    task = { ...task, ...event };
    if (running(task)) {
      if (waitsForRecords(task)) await readLog();
      draw();
      return;
    }
    const full = await readLog(true);
    const finished = { ...task, ...(full ?? {}) };
    ended = endedWords(finished, { dryRun: order?.dryRun === true, name: order?.name ?? '' });
    task = null;
    records = [];
    draw();
    if (!view.signal.aborted) await ctx.reread();
  });

  /** Gives the form these names and this proof, and checks them. */
  function fillForm({ names: given, dns: byDns = false }) {
    namesInput.value = given.join(' ');
    for (const one of proofs) one.input.checked = (one.id === 'dns') === byDns;
    check();
  }

  namesInput.addEventListener('input', () => clearError(namesError, namesInput));
  namesInput.addEventListener('change', () => check());
  for (const one of proofs) one.input.addEventListener('change', () => check());
  email.addEventListener('change', () => draw());

  /** Puts the suggestion for the machine's domain in the field, once the machine is known. */
  function suggest() {
    const word = suggestedDomain(ctx.data.machine);
    namesInput.setAttribute('placeholder', word ?? 'shop.example.org');
    suggestion.hidden = word === null;
    if (word !== null) {
      fill(suggestion, 'This machine\'s name with its domain: ', h('button', { class: 'btn small', type: 'button' }, `Use ${word}`));
      suggestion.querySelector('button').addEventListener('click', () => {
        namesInput.value = word;
        check();
      });
    }
  }

  return {
    element: card.element,
    errors: card.errors,
    draw: () => { suggest(); draw(); },
    fill: fillForm,
    adopt,
    check,
    running: () => running(task),
  };
}
