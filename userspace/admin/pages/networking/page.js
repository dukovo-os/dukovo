// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Networking (design 9): /networking is the cards, their traffic and the
// connections, /networking/CARD the editor of one card. Both show the banner
// of a network trial, whoever began it. The page reads `network show` when it
// opens, every 10 seconds while the tab is shown and when a change or a trial
// says that something moved; what is drawn changes only when what it says does.

import { fill, h } from '../../dom.js';
import { makeTrialBanner } from '../../lib/trial-banner.js';
import { atMostEvery } from '../overview/refresh.js';
import { findCard } from './network-model.js';

/** How often `network show` is read again while the tab is shown. */
const READ_MS = 10_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** The rows of grey bars that stand for the cards while they are read. */
const bar = () => h('span', { class: 'net-bar', 'aria-hidden': 'true' });
const frame = (title) => h('section', { class: 'card', 'aria-hidden': 'true' }, h('h2', {}, title), bar(), bar(), bar());

/** `rest` as a card's name, or null for the list; a name that is not text is none either. */
function cardNamed(rest) {
  const name = rest.replace(/\/$/, '');
  if (name === '') return null;
  try {
    return decodeURIComponent(name);
  } catch {
    return name;
  }
}

/** What the banner says is being tried: the card the journal names, if it does. */
const whatWords = (card) => (card === null ? 'The network settings are being tried.' : `The network settings of ${card} are being tried.`);

export async function show(view) {
  const name = cardNamed(view.rest);
  view.head(name === null
    ? { title: 'Networking', commands: true }
    : { title: name, crumbs: [{ title: 'Networking', path: '/networking' }], commands: true });

  let latest = 0;
  let reading = null;
  let trial = false;
  let answer = null;
  let failed = false;
  let editor = null;
  let cards = null;
  let traffic = null;
  let connections = null;
  const none = h('p', { class: 'muted net-none', hidden: true }, 'This machine has no network card.');

  const content = h('div', { class: 'net' });
  const banner = makeTrialBanner(view, {
    id: 'network',
    what: whatWords,
    keep: 'network.keep',
    undo: 'network.undo',
    onState: ({ running, ended }) => {
      trial = running;
      // Until the first read has answered, it draws what the trial says too.
      if (answer === null) return;
      draw().catch(() => {});
      // A trial that ended is the card's settings that moved back: read them again.
      if (ended) soon();
    },
  });
  view.main.append(banner.element, content);
  fill(content, name === null ? [frame('Cards')] : [frame(name)]);

  /** Draws `answer`: the cards, or the editor of one. */
  async function draw() {
    const list = Array.isArray(answer?.cards) ? answer.cards : [];
    if (answer.trial && !banner.running()) banner.read();
    if (name === null) {
      if (cards === null) {
        const [{ makeCards }, { makeTraffic }, { makeConnections }] = await Promise.all([
          import('./cards.js'), import('./traffic.js'), import('./connections.js'),
        ]);
        if (cards !== null) return;
        cards = makeCards(view);
        traffic = makeTraffic(view);
        connections = makeConnections(view);
        fill(content, none, cards.element, traffic.element, connections.element);
      }
      none.hidden = list.length > 0;
      cards.set(list, Date.now());
      traffic.set(list);
      return;
    }
    const card = findCard(answer, name);
    if (card === null) {
      editor = null;
      fill(content, h('section', { class: 'card' },
        h('h2', {}, 'There is no such network card'),
        h('p', {}, `This machine has no network card called ${name}.`),
        h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/networking' }, 'Back to the cards'))));
      return;
    }
    if (editor === null) {
      const { makeEditor } = await import('./editor.js');
      if (editor !== null) return;
      editor = makeEditor(view, card.name, { banner, reread: soon });
      fill(content, editor.element);
    }
    editor.set(card, trial);
  }

  /** The connections are read with the cards, on the list's route only. */
  async function readConnections() {
    if (name !== null || connections === null) return;
    try {
      connections.set(await view.api.get('/api/1/network/connections'));
    } catch (error) {
      if (error?.name !== 'AbortError') connections.fail(error);
    }
  }

  /** Reads the cards. Only the last read counts, whatever order they answer in; a read that is passed waits for it. */
  function read() {
    latest += 1;
    const mine = latest;
    reading = readCards(mine);
    return reading;
  }

  async function readCards(mine) {
    try {
      const found = await view.api.get('/api/1/network');
      if (mine !== latest) return reading;
      answer = found;
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
      }
      await draw();
      readConnections();
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

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.every(READ_MS, read);
  banner.read();
  await read();
}
