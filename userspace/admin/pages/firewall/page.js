// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Firewall (design 10): the rules and the machine's rule for the rest, the
// form that adds, replaces or removes a rule with a window to undo it, what was
// blocked, a test of a connection and what listens. One route, /firewall; the
// form opens with #add and #add?replace=N. The page reads the rules every 10
// seconds while the tab is shown and when a change or a trial says that
// something moved; what is drawn changes only when what it says does.

import { h, icon } from '../../dom.js';
import { makeTrialBanner } from '../../lib/trial-banner.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeAdd } from './add.js';
import { makeBlocked, makeListening } from './blocked.js';
import { headPill, trialLeft } from './report-model.js';
import { makeRules } from './rules.js';
import { hereOf, rulesOf } from './rules-model.js';
import { makeTest } from './test.js';

/** How often the rules and the service are read again while the tab is shown. */
const READ_MS = 10_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** The form's place in the address: `#add`, `#add?replace=3` or `#add?remove=3`. */
function modeOfHash(hash) {
  const found = /^#add(?:\?(replace|remove)=(\d+))?$/.exec(hash);
  if (!found) return null;
  return found[1] ? { kind: found[1], number: Number(found[2]) } : { kind: 'add' };
}

export async function show(view) {
  let pill = headPill(await view.api.get('/api/1/services').catch(() => null));
  // A person who has gone to another page while this one asked has nothing of this page to see.
  if (view.signal.aborted) return;
  const headOf = () => view.head({ title: 'Firewall', pill, commands: true });
  let h1 = headOf();

  let latest = 0;
  let reading = null;
  let listed = null;
  let services = null;
  let network = null;
  let failed = false;
  let trialRuns = false;
  let hashDone = false;

  const loadNote = h('div', { class: 'fw-load', hidden: true });
  const content = h('div', { class: 'fw' });
  const banner = makeTrialBanner(view, {
    id: 'firewall',
    what: () => 'The firewall rules are being tried.',
    keep: 'firewall.keep',
    undo: 'firewall.undo',
    onState: ({ running, ended }) => {
      trialRuns = running;
      if (listed !== null) draw();
      // A trial that ended is the rules that moved back: read them again.
      if (ended) soon();
    },
  });
  const rules = makeRules(view, {
    onAdd: () => add.open({ kind: 'add' }),
    onChange: (number) => add.open({ kind: 'replace', number }),
    onRemove: (number) => add.open({ kind: 'remove', number }),
  });
  const add = makeAdd(view, { banner, reread: () => soon() });
  const blocked = makeBlocked(view, { onAllow: (words, trigger) => add.open({ kind: 'add', words }, trigger) });
  const test = makeTest(view, { rules: () => rulesOf(listed) });
  const listening = makeListening(view);
  view.main.append(banner.element, loadNote, content);
  content.append(rules.element, add.element, blocked.element, test.element, listening.element);

  const admin = () => view.session.admin === true;
  const running = () => trialRuns || banner.running() || trialLeft(listed) !== null;
  const cardNames = () => (Array.isArray(network?.cards) ? network.cards.map((card) => card.name) : []);

  /** The head's pill, drawn again only when it changes, with focus kept on the title. */
  function drawHead() {
    const next = headPill(services);
    if (JSON.stringify(next) === JSON.stringify(pill)) return;
    pill = next;
    const had = document.activeElement === h1;
    h1 = headOf();
    if (had) h1.focus();
  }

  /** What Spark says when the rules did not load. */
  function drawLoad() {
    const explain = pill?.kind === 'bad' ? pill.explain : null;
    loadNote.hidden = pill?.kind !== 'bad';
    loadNote.replaceChildren(...(pill?.kind === 'bad'
      ? [h('div', { class: 'banner bad', role: 'status' }, icon('warn'), h('div', {}, h('p', {}, h('b', {}, 'The rules did not load. '), explain ?? 'Spark says the firewall service failed.')))]
      : []));
  }

  /** Draws what the last reads say. */
  function draw() {
    if (listed === null || view.signal.aborted) return;
    const trial = running();
    if (trialLeft(listed) !== null && !banner.running()) banner.read();
    rules.set(listed, { admin: admin(), trial });
    add.set({
      rules: rulesOf(listed), cards: cardNames(), here: hereOf(network, view.session), trial,
    });
    test.set({ cards: cardNames() });
    blocked.set({ trial });
    drawHead();
    drawLoad();
    if (!hashDone) {
      hashDone = true;
      const mode = modeOfHash(location.hash);
      if (mode && admin() && !trial) add.open(mode, document.querySelector('main h1'));
    }
  }

  /** Reads the rules, and Spark's document for the pill. Only the last read counts; a read that is passed waits for it. */
  function read() {
    latest += 1;
    const mine = latest;
    reading = readAll(mine);
    return reading;
  }

  async function readAll(mine) {
    try {
      const [found, doc] = await Promise.all([view.api.get('/api/1/firewall'), view.api.get('/api/1/services').catch(() => null)]);
      if (mine !== latest) return reading;
      listed = found;
      services = doc ?? services;
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
      }
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return reading;
      // What is drawn stays while the server does not answer; a first read that fails leaves nothing to show.
      if (listed === null) content.hidden = true;
      if (!failed) {
        failed = true;
        view.problem(error, { buttons: [h('button', { class: 'btn small', type: 'button', onclick: again }, 'Try again')] });
      }
    }
    return undefined;
  }

  /** Reads the cards, for the choice of On, the test and "This network". A machine whose cards cannot be read has those without. */
  async function readNetwork() {
    try {
      network = await view.api.get('/api/1/network');
    } catch {
      network = null;
    }
    draw();
  }

  function again() {
    view.slot.replaceChildren();
    failed = false;
    content.hidden = false;
    read();
  }

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', () => {
    soon();
    readNetwork();
  });
  view.every(READ_MS, read);
  banner.read();
  await Promise.all([read(), readNetwork()]);
  blocked.load();
  listening.load();
}
