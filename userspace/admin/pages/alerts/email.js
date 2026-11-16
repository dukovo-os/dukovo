// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Email card: the one email channel, set from the form and taken away
// with Remove. alerts.toml is the machine's and is never read back, so the
// form says what the status shows (the server, how bad, what is told of) and
// asks for the rest again.

import { fill, h } from '../../dom.js';
import { card, refuse } from './cards.js';

/** How long the command under the fields waits after the last key. */
const TYPING_MS = 400;

/** The Email card. `ctx` is the page's: `{view, load, test}`. */
export function makeEmail(ctx) {
  const email = card('email', 'Email');
  const form = h('div', {});
  const status = h('div', { role: 'status' });
  const command = h('div', {});
  fill(email.body, form, status);
  let fields = null;
  let shape;
  let timer = null;
  const { view } = ctx;
  view.signal.addEventListener('abort', () => { clearTimeout(timer); fields?.clear(); }, { once: true });

  const drawCommand = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const mistake = fields.check();
      fill(command, mistake ? h('p', { class: 'muted' }, 'The command shows when every field is right.') : [h('h3', {}, 'This runs'), view.api.panel('alerts.email', fields.shown())]);
    }, TYPING_MS);
  };

  async function save(button) {
    fill(email.errors);
    const mistake = fields.check();
    if (mistake) {
      refuse(email.errors, mistake.words, mistake.input);
      return;
    }
    const result = await view.api.run('alerts.email', fields.take(), {
      button,
      where: { slot: email.errors },
      words: { unlock: 'set up email alerts', running: 'Setting up email alerts', done: 'Email alerts are set up.', busy: 'Saving…' },
    });
    if (!result.ok) return;
    const test = h('button', { class: 'btn small', type: 'button' }, 'Send a test alert');
    test.addEventListener('click', () => ctx.test(test));
    fill(status, h('p', {}, 'Saved. ', test));
    ctx.load();
  }

  /** Draws the form for `row`, the email row of Where alerts go (null when none). The fields are read when the card is first drawn. */
  async function build(row) {
    const { makeEmailFields } = await import('./email-fields.js');
    if (view.signal.aborted) return;
    fields?.clear();
    fields = makeEmailFields(view, { channel: row?.channel ?? null });
    const save$ = h('button', { class: 'btn default', type: 'button' }, 'Save');
    save$.addEventListener('click', () => save(save$));
    const remove = row && h('button', { class: 'btn', type: 'button' }, 'Remove email');
    remove?.addEventListener('click', async () => {
      const { openRemove } = await import('./row-dialogs.js');
      if (await openRemove(view, row)) ctx.load();
    });
    fill(form,
      row ? h('p', { class: 'muted' }, 'Email is set up. Saving replaces its addresses, server and sign-in: they are not read back here.') : h('p', { class: 'muted' }, 'Send alerts to an email address through your mail provider.'),
      fields.element,
      h('div', { class: 'btns' }, save$, remove),
      command);
    fill(command);
    fields.element.addEventListener('input', drawCommand);
    fields.element.addEventListener('change', drawCommand);
  }

  return {
    element: email.element,
    /** Draws the form again when the email channel is new, gone or changed; `rows` are the rows of Where alerts go. */
    set(rows) {
      const row = rows.find((one) => one.kind === 'email') ?? null;
      const now = row ? JSON.stringify([row.channel.host, row.channel.severity, row.channel.cleared, row.channel.about]) : null;
      if (now === shape && shape !== undefined) return;
      shape = now;
      build(row);
    },
  };
}
