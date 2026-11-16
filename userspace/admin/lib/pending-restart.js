// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The line that says a restart or shut down is waiting, with Cancel: "duke-nas
// restarts at 3:00 AM", then counted in seconds for the last minute. It is
// kept up to date by `event: power` and by `read()`, which asks `power.status`;
// when the minute ends it tells the shell that the server restarts, or shows
// that the machine is shutting down. Hardware, Updates and Move draw it on
// their cards, and lib/restart.js sets it after the action that begins one.

import { focusBack, h, icon } from '../dom.js';
import {
  minuteWords, nowWords, secondsLeft, shutDownNotice,
} from './restart-dialog-model.js';
import { pendingWords, quietMsOf, restartWords } from './restart-model.js';

/** How often the countdown is drawn. */
const TICK_MS = 1000;

/** The last of a restart's wait that is counted in seconds. */
const COUNTED_SECONDS = 60;

/**
 * Makes the line for the page `view`: `{element, expect, set, read}`.
 * `expect(options)` says what the restart is for, `{kind, version, try, of}`
 * as restartWords takes them, so that the banner's words fit it, and the
 * `quietMinutes` the shell stays quiet for when the kind's own wait is too
 * short; `set(shutdown)`
 * shows the shutdown that `power.status` or `event: power` gave, or none;
 * `read()` asks `power.status`.
 */
export function pendingRestart(view, options = {}) {
  let why = { kind: 'machine', ...options };
  let current = null;
  let ticking = null;
  let announced = false;
  let fired = null;

  const words = h('span', { role: 'timer', 'aria-live': 'off' });
  const status = h('span', { class: 'sr-only', role: 'status' });
  const cancel = h('button', { class: 'btn small', type: 'button' }, 'Cancel');
  const buttons = h('div', { class: 'btns' }, cancel);
  // Only an administrator may cancel; anyone else sees the line and not a button that would be refused.
  buttons.hidden = view.session?.admin !== true;
  const element = h('div', { class: 'banner info', hidden: true },
    icon('power'), h('div', {}, h('p', {}, words), buttons, status));

  const keyOf = (shutdown) => `${shutdown.end}:${shutdown.at_ms}`;
  const stop = () => {
    clearInterval(ticking);
    ticking = null;
  };
  view.signal.addEventListener('abort', stop, { once: true });

  /** The machine goes: the shell says the server restarts, or the page says it is shut down. */
  function due(shutdown) {
    if (shutdown.end === 'reboot') {
      // Only an update says which version it brings, so that the banner can tell when it was taken back.
      const version = why.kind === 'update' ? why.version ?? null : null;
      view.expectRestart(restartWords({ host: view.host, ...why }), { quietMs: quietMsOf(why.kind, why.quietMinutes), version });
      return;
    }
    view.main.replaceChildren(h('div', { class: 'banner warn', role: 'status' }, icon('power'), h('div', {}, h('p', {}, h('b', {}, shutDownNotice(view.host))))));
  }

  function tick() {
    const { shutdown, readAt } = current;
    const left = secondsLeft(shutdown, readAt, performance.now());
    const said = pendingWords({ ...shutdown, in_seconds: left ?? shutdown.in_seconds }, view.time);
    words.textContent = `${view.host} ${said.charAt(0).toLowerCase()}${said.slice(1)}`;
    if (left === null) return;
    if (left <= COUNTED_SECONDS && !announced) {
      announced = true;
      status.textContent = minuteWords(shutdown, view.host);
    }
    if (left <= 0 && fired !== keyOf(shutdown)) {
      fired = keyOf(shutdown);
      status.textContent = nowWords(shutdown);
      due(shutdown);
    }
  }

  function set(shutdown) {
    if (!shutdown) {
      current = null;
      announced = false;
      stop();
      element.hidden = true;
      return;
    }
    if (current === null || keyOf(current.shutdown) !== keyOf(shutdown)) announced = false;
    current = { shutdown, readAt: performance.now() };
    element.hidden = false;
    tick();
    ticking ??= setInterval(tick, TICK_MS);
  }

  async function read() {
    try {
      set((await view.api.get('/api/1/power'))?.shutdown ?? null);
    } catch {
      // A server that has no such route has nothing set that a page could cancel.
    }
  }

  cancel.addEventListener('click', async () => {
    const { cancelRestart } = await import('./restart.js');
    if (await cancelRestart(view, { button: cancel })) {
      await read();
      focusBack(null);
    }
  });
  view.on('power', (data) => set(data?.shutdown ?? null));

  return {
    element,
    expect(next) {
      why = { kind: 'machine', ...next };
    },
    set,
    read,
  };
}
