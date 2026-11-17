// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the buttons of a row of the list do. The page reads this file when
// the first of them is pressed: renewing, making this server's own certificate
// anew, revoking, removing, serving one for the console, asking again after a
// failure, stopping the tries for one that could not be got, and the dialogs
// that show or upload one.

import { h } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { fingerprintWords } from './model.js';
import {
  forgetWords, MAKE_ANEW_WORDS, REVOKE_REASONS, REVOKE_WORDS, SERVE_TRY_SECONDS,
} from './row-model.js';

/** Whether the refusal `error` says that the certificate was proved by DNS, which certd cannot repeat. */
const byDnsWords = (error) => /proved by DNS/i.test(`${error?.title ?? ''} ${error?.detail ?? ''} ${error?.fix ?? ''}`);

/** Renews `row`, and says when it is good until. Gives whether it was renewed. */
async function renew(ctx, row, one, { slot = ctx.list.errors, anew = false } = {}) {
  const { view } = ctx;
  const result = await view.api.run('certificate.renew', { name: row.name }, {
    button: one,
    where: { slot },
    words: {
      unlock: anew ? 'make this server\'s certificate anew' : `renew ${row.name}`,
      running: anew ? 'Making the certificate anew' : `Renewing ${row.name}`,
      busy: anew ? 'Making it…' : 'Renewing…',
      done: anew ? 'The certificate is made anew.' : `${row.name} is renewed.`,
    },
  });
  if (!result.ok) {
    if (byDnsWords(result.error)) {
      ctx.byDns.add(row.name);
      ctx.list.draw();
    }
    return false;
  }
  await ctx.reread();
  const now = ctx.rows().find((entry) => entry.name === row.name);
  if (anew) ctx.list.say(now ? `Made anew. Its fingerprint is ${fingerprintWords(now.certificate.fingerprint_sha256)}.` : 'Made anew.');
  else ctx.list.say(now ? `Renewed: good until ${now.end.date}` : 'Renewed.');
  return true;
}

/** The dialog that makes this server's own certificate anew. */
function makeAnew(ctx, row) {
  return openFormDialog(ctx.view, {
    title: 'Make the certificate anew',
    icon: 'certs',
    lead: [MAKE_ANEW_WORDS],
    label: 'Make it anew',
    command: { action: 'certificate.renew', args: { name: row.name } },
    submit: (confirm, slot) => renew(ctx, row, confirm, { slot, anew: true }),
  });
}

/** The dialog that revokes a certificate: the reason, and what it does not do. */
function revoke(ctx, row) {
  const { view } = ctx;
  const choices = REVOKE_REASONS.map((reason, at) => {
    const input = h('input', { type: 'radio', name: 'revoke-reason', value: reason.id, checked: at === 0 });
    return { id: reason.id, input, label: h('label', { class: 'radio-card' }, input, h('span', {}, reason.words)) };
  });
  const reason = () => choices.find((one) => one.input.checked)?.id ?? REVOKE_REASONS[0].id;
  return openFormDialog(view, {
    title: `Revoke ${row.name}`,
    icon: 'warn',
    lead: [REVOKE_WORDS],
    fields: [h('fieldset', { class: 'choices' }, h('legend', {}, 'Why'), h('div', { class: 'certs-reasons' }, choices.map((one) => one.label)))],
    label: 'Revoke it',
    danger: true,
    command: () => ({ action: 'certificate.revoke', args: { name: row.name, reason: reason() } }),
    async submit(confirm, slot) {
      const result = await view.api.run('certificate.revoke', { name: row.name, reason: reason() }, {
        button: confirm,
        where: { slot },
        words: {
          unlock: `revoke ${row.name}`, running: `Revoking ${row.name}`, busy: 'Revoking…', done: `${row.name} is revoked.`,
        },
      });
      if (!result.ok) return false;
      await ctx.reread();
      return true;
    },
  });
}

/** The dialog that removes a certificate no service uses. */
function remove(ctx, row) {
  const { view } = ctx;
  return openFormDialog(view, {
    title: `Remove ${row.name}`,
    icon: 'warn',
    lead: [
      `This server forgets ${row.name} and its key. No service uses it.`,
      'Removing does not tell the authority: revoke a certificate first if its key may have been seen by someone else.',
    ],
    label: 'Remove it',
    danger: true,
    command: { action: 'certificate.remove', args: { name: row.name } },
    async submit(confirm, slot) {
      const result = await view.api.run('certificate.remove', { name: row.name }, {
        button: confirm,
        where: { slot },
        words: {
          unlock: `remove ${row.name}`, running: `Removing ${row.name}`, busy: 'Removing…', done: `${row.name} is removed.`,
        },
      });
      if (!result.ok) return false;
      await ctx.reread();
      return true;
    },
  });
}

/** The dialog that stops certd trying to get a certificate: `record` is its row of the list. */
function forget(ctx, record) {
  const { view } = ctx;
  return openFormDialog(view, {
    title: `Forget ${record.name}`,
    icon: 'warn',
    lead: [forgetWords(record.name)],
    label: 'Forget it',
    danger: true,
    command: { action: 'certificate.forget', args: { name: record.name } },
    async submit(confirm, slot) {
      const result = await view.api.run('certificate.forget', { name: record.name }, {
        button: confirm,
        where: { slot },
        words: {
          unlock: `stop trying to get ${record.name}`, running: `Forgetting ${record.name}`, busy: 'Forgetting…', done: `certd no longer tries to get ${record.name}.`,
        },
      });
      if (!result.ok) return false;
      await ctx.reread();
      return true;
    },
  });
}

/** Has the console serve `row`'s certificate for a while, which the banner of the trial lets the person keep. */
async function serve(ctx, row, one) {
  const { view } = ctx;
  const result = await view.api.run('console.certificate', { name: row.name, try: SERVE_TRY_SECONDS }, {
    button: one,
    where: { slot: ctx.list.errors },
    words: {
      unlock: 'change the certificate this console serves', running: 'Changing the console\'s certificate', busy: 'Changing…', done: `This console serves ${row.name}'s certificate.`,
    },
  });
  if (!result.ok) return;
  ctx.data.serving = row.name;
  const trials = await ctx.trials();
  await trials?.console.begin({ seconds: SERVE_TRY_SECONDS });
  ctx.list.draw();
  await ctx.reread();
}

/** What a button of a row does; `one` is the button. */
export async function act(ctx, id, row, one) {
  ctx.list.say('');
  switch (id) {
    case 'show':
      return (await import('./fingerprint.js')).openFingerprint(ctx, row.name);
    case 'renew':
      return renew(ctx, row, one);
    case 'renew-dns':
    case 'check-again':
      ctx.get?.fill({ names: row.names, dns: id === 'renew-dns' || ctx.byDns.has(row.name) });
      return ctx.show('get');
    case 'replace':
      return (await import('./upload.js')).openUpload(ctx, { name: row.name, replace: true });
    case 'make-anew':
      return makeAnew(ctx, row);
    case 'revoke':
      return revoke(ctx, row);
    case 'remove':
      return remove(ctx, row);
    case 'forget':
      return forget(ctx, row);
    case 'use':
    case 'again':
      return serve(ctx, row, one);
    default:
      return undefined;
  }
}
