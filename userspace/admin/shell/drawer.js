// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The drawer on the right (design 4.1): Tasks, Show command, and a page's
// help. One at a time; opening another replaces it. Wide, it sits beside
// the page and is not modal: focus moves in when it opens, Tab leaves it,
// and Esc closes it and puts focus back on what opened it. Under 720 px it
// fills the screen as a modal dialog: the rest of the page is inert and Tab
// stays inside.

import { focusBack, h } from '../dom.js';

const PHONE = '(max-width: 719.98px)';
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select, textarea, summary, [tabindex]:not([tabindex="-1"])';

/**
 * `aside` is the drawer's element; `behind` are the elements that go inert
 * while it is modal; `host` is the machine's name, in the head.
 */
export function createDrawer({ aside, behind, host }) {
  const phone = matchMedia(PHONE);
  const listeners = new Set();
  let name = null;
  let opener = null;
  let dispose = null;
  let leave = null;

  const heading = h('h2', { id: 'drawer-title', tabindex: '-1' });
  const closeButton = h('button', { class: 'btn small', type: 'button', onclick: () => close() });
  const body = h('div', { class: 'drawer-body' });
  aside.replaceChildren(
    h('div', { class: 'drawer-head' }, heading, h('span', { class: 'small muted' }, `on ${host}`), closeButton),
    body);
  aside.setAttribute('aria-labelledby', 'drawer-title');

  function mode() {
    const modal = name !== null && phone.matches;
    if (modal) {
      aside.setAttribute('role', 'dialog');
      aside.setAttribute('aria-modal', 'true');
    } else {
      aside.removeAttribute('role');
      aside.removeAttribute('aria-modal');
    }
    for (const element of behind) element.inert = modal;
  }
  phone.addEventListener('change', mode);

  function tell() {
    for (const listener of listeners) listener(name);
  }

  function close({ focus = true } = {}) {
    if (name === null) return;
    const back = opener;
    dispose?.();
    leave?.();
    name = null;
    opener = null;
    dispose = null;
    leave = null;
    aside.hidden = true;
    aside.removeAttribute('data-drawer');
    body.replaceChildren();
    mode();
    tell();
    if (focus) focusBack(back);
  }

  /**
   * Opens drawer `which` with `title`. `build(body)` fills the body and may
   * give a function that ends what it started; `onClose` is called when
   * the drawer closes or another replaces it.
   */
  function open(which, { title, build, opener: from = document.activeElement, onClose = null }) {
    if (name !== null) close({ focus: false });
    name = which;
    opener = from;
    leave = onClose;
    heading.textContent = title;
    closeButton.replaceChildren(`Close`, ' ', h('span', { class: 'kbd' }, 'Esc'));
    closeButton.setAttribute('aria-label', `Close ${title}`);
    aside.dataset.drawer = which;
    aside.hidden = false;
    dispose = build(body) ?? null;
    mode();
    tell();
    heading.focus();
  }

  aside.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === 'Tab' && phone.matches && name !== null) {
      const items = [...aside.querySelectorAll(FOCUSABLE)].filter((item) => !item.closest('[hidden]'));
      if (items.length === 0) return;
      const [first, last] = [items[0], items.at(-1)];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === heading)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  });

  return {
    open,
    close,
    toggle(which, options) {
      if (name === which) close();
      else open(which, options);
    },
    current: () => name,
    /** Calls `listener` with the name of the open drawer, or null, whenever that changes. */
    onChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
