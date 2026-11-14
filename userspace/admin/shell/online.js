// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The offline banner and its looks at the server (design 4.2). `health`
// asks `GET /api/1/health`; when it answers, `session` reads the session
// again: the same session means the page is back, a session that is gone
// is for api.js's handler, which shows the sign-in page.

import { h, icon } from '../dom.js';
import {
  asOfWords, DISABLED_TIP, makeConnection, offlineWords, restartingWords, tryingWords,
} from './online-model.js';

/**
 * Makes the offline handling. `region` holds the banner; `stale` holds the
 * "As of" mark; `main` is the page, whose buttons that change something
 * (`data-changes`) are marked while the server does not answer. `time()`
 * gives lib/time.js's formatters. `onBack()` is called when the server
 * answers again with the same session.
 */
export function createOnline({ host, region, stale, main, health, session, time, onBack, onChange }) {
  const link = makeConnection();
  let ticker = null;
  let left = 0;
  let looking = false;
  let went = null;
  const words = h('span', {});
  const tryNow = h('button', { class: 'btn small', type: 'button', onclick: () => look() }, 'Try now');
  const banner = h('div', { class: 'banner bad', role: 'status' }, icon('warn'), h('div', {}, h('p', {}, words), h('div', { class: 'btns' }, tryNow)));
  const observer = new MutationObserver(() => mark());

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
    words.textContent = state === 'restarting' ? restartingWords : offlineWords(host, left);
    tryNow.hidden = state === 'restarting';
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

  async function look() {
    if (looking) return;
    looking = true;
    clearInterval(ticker);
    words.textContent = tryingWords(host);
    try {
      await health();
      await session();
      link.answered();
      render();
      onBack();
    } catch (error) {
      // A session that ended is shown by api.js's handler. Anything else is
      // a server that still does not answer.
      if (error?.status === 401) {
        link.answered();
        render();
      } else {
        schedule();
      }
    } finally {
      looking = false;
    }
  }

  return {
    /** A call got no answer. */
    lost() {
      if (looking) return;
      if (link.lost()) went = Math.floor(Date.now() / 1000);
      schedule();
    },
    /** An action is about to restart the console: quiet for 120 s, then as offline. */
    expectRestart() {
      went = Math.floor(Date.now() / 1000);
      link.expectRestart(Date.now());
      schedule();
    },
    isOffline: () => link.state(Date.now()) !== 'online',
    stop() {
      clearInterval(ticker);
      observer.disconnect();
    },
  };
}
