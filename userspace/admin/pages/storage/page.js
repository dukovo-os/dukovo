// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Storage (design 4): /storage is the cards of use, the disks drawn to scale
// and the log's own space, /storage/DISK the same with that disk's details
// open beside the list (in a view of its own on a phone). The page reads
// `storage show` when it opens, every 30 seconds while the tab is shown,
// when a change or a task says that something moved and after each of its
// own actions; what is drawn changes only when what it says does, so that
// a button in hand stays in hand. The use over 30 days, for the projection,
// and the reads and writes of the last hour are read beside it.

import { fill, h, icon } from '../../dom.js';
import { PAGES } from '../../pages.js';
import { atMostEvery, usageDue } from '../overview/refresh.js';
import { makeDisks } from './disks.js';
import {
  bannersOf, cardsOf, diskHref, diskNamed, findDisk, rowsOf,
} from './model.js';
import { makeScrubber } from './scrub.js';
import { makeUse } from './use.js';

/** How often `storage show` is read again while the tab is shown. */
const READ_MS = 30_000;

/** How often the reads and writes are read again, and how often it is asked whether the 30 days are due. */
const USAGE_MS = 60_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** Whether the page with the id `id` is in this console. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

/** The disk that a banner's Check button was pressed for, which its page checks when it opens. */
let checkOnOpen = null;

/** The steps to use or replace a disk, read when first asked for. */
const steps = async (...call) => (await import('./replace.js')).openSteps(...call);

/** The grey bars that stand where a card's words are read. */
const bars = () => [0, 1].map(() => h('span', { class: 'sto-bar wide' }));

/** The answer of `storage.usage`, or an empty one for a machine that has kept no history (ADM-123). */
const keptOrNone = (error) => {
  if (error?.id === 'ADM-123') return [];
  throw error;
};

export async function show(view) {
  const name = diskNamed(view.rest);
  const links = { hardware: hasPage('hardware'), backups: hasPage('snapshots') };
  let answer = null;
  let usage30 = null;
  let latest = 0;
  let reading = null;
  let failed = false;
  let lastThirty = 0;
  let lastBanner = null;
  let panel = null;
  let diskCode = null;
  let loadingDisk = null;
  let missing = false;

  const useNew = h('button', { class: 'btn', type: 'button' }, icon('plus'), 'Use a new disk…');
  useNew.addEventListener('click', () => steps('new', { answer, backups: links.backups }));
  view.head(name === null
    ? { title: 'Storage', commands: true, buttons: [useNew] }
    : { title: name, crumbs: [{ title: 'Storage', path: '/storage' }], commands: true, buttons: [useNew] });

  const scrubber = makeScrubber(view, { changed: () => read(), repaint: () => answer && draw() });
  const use = makeUse(view);
  const disks = makeDisks({ backups: links.backups });
  const logCard = h('section', { class: 'card', id: 'log', 'aria-labelledby': 'sto-log-h' },
    h('h2', { id: 'sto-log-h' }, 'The log\'s own space'), h('div', { class: 'sto-log-body', 'aria-hidden': 'true' }, bars()));
  const banner = h('div', { class: 'sto-banner' });
  const side = h('div', { class: 'sto-side' }, name !== null && h('section', { class: 'card', 'aria-hidden': 'true' }, bars()));
  const list = h('div', { class: 'sto-list' }, disks.element);
  const content = h('div', { class: `sto${name === null ? '' : ' sto-opened'}` });
  view.main.append(content);
  use.frame();
  disks.frame();
  fill(content, banner, h('div', { class: 'sto-top' }, use.element), name === null ? list : h('div', { class: 'sto-split' }, list, side), logCard);

  /** The banner, the worst thing first, when what it says has changed. */
  function drawBanner() {
    const found = bannersOf(answer, { scrubbing: scrubber.mounts(), time: view.time })[0] ?? null;
    const key = JSON.stringify(found);
    if (key === lastBanner) return;
    lastBanner = key;
    if (found === null) {
      banner.replaceChildren();
      return;
    }
    // A reader cannot check a disk or scrub a volume, and so is not offered them.
    const buttons = found.buttons.filter((one) => view.session.admin || one.id === 'replace' || one.id === 'open').map((one) => {
      if (one.id === 'scrub') return scrubber.button(one.mount, one.label, { kind: 'small' });
      if (one.id === 'open') return h('a', { class: 'btn small', href: diskHref(one.disk) }, one.label);
      const button = h('button', { class: 'btn small', type: 'button' }, one.label);
      button.addEventListener('click', () => {
        if (one.id === 'replace') steps('replace', { answer, disk: name });
        else if (name === one.disk) panel?.check();
        else {
          checkOnOpen = one.disk;
          view.go(diskHref(one.disk));
        }
      });
      return button;
    });
    fill(banner, h('div', { class: `banner ${found.tone}`, role: 'status' },
      icon(found.tone === 'info' ? 'info' : 'warn'),
      h('div', {}, h('p', {}, h('b', {}, found.lead), ' ', found.detail), buttons.length > 0 && h('div', { class: 'btns' }, buttons))));
    scrubber.paint();
  }

  /** The panel of the disk `name`, made when the page first has it, or the words that say there is none. */
  function drawPanel() {
    if (name === null) return;
    if (findDisk(answer, name) === null) {
      panel = null;
      missing = true;
      fill(side, h('section', { class: 'card' },
        h('h2', {}, `There is no disk called ${name}`),
        h('p', {}, 'The machine has no disk of that name now. A disk that was unplugged is gone from the list.'),
        h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/storage' }, 'Back to the disks'))));
      return;
    }
    if (diskCode === null) {
      loadDisk();
      return;
    }
    if (panel === null || missing) {
      missing = false;
      panel = diskCode.makeDisk(view, { name, scrubber, steps, reread: () => read(), links });
      fill(side, h('a', { class: 'btn sto-back', href: '/storage' }, 'Back to the disks'), panel.element);
      if (checkOnOpen === name && view.session.admin) {
        checkOnOpen = null;
        panel.check();
      }
    }
    panel.set(answer);
  }

  /** Reads the code of a disk's panel the first time it is wanted, and draws the panel when it is here. */
  function loadDisk() {
    loadingDisk ??= import('./disk.js').then((code) => {
      diskCode = code;
      if (answer !== null && !view.signal.aborted) drawPanel();
    }, (error) => {
      loadingDisk = null;
      view.problem(error);
    });
  }

  /** Reads the log's code, and then the log, which takes its place in the card. */
  function loadLog() {
    import('./log.js').then(({ makeLog }) => {
      if (!view.signal.aborted) makeLog(view, logCard).load();
    }, (error) => view.problem(error));
  }

  /** Draws `answer`. */
  function draw() {
    drawBanner();
    use.set(cardsOf(answer, usage30, { scrubbing: scrubber.mounts() }));
    disks.set(rowsOf(answer), name);
    drawPanel();
  }

  /** Reads the disks. Only the last read counts, whatever order they answer in. */
  function read() {
    latest += 1;
    const mine = latest;
    reading = readDisks(mine);
    return reading;
  }

  async function readDisks(mine) {
    try {
      const found = await view.api.get('/api/1/storage');
      if (mine !== latest) return reading;
      answer = found;
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
        content.hidden = false;
      }
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return reading;
      // What is drawn stays while the server does not answer; a first read that fails leaves nothing to show.
      if (answer === null) content.hidden = true;
      if (!failed) {
        failed = true;
        view.problem(error, { buttons: [h('button', { class: 'btn small', type: 'button', onclick: again }, 'Try again')] });
      }
    }
  }

  function again() {
    view.slot.replaceChildren();
    failed = false;
    content.hidden = false;
    read();
  }

  /** Reads the last hour of reads and writes and, when it is due, the 30 days of use that the projections are made from. */
  async function readUsage() {
    try {
      use.setUsage(await view.api.get('/api/1/storage/usage/1h').catch(keptOrNone));
    } catch (error) {
      if (error?.name !== 'AbortError') use.fail(error);
    }
    if (!usageDue(lastThirty, Date.now(), '24h')) return;
    lastThirty = Date.now();
    try {
      usage30 = await view.api.get('/api/1/storage/usage/30d').catch(keptOrNone);
      if (answer !== null) draw();
    } catch {
      // Without the history the cards say nothing of the rate; the use itself is still right.
    }
  }

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.every(READ_MS, read);
  view.every(USAGE_MS, readUsage);
  scrubber.load();
  loadLog();
  readUsage();
  await read();
}
