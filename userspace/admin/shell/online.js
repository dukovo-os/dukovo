// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The offline banner and its looks at the server. A look asks `GET
// /api/1/health`; when it answers, it reads the session again: the same
// session means the page is back, a session that is gone is for api.js's
// handler, which shows the sign-in page. Each is given up after LOOK_MS: the
// connection a server had when it went away may never answer again, and the
// next look then comes on another. A look is made at once, and not at its
// time, when any other call is answered meanwhile. After a restart that a
// page asked for, the banner says what it was told and "Waiting since" under
// it, and when the server answers it says "Back", or that an update was taken
// back when `about` says the start is a fallback. A restart ends the session,
// though: the sign-in page comes first, and the banner made for the new
// session says it, when the machine has started since. An update's try is a
// start that is no fallback, and the machine may go back from it at once, so
// the release is kept again for the sign-in after that.

import { whenAnswered } from '../api.js';
import { h, icon } from '../dom.js';
import { forget, keep, recall } from '../lib/store.js';
import {
  asOfWords, BACK_KEPT_MS, BACK_MS, backWords, DISABLED_TIP, LOOK_MS, makeConnection, offlineWords, tryingWords, waitingWords,
} from './online-model.js';

// Where an update's restart keeps the release it was to bring, for the next sign-in.
const KEPT = 'updates.back';

/**
 * Makes the offline handling. `region` holds the banner; `stale` holds the
 * "As of" mark; `main` is the page, whose buttons that change something
 * (`data-changes`) are marked while the server does not answer. `get` is
 * api.js's, and `time()` gives lib/time.js's formatters.
 * `onBack(restarted)` is called when the server answers again with the same
 * session, and says whether a page had asked for a restart.
 */
export function createOnline({ host, region, stale, main, get, time, onBack, onChange }) {
  const link = makeConnection();
  let ticker = null;
  let left = 0;
  let looking = false;
  let went = null;
  let expected = null;
  let welcome = null;
  const words = h('span', {});
  const since = h('p', { class: 'muted', hidden: true });
  const tryNow = h('button', { class: 'btn small', type: 'button', onclick: () => look() }, 'Try now');
  const banner = h('div', { class: 'banner bad', role: 'status' }, icon('warn'), h('div', {}, h('p', {}, words), since, h('div', { class: 'btns' }, tryNow)));
  const observer = new MutationObserver(() => mark());
  // Any call that is answered is a server that answers again: look at once.
  // A restart's quiet time is waited out, since the old console may answer.
  whenAnswered(() => {
    if (link.state(Date.now()) === 'offline') look();
  });

  /** Asks `path`, and gives the call up after LOOK_MS. */
  function ask(path) {
    const giveUp = new AbortController();
    const timer = setTimeout(() => giveUp.abort(), LOOK_MS);
    return get(path, { abandon: giveUp.signal }).finally(() => clearTimeout(timer));
  }

  function mark() {
    const offline = link.state(Date.now()) !== 'online';
    for (const button of main.querySelectorAll('[data-changes]')) {
      if (offline) {
        button.setAttribute('aria-disabled', 'true');
        button.setAttribute('title', DISABLED_TIP);
      } else if (button.getAttribute('title') === DISABLED_TIP) {
        button.removeAttribute('aria-disabled');
        button.removeAttribute('title');
      }
    }
  }

  function render() {
    const state = link.state(Date.now());
    onChange(state !== 'online');
    if (state === 'online') {
      banner.remove();
      stale.replaceChildren();
      observer.disconnect();
      mark();
      return;
    }
    const restarting = state === 'restarting';
    words.textContent = restarting ? link.words() : offlineWords(host, left);
    since.textContent = restarting ? waitingWords(time(), went) : '';
    since.hidden = !restarting;
    tryNow.hidden = restarting;
    if (!banner.isConnected) {
      region.append(banner);
      observer.observe(main, { childList: true, subtree: true });
    }
    stale.replaceChildren(went === null ? '' : asOfWords(time(), went));
    mark();
  }

  function schedule() {
    clearInterval(ticker);
    left = link.nextRetry();
    render();
    ticker = setInterval(() => {
      left -= 1;
      if (left <= 0) look();
      else render();
    }, 1000);
  }

  /**
   * Says that the server is back: "Back" for a few seconds, or what happened
   * to an update, until it is closed. `since` is when a greeting kept for
   * the next session was kept: it is said only when the machine has started
   * since, as `about`'s `up` tells. The release is kept again while the
   * update is on its try.
   */
  async function greet(version, since = null) {
    const about = version ? await ask('/api/1/console/about').catch(() => null) : null;
    if (since !== null && !(Number.isFinite(about?.up) && Date.now() - about.up * 1000 > since)) return;
    const back = backWords(about, version);
    if (back.trying) keep(KEPT, { version, at: Date.now() });
    const close = () => {
      clearTimeout(timer);
      note.remove();
    };
    const note = h('div', { class: `banner ${back.link ? 'warn' : 'ok'}`, role: 'status' },
      icon(back.link ? 'warn' : 'check'),
      h('div', {},
        h('p', {}, back.words, back.link && [' ', h('a', { href: back.link.href, onclick: close }, back.link.label)]),
        back.link && h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: close }, 'Close'))));
    const timer = back.link ? null : setTimeout(close, BACK_MS);
    welcome?.();
    welcome = close;
    region.append(note);
  }

  async function look() {
    if (looking) return;
    looking = true;
    clearInterval(ticker);
    if (link.state(Date.now()) !== 'restarting') words.textContent = tryingWords(host);
    try {
      await ask('/api/1/health');
      await ask('/api/1/session');
      const restart = expected;
      expected = null;
      link.answered();
      render();
      onBack(restart !== null);
      if (restart) {
        forget(KEPT);
        greet(restart.version);
      }
    } catch (error) {
      // A session that ended is shown by api.js's handler. Anything else is
      // a server that still does not answer.
      if (error?.status === 401) {
        expected = null;
        link.answered();
        render();
      } else {
        schedule();
      }
    } finally {
      looking = false;
    }
  }

  const kept = recall(KEPT);
  if (kept) {
    forget(KEPT);
    if (typeof kept.version === 'string' && Date.now() - kept.at < BACK_KEPT_MS) greet(kept.version, kept.at);
  }

  return {
    /** A call got no answer. */
    lost() {
      if (looking) return;
      if (link.lost()) went = Math.floor(Date.now() / 1000);
      schedule();
    },
    /**
     * An action is about to restart the console or the machine: the banner
     * says `restartWords` and is quiet for `options.quietMs` (120 s unless it
     * says), then as offline. `options.version` is the release an update was
     * to bring, which is also kept for the next session: whichever call finds
     * that the restart ended the session, the sign-in page comes next.
     */
    expectRestart(restartWords, options = {}) {
      went = Math.floor(Date.now() / 1000);
      expected = { version: options.version ?? null };
      if (expected.version) keep(KEPT, { version: expected.version, at: Date.now() });
      welcome?.();
      link.expectRestart(Date.now(), { words: restartWords, quietMs: options.quietMs });
      schedule();
    },
    isOffline: () => link.state(Date.now()) !== 'online',
    stop() {
      clearInterval(ticker);
      welcome?.();
      observer.disconnect();
      whenAnswered(null);
    },
  };
}
