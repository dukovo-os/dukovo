// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Copy settings to Git (design 7.3): the form that asks for a folder and runs
// `console.export` as the person, who reads the settings everyone may read
// and writes in their own folder. The page reads this file when the button
// is pressed.

import { fill, h } from '../../dom.js';

/**
 * The form: `{element, focus}`. `close()` is called when the person is done.
 * The folder is filled with `/users/NAME/HOST-settings`, which the person's
 * own account may write.
 */
export function makeExport(view, { close }) {
  const input = h('input', {
    id: 'rel-export-dir', class: 'input', type: 'text', value: `/users/${view.session.name}/${view.host || 'server'}-settings`,
    autocomplete: 'off', spellcheck: 'false', 'aria-describedby': 'rel-export-hint',
  });
  const run = h('button', { class: 'btn default', type: 'submit' }, 'Copy settings');
  const command = h('div', { class: 'rel-export-command' }, view.api.inline('console.export', { dir: input.value }));
  const output = h('div', { class: 'rel-export-output' });
  const heading = h('h2', { id: 'rel-export-h', tabindex: '-1' }, 'Copy settings to Git');
  const form = h('form', { class: 'rel-export-form', novalidate: true },
    h('p', { id: 'rel-export-hint', class: 'muted' },
      'Copies the files under /system/settings that everyone may read into this folder as a Git commit. Secrets are left out and listed in SECRETS.md.'),
    h('div', { class: 'field' }, h('label', { for: 'rel-export-dir' }, 'Folder'), input),
    h('div', { class: 'btns' }, run, h('button', { class: 'btn', type: 'button', onclick: close }, 'Close')),
    command, output);
  const element = h('section', { class: 'card', 'aria-labelledby': 'rel-export-h' }, heading, form);

  // A folder that was typed is the one the command shown names.
  input.addEventListener('change', () => command.replaceChildren(view.api.inline('console.export', { dir: input.value.trim() })));

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const dir = input.value.trim();
    output.replaceChildren();
    const result = await view.api.run('console.export', { dir }, {
      words: { done: 'Settings copied to Git.', unchanged: 'The settings are in Git already.', busy: 'Copying…' },
      button: run,
      where: { input, slot: output },
    });
    if (!result.ok) return;
    const lines = Array.isArray(result.result?.output) ? result.result.output : [];
    fill(output,
      h('p', { role: 'status' }, typeof result.task === 'number' ? `Copying to ${dir} runs as task ${result.task}.` : `Copied to ${dir}.`),
      lines.length > 0 && h('pre', { class: 'cmdtext', tabindex: '0', 'aria-label': 'What the command said' }, lines.join('\n')));
  });

  return { element, focus: () => input.focus() };
}
