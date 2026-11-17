// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What happens, on Add a website: the rows that say what adding the site does
// (the certificate, the folder, nginx, the firewall, the backups), from the
// plan that `website add --dry-run` makes and from what the page reads
// itself, the checks of the plan that stop the add, the commands, and the
// buttons. The plan is read as an administrator, a moment after the form stops
// changing; when the console is locked the card says so, and the rows are
// those the page can tell without it.

import {
  busy, fill, h, icon,
} from '../../dom.js';
import {
  openPortArgs, planChecks, planRows, portsFor, siteArgs, stepsOf, testArgs,
} from '../../lib/website-model.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';

/** How long the plan waits after the last change of the form, in milliseconds. */
const PLAN_MS = 600;

/** The words of the plan's read, for the unlock dialog. */
const UNLOCK_WORDS = 'plan the website';

/** The words of a port that is opened for a while, for the runner. */
const openWords = (port) => ({
  unlock: `open port ${port}`, running: `Opening port ${port}`, busy: 'Opening…', done: `Port ${port} is open for a while.`,
});

/**
 * Opens `port` for a while, a trial the person keeps or undoes. Gives
 * whether the firewall took the rule.
 */
export async function openFor(view, port, { button, slot }) {
  const ran = await view.api.run('firewall.add', openPortArgs(port), { button, where: { slot }, words: openWords(port) });
  return ran.ok;
}

/**
 * The card. `get()` gives what the page asks of it: `{form, ready, reason,
 * missing}`: the form's value (null while the names are not right), whether
 * the plan may be asked for, why the add must wait (words, or ''), and the
 * apps to install first (`{id, name, sha256}`). `world` is `{backups,
 * snapshots}`. `onAdd()` and `onPreview()` are the buttons'; `openPort(port,
 * button)` opens a port; `onRead()` is called when a plan has been read.
 */
export function makePlan(view, {
  get, world, onAdd, onPreview, openPort, onRead = () => {},
}) {
  const rows = h('ul', { class: 'chk-list web-rows' });
  const checks = h('div', { class: 'web-checks' });
  const unlock = h('div', { class: 'web-unlock', hidden: true });
  const errors = h('div', { class: 'web-errors' });
  const runs = h('div', { class: 'web-runs', hidden: true });
  const why = h('p', { class: 'small muted', role: 'status' });
  const add = h('button', { class: 'btn default', type: 'button', 'data-action': 'add' }, 'Add the website');
  const preview = h('button', { class: 'btn', type: 'button', 'data-action': 'preview' }, 'Preview the files first');
  const element = h('section', { class: 'card web-side', id: 'plan', 'aria-labelledby': 'plan-h' },
    h('h2', { id: 'plan-h' }, 'What happens'),
    errors, rows, checks, unlock, runs,
    h('div', { class: 'btns' }, add, preview, h('a', { class: 'btn', href: '/websites' }, 'Cancel')),
    why);

  let plan = null;
  let planned = '';
  let latest = 0;
  let timer = null;
  let tests = new Map();
  let asked = new Set();
  let drawn = '';

  /** Asks the firewall about each port that matters, once. */
  function test(ports) {
    for (const port of ports.filter((one) => !asked.has(one))) {
      asked.add(port);
      view.api.run('firewall.test', testArgs(port), { where: { slot: errors } }).then((ran) => {
        if (view.signal.aborted || !ran.ok || !ran.result) return;
        tests.set(port, { port, allowed: ran.result.allowed !== false, rule: ran.result.rule ?? null, text: ran.result.text ?? '' });
        draw();
      });
    }
  }

  /** The commands the steps run, one for each, while commands are shown. */
  function drawRuns({ form, ready, missing }) {
    runs.hidden = view.prefs?.commands !== 'shown';
    if (runs.hidden) return;
    // A panel asks the server for its command, so one is drawn again only when the steps change.
    const steps = form !== null && ready ? stepsOf(form, missing) : null;
    const key = JSON.stringify(steps?.map((one) => [one.action, one.args]) ?? null);
    if (key === drawn) return;
    drawn = key;
    fill(runs, h('h3', {}, 'This runs'), steps !== null
      ? steps.map((one) => view.api.panel(one.action, one.args))
      : h('p', { class: 'muted' }, 'The commands show when every field is right.'));
  }

  function draw() {
    const now = get();
    const { form, missing } = now;
    const { blocking, warnings } = planChecks(plan);
    if (form === null) {
      fill(rows, h('li', {}, icon('info'), h('span', { class: 'muted' }, 'Give the website\'s domain to see what happens.')));
      fill(checks);
    } else {
      const made = planRows(form, plan, {
        install: missing, firewall: portsFor(form.how).map((port) => tests.get(port)).filter(Boolean), backups: world.backups, snapshots: world.snapshots,
      });
      fill(rows, made.map((row) => {
        const mark = icon(row.icon);
        if (row.tone) mark.classList.add(row.tone);
        const press = row.port !== null && h('button', { class: 'btn small', type: 'button', 'data-action': `open-port-${row.port}` }, `Open port ${row.port}`);
        if (press) press.addEventListener('click', () => openPort(row.port, press));
        return h('li', { 'data-row': row.key }, mark,
          h('span', row.tone === 'ok' ? { class: 'muted' } : {}, row.words, row.link && [' ', h('a', { href: row.link.href }, row.link.words)]),
          press);
      }));
      fill(checks,
        blocking.map((one) => h('p', { class: 'msg', 'data-check': one.id ?? 'failed' }, icon('warn'), h('span', {}, one.words, one.id && h('span', { class: 'mono muted' }, ` ${one.id}`)))),
        warnings.map((one) => h('p', { class: 'small', 'data-check': one.id ?? 'warning' }, icon('warn'), ' ', one.words)));
    }
    const reason = blocking.length > 0 ? 'Put right what the plan says first.' : now.reason;
    add.setAttribute('aria-disabled', String(reason !== ''));
    why.textContent = reason;
    preview.setAttribute('aria-disabled', String(form === null));
    drawRuns(now);
  }

  /** The words and the button that stand for a plan that is not read: the console is locked. */
  function drawUnlock(locked) {
    unlock.hidden = !locked;
    if (!locked) {
      fill(unlock);
      return;
    }
    const press = h('button', { class: 'btn small', type: 'button', 'data-action': 'unlock' }, 'Unlock');
    press.addEventListener('click', async () => {
      if (await askToUnlock(view.session, UNLOCK_WORDS)) read(true);
    });
    fill(unlock, h('p', { class: 'small muted' }, 'The checks of the names, nginx and the folder, and the files, are read as an administrator. Unlock to see them.'), press);
  }

  /** Reads the plan of the form as it is, as an administrator; `ask` lets it ask to unlock. Gives the plan, or null. */
  async function read(ask = false) {
    const { form, ready } = get();
    if (form === null || !ready) return null;
    const args = siteArgs(form);
    const key = JSON.stringify(args);
    if (!ask && key === planned) return plan;
    if (!ask && !isUnlocked()) {
      drawUnlock(true);
      return null;
    }
    latest += 1;
    const mine = latest;
    fill(errors);
    const ran = await view.api.run('websites.plan', args, { words: { unlock: UNLOCK_WORDS }, where: { slot: errors } });
    if (mine !== latest || view.signal.aborted) return null;
    if (!ran.ok || ran.result === null || typeof ran.result !== 'object') {
      if (ran.cancelled) drawUnlock(true);
      return null;
    }
    drawUnlock(false);
    plan = ran.result;
    planned = key;
    draw();
    onRead();
    return plan;
  }

  /** The form changed: draws, asks the firewall, and reads the plan once the changes stop. */
  function refresh() {
    clearTimeout(timer);
    const { form, ready } = get();
    if (form !== null) test(portsFor(form.how));
    if (form === null || !ready) {
      latest += 1;
      plan = null;
      planned = '';
      drawUnlock(false);
    } else {
      timer = setTimeout(() => read(), PLAN_MS);
    }
    draw();
  }

  /** A rule changed: the firewall is asked about the ports again. */
  function retest() {
    tests = new Map();
    asked = new Set();
    refresh();
  }

  add.addEventListener('click', () => {
    if (add.getAttribute('aria-disabled') !== 'true') busy([add], () => onAdd(add));
  });
  preview.addEventListener('click', () => {
    if (preview.getAttribute('aria-disabled') !== 'true') busy([preview], () => onPreview(preview));
  });
  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
  view.on('unlock', () => {
    if (isUnlocked()) read();
  });

  return {
    element, errors, add, preview, refresh, retest, read, plan: () => plan,
  };
}
