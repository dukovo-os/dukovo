// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Certificate card of Add a website: the ways the site may have its
// certificate (Let's Encrypt, this machine's own, one that is here, none),
// drawn again as the names change, with the terms and the email for Let's
// Encrypt, a shorter name when the first name is too long to be a
// certificate's, and the choice of one that is here.

import { fill, h } from '../../dom.js';
import {
  CERTIFICATE_NAME_MOST, certificateChoices, certificateNameOf, certificateNameProblem, certificateOf,
} from '../../lib/website-model.js';
import { checkField, cardsField, selectField, textField } from './fields.js';

/** What the email is for: Let's Encrypt keeps it, and this server warns of an end by itself. */
const EMAIL_HINT = 'Let\'s Encrypt keeps it; it no longer mails notices of expiry, and this server warns you itself. Left empty, the account has none.';

/**
 * The card. `certificates` is `cert list --json`'s answer, `preset` the
 * certificate the page was sent here to use, `onChange()` is called when
 * anything of it changes. Gives `{element, update(names), value(), setTerms(url),
 * faults(names)}`: `value()` is `{how, certificate, there, short, email,
 * agreed}`, and `faults(names)` gives `{terms, short}` for what is wrong.
 */
export function makeCertificateCard({ certificates, preset, onChange }) {
  const terms = checkField({ label: 'I agree to Let\'s Encrypt\'s terms of service' });
  const termsLabel = terms.element.querySelector('label span');
  const email = textField({ label: 'Contact email (optional)', hint: EMAIL_HINT });
  email.input.setAttribute('type', 'email');
  email.input.classList.remove('mono');
  const short = textField({ label: 'A shorter name for the certificate', hint: `The first name is longer than ${CERTIFICATE_NAME_MOST} characters, which a certificate's name cannot be.` });
  const there = selectField({ label: 'The certificate', options: [{ value: '', label: '' }] });
  const letsencrypt = h('div', { class: 'web-sub' }, terms.element, email.element, short.element);
  const here = h('div', { class: 'web-sub' }, there.element);
  const body = h('div', { class: 'web-card-body' });
  const element = h('section', { class: 'card', id: 'certificate', 'aria-labelledby': 'certificate-h' }, h('h2', { id: 'certificate-h' }, 'Certificate'), body);
  let how = null;
  let names = [];
  let choices = null;

  /** The place the terms are linked from, once `certificate.check` has given the link. */
  function setTerms(url) {
    fill(termsLabel, url ? h('a', { href: url, rel: 'noopener', target: '_blank' }, 'I agree to Let\'s Encrypt\'s terms of service') : 'I agree to Let\'s Encrypt\'s terms of service');
  }

  function draw() {
    choices = certificateChoices({ names, list: certificates, preset });
    const usable = (id) => choices.options.some((one) => one.id === id && !one.off);
    if (how === null || !usable(how)) how = choices.initial;
    short.element.hidden = (names[0] ?? '').length <= CERTIFICATE_NAME_MOST;
    const chosen = there.input.value;
    fill(there.input, choices.here.map((one) => h('option', { value: one.name }, `${one.name} (${(one.names ?? []).join(', ')})`)));
    there.input.value = choices.here.some((one) => one.name === chosen) ? chosen : (choices.here.find((one) => one.name === preset) ?? choices.here[0])?.name ?? '';
    const subs = { letsencrypt, there: here };
    const cards = cardsField({
      legend: 'How the website gets its certificate',
      value: how,
      choices: choices.options.map((one) => ({
        value: one.id, title: one.title, words: one.words, off: one.off, why: one.why, warn: one.warn, sub: subs[one.id],
      })),
      onChange: (next) => {
        how = next;
        onChange();
      },
    });
    fill(body, cards.element);
  }

  for (const field of [terms, email, short, there]) {
    field.input.addEventListener('input', () => onChange());
    field.input.addEventListener('change', () => onChange());
  }

  return {
    element,
    setTerms,
    update(next) {
      names = next;
      draw();
    },
    value() {
      return {
        how: how ?? 'none',
        certificate: certificateOf({
          how, names, there: there.input.value, short: short.input.value,
        }),
        there: there.input.value,
        short: short.input.value.trim(),
        email: email.input.value,
        agreed: terms.input.checked,
      };
    },
    /** What is wrong with what the card holds for `names`, as `{terms, short}` (each empty when right). */
    faults() {
      if (how !== 'letsencrypt') return { terms: '', short: '' };
      const name = certificateNameOf(names, short.input.value);
      const problem = name === null ? `Give the certificate a name of at most ${CERTIFICATE_NAME_MOST} characters.` : certificateNameProblem(name);
      return {
        terms: terms.input.checked ? '' : 'Agree to Let\'s Encrypt\'s terms of service to get a certificate.',
        short: problem,
      };
    },
    fields: { short, there, email },
    termsBox: terms,
  };
}
