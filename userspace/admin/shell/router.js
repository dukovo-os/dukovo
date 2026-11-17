// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Routing (design 3.4): the History API, clicks on the console's own links,
// and each route's steps: end the old page's work, empty `main`, load the
// page and its styles, call `show`, focus the h1, scroll to the top or to
// the id after the `#`.

import { h, sheet } from '../dom.js';
import { chooseView, followsInPage } from './route-model.js';

/** "Loading…" shows only after this long, so that a quick page never flashes. */
const LOADING_AFTER_MS = 300;

/**
 * `pages` is the list; `shell` is the shell's services; `main` is the
 * `<main>` element and `asof` the mark for a page that is stale. Gives
 * `{start, go, refresh, helpFor, confirmLeave, current}`.
 */
export function createRouter({ pages, shell, main, asof }) {
  let controller = null;
  let active = null;
  let url = '';

  // Resolves once each sheet has loaded or failed, so that a page never draws bare.
  const linkStyles = (css) => Promise.all(css.map((href) => sheet(`/${href}`)));

  async function resolve(decision, path) {
    const own = async (title, make) => ({ module: make(await import('./static-pages.js')), entry: { title, css: [] }, rest: '' });
    switch (decision.kind) {
      case 'page':
        if (decision.entry.load !== null) return { module: await decision.entry.load(), entry: decision.entry, rest: decision.rest };
        break;
      case 'admins-only': {
        const module = await decision.entry.load?.();
        if (typeof module?.reader === 'function') return { module: { show: module.reader }, entry: decision.entry, rest: decision.rest };
        return { ...(await own(decision.entry.title, (pages) => pages.adminsOnly(shell, decision.entry))), entry: decision.entry, rest: decision.rest };
      }
      case 'help':
        return { module: await import('../help/help.js'), entry: { title: 'Help', css: ['help/help.css'] }, rest: decision.rest };
      case 'second-step-page':
        return own('Second step', (pages) => pages.secondStepPage(shell));
      default:
    }
    return own('No page here', (pages) => pages.noPage(shell, path));
  }

  async function render() {
    const place = new URL(location.href);
    const decision = chooseView({ pages, session: shell.session(), prefs: shell.prefs.value, path: place.pathname });
    if (decision.kind === 'redirect') {
      history.replaceState(null, '', `${decision.to}${place.search}${place.hash}`);
      return render();
    }
    if (decision.kind === 'welcome') {
      history.replaceState(null, '', `/welcome?then=${encodeURIComponent(decision.then + place.search)}`);
      return render();
    }

    controller?.abort();
    const mine = new AbortController();
    controller = mine;
    const regions = { ph: h('div', { class: 'ph' }), slot: h('div', { class: 'slot' }), page: h('div', { class: 'page' }) };
    main.replaceChildren(asof, regions.ph, regions.slot, regions.page);
    url = `${place.pathname}${place.search}`;
    shell.commands.reset();
    shell.closeTransient();
    shell.nav.update(place.pathname);
    shell.bus.emit('route');
    const focusAtStart = document.activeElement;

    const state = { first: true };
    let found = null;
    const view = shell.makeView({ path: place.pathname, rest: '', query: place.searchParams, signal: mine.signal, state, ...regions });
    active = { view, module: null, entry: null };
    const loading = setTimeout(() => regions.page.prepend(h('p', { class: 'muted', role: 'status', 'data-loading': '' }, 'Loading…')), LOADING_AFTER_MS);
    try {
      found = await resolve(decision, place.pathname);
      if (mine.signal.aborted) return;
      await linkStyles(found.entry.css ?? []);
      if (mine.signal.aborted) return;
      view.rest = found.rest;
      active = { view, module: found.module, entry: found.entry };
      await found.module.show(view);
    } catch (error) {
      if (mine.signal.aborted || error?.name === 'AbortError') return;
      if (!regions.ph.firstChild) view.head({ title: found?.entry.title ?? 'This page could not be shown' });
      shell.problem(error, { slot: regions.slot, first: true });
    } finally {
      clearTimeout(loading);
      regions.page.querySelector('[data-loading]')?.remove();
      state.first = false;
    }
    if (mine.signal.aborted) return;
    settle(place, focusAtStart);
  }

  /**
   * Focus on the h1, unless the person moved it out of the page while it
   * loaded, into a menu of the header say; and the scroll: to the id after
   * the `#`, or to the top.
   */
  function settle(place, focusAtStart) {
    const target = place.hash ? document.getElementById(decodeURIComponent(place.hash.slice(1))) : null;
    const heading = main.querySelector('h1');
    const now = document.activeElement;
    const moved = now && now !== focusAtStart && now !== document.body && !main.contains(now);
    if (!moved) (heading ?? main).focus({ preventScroll: true });
    if (target) target.scrollIntoView();
    else main.scrollTop = 0;
  }

  /** The words a page that has unsaved input gives before it is left, or null. */
  const leavingWords = () => active?.view.leaving?.() || null;

  async function confirmLeave() {
    const words = leavingWords();
    if (!words) return true;
    const { ask } = await import('./dialogs.js');
    return (await ask({
      title: 'Leave this page?',
      words,
      iconName: 'warn',
      choices: [{ label: 'Stay on this page', value: 'stay', kind: 'default' }, { label: 'Leave', value: 'leave' }],
    })) === 'leave';
  }

  async function go(path, { replace = false } = {}) {
    if (!(await confirmLeave())) return;
    if (replace) history.replaceState(null, '', path);
    else history.pushState(null, '', path);
    await render();
  }

  return {
    /** Starts following clicks and the browser's back and forward buttons, and shows the first page. */
    start() {
      document.addEventListener('click', async (event) => {
        const link = event.target.closest?.('a[href^="/"]');
        if (!link) return;
        const click = {
          button: event.button, ctrlKey: event.ctrlKey, metaKey: event.metaKey, shiftKey: event.shiftKey, altKey: event.altKey,
          defaultPrevented: event.defaultPrevented,
        };
        const where = { href: link.getAttribute('href'), target: link.getAttribute('target'), download: link.hasAttribute('download') };
        if (!followsInPage(where, click, location.origin)) return;
        event.preventDefault();
        const next = new URL(where.href, location.origin);
        // A link to a place on the page that is open only scrolls there.
        if (next.pathname + next.search === location.pathname + location.search && next.hash) {
          history.pushState(null, '', where.href);
          settle(next);
          return;
        }
        await go(where.href);
      });
      window.addEventListener('popstate', async () => {
        if (await confirmLeave()) {
          render();
        } else {
          history.pushState(null, '', url);
        }
      });
      window.addEventListener('beforeunload', (event) => {
        if (leavingWords()) event.preventDefault();
      });
      return render();
    },
    go,
    /** Shows the page again, as when the server answers after being away: its `online` event is the shell's. */
    refresh: render,
    /** What the Help menu offers for the page that is open: `{open(body)}`, or null when it has no help. */
    helpFor: () => (typeof active?.module?.help === 'function' ? { show: (body) => active.module.help(Object.create(active.view, { main: { value: body } })) } : null),
    confirmLeave,
    current: () => active,
    /** Ends the page's work, as when the session ends. */
    stop() {
      controller?.abort();
    },
  };
}
