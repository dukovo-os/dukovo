// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The signed-in frame (design 4): reads the preferences, draws the header,
// navigation, `main`, drawer, notices and tab bar, follows the session's
// event stream and hands each address to the router. app.js calls
// `enterShell` once the session is known, and `stop` when it ends.

import { get, post, request, whenUnreachable } from '../api.js';
import { h, sheet } from '../dom.js';
import { makeCommands } from '../lib/command.js';
import { makeTime } from '../lib/time.js';
import { PAGES } from '../pages.js';
import { createTasks } from '../tasks/drawer.js';
import { askToUnlock, onUnlockChange, setUnlockSeconds, unlockMinutes } from '../unlock.js';
import { makeBus } from './bus.js';
import { lazyDrawer } from './drawers.js';
import { followEvents, makeStreamWatch } from './events.js';
import { createHeader } from './header.js';
import { followKeys } from './keys.js';
import { createNav } from './nav.js';
import { createPrefs } from './prefs.js';
import { createRouter } from './router.js';
import { makeView } from './view.js';

// How long to wait before looking at the session again when the stream is closed for good.
const STREAM_RETRY_MS = 5000;

// The unlock's warning comes with this many minutes left.
const LOCK_WARNING_MINUTES = 2;

/**
 * Enters the shell for `session`. `signOut()` ends it and shows the sign-in
 * page; `signInAgain()` is for a session limited to adding a second step;
 * `onEnded(reason)` hears that the stream says the session ended, and
 * `onClosed()` that it gave up. Gives `{stop, wasOffline, terminals}`, or
 * null when the preferences could not be read.
 */
export async function enterShell({ app, session: first, signOut, signInAgain, onEnded, onClosed, onFailure }) {
  let session = first;
  const prefs = createPrefs({ get, post, display: globalThis.dukovoDisplay ?? null });
  try {
    await prefs.load();
  } catch (error) {
    if (error.id !== 'ADM-107') onFailure(error);
    return null;
  }

  const host = session.host;
  let zone = prefs.value.zone;
  let time = makeTime(zone);
  prefs.onChange((value) => {
    if (value.zone !== zone) {
      zone = value.zone;
      time = makeTime(zone);
    }
  });

  const bus = makeBus();
  const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);
  let terminalsRunning = 0;
  let actionsList = null;
  let stopped = false;
  let wentAway = false;
  // What the shell registers outside its own elements ends with it.
  const open = new AbortController();

  // The regions of the frame.
  const asof = h('p', { class: 'asof small muted', role: 'status' });
  const statusRegion = h('div', { class: 'status-region' });
  const main = h('main', { id: 'main', class: 'main', tabindex: '-1' });
  const aside = h('aside', { id: 'drawer', class: 'drawer', hidden: true });
  const noticesRegion = h('div', { class: 'notices', role: 'status', 'aria-live': 'polite' });
  const content = h('div', { class: 'content' }, statusRegion, main);
  const frame = h('div', { class: 'frame' });

  // Notices are another module, read when one is first shown; the call gives
  // a function that closes the notice.
  let noticing = null;
  const notice = (words, options) => {
    noticing ??= import('./notices.js').then(({ noticesIn }) => noticesIn(noticesRegion));
    const shown = noticing.then((show) => show(words, options), () => () => {});
    return () => shown.then((close) => close());
  };
  // The parts of the page that go inert while a drawer is modal at phone width; filled below.
  const behind = [];
  const drawer = lazyDrawer({ aside, behind, host, signal: open.signal });
  const commands = makeCommands({ get, post, drawer });
  commands.setOn(prefs.value.commands === 'shown');
  prefs.onChange((value) => commands.setOn(value.commands === 'shown'));
  const shell = {
    host,
    signal: open.signal,
    get,
    post,
    prefs,
    bus,
    commands,
    notice,
    hasPage,
    session: () => session,
    time: () => time,
  };

  // The error view is another module, read when an error is first shown;
  // when even that cannot be read, the title goes in a notice.
  const problem = async (error, where = {}) => {
    try {
      const { showProblem } = await import('../lib/problem.js');
      showProblem(error, where, {
        session,
        slot: where.slot ?? router.current()?.view.slot,
        signal: router.current()?.view.signal,
        path: location.pathname,
        time,
        hasPage,
        checkSession: () => get('/api/1/session').catch(() => null),
      });
    } catch {
      notice(error?.title ?? 'Something went wrong', { kind: 'warn' });
    }
  };
  shell.problem = problem;

  const loadActions = () => {
    actionsList ??= get('/api/1/actions').catch((error) => {
      actionsList = null;
      throw error;
    });
    return actionsList;
  };

  const tasks = createTasks({
    get, post, session: () => session, time: () => time, notice, problem, commands, drawer, hasPage,
  });
  shell.tasks = tasks;

  // Running the pages' parts is another module, read when they are first asked for.
  let running = null;
  let made = null;
  const parts = {
    collect: async (name, call) => {
      running ??= import('./parts.js').then(({ makeParts }) => {
        made = makeParts({ pages: PAGES, get, session: () => session, time: () => time });
        return made;
      });
      return (await running).collect(name, call);
    },
    forget: () => made?.forget(),
  };
  shell.parts = parts;

  // The offline banner is another module, read once the first page is shown,
  // so that it is here when the server does not answer. `offline` is what the
  // rest of the shell asks.
  let offline = false;
  let waiting = null;
  const online = {
    isOffline: () => offline,
    async lost() {
      wentAway = true;
      offline = true;
      (await begin()).lost();
    },
    async expectRestart(words, options) {
      offline = true;
      (await begin()).expectRestart(words, options);
    },
    stop: () => waiting?.then((made) => made.stop()),
  };
  const begin = () => {
    waiting ??= import('./online.js').then(({ createOnline }) => createOnline({
      host,
      region: statusRegion,
      stale: asof,
      main,
      get,
      time: () => time,
      onChange: (now) => { offline = now; },
      onBack(restarted) {
        // The page that asked for a restart has lost its state, and is shown again.
        if (restarted) router.refresh();
        else bus.emit('online');
        tasks.load().catch(() => {});
        nav.readBadges('event');
      },
    }));
    return waiting;
  };
  shell.online = online;
  whenUnreachable(() => online.lost());

  // Running an action is another module, read the first time a page runs one.
  shell.run = (signal) => async (...call) => {
    const { makeRunner } = await import('../lib/action.js');
    return makeRunner({
      request: (method, path, body, more) => request(method, path, body, { ...more, signal }),
      actions: loadActions,
      session: () => session,
      askToUnlock,
      notice,
      problem,
      openTasks: () => tasks.open(null),
      learnTask: (id, close) => tasks.started(id, close),
      isOffline: () => online.isOffline(),
    })(...call);
  };

  // The palette is another module, read when it is first opened.
  shell.openPalette = async () => {
    const { togglePalette } = await import('../palette/palette.js');
    togglePalette({
      get,
      post,
      session: () => session,
      time: () => time,
      parts,
      go: (path) => router.go(path),
      run: shell.run(new AbortController().signal),
      commands,
      actions: loadActions,
    });
  };
  const { openPalette } = shell;

  shell.openShortcuts = async () => (await import('./dialogs.js')).shortcutsDialog();
  shell.openAbout = async () => (await import('./dialogs.js')).aboutDialog({ get });

  // The unlock's warning, once per unlock; also for a page opened with the
  // end already near. Its notice is another module, read when it is first due.
  let warned = false;
  const warnOfLock = () => {
    const minutes = unlockMinutes();
    if (minutes === null || minutes > LOCK_WARNING_MINUTES) {
      warned = false;
    } else if (!warned) {
      warned = true;
      import('./lock-warning.js').then((module) => module.warnOfLock({ minutes, post, notice }));
    }
  };
  open.signal.addEventListener('abort', onUnlockChange(warnOfLock));
  warnOfLock();

  async function signOutAsked() {
    if (await (await import('./dialogs.js')).signOutOk({ get, post })) signOut();
  }

  const nav = createNav({
    pages: PAGES, session: () => session, host, parts: (name, call) => parts.collect(name, call),
    openMore: async (opener) => (await import('./more.js')).openMore({ opener, pages: PAGES, session: () => session, items: [header.accountItems(), header.helpItems()] }),
  });

  let navOpen = false;
  const togglePages = (to = !navOpen) => {
    navOpen = to;
    frame.classList.toggle('nav-open', navOpen);
    header.pagesButton.setAttribute('aria-expanded', String(navOpen));
  };

  const router = createRouter({
    pages: PAGES,
    shell: {
      ...shell,
      makeView: (route) => makeView(shell, route),
      go: (path) => router.go(path),
      openPalette,
      nav,
      closeTransient: () => togglePages(false),
      signInAgain,
    },
    main,
    asof,
  });
  shell.go = (path) => router.go(path);

  shell.pageHelp = () => router.helpFor();
  shell.openPageHelp = () => {
    const help = router.helpFor();
    if (help) drawer.open('help', { title: 'Help for this page', build: (body) => help.show(body) });
  };
  shell.signOut = signOutAsked;
  shell.togglePages = () => togglePages();

  const header = createHeader(shell);
  drawer.onChange((name) => {
    frame.classList.toggle('with-drawer', name !== null);
    header.tasksButton.setAttribute('aria-expanded', String(name === 'tasks'));
  });

  frame.append(
    h('a', { class: 'skip', href: '#main' }, 'Skip to the page'),
    header.element,
    nav.side,
    content,
    aside,
    noticesRegion,
    nav.tabbar);
  app.replaceChildren(frame);
  behind.push(header.element, nav.side, content, nav.tabbar);

  // The stream. An event goes to the page, to Tasks and to the badges.
  const watch = makeStreamWatch();
  const stream = followEvents({
    onOpen: () => watch.opened(),
    onFailure: (closed) => {
      if (watch.failed(Date.now())) online.lost();
      if (closed && !stopped) setTimeout(() => !stopped && onClosed(), STREAM_RETRY_MS);
    },
    onEvent(name, data) {
      if (name === 'unlock') setUnlockSeconds(data.unlock_seconds);
      // A restart ends every session: the stream is over, and the banner waits for the server.
      if (name === 'session' && data.ended === 'restarted') {
        stream.close();
        online.lost();
      } else if (name === 'session') onEnded(data.ended);
      if (name === 'trial') header.trials(data.trials);
      if (name === 'task') tasks.event(name, data);
      // What the parts kept is old now, before the page reads again.
      if (name === 'task' || name === 'change') parts.forget();
      bus.emit(name, data);
      if (name === 'task' || name === 'change') nav.readBadges('event');
    },
  });

  const stopKeys = followKeys({ openPalette, openShortcuts: shell.openShortcuts });
  // How many terminals the account has, read at the start and each minute,
  // for the words of a session that ends.
  const countTerminals = () => get('/api/1/terminals').then((answer) => {
    terminalsRunning = (Array.isArray(answer) ? answer : answer?.terminals ?? []).length;
  }, () => {});
  const poll = setInterval(() => {
    nav.readBadges('poll');
    countTerminals();
  }, 60_000);
  countTerminals();

  tasks.load().catch(() => {});
  nav.readBadges('start');
  await router.start();
  begin();
  // Every dialog from here on has its look; nothing waits for the sheet.
  sheet('/dialogs.css');

  return {
    stop() {
      stopped = true;
      open.abort();
      stopKeys();
      clearInterval(poll);
      stream.close();
      online.stop();
      router.stop();
      whenUnreachable(null);
    },
    /** Whether the server stopped answering at any time while the shell was open. */
    wasOffline: () => wentAway || online.isOffline(),
    terminals: () => terminalsRunning,
  };
}
