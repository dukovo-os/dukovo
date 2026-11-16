// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Download by QR code: a one-time link to the report for a phone on the same
// network. admind keeps the text under the link for 10 minutes and gives it
// to the first request; the dialog draws the link as a QR code with the
// fingerprint to compare. Read when the button is pressed, with the QR code's
// own files.

import {
  asSentence, fill, h, icon, uniqueId,
} from '../../dom.js';
import { phoneLink } from '../../lib/qr-view.js';
import { head, openModal } from '../../shell/dialogs.js';
import { draftEnded } from './model.js';

/**
 * Opens the dialog for the draft `draft` whose text hashes to `sha256`.
 * `again()` drafts the report again: it is called after the dialog closes
 * when the person chose that. Resolves when the dialog has closed.
 */
export async function openPhone(view, { draft, sha256, again }) {
  const id = uniqueId('report-phone');
  const body = h('div', { class: 'rp-phone' });
  const errors = h('div', { class: 'rp-errors' });
  const status = h('p', { class: 'small', role: 'status' });
  const renew = h('button', { class: 'btn', type: 'button', hidden: true }, 'Make a new link');
  let link = null;

  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': `${id}-h` },
    h('form', { method: 'dialog' },
      head(`${id}-h`, 'info', 'Download by QR code'),
      h('div', { class: 'dialog-body' }, body, errors, status),
      h('div', { class: 'dialog-foot' }, renew, h('button', { class: 'btn default', type: 'submit', value: 'close', autofocus: true }, 'Close'))));

  /** Says what went wrong with the link, and lets the person try again. */
  function fail(error) {
    fill(body);
    renew.hidden = false;
    renew.textContent = 'Try again';
    if (!draftEnded(error)) {
      view.problem(error, { slot: errors, button: renew });
      return;
    }
    fill(errors, h('div', { class: 'banner warn', role: 'alert' }, icon('warn'), h('div', {},
      h('p', {}, `${asSentence(error.title)} Draft the report again to make a link.`),
      h('div', { class: 'btns' }, h('button', { class: 'btn', type: 'submit', value: 'again' }, 'Draft again')))));
    renew.hidden = true;
  }

  /** Asks admind for a link, and draws it. */
  async function make() {
    link?.stop();
    renew.hidden = true;
    status.textContent = '';
    fill(errors);
    fill(body, h('p', { class: 'note' }, 'Making the link…'));
    try {
      const answer = await view.api.post(`/api/1/reports/drafts/${draft}/phone`, { sha256 });
      if (view.signal.aborted) return;
      link = phoneLink({
        url: answer.url,
        fingerprint: answer.fingerprint_sha256,
        expires_ms: answer.expires_ms,
        signal: view.signal,
        onEnded: () => {
          status.textContent = 'The link has ended. Make a new one to show another code.';
          renew.textContent = 'Make a new link';
          renew.hidden = false;
        },
      });
      fill(body, link.element);
    } catch (error) {
      if (error?.name !== 'AbortError') fail(error);
    }
  }

  renew.addEventListener('click', make);
  view.signal.addEventListener('abort', () => dialog.close(''), { once: true });
  make();
  const how = await openModal(dialog);
  link?.stop();
  if (how === 'again' && !view.signal.aborted) await again();
}
