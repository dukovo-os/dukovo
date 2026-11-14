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
  element.append(...children.flat().filter((child) => child != null && child !== false));
  return element;
}

/** An icon of the console's set, drawn in the text's colour. */
export function icon(name) {
  return h('i', { class: `ic ic-${name}`, 'aria-hidden': 'true' });
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
      h('b', {}, error.title), ' ', [error.detail, error.fix, wait].filter(Boolean).join(' '),
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
