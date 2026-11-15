// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Snapshots and backups (design 5): /snapshots is the banner, What is
// protected, the snapshots by day and the backup places; /snapshots/backups
// is the same scrolled to the backup places, and /snapshots/backups/NAME
// has one place open. /snapshots/browse is a snapshot's folder, read as its
// own graph. The page reads the snapshots and the backup places when it
// opens, every minute while the tab is shown, when a change says that
// something moved and when a backup, check or restore ends; what is drawn
// changes only when what it says does, so that a button in hand stays in
// hand. What a place holds and its key are read only on its own page.

import { fill, h, icon } from '../../dom.js';
import { mergeSnapshots } from '../../lib/snapshot-model.js';
import { browseHref } from '../../lib/snapshot-words.js';
import { PAGES } from '../../pages.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeJobsUi } from './jobs.js';
import { makeList } from './list.js';
import {
  backupPlaces, bannersOf, lateOf, placeHref, removableVolumes, routeOf, scheduledPlaces, snapPlaces, snapshotsCurrent, stateOf,
} from './model.js';
import { makePlaces } from './places.js';
import { makeProtected } from './protected.js';

/** How often the snapshots and the backup places are read again while the tab is shown. */
const READ_MS = 60_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** The places that snap makes snapshots of by default, for a Take a snapshot now on a machine that did not say. */
const DEFAULT_PLACES = Object.freeze(['/users', '/shared', '/system/settings']);

/** Whether the page with the id `id` is in this console. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

export async function show(view) {
  const route = routeOf(view.rest);
  if (route.browse) {
    try {
      await (await import('./browse.js')).show(view);
    } catch (error) {
      view.problem(error);
    }
    return;
  }

  const links = { move: hasPage('move'), updates: hasPage('updates'), journal: hasPage('reliability') };
  let list = null;
  let perPlace = [];
  let backups = null;
  let storage = null;
  let listFailed = null;
  let backupsFailed = null;
  let latestList = 0;
  let latestBackups = 0;
  let wholeFailed = false;
  let lastBanners = null;
  let scrolled = false;

  const admin = view.session.admin;
  const take = h('button', { class: 'btn', type: 'button' }, 'Take a snapshot now');
  const add = h('button', { class: 'btn', type: 'button' }, icon('plus'), 'Add a backup place…');
  view.head({ title: 'Snapshots and backups', commands: true, buttons: admin ? [take, add] : [] });

  const jobs = makeJobsUi(view, { ended, repaint: () => {} });
  const protectedCard = makeProtected(view, { jobs, journal: links.journal });
  const listCard = makeList({ links });
  const places = makePlaces(view, {
    jobs, links, add: (volume) => addPlace(volume), restore: (...call) => restoreFiles(...call), changed: () => soon(),
  });
  const banners = h('div', { class: 'snap-banners' });
  const content = h('div', { class: 'snap' });
  view.main.append(content);
  fill(content, banners, protectedCard.element, h('div', { class: 'snap-grid' }, listCard.element, places.element));

  // ---- what the buttons do ----

  /** Asks an administrator whose console is locked to unlock, before a dialog that would only ask later. */
  const unlockFor = async (words) => !view.session.admin || isUnlocked() || askToUnlock(view.session, words);

  /** The disks a backup could go to, from the last read of the disks. */
  const volumes = () => removableVolumes(storage, backups);

  /** Opens Add a backup place, with `volume` chosen when a banner offered a disk. The code is read when it is first wanted. */
  async function addPlace(volume = null) {
    if (!(await unlockFor('add a backup place'))) return;
    try {
      const fresh = await view.api.get('/api/1/storage').catch(() => storage);
      const { openAdd } = await import('./add-place.js');
      const name = await openAdd(view, {
        storage: fresh, backups, volume, started: (id, action, args) => jobs.started(id, action, args),
      });
      if (name !== null && !view.signal.aborted) {
        await read();
        view.go(placeHref(name));
      }
    } catch (error) {
      view.problem(error);
    }
  }

  /** Opens Take a snapshot now. */
  async function takeSnapshot() {
    if (!(await unlockFor('take a snapshot'))) return;
    try {
      const { openTake } = await import('./take.js');
      const paths = list === null ? DEFAULT_PLACES : [...new Set([...scheduledPlaces(list), ...snapPlaces(list).map((one) => one.place)])];
      await openTake(view, { paths, changed: () => soon() });
    } catch (error) {
      view.problem(error);
    }
  }

  /** Opens Restore files…: `place` is the backup place, `show` what it holds, `preset` a backup to start from. */
  async function restoreFiles(place, show, preset) {
    try {
      const { openRestore } = await import('./restore.js');
      await openRestore(view, { place, show, preset, jobs, time: view.time, now: Date.now() });
    } catch (error) {
      view.problem(error);
    }
  }

  take.addEventListener('click', takeSnapshot);
  add.addEventListener('click', () => addPlace());

  /** A backup, check or restore ended: the places are read again, and a restore that this page began is shown. */
  function ended(job, state) {
    soon();
    places.ended(job.name);
    if (job.kind === 'restore' && job.mine && state === 'done') showRestored(job.args);
  }

  /** Opens the browse of the snapshot that a restore made, once it is in the list. */
  async function showRestored(args) {
    const path = args?.place;
    const snapshot = `restored-${args?.at}`;
    if (typeof path !== 'string' || !args?.at) return;
    const answer = await view.api.get(`/api/1/snapshots?${new URLSearchParams({ place: path })}`).catch(() => null);
    if (view.signal.aborted) return;
    if ((Array.isArray(answer?.snapshots) ? answer.snapshots : []).some((one) => one.name === snapshot)) {
      view.go(browseHref(path, snapshot, path));
    } else {
      view.notice(`The restore from ${args.name} is done. Its snapshot is in the list.`, { kind: 'ok' });
      soon();
    }
  }

  // ---- the banners ----

  /** The banners, worst first, when what they say has changed. */
  function drawBanners(context) {
    const found = bannersOf(backups, { ...context, current: list === null ? null : snapshotsCurrent(list, context.now), disks: admin && volumes().length > 0 });
    const key = JSON.stringify([found, admin]);
    if (key === lastBanners) return;
    lastBanners = key;
    fill(banners, ...found.map((one) => {
      // A reader cannot back up or check, and is told who can.
      const buttons = admin ? one.buttons.map((button) => {
        if (button.id === 'run' || button.id === 'verify') return jobs.button(button.id, button.name, button.label, { size: button.id === 'run' ? 'small default' : 'small' });
        if (button.id === 'move') return h('a', { class: 'btn small', href: button.href }, button.label);
        const choose = h('button', { class: 'btn small', type: 'button' }, button.label);
        choose.addEventListener('click', () => addPlace(volumes()[0] ?? null));
        return choose;
      }) : [];
      return h('div', { class: `banner ${one.tone}`, role: one.tone === 'bad' ? 'alert' : 'status' },
        icon('warn'),
        h('div', {},
          h('p', {}, h('b', {}, one.lead), ' ', one.detail),
          buttons.length > 0 && h('div', { class: 'btns' }, buttons),
          !admin && one.buttons.some((button) => button.id === 'run') && h('p', { class: 'small muted' }, 'An administrator can back up now.')));
    }));
    jobs.paint();
  }

  // ---- reading ----

  /** Reads the snapshots: the places, and the snapshots of each. Only the last read counts. */
  async function readSnapshots() {
    latestList += 1;
    const mine = latestList;
    try {
      const overview = await view.api.get('/api/1/snapshots');
      const reads = await Promise.all(snapPlaces(overview).filter((one) => one.count !== 0).map((one) => view.api.get(`/api/1/snapshots?${new URLSearchParams({ place: one.place })}`).then(
        (answer) => ({ place: one.place, snapshots: answer?.snapshots ?? [] }),
        (error) => {
          if (error?.name === 'AbortError') throw error;
          // One place that fails is left out; the others are still the list.
          return { place: one.place, snapshots: [] };
        },
      )));
      if (mine !== latestList) return;
      list = overview;
      perPlace = reads;
      listFailed = null;
    } catch (error) {
      if (mine === latestList && error?.name !== 'AbortError') listFailed = error;
    }
  }

  /** Reads the backup places, and the disks when a banner may offer one. Only the last read counts. */
  async function readBackups() {
    latestBackups += 1;
    const mine = latestBackups;
    try {
      const answer = await view.api.get('/api/1/backups');
      if (mine !== latestBackups) return;
      backups = answer;
      backupsFailed = null;
      jobs.lock(backupPlaces(answer).filter((one) => one.running).map((one) => one.name));
      const behind = backupPlaces(answer).some((one) => stateOf(one).failed || lateOf(one));
      if (admin && behind) storage = await view.api.get('/api/1/storage').catch(() => storage);
    } catch (error) {
      if (mine === latestBackups && error?.name !== 'AbortError') backupsFailed = error;
    }
  }

  /** Draws what was read. */
  function draw() {
    const context = { time: view.time, now: Date.now() };
    drawBanners(context);
    const retry = () => read();
    if (list !== null && backups !== null) protectedCard.set(list, backups, context);
    else protectedCard.fail('What is protected needs both the snapshots and the backup places, and one could not be read.', retry);
    if (list !== null) {
      const only = backupPlaces(backups).length === 1 ? backupPlaces(backups)[0].name : null;
      listCard.set({
        rows: mergeSnapshots(perPlace, list.update), settings: list.settings ?? null, from: only, time: context.time, now: context.now,
      });
    } else {
      listCard.fail('The snapshots could not be read.', retry);
    }
    if (backups !== null) places.set({ ...context, backups, open: route.open });
    else places.fail('The backup places could not be read.', retry);
    jobs.paint();
  }

  /** Reads both, and draws. When neither could be read there is nothing to show but the error. */
  async function read() {
    await Promise.all([readSnapshots(), readBackups()]);
    if (view.signal.aborted) return;
    if (list === null && backups === null) {
      if (!wholeFailed && (listFailed || backupsFailed)) {
        wholeFailed = true;
        content.hidden = true;
        view.problem(listFailed ?? backupsFailed, { buttons: [h('button', { class: 'btn small', type: 'button', onclick: again }, 'Try again')] });
      }
      return;
    }
    if (wholeFailed) {
      wholeFailed = false;
      view.slot.replaceChildren();
      content.hidden = false;
    }
    draw();
    if (!scrolled) {
      scrolled = true;
      if (route.backups) {
        const target = route.open === null ? places.element.querySelector('h2') : places.element.querySelector('#snap-open-h') ?? places.element.querySelector('h2');
        target?.focus();
        places.element.scrollIntoView?.({ block: 'start' });
      }
    }
  }

  function again() {
    view.slot.replaceChildren();
    wholeFailed = false;
    content.hidden = false;
    read();
  }

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('unlock', () => {
    places.unlocked();
    soon();
  });
  view.every(READ_MS, read);
  jobs.load();
  await read();
}
