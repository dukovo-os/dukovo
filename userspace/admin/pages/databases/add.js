// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a database: the name, the role that owns it, the text and sorting, and
// who connects, with what happens drawn beside the form before anything is
// done. A reader sees the form with its button off. Pressing Add the database
// hands what was asked to add-run.js, which runs the steps and shows the
// password once; that file is read then.

import {
  busy, fill, h, icon,
} from '../../dom.js';
import {
  addArgs, allowArgs, DEFAULT_LOCALE, isSiteName, LOCALES, machineOf, nameFromSite, nameProblem, planRows, WAYS, wayWords,
} from '../../lib/database-model.js';
import { serverCard } from './cards.js';
import {
  accessFields, cardsField, selectField, textField,
} from './fields.js';
import {
  accountChoices, backupWords, guessMachine, leftOutWords, serverState,
} from './model.js';
import { makeRead } from './read.js';

/** How long the command waits after the last key. */
const TYPING_MS = 300;

/** `promise`'s answer, or null when it could not be had: what the form reads besides the databases is not needed to show it. */
const soft = (promise) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return null;
});

/** The crumbs of the page: Databases, or the site it was reached from. */
const crumbsOf = (site) => (site === ''
  ? [{ title: 'Apps', path: '/apps' }, { title: 'Databases', path: '/databases' }]
  : [{ title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }, { title: site, path: `/websites/${encodeURIComponent(site)}` }]);

/** The list of what happens, as the board draws it: an icon and the words of each row. */
function planList(rows) {
  return h('ul', { class: 'chk-list' }, rows.map((row) => {
    const mark = icon(row.icon);
    if (row.ok) mark.classList.add('ok');
    return h('li', {}, mark, h('span', row.ok ? { class: 'muted' } : {}, row.words));
  }));
}

export async function showAdd(view) {
  const admin = view.session.admin === true;
  const asked = view.query.get('site') ?? '';
  const site = isSiteName(asked) ? asked : '';
  view.head({ title: 'Add a database', crumbs: crumbsOf(site), commands: true });

  const [backups, certificates, accounts, machineAnswer] = await Promise.all([
    soft(view.api.get('/api/1/backups')),
    soft(view.api.get('/api/1/certificates')),
    soft(view.api.get('/api/1/accounts')),
    soft(view.api.get('/api/1/machine/name')),
  ]);
  if (view.signal.aborted) return;
  const machine = machineOf(machineAnswer) ?? guessMachine(view.host ?? '');
  const people = accountChoices(accounts);

  const errors = h('div', { class: 'db-errors' });
  const read = makeRead(view, { errors, place: false });
  const taken = () => new Set([...(read.data?.databases ?? []).map((one) => one.name), ...(read.data?.roles ?? []).map((one) => one.name)]);

  // ---- the fields ----

  const first = site === '' ? '' : nameFromSite(site);
  const name = textField({ label: 'Name', value: first });
  const owner = textField({ label: 'Owned by', value: first, hint: 'A new PostgreSQL role, with rights over this database only.' });
  const locale = selectField({
    label: 'Text and sorting', options: LOCALES.map((one) => ({ value: one.value, label: one.label })), value: DEFAULT_LOCALE, hint: LOCALES[0].note,
  });
  const account = selectField({
    label: 'Account',
    options: people.names.map((one) => ({ value: one, label: one })),
    hint: leftOutWords(people.left) ?? 'The role is named for it, so it owns the database.',
  });
  const access = accessFields({ certificates, machine });
  let ownerTouched = false;
  let nameTouched = false;
  name.input.addEventListener('input', () => {
    nameTouched = true;
    if (!ownerTouched) owner.input.value = name.input.value;
  });
  owner.input.addEventListener('input', () => {
    ownerTouched = true;
  });

  const peopleNote = people.names.length === 0 ? h('p', { class: 'small muted' }, 'No account can be chosen.') : null;
  const subs = {
    local: null,
    socket: h('div', { class: 'db-sub' }, account.element, peopleNote, h('p', { class: 'small muted' }, WAYS.find((way) => way.id === 'socket').note)),
    network: h('div', { class: 'db-sub' }, ...access.elements),
  };
  const ways = cardsField({
    legend: 'Who connects',
    value: 'local',
    choices: WAYS.map((way) => ({
      value: way.id,
      title: way.title,
      words: wayWords(way.id, first || 'the role'),
      sub: subs[way.id],
    })),
    onChange: () => refresh(),
  });

  // ---- what is asked ----

  /** What the fields hold, and what is wrong with it: `{problems, ...}`, `problems` being `{field: words}`. */
  function check() {
    const way = ways.value();
    const wanted = name.input.value.trim();
    const role = way === 'socket' ? account.input.value : owner.input.value.trim();
    const used = taken();
    const problems = {};
    const nameWhy = nameProblem(wanted) || (used.has(wanted) ? `A database or a role named ${wanted} is there already.` : '');
    if (nameWhy) problems.name = nameWhy;
    if (way === 'socket') {
      if (role === '') problems.account = 'Choose the account.';
      else if (used.has(role)) problems.account = `A PostgreSQL role named ${role} is there already.`;
    } else {
      const ownerWhy = nameProblem(role, 'role') || (used.has(role) ? `A database or a role named ${role} is there already.` : '');
      if (ownerWhy) problems.owner = ownerWhy;
    }
    const asked = access.check();
    if (way === 'network') Object.assign(problems, asked.problems);
    return {
      problems, way, name: wanted, owner: role, found: asked.found, locale: locale.input.value, certificate: asked.certificate,
    };
  }

  /** The arguments of the calls, for a form without a problem. */
  const callsOf = (now) => ({
    add: addArgs({
      name: now.name, owner: now.owner, account: now.owner, locale: now.locale, site, way: now.way,
    }),
    allow: now.way === 'network' ? allowArgs(now.name, now.found.networks, now.certificate) : null,
  });

  // ---- what is drawn from them ----

  const plan = h('div', { class: 'db-plan-rows' });
  const runs = h('div', { class: 'db-runs', hidden: true });
  const serverSlot = h('div', { class: 'db-server-slot' });
  const note = h('p', { class: 'small muted', role: 'status' });
  const add = h('button', { class: 'btn default', type: 'button', 'data-action': 'add' }, 'Add the database');
  let timer = null;

  function drawCommands(now) {
    clearTimeout(timer);
    runs.hidden = view.prefs?.commands !== 'shown';
    if (runs.hidden) return;
    timer = setTimeout(() => {
      const calls = Object.keys(now.problems).length === 0 ? callsOf(now) : null;
      fill(runs, h('h3', {}, 'This runs'), calls === null
        ? h('p', { class: 'muted' }, 'The commands show when every field is right.')
        : [view.api.panel('databases.add', calls.add), calls.allow && view.api.panel('databases.allow', calls.allow)]);
    }, TYPING_MS);
  }

  function refresh() {
    const now = check();
    const socket = now.way === 'socket';
    owner.input.disabled = socket;
    if (socket && account.input.value !== '') owner.input.value = account.input.value;
    else if (!ownerTouched && !socket) owner.input.value = name.input.value;
    access.update();
    ways.describe((id) => wayWords(id, now.owner || 'the role'));
    locale.note.textContent = LOCALES.find((one) => one.value === locale.input.value)?.note ?? '';
    const label = LOCALES.find((one) => one.value === now.locale)?.label ?? '';
    plan.replaceChildren(planList(planRows({
      name: now.name || 'the database', owner: now.owner || 'the role', account: now.owner || 'the account', way: now.way, locale: label, networks: now.found.networks, certificate: now.certificate,
    }, read.data?.server ?? null)));
    const state = read.data ? serverState(read.data.server) : null;
    add.disabled = !admin || (state !== null && state !== 'on');
    note.textContent = admin ? '' : 'Only an administrator can add a database.';
    drawCommands(now);
  }

  const keep = backupWords(backups);
  const backupCard = h('section', { class: 'card', 'aria-labelledby': 'db-backups-h' },
    h('h2', { id: 'db-backups-h' }, 'Backups'),
    h('ul', { class: 'chk-list' }, h('li', {}, icon(keep.covered === false ? 'warn' : 'check'), h('span', {}, keep.words))),
    keep.covered === false && h('p', { class: 'small' }, h('a', { href: '/snapshots/backups' }, 'Add a backup place')));

  const form = h('div', { class: 'db-form' },
    h('section', { class: 'card', 'aria-labelledby': 'db-database-h' },
      h('h2', { id: 'db-database-h' }, 'Database'),
      h('div', { class: 'db-grid' }, name.element, owner.element, locale.element)),
    h('section', { class: 'card', 'aria-labelledby': 'db-who-h' }, h('h2', { id: 'db-who-h' }, 'Who connects'), ways.element),
    backupCard);

  const back = site === '' ? null : h('a', { class: 'btn', href: `/websites/${encodeURIComponent(site)}` }, 'Back');
  const side = h('section', { class: 'card db-side', 'aria-labelledby': 'db-plan-h' },
    h('h2', { id: 'db-plan-h' }, 'What happens'),
    plan, runs,
    h('div', { class: 'btns' }, add, back, h('a', { class: 'btn', href: '/databases' }, 'Cancel')),
    note);

  const content = h('div', { class: 'db-add' }, form, side);
  const run = h('div', { class: 'db-run-slot' });
  view.main.append(h('div', { class: 'db-page' },
    h('p', { class: 'muted' }, `A PostgreSQL 19 database and the role that owns it${site === '' ? '' : `, for ${site}`}. Snapshots and backups copy it with /shared.`),
    serverSlot, errors, content, run));

  // ---- pressing it ----

  /** Puts each fault under its field, the first last, so that the focus lands on the first. */
  function fault(problems) {
    const fields = {
      name, owner, account, networks: access.networks, anywhere: access.anywhere,
    };
    for (const key of Object.keys(fields).reverse()) if (problems[key]) fields[key].fault(problems[key]);
  }

  add.addEventListener('click', () => busy([add], async () => {
    const now = check();
    if (Object.keys(now.problems).length > 0) {
      fault(now.problems);
      return;
    }
    const { runAdd } = await import('./add-run.js');
    const calls = callsOf(now);
    content.hidden = true;
    runAdd(view, {
      host: run,
      name: now.name,
      role: now.owner,
      way: now.way,
      site,
      machine,
      certificate: now.certificate,
      add: calls.add,
      allow: calls.allow,
      again: () => {
        run.replaceChildren();
        content.hidden = false;
        name.input.focus();
        read.read();
      },
    });
  }));

  // ---- the read, which says what is taken and whether PostgreSQL runs ----

  for (const field of [name, owner, account, access.networks, access.certificate, access.anywhere, locale]) field.input.addEventListener('input', refresh);
  for (const field of [account, access.certificate, locale]) field.input.addEventListener('change', refresh);
  view.onPrefs(refresh);
  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
  read.on(({ read: data }) => {
    serverSlot.replaceChildren();
    if (data !== null && serverState(data.server) !== 'on') serverSlot.append(serverCard(view, data, () => read.read()));
    // The name that a site gives is made new again, now that it is known what is taken.
    if (site !== '' && !nameTouched) {
      name.input.value = nameFromSite(site, taken());
      if (!ownerTouched) owner.input.value = name.input.value;
    }
    refresh();
  });
  refresh();
  await read.load();
}
