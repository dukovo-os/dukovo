// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Draws an error where it belongs (design 9.1): under the field it is
// about, as a banner in the page's head, as the unlock dialog, as a
// countdown, as plain words in place, or as the error view of the Errors
// board. problem-model.js picks the place and holds the words.
//
// `cx` is what the shell knows: `{ session, slot, signal, path, time,
// hasPage(id), checkSession() }`. `where` is what the page that asked for
// the call knows: `{ slot, fields: {name: input}, input, button, retry,
// buttons, first }`.

import { ApiError } from '../api.js';
import { asSentence, fill, h, icon, uniqueId } from '../dom.js';
import { askToUnlock } from '../unlock.js';
import { COPIED, copyText, PRESS_CTRL_C } from './copy.js';
import {
  detailsText, errorParts, HELPER_WORDS, inPlaceWords, PLACE, placeOf, waitWords,
} from './problem-model.js';

/** An error that is not admind's, as an ApiError, so that the error view can say it. */
function asApiError(error) {
  if (error instanceof ApiError) return error;
  return new ApiError({
    title: 'This page could not be shown',
    detail: error?.message ?? 'Something went wrong while the page was loading.',
    fix: 'Reload the page. If it keeps happening, report a problem.',
  });
}

/** Shows `error` in the place it belongs; an error that is not admind's is said as one. */
export function showProblem(thrown, where, cx) {
  const error = asApiError(thrown);
  const slot = where.slot ?? cx.slot;
  switch (placeOf(error)) {
    case PLACE.field:
      if (!underField(error, where)) errorView(error, where, cx, slot);
      break;
    case PLACE.banner:
      bannerFor(error, where, cx, slot);
      break;
    case PLACE.unlock:
      unlockThenRetry(where, cx, slot);
      break;
    case PLACE.wait:
      countdown(error, where, cx, slot);
      break;
    case PLACE.helper:
      helperStopped(where, cx, slot);
      break;
    case PLACE.inPlace:
      slot.replaceChildren(h('p', { class: 'note', role: 'status' }, inPlaceWords(error)));
      break;
    case PLACE.signIn:
    case PLACE.offline:
      // api.js tells the shell, which shows the sign-in page or the banner
      // of 4.2; there is nothing for the page to add.
      break;
    default:
      errorView(error, where, cx, slot);
  }
}

/**
 * Puts the words of `error` under the input it is about, marks the input
 * and moves focus to it. Gives false when the page named no such input.
 */
function underField(error, where) {
  const input = where.fields?.[error.field] ?? where.input;
  if (!input) return false;
  const field = input.closest('.field') ?? input.parentElement;
  const id = `${input.id || uniqueId('field')}-problem`;
  field.querySelector(`#${CSS.escape(id)}`)?.remove();
  const words = [error.detail, error.fix].filter(Boolean).join(' ') || asSentence(error.title);
  field.append(h('span', { id, class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, words)));
  input.classList.add('bad');
  input.setAttribute('aria-invalid', 'true');
  input.setAttribute('aria-describedby', [input.getAttribute('aria-describedby'), id].filter(Boolean).join(' '));
  input.addEventListener('input', () => {
    document.getElementById(id)?.remove();
    input.classList.remove('bad');
    input.removeAttribute('aria-invalid');
    input.setAttribute(
      'aria-describedby',
      (input.getAttribute('aria-describedby') ?? '').split(' ').filter((one) => one && one !== id).join(' '),
    );
    if (input.getAttribute('aria-describedby') === '') input.removeAttribute('aria-describedby');
  }, { once: true });
  input.focus();
  return true;
}

/** A banner of `kind` with an `iconName`, `words` and `buttons`. */
function banner(kind, iconName, words, buttons = []) {
  return h('div', { class: `banner ${kind}`, role: 'status' },
    icon(iconName),
    h('div', {}, h('p', {}, words), buttons.length > 0 && h('div', { class: 'btns' }, ...buttons)));
}

/** admind's words for an account that may look and not change, with a way to the administrators when the console has that page. */
function bannerFor(error, where, cx, slot) {
  const buttons = cx.hasPage('accounts')
    ? [h('a', { class: 'btn small', href: '/accounts' }, 'Who are the administrators?')]
    : [];
  const words = [h('b', {}, asSentence(error.title)), ' ', [error.detail, error.fix].filter(Boolean).join(' ')];
  slot.replaceChildren(banner('warn', 'lock', words, buttons));
}

async function unlockThenRetry(where, cx, slot) {
  const unlocked = await askToUnlock(cx.session);
  if (!unlocked) return;
  if (where.retry) {
    where.retry();
  } else {
    slot.replaceChildren(banner('info', 'info', 'The console is unlocked. Press the button again.'));
  }
}

/** "Too many tries. Try again in 40 s", counting down, with the button that tried held for that long. */
function countdown(error, where, cx, slot) {
  let left = error.retryAfter ?? 10;
  const words = h('span', {}, waitWords(left));
  const shown = banner('warn', 'warn', words);
  slot.replaceChildren(shown);
  if (where.button) where.button.disabled = true;
  const ticker = setInterval(() => {
    left -= 1;
    if (left > 0 && shown.isConnected) {
      words.textContent = waitWords(left);
      return;
    }
    clearInterval(ticker);
    shown.remove();
    if (where.button) where.button.disabled = false;
  }, 1000);
  cx.signal?.addEventListener('abort', () => clearInterval(ticker), { once: true });
}

/** The helper for the session stopped: the shell looks at the session, and the page's call is tried again. */
function helperStopped(where, cx, slot) {
  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  const shown = banner('warn', 'warn', HELPER_WORDS, [again]);
  again.addEventListener('click', async () => {
    shown.remove();
    await cx.checkSession();
    where.retry?.();
  });
  slot.replaceChildren(shown);
}

/**
 * The error view of the Errors board. It is an alert only when it comes
 * after the person did something: on a page's first load (`where.first`)
 * it is a plain region, so that the page is not read out as an alarm.
 */
export function errorView(error, where, cx, slot) {
  const parts = errorParts(error);
  const id = uniqueId('err');
  const details = detailsText(error, { path: cx.path, at: cx.time.full(Math.floor(Date.now() / 1000)) });
  const status = h('span', { class: 'small', role: 'status' });
  const copyButton = h('button', { class: 'btn small', type: 'button' }, icon('copy'), 'Copy the details');
  const shown = h('pre', { class: 'sr-only', 'aria-hidden': 'true' }, details);
  copyButton.addEventListener('click', async () => {
    status.textContent = (await copyText(details, shown)) ? COPIED : PRESS_CTRL_C;
  });
  const rows = [
    h('dt', {}, 'What happened'),
    h('dd', {}, parts.happened ?? 'The console did not say more.'),
  ];
  if (parts.todo || where.buttons?.length) {
    rows.push(
      h('dt', {}, 'What to do'),
      h('dd', {}, parts.todo && h('p', {}, parts.todo), where.buttons?.length > 0 && h('div', { class: 'btns' }, ...where.buttons)),
    );
  }
  const attributes = where.first
    ? { class: 'err', 'aria-labelledby': id }
    : { class: 'err', role: 'alert', 'aria-labelledby': id };
  fill(slot, h('section', attributes,
    h('h2', { id }, icon('warn'), parts.title),
    h('dl', {}, ...rows),
    parts.ownWords && h('details', {},
      h('summary', {}, 'The program\'s own message'),
      h('pre', { class: 'raw' }, parts.ownWords)),
    h('div', { class: 'err-foot' },
      parts.id && h('span', { class: 'mono' }, parts.id),
      parts.helpPath && h('a', { href: parts.helpPath }, parts.helpWords),
      copyButton,
      cx.hasPage('report') && h('a', { href: '/report' }, 'Report a problem'),
      status),
    shown));
}
