// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Apps: /apps is the list of installed apps and /apps/ID is one app's page,
// beside the list on a wide screen and alone on a phone. The page reads the
// apps and the last check for updates when it opens, every 30 seconds while
// the tab is shown, when a change says that something moved and when a task
// ends; what is drawn changes only when what it says does, so that a button
// in hand stays in hand. An update, going back and removing are tasks: the
// rows and the page say how far each is, and the buttons that would start
// another stay off. An app's panel and the dialogs are read when they are
// wanted.

import { h, icon } from '../../dom.js';
import { PAGES } from '../../pages.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { atMostEvery } from '../overview/refresh.js';
import { askedMore, sheetOf } from './apps-model.js';
import { makeList } from './list.js';
import {
  ADD_LEAD, checkedWords, idOf, noSuchApp, rowsOf,
} from './list-model.js';
import {
  holdCall, makeJobs, runWords, UPDATE_ALL_WORDS, updateAllCall, updateCall,
} from './run-model.js';

/** How often the apps are read again while the tab is shown. */
const READ_MS = 30_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** Where an app is added: a page of its own, when this console has one. */
const ADD_PATH = '/apps/add';

/** Whether the console has a page at `path`. */
const hasPath = (path) => PAGES.some((entry) => entry.path === path && entry.load !== null);

/** `promise`'s answer, or `fallback` when it could not be had: what the page reads besides the apps is not needed to show them. */
const soft = (promise, fallback) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return fallback;
});

export async function show(view) {
  const wanted = idOf(view.rest);
  const isApp = wanted !== '';
  const admin = view.session.admin;
  const links = { updates: hasPath('/updates'), websites: hasPath('/websites'), databases: hasPath('/databases') };
  const addable = hasPath(ADD_PATH);

  let apps = null;
  let last;
  let info = null;
  let services = null;
  let settings = null;
  let managed = null;
  let row = null;
  let latest = 0;
  let failed = false;
  let headed = false;
  const stopped = new Map();
  const jobs = makeJobs();

  /** Draws the head: the list's, or an app's, under the crumb that leads back to the list. */
  function drawHead(name) {
    headed = true;
    const add = admin && addable && !isApp
      ? h('a', { class: 'btn default', href: ADD_PATH }, icon('plus'), 'Add an app')
      : null;
    view.head({
      title: name ?? 'Apps',
      crumbs: name ? [{ title: 'Apps', path: '/apps' }] : [],
      commands: true,
      buttons: [add],
      lead: !isApp && !addable ? ADD_LEAD : null,
    });
  }

  // ---- what the buttons do ----

  /** Asks an administrator whose console is locked to unlock, before a dialog that would only ask later. */
  const unlockFor = async (words) => !admin || isUnlocked() || askToUnlock(view.session, words);

  /** Keeps a task that began on this page, and says so in the rows. */
  function started(id, action, args) {
    jobs.started(id, action, args);
    paint();
  }

  /** Runs the call of `call` for the button, and keeps the task that it begins. */
  async function run(call, button, words) {
    const result = await view.api.run(call.action, call.args, { button, words });
    if (typeof result.task === 'number') started(result.task, call.action, call.args);
    return result;
  }

  /** Opens a dialog for the app on show, once the person has unlocked; `open` gives the dialog's function. Focus goes back to the button `action`, which a redraw while the dialog was open may have made new. */
  async function ask(words, open, action) {
    if (!(await unlockFor(words))) return;
    try {
      await (await open())(view, { info, started });
    } catch (error) {
      view.problem(error);
    }
    app?.focus(action);
  }

  const act = {
    async update(button, { yes = false }) {
      const result = await run(updateCall(info.id, { yes }), button, runWords('update', info.name, row.update.to));
      // The sheet of an update that stopped to ask has been answered.
      if (result.ok) {
        stopped.delete(info.id);
        draw();
      }
    },
    async hold(button, held) {
      const result = await run(holdCall(info.id, held), button, runWords(held ? 'unhold' : 'hold', info.name, info.version));
      if (result.ok) {
        await read();
        app?.focus('hold');
      }
    },
    back: () => ask(runWords('rollback', info.name).unlock, async () => (await import('./back.js')).openBack, 'back'),
    remove: () => ask(runWords('remove', info.name).unlock, async () => (await import('./remove.js')).openRemove, 'remove'),
  };

  const list = makeList(view, {
    updateAll: (button) => run(updateAllCall(), button, UPDATE_ALL_WORDS),
  });
  // The panel and its words are read only for an app's page.
  const app = isApp ? (await import('./app.js')).makeApp(view, act) : null;
  const model = isApp ? await import('./app-model.js') : null;
  const missingWords = h('p', {});
  const missing = h('div', { class: 'card apps-missing', role: 'status', hidden: true },
    missingWords, h('p', {}, h('a', { href: '/apps' }, 'All the apps')));
  const content = h('div', { class: `apps${isApp ? ' has-app' : ''}` }, list.element, app?.element, isApp && missing);
  view.main.append(content);

  // ---- the tasks ----

  /** Puts the words of the tasks that run in the list and the page. */
  function paint() {
    list.paint(jobs);
    app?.paint(jobs);
  }

  /** Reads the list of tasks, which says which run now. */
  async function loadJobs() {
    try {
      jobs.learn((await view.api.get('/api/1/tasks'))?.tasks);
      paint();
    } catch {
      // The Tasks list is where tasks are shown; without it the rows say what they always say.
    }
  }

  /** Reads one task whole: an event holds no arguments. */
  async function learn(id) {
    try {
      jobs.put(await view.api.get(`/api/1/tasks/${id}`));
      paint();
    } catch {
      // The next list has it.
    }
  }

  /** An update, going back or a removal ended: an update that stopped to ask is read for its sheet, a removal of the app on show leaves its page. */
  async function ended(job, state) {
    if (job.app !== null) stopped.delete(job.app);
    if (job.kind === 'update' && state === 'failed' && job.app !== null) {
      try {
        const task = await view.api.get(`/api/1/tasks/${job.id}`);
        if (askedMore(task)) stopped.set(job.app, sheetOf(task));
      } catch {
        // The task's log is in the Tasks list.
      }
    }
    if (job.kind === 'remove' && state === 'done' && job.app === wanted) {
      view.go('/apps');
      return;
    }
    soon();
  }

  // ---- reading ----

  /** Reads the apps, the last check and, for an app's page, the app and what it manages: only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    try {
      // An app that is asked for is read apart, so that its failing does not hide the list.
      const one = wanted
        ? view.api.get(`/api/1/apps/${encodeURIComponent(wanted)}`).then((answer) => ({ answer }), (error) => {
          if (error?.name === 'AbortError') throw error;
          return { error };
        })
        : null;
      const counted = one && model.manageRead(wanted, admin);
      const [bundles, checked, found, spark, rule, manage] = await Promise.all([
        view.api.get('/api/1/apps'),
        soft(view.api.get('/api/1/updates'), undefined),
        one,
        one && soft(view.api.get('/api/1/services'), null),
        one && soft(view.api.get('/api/1/updates/settings'), null),
        counted && soft(view.api.get(counted), null),
      ]);
      if (mine !== latest) return;
      const listed = (Array.isArray(bundles?.bundles) ? bundles.bundles : []).some((bundle) => bundle?.id === wanted);
      // An app that cannot be read is not installed when the list does not have it, and a problem when it does.
      if (found?.error && listed) throw found.error;
      apps = bundles;
      last = checked;
      info = found?.answer ?? null;
      services = spark;
      settings = rule;
      managed = manage;
      failed = false;
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      // What is drawn stays while the server does not answer; a first read that fails leaves nothing to show.
      if (apps === null) content.hidden = true;
      if (!headed) drawHead(null);
      if (failed) return;
      failed = true;
      view.problem(error, { buttons: [again] });
    }
  }

  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  again.addEventListener('click', () => {
    view.slot.replaceChildren();
    failed = false;
    content.hidden = false;
    read();
  });

  /** Draws what was read. */
  function draw() {
    view.slot.replaceChildren();
    content.hidden = false;
    const rows = rowsOf(apps, last);
    if (!headed) drawHead(info?.name ?? null);
    list.set({
      rows, selected: wanted || null, line: checkedWords(last, view.time), admin,
    });
    if (!isApp) return;
    app.element.hidden = info === null;
    missing.hidden = info !== null;
    if (info === null) {
      missingWords.textContent = noSuchApp(wanted ?? view.rest);
      return;
    }
    const own = rowsOf({ bundles: [info] }, last);
    row = [...rows.everyone, ...rows.yours].find((one) => one.id === info.id) ?? [...own.everyone, ...own.yours][0];
    app.set({
      info, row, show: services, settings, managed, admin, links, sheet: row.update.state === 'update' ? stopped.get(info.id) ?? null : null,
    });
  }

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('task', (data) => {
    const found = jobs.event(data);
    if (found.learn !== undefined) learn(found.learn);
    paint();
    if (found.ended !== undefined) ended(found.ended, found.state);
  });
  view.on('online', loadJobs);
  view.every(READ_MS, read);
  if (!isApp) drawHead(null);
  loadJobs();
  await read();
}
