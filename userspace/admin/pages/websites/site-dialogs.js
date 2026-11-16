// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The questions a site's page asks: which certificate to serve it with, whether
// to get one from Let's Encrypt, whether to turn HTTPS off and whether to remove
// the site, and its folder. Each says what the change does before it is made
// and shows the command while commands are shown. A dialog gives what was
// decided, or null when the person cancelled; the page runs the steps, so
// that each says its state. Read when the first is opened.

import { fill, h } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import {
  CERTIFICATE_NAME_MOST, certificateNameOf, certificateNameProblem, certificatesFor, folderBoxWords, getArgs, httpsArgs, machineNamesOf,
  offLead, removeArgs, removeLead, SELF,
} from '../../lib/website-model.js';
import { checkPath, checkRows, isReport, termsLink } from '../certificates/get-model.js';
import { checkField, selectField, textField } from './fields.js';

/** How the answer of a check is drawn in the dialog: each row's word, which says what the colour does. */
const WORDS = { ok: 'Passed', warn: 'Warning', bad: 'Failed', skip: 'Left out' };

/**
 * Asks whether to get a certificate for the names of `site` from Let's
 * Encrypt, once the terms are agreed. The names are checked first: what the
 * checks found is shown. Gives `{certificate, email}`, or null.
 */
export async function askGet(view, { site }) {
  const names = site.names?.length > 0 ? site.names : [site.name];
  const report = await view.api.get(checkPath(names, false)).catch(() => null);
  const rows = isReport(report) ? checkRows(report, false) : [];
  const url = termsLink(report);
  const terms = checkField({ label: 'I agree to Let\'s Encrypt\'s terms of service' });
  fill(terms.element.querySelector('label span'), url ? h('a', { href: url, rel: 'noopener', target: '_blank' }, 'I agree to Let\'s Encrypt\'s terms of service') : 'I agree to Let\'s Encrypt\'s terms of service');
  const email = textField({ label: 'Contact email (optional)', hint: 'Let\'s Encrypt keeps it; this server warns you itself when the certificate nears its end.' });
  email.input.setAttribute('type', 'email');
  const short = textField({ label: 'A shorter name for the certificate', hint: `The first name is longer than ${CERTIFICATE_NAME_MOST} characters, which a certificate's name cannot be.` });
  short.element.hidden = names[0].length <= CERTIFICATE_NAME_MOST;
  const checks = rows.length > 0 && h('ul', { class: 'chk-list web-name-checks', 'aria-label': 'The checks of the names' }, rows.map((row) => h('li', { 'data-id': row.id ?? row.key },
    h('span', { class: `st ${row.tone === 'skip' ? 'off' : row.tone}` }, WORDS[row.tone] ?? WORDS.warn), ' ', h('span', {}, row.words, row.id && h('span', { class: 'mono muted' }, ` ${row.id}`)))));
  const closed = rows.some((row) => row.button === 'open-port') && h('p', { class: 'small' }, h('a', { href: '/firewall' }, 'Open the firewall'), ' to let port 80 in, then try again.');
  let chosen = null;
  const argsNow = () => getArgs({
    names, certificate: certificateNameOf(names, short.input.value) ?? '', email: email.input.value,
  });
  const done = await openFormDialog(view, {
    title: `Get a certificate for ${names[0]}`,
    icon: 'certs',
    lead: [
      'Let\'s Encrypt checks that the names lead here, over port 80, then gives a certificate for 90 days; certd renews it. When it is got, the site is served over HTTPS and nginx restarts once: Spark holds its ports, so no visitor is turned away.',
      checks,
      closed,
    ].filter(Boolean),
    fields: [terms.element, email.element, short.element],
    label: 'Get the certificate',
    command: () => ({ action: 'certificate.get', args: argsNow() }),
    async submit() {
      if (!terms.input.checked) {
        terms.fault('Agree to Let\'s Encrypt\'s terms of service to get a certificate.');
        return false;
      }
      const name = certificateNameOf(names, short.input.value);
      const problem = name === null ? `Give the certificate a name of at most ${CERTIFICATE_NAME_MOST} characters.` : certificateNameProblem(name);
      if (problem) {
        short.fault(problem);
        return false;
      }
      chosen = { certificate: name, email: email.input.value };
      return true;
    },
  });
  return done ? chosen : null;
}

/**
 * Asks which certificate to serve `site` with: this machine's own when every
 * name is one of its own, or one that certd has and that names every name.
 * `certificates` is `cert list --json`'s answer and `first` the one to start
 * with. Gives the certificate's name, or null.
 */
export async function askUse(view, { site, certificates, first = null }) {
  const names = site.names?.length > 0 ? site.names : [site.name];
  const own = machineNamesOf(certificates);
  const options = [
    ...(names.every((name) => own.includes(name)) ? [{ value: SELF, label: 'This machine\'s own certificate' }] : []),
    ...certificatesFor(names, certificates).map((one) => ({ value: one.name, label: `${one.name} (${(one.names ?? []).join(', ')})` })),
  ];
  const which = selectField({ label: 'The certificate', options, value: options.some((one) => one.value === first) ? first : options[0]?.value });
  const note = h('p', { class: 'small muted' });
  const words = () => {
    note.textContent = which.input.value === SELF ? 'Browsers warn until they trust this machine\'s certificate; Certificates says how.' : 'It names every name of the site.';
  };
  words();
  which.input.addEventListener('change', words);
  let chosen = null;
  const done = await openFormDialog(view, {
    title: `Serve ${names[0]} over HTTPS`,
    icon: 'lock',
    lead: ['Serves the site over HTTPS with the certificate you choose, and sends visitors of http:// to https://. nginx restarts once, to give it the certificate and port 443: Spark holds its ports, so no visitor is turned away.', note],
    fields: [which.element],
    label: 'Serve it over HTTPS',
    command: () => ({ action: 'websites.https', args: httpsArgs(site.name, which.input.value) }),
    async submit() {
      chosen = which.input.value === '' ? null : which.input.value;
      return chosen !== null;
    },
  });
  return done ? chosen : null;
}

/** Asks whether to turn HTTPS off for `site`. `sites` is the list, for what that does to nginx. Gives whether to go on. */
export async function askOff(view, { site, sites }) {
  return openFormDialog(view, {
    title: `Turn HTTPS off for ${site.names?.[0] ?? site.name}?`,
    icon: 'warn',
    lead: offLead(site, sites),
    label: 'Serve it over HTTP only',
    danger: true,
    command: { action: 'websites.https', args: httpsArgs(site.name, null) },
    submit: async () => true,
  });
}

/**
 * Asks whether to remove `site`, and whether its folder goes too, with its
 * name typed first. Gives `{files}`, or null.
 */
export async function askRemove(view, { site, sites }) {
  const box = checkField({ label: folderBoxWords(site), hint: site.folder ? 'Everything in it is deleted, and cannot be brought back from this server unless a snapshot has it.' : null });
  let chosen = null;
  const done = await openFormDialog(view, {
    title: `Remove ${site.names?.[0] ?? site.name}?`,
    icon: 'warn',
    lead: removeLead(site, sites),
    fields: site.folder ? [box.element] : [],
    label: 'Remove the website',
    danger: true,
    typed: { name: site.name, label: `Type ${site.name} to go on`, why: 'The name is typed so that the website is not removed by a slip.' },
    command: () => ({ action: 'websites.remove', args: removeArgs(site.name, box.input.checked) }),
    async submit() {
      chosen = { files: Boolean(site.folder) && box.input.checked };
      return true;
    },
  });
  return done ? chosen : null;
}
