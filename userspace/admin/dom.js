// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the console's views share: building elements, showing an error under
// a field, and keeping a button from being pressed twice. Text always goes
// in as text, never as markup, so nothing a server or a person sends can
// become a tag.

/**
 * Makes an element. An attribute that is false, null or undefined is left
 * out, one that is true is set empty (`required`), and one that starts with
 * "on" is an event listener. A child that is null or false is left out.
 */
export function h(tag, attributes = {}, ...children) {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (value === false || value == null) continue;
    if (name.startsWith('on')) element.addEventListener(name.slice(2), value);
    else element.setAttribute(name, value === true ? '' : value);
  }
  return fill(element, ...children);
}

/** Replaces what `element` holds with `children`, which may be nested lists, leaving out those that are null or false. */
export function fill(element, ...children) {
  element.replaceChildren(...children.flat(Infinity).filter((child) => child != null && child !== false));
  return element;
}

/** An icon of the console's set, drawn in the text's colour. */
export function icon(name) {
  return h('i', { class: `ic ic-${name}`, 'aria-hidden': 'true' });
}

const sheets = new Map();

/**
 * Links the style sheet at `href` once. Gives a promise that resolves when it
 * has loaded, at once when it did already, and when the load fails, so that
 * what waits for it is shown bare and not left out.
 */
export function sheet(href) {
  if (!sheets.has(href)) {
    const link = h('link', { rel: 'stylesheet', href });
    sheets.set(href, new Promise((done) => {
      link.addEventListener('load', done);
      link.addEventListener('error', done);
    }));
    document.head.append(link);
  }
  return sheets.get(href);
}

/** `words` as a sentence: admind's titles name what happened and carry no full stop of their own. */
export function asSentence(words) {
  return /[.!?:]$/.test(words) ? words : `${words}.`;
}

/** The seconds to wait that an error asks for, in words. */
export function waitInWords(seconds) {
  if (seconds < 60) return `${seconds} ${seconds === 1 ? 'second' : 'seconds'}`;
  const minutes = Math.ceil(seconds / 60);
  return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
}

/** The element under a field that an error goes into: announced when it is filled, and able to take focus. */
export function errorSlot(id) {
  return h('div', { id, class: 'msg', role: 'alert', tabindex: '-1' });
}

/**
 * Puts `error` into `slot`, made by `errorSlot`, and moves focus to it, so
 * that it is announced and a person at the keyboard lands on it. `input`,
 * when given, is marked as the field it is about. A 429 says when to try
 * again.
 */
export function showError(slot, error, input) {
  const wait = error.retryAfter == null ? '' : `Try again in ${waitInWords(error.retryAfter)}.`;
  slot.replaceChildren(
    icon('warn'),
    h('span', {},
      h('b', {}, asSentence(error.title)), ' ', [error.detail, error.fix, wait].filter(Boolean).join(' '),
      error.id && h('span', { class: 'mono muted' }, ` ${error.id}`)),
  );
  if (input) {
    input.classList.add('bad');
    input.setAttribute('aria-invalid', 'true');
    input.setAttribute('aria-describedby', slot.id);
  }
  slot.focus();
}

/** Takes the error away that `showError` put in `slot` for `input`. */
export function clearError(slot, input) {
  slot.replaceChildren();
  if (input) {
    input.classList.remove('bad');
    input.removeAttribute('aria-invalid');
    input.removeAttribute('aria-describedby');
  }
}

/**
 * Runs `work` with `buttons` disabled, so that a press while the server is
 * answering does nothing, and enables them again however it ends.
 */
export async function busy(buttons, work) {
  for (const button of buttons) button.disabled = true;
  try {
    return await work();
  } finally {
    for (const button of buttons) button.disabled = false;
  }
}

/** How long after a press is let go its click can still come: a finger's comes a little after a mouse's. */
export const CLICK_WAIT_MS = 400;

/**
 * Watches the presses of a pointer on `target`, the document: a press runs
 * from its pointerdown to the click that ends it, or to a moment after it
 * is let go when no click comes. Gives `afterPress(work)`, which runs
 * `work` at once when no press is under way, and as the press ends when
 * one is. `later` and `cancel` are the timer's, given for a test.
 */
export function pressWatch(target, { later = setTimeout, cancel = clearTimeout } = {}) {
  let pressing = false;
  let timer = null;
  const waiting = [];
  const wait = (ms) => {
    cancel(timer);
    timer = later(end, ms);
  };
  function end() {
    cancel(timer);
    timer = null;
    pressing = false;
    for (const work of waiting.splice(0)) work();
  }
  target.addEventListener('pointerdown', () => {
    cancel(timer);
    timer = null;
    pressing = true;
  }, true);
  target.addEventListener('pointerup', () => {
    if (pressing) wait(CLICK_WAIT_MS);
  }, true);
  target.addEventListener('pointercancel', () => {
    if (pressing) end();
  }, true);
  // In the bubble phase, so the page's own handlers have had the click first.
  target.addEventListener('click', () => {
    if (pressing) wait(0);
  });
  return {
    afterPress(work) {
      if (pressing) waiting.push(work);
      else work();
    },
  };
}

const presses = globalThis.document ? pressWatch(globalThis.document) : null;

/**
 * Runs `work`, which shows something that moves the page, once no press is
 * under way. A sentence that appears under a field as the person leaves it
 * moves what is below the field; when they leave it by pressing a button
 * below it, the button would move from under the pointer between the press
 * and its release, and the browser would give the click to whatever is
 * under the pointer then, so the press would do nothing. Waiting for the
 * press's click keeps the button where it was pressed.
 */
export function afterPress(work) {
  if (presses) presses.afterPress(work);
  else work();
}

/**
 * Gives focus back when a dialog, drawer or menu closes (design 4.6): to
 * `opener` while it is in the page, else to the element that took its
 * place (the one with its id, when the page drew it again), else to the
 * page's h1.
 */
export function focusBack(opener) {
  const back = opener?.isConnected ? opener : (opener?.id && document.getElementById(opener.id)) || document.querySelector('main h1');
  back?.focus();
}

let lastId = 0;

/** An `id` for an element of a view that is made more than once, such as a field the form builds. */
export function uniqueId(prefix) {
  lastId += 1;
  return `${prefix}-${lastId}`;
}

/**
 * The mark of Dukovo at `width` x `height`, in the two colourings the
 * stylesheet shows one of by theme.
 */
export function marks(width, height) {
  return ['dark', 'light'].map((theme) =>
    h('img', { class: `mark-${theme}`, src: `/mark-${theme}.svg`, alt: '', width, height }));
}
