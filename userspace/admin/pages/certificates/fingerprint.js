// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The panel of a certificate's fingerprint: the SHA-256 fingerprint in the
// form browsers show it, the names, who made it, the dates and the services
// that use it, with a button that saves the certificate for a browser or a
// phone to trust. The page reads this file when Show fingerprint is pressed.

import { h } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { saveText } from '../../lib/save-file.js';
import {
  dateWords, fingerprintWords, fromWords, secondsOf, serviceHref,
} from './model.js';

/** Opens the panel of the certificate `name`, which is read first. */
export async function openFingerprint(ctx, name) {
  const { view } = ctx;
  let shown;
  try {
    shown = await view.api.get(`/api/1/certificates/${encodeURIComponent(name)}`);
  } catch (error) {
    view.problem(error, { slot: ctx.list.errors });
    return false;
  }
  const fingerprint = fingerprintWords(shown.fingerprint_sha256);
  const value = h('span', { class: 'mono certs-fingerprint' }, fingerprint);
  const status = h('span', { class: 'small muted', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button' }, 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(fingerprint, value)) ? COPIED : PRESS_CTRL_C;
  });
  const from = secondsOf(shown.not_before);
  const until = secondsOf(shown.not_after);
  const users = shown.used_by ?? [];
  const rows = [
    ['SHA-256 fingerprint', [value, ' ', copy, ' ', status]],
    ['Names', (shown.names ?? []).join(', ')],
    ['Made by', fromWords(shown).words],
    ['Good', from !== null && until !== null ? `from ${dateWords(from, view.time)} until ${dateWords(until, view.time)}` : null],
    ['Used by', users.length > 0 ? users.flatMap((one, at) => [at > 0 && ', ', h('a', { href: serviceHref(one) }, one)]) : 'no service'],
  ].filter(([, words]) => words !== null);
  return openFormDialog(view, {
    title: name,
    icon: 'certs',
    lead: ['Compare this fingerprint with the one a browser shows. Save the certificate for a browser or phone to trust it.'],
    fields: [h('dl', { class: 'rows' }, rows.flatMap(([label, words]) => [h('dt', {}, label), h('dd', {}, words)]))],
    label: 'Save the certificate',
    cancel: 'Close',
    submit() {
      saveText(`${name}.crt`, shown.chain_pem ?? '');
      return false;
    },
  });
}
