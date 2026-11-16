// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The connection settings of a database, for each way that programs connect to
// it: the host, port, database and user; where the password is; whether TLS is
// needed; and examples to copy. The password is never here: the examples read
// it from where the person keeps it. Shown after an add and on a database's
// page.

import { h } from '../../dom.js';
import {
  connectionSettings, rootFile, WAYS,
} from '../../lib/database-model.js';
import { saveText } from '../../lib/save-file.js';
import { copyBox } from '../snapshots/copy-box.js';

/** What the card says when no way is known, as for a database that was made some other way. */
const UNKNOWN = 'How programs connect to this database is not known: it was not made here, or its role cannot sign in.';

/** A way's title, as the Add page words it. */
const titleOf = (id) => WAYS.find((way) => way.id === id).title;

/** The button that saves the server's certificate, which other machines check the server's name against. */
function downloadButton(view, { machine, certificate }) {
  const name = rootFile(machine);
  const status = h('span', { class: 'small', role: 'status' });
  const button = h('button', { class: 'btn small', type: 'button' }, `Download the certificate (${name})`);
  button.addEventListener('click', async () => {
    try {
      const shown = await view.api.get(`/api/1/certificates/${encodeURIComponent(certificate)}`);
      if (typeof shown?.chain_pem !== 'string' || shown.chain_pem === '') throw new Error('empty');
      saveText(name, shown.chain_pem);
      status.textContent = `Saved as ${name}.`;
    } catch (error) {
      if (error?.name === 'AbortError') return;
      status.textContent = 'The certificate could not be read. Download it on the Certificates page.';
    }
  });
  return h('div', { class: 'btns' }, button, status);
}

/** One way: its rows and its examples. */
function wayOf(view, { id, database, role, machine, certificate }) {
  const found = connectionSettings({
    way: id, database, role, machine,
  });
  return h('section', { class: 'db-way', 'aria-label': titleOf(id) },
    h('h3', {}, titleOf(id)),
    h('dl', { class: 'rows' }, found.rows.flatMap((row) => [h('dt', {}, row.label), h('dd', { class: 'mono' }, row.value)])),
    id === 'network' && downloadButton(view, { machine, certificate }),
    found.examples.map((example) => h('div', { class: 'db-example' },
      h('b', { class: 'small' }, example.label),
      copyBox(example.text, `${example.label} example for ${database}`))));
}

/**
 * The card for `database`. `ways` are the ids of the ways that apply,
 * `role` the user, `machine` is `machineOf`'s (what other machines call this
 * one) and `certificate` the name of the certificate PostgreSQL shows them.
 * Gives the card's element.
 */
export function connectionCard(view, {
  database, role, ways, machine, certificate,
}) {
  const head = h('h2', { id: `db-settings-${database}` }, 'Connection settings');
  return h('section', { class: 'card db-settings', 'aria-labelledby': head.id },
    head,
    ways.length === 0
      ? h('p', { class: 'muted' }, UNKNOWN)
      : ways.map((id) => wayOf(view, {
        id, database, role, machine, certificate,
      })));
}
