// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a website: the name, what it serves, its folder and who owns it, and
// the certificate, with what happens drawn beside the form before anything is
// done. A reader is told who adds a site, and the command. Adding hands the
// form to run.js, which runs the steps; Preview the files first opens
// preview.js. Both are read when their button is pressed.

import { fill, h, icon } from '../../dom.js';
import {
  allNames, folderFor, folderProblem, folderWords, KINDS, machineNamesOf, missingApps, namesOf, namesProblem, ownerChoices,
  PHP_APP, PORT_TRY_SECONDS, READER_WORDS, wwwDefault, wwwOf,
} from '../../lib/website-model.js';
import { checkPath, checkRows, isReport, termsLink } from '../certificates/get-model.js';
import { copyBox } from '../snapshots/copy-box.js';
import { makeCertificateCard } from './certificate-card.js';
import {
  areaField, cardsField, checkField, selectField, textField,
} from './fields.js';
import { makePlan, openFor } from './plan.js';
import { hasPage, soft } from './read.js';

/** What the page says it does. */
const INTRO = 'One step for the address, the certificate, the web server and the files. The firewall and backups are checked too; the page changes them only when you press their buttons.';

/** How long the checks of the names wait after the last key. */
const CHECK_MS = 600;

/** The mark of a check: its word, which says what the colour does. */
const MARKS = Object.freeze({
  ok: ['ok', 'Passed'], warn: ['warn', 'Warning'], bad: ['bad', 'Failed'], skip: ['off', 'Left out'],
});

/** A card: a heading that names it and its body. */
const card = (id, title, ...body) => h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` },
  h('h2', { id: `${id}-h` }, title), h('div', { class: 'web-body' }, ...body));

/** What a reader sees: who adds a site, and the command. */
function drawReader(view) {
  view.main.append(h('div', { class: 'web-page' },
    h('p', { class: 'muted' }, INTRO),
    h('div', { class: 'banner info' }, icon('info'), h('div', {}, h('p', {}, READER_WORDS), copyBox('elevate website add NAME', 'The command that adds a website'))),
    h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/websites' }, 'Back to the websites'))));
}

export async function showAdd(view) {
  view.head({
    title: 'Add a website', crumbs: [{ title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }], commands: true,
  });
  if (view.session.admin !== true) {
    drawReader(view);
    return;
  }

  const [sites, certificates, accounts, backups, snapshots, ssh] = await Promise.all([
    soft(view.api.get('/api/1/websites')),
    soft(view.api.get('/api/1/certificates')),
    soft(view.api.get('/api/1/accounts')),
    soft(view.api.get('/api/1/backups')),
    soft(view.api.get('/api/1/snapshots')),
    soft(view.api.get('/api/1/ssh')),
  ]);
  if (view.signal.aborted) return;
  const machineNames = machineNamesOf(certificates);
  const owners = ownerChoices(accounts, view.session.name);
  const asked = view.query;
  const preset = { certificate: asked.get('certificate'), name: asked.get('name') ?? '' };

  // ---- the fields ----

  const domain = textField({ label: 'Domain', hint: 'The name people type to reach the site, such as shop.example.org.', value: preset.name, placeholder: 'shop.example.org' });
  const www = checkField({ label: 'Also answer for www.', checked: false });
  const wwwLabel = www.element.querySelector('label span');
  const more = areaField({ label: 'More names', hint: 'One name a line. A website answers to at most 20 names.' });
  const moreLink = h('button', { class: 'btn small', type: 'button', 'data-action': 'more-names' }, 'More names');
  more.element.hidden = true;
  const checkList = h('ul', { class: 'chk-list web-name-checks', 'aria-label': 'The checks of the names' });
  const checkNote = h('p', { class: 'small muted', role: 'status' });
  const checkSlot = h('div', { class: 'web-errors' });

  const phpMissing = missingApps(sites, 'php').some((one) => one.id === PHP_APP);
  const database = checkField({ label: 'Also make a PostgreSQL database for it · next', checked: asked.get('database') === '1' });
  const phpNote = h('p', { class: 'small', hidden: !phpMissing });
  const kindSubs = { static: null, php: h('div', { class: 'web-sub' }, database.element, phpNote) };
  const kinds = cardsField({
    legend: 'What it serves',
    value: asked.get('kind') === 'php' ? 'php' : 'static',
    choices: KINDS.map((one) => ({
      value: one.id, title: one.title, words: one.words, sub: kindSubs[one.id],
    })),
    onChange: () => refresh(),
  });

  const folder = textField({ label: 'Folder', hint: `A folder under ${folderFor('')}`, placeholder: folderFor('NAME') });
  const folderNote = folder.note;
  const owner = selectField({ label: 'Who may change them', options: owners.map((one) => ({ value: one, label: one })), hint: 'The account that owns the folder.' });
  const ownerNote = owner.note;

  const certificate = makeCertificateCard({
    certificates, preset: preset.certificate, onChange: () => refresh(),
  });

  // ---- what is asked ----

  const sheets = new Map();
  let touched = { folder: false, www: false };
  let namesKey = null;
  let checkTimer = null;
  let checkLatest = 0;
  let banners = null;

  const domainNow = () => domain.input.value.trim().toLowerCase();
  const namesNow = () => allNames({ domain: domainNow(), www: www.input.checked, more: namesOf(more.input.value) });
  const kindNow = () => kinds.value() ?? 'static';

  /** What is wrong with each field, `{field: words}`: the names and the folder only once there is a name. */
  function problemsOf() {
    const problems = {};
    const list = namesNow();
    const first = domainNow();
    if (first === '') problems.domain = 'Give the website\'s domain, such as shop.example.org.';
    else if (namesProblem(list)) problems.domain = namesProblem(list);
    if (first !== '') {
      const why = folderProblem(folder.input.value.trim());
      if (why) problems.folder = why;
    }
    if (owner.input.value === '') problems.owner = 'Choose the account that owns the folder.';
    const faults = certificate.faults();
    if (faults.terms) problems.terms = faults.terms;
    if (faults.short) problems.short = faults.short;
    return problems;
  }

  /** The form as the page sends it, or null while the names are not right. */
  function formOf() {
    const list = namesNow();
    if (domainNow() === '' || namesProblem(list) !== '') return null;
    const now = certificate.value();
    return {
      names: list,
      kind: kindNow(),
      folder: folder.input.value.trim(),
      owner: owner.input.value,
      how: now.how,
      certificate: now.certificate,
      email: now.email,
    };
  }

  /** The apps this kind needs that are not installed, with what each sheet says. */
  function missingNow() {
    return missingApps(sites, kindNow()).map((one) => ({ ...one, sha256: sheets.get(one.id)?.sha256 ?? null }));
  }

  /** Reads the sheet of each app that is missing, once: its hash is what the install is held to. */
  function readSheets() {
    for (const app of missingApps(sites, kindNow())) {
      if (sheets.has(app.id)) continue;
      sheets.set(app.id, null);
      view.api.get(`/api/1/apps/catalog/${encodeURIComponent(app.id)}/sheet`).then((sheet) => {
        sheets.set(app.id, { sha256: typeof sheet?.sha256 === 'string' ? sheet.sha256 : null });
      }, () => {
        sheets.set(app.id, { sha256: null });
      }).then(() => { if (!view.signal.aborted) refresh(); });
    }
  }

  /** Why the add has to wait: an app's sheet is not in yet, or could not be read. */
  function waiting() {
    for (const app of missingNow()) {
      if (sheets.get(app.id) === null || sheets.get(app.id) === undefined) return `Reading the sheet of ${app.name}…`;
      if (app.sha256 === null) return `The sheet of ${app.name} could not be read, so it cannot be installed from here: read it in the apps first.`;
    }
    return '';
  }

  const get = () => {
    const form = formOf();
    const problems = problemsOf();
    const ready = form !== null && !problems.folder && !problems.owner;
    return {
      form, ready, reason: waiting(), missing: missingNow(),
    };
  };

  const plan = makePlan(view, {
    get,
    world: { backups, snapshots },
    onAdd: () => add(),
    onPreview: () => previewFiles(),
    openPort: (port, button) => openPort(port, button),
    onRead: () => drawNotes(),
  });

  // ---- what is drawn from them ----

  /** The firewall banner, made when a port is opened or a trial may run. */
  function trials() {
    banners ??= import('./trial.js').then(async ({ makeFirewallTrial }) => {
      const made = makeFirewallTrial(view, { onEnd: () => { plan.retest(); checkNow(); } });
      trialHost.append(made.element);
      await made.read();
      return made;
    }).catch(() => null);
    return banners;
  }

  function drawChecks(rows, note) {
    fill(checkList, (rows ?? []).map((row) => {
      const [tone, word] = MARKS[row.tone] ?? MARKS.warn;
      const open = row.button === 'open-port' && h('button', { class: 'btn small', type: 'button', 'data-action': 'open-port-80' }, 'Open port 80');
      if (open) open.addEventListener('click', () => openPort(80, open));
      return h('li', { 'data-id': row.id ?? row.key }, h('span', { class: `st ${tone}` }, word), ' ', h('span', {}, row.words, row.id && h('span', { class: 'mono muted' }, ` ${row.id}`)), open && [' ', open]);
    }));
    checkNote.textContent = note;
  }

  /** Asks the checks of the names, for names with a dot (a name of this machine's own is not looked up). */
  async function checkNow() {
    clearTimeout(checkTimer);
    checkLatest += 1;
    const mine = checkLatest;
    const form = formOf();
    fill(checkSlot);
    if (form === null) {
      drawChecks(null, '');
      return;
    }
    if (form.how === 'machine' || form.names.some((one) => !one.includes('.'))) {
      drawChecks(null, form.how === 'machine' ? 'A name of this machine is not looked up: it is served by this machine\'s own certificate.' : 'A name without a dot cannot lead anywhere on the internet, so it is not looked up.');
      return;
    }
    drawChecks(null, 'Checking…');
    try {
      const report = await view.api.get(checkPath(form.names, false));
      if (mine !== checkLatest || view.signal.aborted) return;
      if (!isReport(report)) throw new Error('The checks gave no answer.');
      certificate.setTerms(termsLink(report));
      drawChecks(checkRows(report, false), '');
    } catch (error) {
      if (mine !== checkLatest || view.signal.aborted) return;
      drawChecks(null, '');
      view.problem(error, { slot: checkSlot });
    }
  }

  /** Draws what depends on the fields: the www name, the folder's default, the apps, the certificate and what happens. */
  function refresh() {
    const first = domainNow();
    const other = wwwOf(first);
    www.element.hidden = other === null;
    fill(wwwLabel, `Also answer for ${other ?? 'www.NAME'}`);
    if (!touched.www) www.input.checked = wwwDefault(first, machineNames);
    if (!touched.folder) folder.input.value = first === '' ? '' : folderFor(first);
    const key = JSON.stringify(namesNow());
    if (key !== namesKey) {
      namesKey = key;
      certificate.update(namesNow());
      clearTimeout(checkTimer);
      checkTimer = setTimeout(checkNow, CHECK_MS);
    }
    const phpNow = kindNow() === 'php';
    const missingPhp = phpNow && phpMissing;
    phpNote.hidden = !missingPhp;
    fill(phpNote, 'Installs PHP-FPM first (net.php.php) · ', hasPage(`/apps/add/${PHP_APP}`) ? h('a', { href: `/apps/add/${PHP_APP}` }, 'Read its sheet') : 'its sheet is in the apps');
    drawNotes();
    readSheets();
    plan.refresh();
  }

  /** The words under the folder and its owner: what the plan found of the folder, and who may do what with the files. */
  function drawNotes() {
    folderNote.textContent = folderWords(plan.plan()?.folder) || `A folder under ${folderFor('')}`;
    const who = owner.input.value || 'The owner';
    ownerNote.textContent = `${who} owns the folder and changes its files${ssh?.running === true ? ', over SSH (sftp)' : ''}. nginx reads them${kindNow() === 'php' ? '; the site\'s PHP reads them and writes only in its uploads folder' : ''}.`;
  }

  // ---- pressing the buttons ----

  /** Puts each fault under its field, the first last, so that the focus lands on the first. */
  function fault(problems) {
    const fields = {
      domain, folder, owner, terms: certificate.termsBox, short: certificate.fields.short,
    };
    for (const key of Object.keys(fields).reverse()) if (problems[key]) fields[key].fault(problems[key]);
  }

  /** Opens `port` for a while, and asks again what depends on it. */
  async function openPort(port, button) {
    const opened = await openFor(view, port, { button, slot: plan.errors });
    if (!opened) return;
    const made = await trials();
    await made?.begin({ seconds: PORT_TRY_SECONDS });
    plan.retest();
    checkNow();
  }

  /** Adds the site: the form is checked, and the steps run. */
  async function add() {
    const problems = problemsOf();
    const form = formOf();
    if (form === null || Object.keys(problems).length > 0) {
      fault(problems);
      return;
    }
    const { runAdd } = await import('./run.js');
    content.hidden = true;
    runAdd(view, {
      host: runSlot,
      form,
      missing: missingNow(),
      database: database.input.checked && form.kind === 'php',
      again: () => {
        fill(runSlot);
        content.hidden = false;
        domain.input.focus();
      },
    });
  }

  /** Opens the files of the plan. */
  async function previewFiles() {
    const problems = problemsOf();
    if (formOf() === null || problems.domain || problems.folder || problems.owner) {
      fault({ domain: problems.domain, folder: problems.folder, owner: problems.owner });
      return;
    }
    const read = await plan.read(true);
    if (read === null) return;
    const { openPreview } = await import('./preview.js');
    await openPreview(read);
  }

  // ---- the page ----

  const trialHost = h('div', { class: 'web-trials' });
  const content = h('div', { class: 'web-add' },
    h('div', { class: 'web-form' },
      card('address', 'Address', domain.element, www.element, moreLink, more.element, h('div', { class: 'web-checks-box' }, h('h3', {}, 'The checks'), checkNote, checkList, checkSlot)),
      card('serves', 'What it serves', kinds.element),
      card('files', 'Files', folder.element, owner.element),
      certificate.element),
    plan.element);
  const runSlot = h('div', { class: 'web-run-slot' });
  view.main.append(h('div', { class: 'web-page' }, h('p', { class: 'muted' }, INTRO), trialHost, content, runSlot));

  moreLink.addEventListener('click', () => {
    more.element.hidden = !more.element.hidden;
    if (!more.element.hidden) more.input.focus();
  });
  domain.input.addEventListener('input', refresh);
  domain.input.addEventListener('change', () => {
    const problems = problemsOf();
    if (domainNow() !== '' && problems.domain) domain.fault(problems.domain);
    refresh();
  });
  more.input.addEventListener('input', refresh);
  www.input.addEventListener('change', () => {
    touched.www = true;
    refresh();
  });
  folder.input.addEventListener('input', () => {
    touched.folder = true;
    refresh();
  });
  owner.input.addEventListener('change', refresh);
  view.onPrefs(() => plan.refresh());
  view.signal.addEventListener('abort', () => clearTimeout(checkTimer), { once: true });

  refresh();
  // A trial that already runs is shown with the page.
  trials();
}
