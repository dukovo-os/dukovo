// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Certificates: the certificates this server keeps, getting one from Let's
// Encrypt, uploading one, renewing, revoking and removing, what certd and
// `cert` did, the machine's name and domain, and the certificate the console
// serves. The page reads what it shows when it opens, and again when a change
// of its own is made and every minute while it is seen. What a button asks
// for and the dialogs are read when one is pressed; the banners of a tried
// change are read when the page opens or a change begins one.

import { h } from '../../dom.js';
import { button } from './cards.js';
import { failuresOf } from './history-model.js';
import { makeList } from './list.js';
import { CERTD_WORDS, rowsOf } from './model.js';
import { makeName } from './name.js';
import { machineOf } from './name-model.js';
import { makeRenewals } from './renewals.js';

const READS = {
  list: '/api/1/certificates', history: '/api/1/certificates/history/all', signIn: '/api/1/security/sign-in', sites: '/api/1/websites',
};
const ABOUT = '/api/1/console/about';
const MACHINE = '/api/1/machine/name';
const TASKS = '/api/1/tasks';

/** How often the page reads again while it is seen. */
const READ_MS = 60_000;

export async function show(view) {
  // The form is the biggest part of the page, and is read while the first reads are asked.
  const gotGet = import('./get.js');
  const admin = view.session.admin === true;
  // `sites` is what the websites say they serve with; null until read, and left so when they could not be.
  const data = {
    list: null, history: null, serving: null, machine: null, about: null, sites: null,
  };
  let latest = 0;
  let stopped = false;
  let loaded = false;
  let banners = null;
  let get = null;
  let pending = null;

  const bannerHost = h('div', { class: 'certs-banners' });
  const ctx = {
    view,
    admin,
    data,
    byDns: new Set(),
    rows: () => rowsOf(data.list, view.time, { failures: failuresOf(data.history), serving: data.serving, byDns: ctx.byDns }),
    /** Scrolls to the card `id` and moves focus to its heading. */
    show(id) {
      const target = view.main.querySelector(`#${id}`);
      target?.scrollIntoView?.();
      target?.querySelector('h2')?.focus();
    },
    /** What a button of a row asks for; the module that knows is read at the first press. */
    async act(id, row, one) {
      const { act } = await import('./row-actions.js');
      return act(ctx, id, row, one);
    },
    async rename(one) {
      const { openRename } = await import('./rename.js');
      return openRename(ctx, one);
    },
    /** Gives the banners of the tried changes, read the first time they are asked for. */
    trials() {
      banners ??= import('./trials.js').then(async ({ makeTrials }) => {
        const made = makeTrials(ctx);
        bannerHost.append(...made.elements);
        await made.read();
        return made;
      }).catch(() => null);
      return banners;
    },
    /** Reads what changes with the certificates again; only the last read to ask is drawn. */
    reread: () => read(false),
    /** The machine was renamed: the page asks for nothing more, and shows the way to the new address. */
    renamed(info) {
      stopped = true;
      latest += 1;
      for (const card of [list, get, renewals, lead]) if (card) (card.element ?? card).hidden = true;
      name.renamed(info);
      ctx.show('name');
    },
  };

  const lead = h('p', { class: 'certs-lead' }, CERTD_WORDS);
  const list = makeList(ctx);
  const getSlot = h('div', { id: 'get-slot' });
  const renewals = makeRenewals(ctx);
  const name = makeName(ctx);
  ctx.list = list;

  const upload = button('Upload your own…', async () => {
    const { openUpload } = await import('./upload.js');
    await openUpload(ctx);
  });
  const getOne = button('Get one from Let\'s Encrypt', () => ctx.show('get'), 'default');
  view.head({ title: 'Certificates', commands: true, buttons: admin ? [upload, getOne] : [] });
  view.main.append(h('div', { class: 'certs' },
    bannerHost,
    lead,
    !admin && h('p', { class: 'muted' }, 'An administrator can get, upload, renew, revoke and remove certificates and change this machine\'s name.'),
    list.element, getSlot, renewals.element, name.element));

  function drawAll() {
    list.draw();
    get?.draw();
    renewals.draw();
    name.draw();
  }

  /** Reads what the page shows (all of it at the first read), and draws it. */
  async function read(first) {
    if (stopped) return;
    latest += 1;
    const mine = latest;
    const asked = { ...READS, ...(first ? { about: ABOUT, machine: MACHINE, tasks: TASKS } : {}) };
    const names = Object.keys(asked);
    const found = await Promise.allSettled(names.map((one) => view.api.get(asked[one])));
    if (mine !== latest || view.signal.aborted || stopped) return;
    const got = Object.fromEntries(names.map((one, at) => [one, found[at]]));
    const value = (one) => (got[one]?.status === 'fulfilled' ? got[one].value : undefined);
    data.list = Array.isArray(value('list')) ? value('list') : undefined;
    data.history = Array.isArray(value('history')) ? value('history') : undefined;
    data.serving = value('signIn')?.console?.certificate ?? data.serving;
    data.sites = value('sites') ?? data.sites;
    if (first) {
      data.about = value('about') ?? null;
      data.machine = value('machine') === undefined ? undefined : machineOf(value('machine'));
    }
    loaded = true;
    drawAll();
    if (first) {
      for (const one of ['list', 'history', 'machine']) if (got[one]?.status === 'rejected') view.problem(got[one].reason, { slot: one === 'machine' ? name.errors : list.errors });
      pending = (value('tasks')?.tasks ?? []).find((task) => task.action === 'certificate.get' && (task.state === 'running' || task.state === 'waiting')) ?? null;
    }
  }

  drawAll();
  view.on('online', () => read(false));
  view.every(READ_MS, () => read(false));
  await read(true);
  try {
    const { makeGet } = await gotGet;
    get = makeGet(ctx);
    ctx.get = get;
    getSlot.append(get.element);
    get.draw();
    if (admin && pending) get.adopt(pending);
  } catch (error) {
    view.problem(error, { slot: list.errors });
  }
  // A trial that runs is shown once the page is.
  if (loaded) ctx.trials();

  const wanted = decodeURIComponent(view.rest ?? '');
  const row = wanted === '' ? null : ctx.rows().find((one) => one.name === wanted);
  if (row) ctx.act('show', row);
  else if (wanted !== '' && data.list) list.say(`There is no certificate called ${wanted}.`);
  const target = globalThis.location?.hash?.slice(1);
  if (target === 'upload') {
    if (admin) upload.click();
  } else if (target) {
    ctx.show(target);
  }
}
