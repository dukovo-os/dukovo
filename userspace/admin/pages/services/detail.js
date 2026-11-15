// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page of one service (design 4.4): its state, why it is in it, the
// buttons that start, stop and restart it, and a card for what it runs, what
// it needs, what it holds, what it uses, its limits, its sockets, its timer,
// a trial of its file and its recent log. It is read
// every 5 seconds while the tab is shown and after each action; what is drawn
// changes only when what it says does, so that a button in hand stays in
// hand. A name Spark does not know gets the page's own words and a way back
// to the list.

import { ApiError } from '../../api.js';
import { fill, h, icon } from '../../dom.js';
import { atMostEvery } from '../overview/refresh.js';
import {
  copyBox, frameOf, makeCard, quiet, rowsList, stable,
} from './cards.js';
import { makeControls } from './controls.js';
import {
  bannerOf, complaintOf, explainable, healthOf, holdsRows, needsRows, runsOf, triedOf, unknownService, unknownWords,
} from './detail-model.js';
import { limitsRows, socketsRows, tryCommand } from './limits-model.js';
import { atBootOf, isServiceName, stateOf } from './model.js';
import { serviceHref } from './needs.js';
import { makeRecentLog } from './recent-log.js';
import { makeTimer } from './timer-card.js';
import { makeUse } from './use.js';

/** How often the service is read again while the tab is shown. */
const READ_MS = 5000;

/** The least time between two reads that a task or a change asks for. */
const EVENT_GAP_MS = 2000;

/** The name in the address, undone from its percent-encoding; null when the address is not one. */
function nameOf(rest) {
  try {
    return decodeURIComponent(rest);
  } catch {
    return null;
  }
}

/** The banner above the cards: the explanation of why the service is not running. */
function bannerNode(banner) {
  return h('div', { class: `banner ${banner.kind}` },
    icon('warn'),
    h('div', {},
      h('p', {}, banner.sentence),
      banner.fix && h('p', {}, banner.fix),
      banner.root && h('p', {}, 'Look at ', h('a', { href: serviceHref(banner.root) }, banner.root), ', which it depends on.')));
}

/** A warning under the state, as a short banner. */
const warningNode = (words) => h('div', { class: 'banner warn' }, icon('warn'), h('div', {}, h('p', {}, words)));

export async function show(view) {
  const name = nameOf(view.rest);
  view.head({ title: name ?? view.rest, crumbs: [{ title: 'Services', path: '/services' }], commands: true });

  const toList = h('a', { class: 'btn small', href: '/services' }, 'Open the list');
  if (name === null || !isServiceName(name)) {
    view.problem(new ApiError(unknownWords(name ?? view.rest, null)), { buttons: [toList] });
    return;
  }

  const path = `/api/1/services/${encodeURIComponent(name)}`;
  let latest = 0;
  let drawn = false;
  let failed = false;
  let gone = false;
  let explained = { key: null, value: null };

  const top = h('div', { class: 'svc-top' });
  const banners = h('div', { class: 'svc-banners' });
  const setTop = stable(top);
  const setBanners = stable(banners);
  const controls = makeControls(view, name, () => read());
  const runs = makeCard('runs', 'Runs', (rows) => (rows.length > 0 ? rowsList(rows) : quiet('Spark has nothing more to say of a run.')));
  const needs = makeCard('needs', 'Needs', (rows) => (rows.length > 0 ? rowsList(rows) : quiet('It does not need or wait for another service.')));
  const holds = makeCard('holds', 'Holds', (rows) => rowsList(rows));
  const tried = makeCard('tried', 'Being tried', (one) => [
    h('p', {}, one.words),
    copyBox(one.keep, 'To keep the change'),
    copyBox(one.undo, 'To undo the change'),
  ]);
  const use = makeUse(view, name);
  const limits = makeCard('limits', 'Limits', (rows) => [rowsList(rows), copyBox(tryCommand(name), 'To change the limits')]);
  const sockets = makeCard('sockets', 'Sockets', (rows) => rowsList(rows));
  const timer = makeTimer(view, name);
  const log = makeRecentLog(view, name);
  const cards = h('div', { class: 'cards svc-cards' }, frameOf('runs', 'Runs'), frameOf('needs', 'Needs'), frameOf('holds', 'Holds'));
  // The way to the log that a phone's page has beside the buttons, since its log is far below them.
  const toLog = h('a', { class: 'btn svc-to-log', href: `${serviceHref(name)}#log` }, 'Open its log');
  const content = h('div', { class: 'svc', 'aria-busy': 'true' }, top, banners, controls.element, toLog, cards, log.element);
  view.main.append(content);

  /** Why the service is not running, read again only when its state changed. */
  async function explainedBanner(service) {
    if (!explainable(service)) {
      explained = { key: null, value: null };
      return null;
    }
    const key = [service.state, service.since_ms, service.failures, service.why].join('|');
    if (explained.key !== key) {
      const value = await view.api.get(`${path}/explain`).catch(() => null);
      explained = { key, value };
    }
    return bannerOf(service, explained.value);
  }

  /** Puts the service in the page, if no later read has begun. */
  async function draw(answer, mine) {
    const { service } = answer;
    const now = Math.floor(Date.now() / 1000);
    const state = stateOf(service);
    const boot = atBootOf(service);
    const banner = await explainedBanner(service);
    if (view.signal.aborted || mine !== latest) return;
    setTop({ state, boot, tried: answer.trial != null, description: service.description ?? '' }, (model) => h('div', { class: 'svc-pills' },
      h('span', { class: `st ${model.state.kind}` }, model.state.word),
      h('span', { class: `pill ${model.boot.kind}`.trim() }, model.boot.word),
      model.tried && h('span', { class: 'pill warn' }, 'Being tried'),
      model.description && h('p', { class: 'svc-desc' }, model.description)));
    setBanners({ banner, health: healthOf(service), complaint: complaintOf(service) }, (model) => h('div', { class: 'svc-warnings' },
      model.banner && bannerNode(model.banner),
      model.health && warningNode(model.health),
      model.complaint && warningNode(`Its file has a mistake. ${model.complaint} Spark runs the last good copy.`)));
    controls.set(service, view.session.admin);
    runs.set(runsOf(answer, now, view.time));
    needs.set(needsRows(service));
    const held = holdsRows(service);
    holds.set(held.length > 0 ? held : null);
    use.set(service);
    const limited = limitsRows(service);
    limits.set(limited.length > 0 ? limited : null);
    sockets.set(socketsRows(service));
    timer.set(service);
    tried.set(triedOf(answer, now, view.time));
    if (!drawn) {
      drawn = true;
      fill(cards, runs.element, needs.element, holds.element, use.element, limits.element, sockets.element, timer.element, tried.element);
      content.removeAttribute('aria-busy');
      log.load();
    }
  }

  /** The words of a name that is no service, in place of the page. */
  function showGone(error) {
    gone = true;
    content.hidden = true;
    view.problem(new ApiError(unknownWords(name, error)), { buttons: [toList] });
  }

  /** Reads the service and draws it. Only the last read counts, whatever order they answer in. */
  async function read() {
    if (gone) return;
    latest += 1;
    const mine = latest;
    try {
      const answer = await view.api.get(path);
      if (mine !== latest) return;
      if (!answer?.service || typeof answer.service !== 'object') {
        throw new ApiError({ title: 'The console got an answer it cannot read', detail: 'The answer is not a service.', fix: 'Try again. If it keeps happening, look at the server\'s log.' });
      }
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
      }
      await draw(answer, mine);
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      if (unknownService(error)) {
        showGone(error);
        return;
      }
      // What is drawn stays while the server does not answer; a first read that fails leaves nothing to show.
      if (!drawn) content.hidden = true;
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

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('task', soon);
  view.every(READ_MS, read);
  await read();
}
