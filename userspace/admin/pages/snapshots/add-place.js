// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a backup place (design 5.8): where the backups go (a plugged-in disk,
// an SSH server or S3 storage, which only the command adds), its name, how
// often it is backed up to and checked, and how it keeps them. The fields
// are one piece, `makeAddForm`, which the dialog of the page and the step
// "Copy your data off this server" both draw. Adding a place makes its
// recovery key and answers with it once: `keepKey` shows it as a secret and
// then offers the first backup. The page reads this file when Add a backup
// place is first pressed.

import {
  busy, errorSlot, fill, h, showError, uniqueId,
} from '../../dom.js';
import { bytesWords } from '../../lib/size-words.js';
import { ask, head, openModal } from '../../shell/dialogs.js';
import { copyBox } from './copy-box.js';
import {
  addArgs, EVERY, S3_WORDS, sshKeyOf, suggestName, VERIFY,
} from './form-model.js';
import { backupPlaces, removableVolumes } from './model.js';

/** How long the command under the form waits after a field changes, so that typing is one call. */
const TYPING_MS = 400;

/** The three places backups go, with how each is said. */
const KINDS = Object.freeze([
  { value: 'disk', title: 'A plugged-in disk', words: 'A USB disk or another removable disk that is mounted.' },
  { value: 'ssh', title: 'An SSH server', words: 'Another machine that you can sign in to over SSH, such as a NAS.' },
  { value: 's3', title: 'S3 storage', words: 'A bucket at a provider, or one on your network.' },
]);

/** A select named by `label` with `options` `[{value, label}]`, and the one chosen. */
function selectField(id, label, options, chosen, hint = null) {
  const select = h('select', { id, class: 'input', 'aria-describedby': hint ? `${id}-hint` : null },
    options.map((one) => h('option', { value: one.value, selected: one.value === chosen }, one.label)));
  return { select, field: h('div', { class: 'field' }, h('label', { for: id }, label), select, hint && h('span', { id: `${id}-hint`, class: 'hint' }, hint)) };
}

/**
 * The fields of Add a backup place. `storage` and `backups` are what `GET
 * /api/1/storage` and `GET /api/1/backups` gave (either may be null);
 * `volume`, one of `removableVolumes`' rows, opens it with that disk
 * chosen and "By hand"; `command` is false where another part shows the
 * command that runs. Gives `{element, errors, fields, args, check, after}`:
 * `args()` gives the action's arguments or throws an error whose `words`
 * say what is wrong, and shows nothing; `check()` does the same for the
 * dialog, putting each fault under its field; `after(answer)` shows the
 * recovery key that the answer holds, once, and offers the first backup.
 */
export function makeAddForm(view, {
  storage = null, backups = null, volume = null, command = true, started = () => {},
} = {}) {
  const id = uniqueId('snap-add');
  const taken = backupPlaces(backups).map((place) => place.name);
  const found = removableVolumes(storage, backups);
  const start = volume ? found.find((one) => one.mount === volume.mount) ?? volume : null;
  const volumes = start && !found.includes(start) ? [start, ...found] : found;
  let where = volumes.length > 0 ? 'disk' : 'ssh';
  let touched = false;
  let suggested = '';
  let pending = null;
  let keyState = 'idle';

  // ---- Where ----
  const errors = h('div', { class: 'snap-errors' });
  const panes = {};
  const cards = {};
  const radios = KINDS.map((kind) => {
    const disabled = kind.value === 'disk' && volumes.length === 0;
    const input = h('input', { type: 'radio', name: `${id}-where`, value: kind.value, checked: kind.value === where, disabled });
    cards[kind.value] = h('label', { class: `radio-card${kind.value === where ? ' on' : ''}` }, input,
      h('span', {}, h('b', {}, kind.title), h('span', { class: 'small muted' }, disabled ? 'No removable disk is plugged in and mounted.' : kind.words)));
    input.addEventListener('change', () => choose(kind.value));
    return cards[kind.value];
  });

  const diskSelect = h('select', { id: `${id}-disk`, class: 'input', 'aria-describedby': `${id}-disk-hint` },
    volumes.map((one) => h('option', { value: one.mount, selected: one.mount === start?.mount },
      `${one.label} (${one.disk})${one.free === null ? '' : `, ${bytesWords(one.free)} free`}`)));
  const diskSlot = errorSlot(`${id}-disk-error`);
  panes.disk = h('div', { class: 'field' }, h('label', { for: `${id}-disk` }, 'Disk'), diskSelect,
    h('span', { id: `${id}-disk-hint`, class: 'hint' }, 'The backups go in the folder dukovo-backup on it.'), diskSlot);

  const address = h('input', {
    id: `${id}-address`, class: 'input mono', type: 'text', placeholder: 'backup@192.168.1.20:/srv/dukovo', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-address-hint`,
  });
  const addressSlot = errorSlot(`${id}-address-error`);
  const keyBox = h('div', { class: 'snap-key' });
  panes.ssh = h('div', { class: 'snap-pane' },
    h('div', { class: 'field' }, h('label', { for: `${id}-address` }, 'Server and folder'), address,
      h('span', { id: `${id}-address-hint`, class: 'hint' }, 'As user@host:/folder. For another port, write ssh://user@host:2222/folder.'), addressSlot),
    keyBox);

  const s3Slot = errorSlot(`${id}-s3-error`);
  panes.s3 = h('div', { class: 'snap-pane' }, h('p', {}, S3_WORDS),
    copyBox('elevate backup add NAME s3://HOST/BUCKET', 'The command for S3 storage'),
    h('p', { class: 'small muted' }, 'Put the keys in AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY first. backup(8) says more.'), s3Slot);

  // ---- How ----
  const name = h('input', { id: `${id}-name`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-name-hint` });
  const nameSlot = errorSlot(`${id}-name-error`);
  const every = selectField(`${id}-every`, 'Back up', EVERY, volume ? 'hand' : 'day');
  const verify = selectField(`${id}-verify`, 'Check by restoring', VERIFY, 'week', 'Restores the newest backups and compares them with the files.');
  const format = selectField(`${id}-format`, 'How the backups are kept', [
    { value: 'btrfs', label: 'As Btrfs streams (the default)' },
    { value: 'restic', label: 'As a restic repository' },
  ], 'btrfs', 'Btrfs streams suit a Dukovo server or a disk. A restic repository suits a share that a NAS keeps.');
  const more = h('details', { class: 'snap-more' }, h('summary', {}, 'More'), format.field);
  const commandBox = h('div', { class: 'snap-command' });

  const element = h('div', { class: 'snap-add' },
    errors,
    h('fieldset', { class: 'choices' }, h('legend', {}, 'Where do the backups go?'), radios),
    panes.disk, panes.ssh, panes.s3,
    h('div', { class: 'field' }, h('label', { for: `${id}-name` }, 'Name'), name,
      h('span', { id: `${id}-name-hint`, class: 'hint' }, '1 to 32 letters, digits, - and _. It is how this place is named on the page and in commands.'), nameSlot),
    h('div', { class: 'snap-fields' }, every.field, verify.field),
    more,
    command && commandBox);

  /** What the fields say now, as `addArgs` reads it. */
  const read = () => ({
    where,
    volume: volumes.find((one) => one.mount === diskSelect.value) ?? null,
    address: address.value,
    name: name.value,
    every: every.select.value,
    verify: verify.select.value,
    format: format.select.value,
  });

  /** The slot and field that a fault of the address belongs under. */
  const target = () => ({ disk: [diskSlot, diskSelect], ssh: [addressSlot, address], s3: [s3Slot, null] })[where];

  const clear = () => {
    for (const slot of [diskSlot, addressSlot, s3Slot, nameSlot]) slot.replaceChildren();
    for (const input of [diskSelect, address, name]) {
      input.classList.remove('bad');
      input.removeAttribute('aria-invalid');
      input.removeAttribute('aria-describedby');
    }
    diskSelect.setAttribute('aria-describedby', `${id}-disk-hint`);
    address.setAttribute('aria-describedby', `${id}-address-hint`);
    name.setAttribute('aria-describedby', `${id}-name-hint`);
  };

  /** Puts a name in the name field when its owner has not typed one. */
  function suggest() {
    if (touched) return;
    suggested = suggestName({ where, volume: read().volume, address: address.value });
    name.value = suggested;
  }

  let timer = null;
  /** Shows the command that the fields make, once they are right, after the typing stops. */
  function showCommand() {
    if (!command) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      const made = addArgs(read(), taken);
      fill(commandBox, ...(made.args ? [h('h3', {}, 'This runs'), view.api.panel('backups.add', made.args)] : []));
    }, TYPING_MS);
  }
  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });

  /** Reads this machine's SSH key, which a server must be given first. */
  async function loadKey() {
    if (keyState === 'loading' || keyState === 'done') return;
    keyState = 'loading';
    fill(keyBox, h('p', { class: 'muted', role: 'status' }, 'Reading this machine\'s SSH key…'));
    const done = await view.api.run('backups.ssh-key', {}, {
      words: { unlock: 'show this machine\'s SSH key', busy: 'Reading…' }, where: { slot: errors },
    }).catch(() => ({ ok: false }));
    if (view.signal.aborted) return;
    const key = done.ok ? sshKeyOf(done.result) : null;
    if (key) {
      keyState = 'done';
      fill(keyBox, h('p', {}, 'Add this key to the server\'s authorized_keys first, on the account the backups sign in as.'), copyBox(key, 'This machine\'s SSH key'));
      return;
    }
    keyState = 'failed';
    const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
    again.addEventListener('click', loadKey);
    fill(keyBox, h('p', { class: 'muted', role: 'status' }, done.ok ? 'The answer held no key.' : 'This machine\'s SSH key could not be read.'), h('div', { class: 'btns' }, again));
  }

  /** Shows the part of the form for `value`. */
  function choose(value) {
    where = value;
    for (const kind of KINDS) {
      panes[kind.value].hidden = kind.value !== value;
      cards[kind.value].classList.toggle('on', kind.value === value);
    }
    clear();
    suggest();
    if (value === 'ssh') loadKey();
    showCommand();
  }

  diskSelect.addEventListener('change', () => { clear(); suggest(); showCommand(); });
  address.addEventListener('input', () => { clear(); suggest(); showCommand(); });
  name.addEventListener('input', () => {
    touched = name.value !== suggested;
    nameSlot.replaceChildren();
    name.classList.remove('bad');
    name.removeAttribute('aria-invalid');
    showCommand();
  });
  for (const field of [every, verify, format]) field.select.addEventListener('change', showCommand);
  choose(where);

  return {
    element,
    errors,
    /** The fields a refusal of the server may name, by the argument's name. */
    fields: () => ({ name, ...(target()[1] ? { to: target()[1] } : {}) }),
    args() {
      const made = addArgs(read(), taken);
      if (made.errors) throw Object.assign(new Error('the form has mistakes'), { words: Object.values(made.errors)[0] });
      pending = made.args.name;
      return made.args;
    },
    check() {
      clear();
      const made = addArgs(read(), taken);
      if (made.errors) {
        // The name is shown last, so that the focus lands on the first field.
        if (made.errors.to) {
          const [slot, input] = target();
          showError(slot, { title: made.errors.to }, input);
        }
        if (made.errors.name) showError(nameSlot, { title: made.errors.name }, name);
        return null;
      }
      pending = made.args.name;
      return made.args;
    },
    async after(answer) {
      await keepKey(view, { name: pending, answer, started });
    },
  };
}

/**
 * Shows the recovery key that `answer` of `backups.add` holds, once, and,
 * when the key was kept, asks whether to back up to `name` now. `started(id,
 * action, args)` is told of the task that starts. Gives true when the key
 * was kept.
 */
export async function keepKey(view, { name, answer, started = () => {} }) {
  const { showRecoveryKey } = await import('./key.js');
  if (!(await showRecoveryKey(view, { name, answer, added: true })) || view.signal.aborted) return false;
  const choice = await ask({
    title: `Back up to ${name} now?`,
    words: 'The first backup sends everything to it, which can take a while. It runs as a task, and you can leave this page meanwhile.',
    choices: [{ label: 'Back up now', value: 'now', kind: 'default', safe: true }, { label: 'Later', value: 'later' }],
  });
  if (choice === 'now') {
    const done = await view.api.run('backups.run', { name }, { words: { unlock: `back up to ${name}`, running: `Backing up to ${name}`, busy: 'Starting…' } }).catch(() => ({ ok: false }));
    if (typeof done.task === 'number') started(done.task, 'backups.run', { name });
  }
  return true;
}

/**
 * Opens Add a backup place as a dialog, with the options of `makeAddForm`.
 * Resolves with the name of the place that was added, after its key was
 * shown, or null.
 */
export async function openAdd(view, options = {}) {
  const form = makeAddForm(view, options);
  const headingId = uniqueId('snap-add-h');
  const add = h('button', { class: 'btn default', type: 'submit' }, 'Add');
  const cancel = h('button', { class: 'btn', type: 'button' }, 'Cancel');
  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': headingId },
    h('form', { method: 'dialog', novalidate: true },
      head(headingId, 'plus', 'Add a backup place'),
      h('div', { class: 'dialog-body' }, form.element),
      h('div', { class: 'dialog-foot' }, cancel, add)));
  let added = null;
  let working = false;
  cancel.addEventListener('click', () => dialog.close('cancel'));
  // What the server is answering must not be left: its answer holds the key, shown once.
  dialog.addEventListener('cancel', (event) => {
    if (working) event.preventDefault();
  });
  dialog.querySelector('form').addEventListener('submit', async (event) => {
    event.preventDefault();
    const args = form.check();
    if (!args) return;
    working = true;
    const done = await busy([cancel], () => view.api.run('backups.add', args, {
      words: { unlock: 'add a backup place', running: `Adding ${args.name}`, done: `Added ${args.name}.`, busy: 'Adding…' },
      button: add,
      where: { slot: form.errors, fields: form.fields() },
    })).catch(() => ({ ok: false }));
    working = false;
    if (!done.ok) return;
    added = { name: args.name, answer: done.result };
    dialog.close('added');
  });
  await openModal(dialog);
  if (added === null) return null;
  const { name, answer } = added;
  added = null;
  await keepKey(view, { name, answer, started: options.started });
  return name;
}
