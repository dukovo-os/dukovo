// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A site's page: its names, what it serves, its HTTPS and what to do about it
// (get a certificate, use one that is here, turn HTTPS off), its database, its
// file with a hand edit shown and kept, the chart of its visits, and Remove. Anyone reads it; an
// administrator changes it. A change is a list of steps, drawn on the page, so
// that each says its state.

import { fill, h, icon } from '../../dom.js';
import { hrefOf as databaseHref } from '../../lib/database-route.js';
import {
  certificateChoices, certificateSteps, COUNTED_WORDS, GETTING_WORDS, gettingOne, handEditWords, httpsLocked, httpsOf, httpsStep, NOT_COUNTED_WORDS, offStep, READER_WORDS, removeStep, serveWords, stateOf,
} from '../../lib/website-model.js';
import { remembered } from '../databases/seen.js';
import { makeCheck } from './check.js';
import {
  certificateLink, dateOf, hasPage, readWebsites,
} from './read.js';

/** A banner of `kind` with `iconName`, the `words` and what else is in `more`. */
const banner = (kind, iconName, words, ...more) => h('div', { class: `banner ${kind}` }, icon(iconName), h('div', {}, h('p', {}, words), ...more));

/** The pill's colour for a tone of a state. */
const PILL = { ok: 'ok', warn: 'warn', bad: 'bad' };

export async function showSite(view, name) {
  const admin = view.session.admin === true;
  const errors = h('div', { class: 'web-errors' });
  const body = h('div', { class: 'web-site' });
  const flow = h('div', { class: 'web-flow' });
  view.main.append(h('div', { class: 'web-page' }, errors, body, flow));
  let data = null;
  let headed = false;

  const siteOf = () => data?.read?.sites.find((one) => one.name === name) ?? null;
  const check = admin ? makeCheck(view, { errors, onResult: () => draw() }) : null;

  /** The head: it names the site by its first name, so it is drawn once the site is read. */
  function drawHead(site) {
    if (headed) return;
    headed = true;
    const state = site ? stateOf(site, { nginx: data.read.nginx, phpFpm: data.read.phpFpm }) : null;
    view.head({
      title: site?.names?.[0] ?? name,
      crumbs: [{ title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }],
      commands: true,
      pill: state ? { words: state.words, kind: PILL[state.tone] } : null,
      buttons: admin && site ? [check.button] : [],
    });
  }

  // ---- the steps a change is ----

  /** Runs `steps` in the flow card; `after(result)` is called when they have ended. */
  async function run(title, steps, { onDone } = {}) {
    const { runSteps } = await import('../../lib/steps.js');
    const heading = h('h2', { id: 'web-flow-h', tabindex: '-1' }, title);
    const list = runSteps(view, steps);
    fill(flow, h('section', { class: 'card', 'aria-labelledby': heading.id }, heading, list.element));
    heading.focus();
    const result = await list.finished;
    if (result.left || view.signal.aborted) return;
    const failed = result.failed === null ? null : steps[result.failed];
    const again = failed && failed.id !== 'get' ? h('button', { class: 'btn default', type: 'button', 'data-action': 'try-again' }, 'Try again') : null;
    again?.addEventListener('click', () => run(title, steps.slice(result.failed), { onDone }));
    fill(flow.querySelector('section'), heading, list.element,
      failed && banner('warn', 'warn', failed.words.stands, again && h('div', { class: 'btns' }, again)));
    if (result.done) await onDone?.();
    else await reread();
  }

  /** Reads the sites again and draws them. */
  async function reread() {
    fill(errors);
    const got = await readWebsites(view, errors, { wanted: true });
    if (got === null) return;
    data = got;
    draw();
  }

  // ---- what a button does ----

  async function getCertificate(site) {
    const { askGet } = await import('./site-dialogs.js');
    const chosen = await askGet(view, { site });
    if (chosen === null) return;
    const names = site.names?.length > 0 ? site.names : [site.name];
    await run(`Getting a certificate for ${names[0]}`, certificateSteps({ names, certificate: chosen.certificate, email: chosen.email }), { onDone: reread });
  }

  async function use(site, first = null) {
    const { askUse } = await import('./site-dialogs.js');
    const certificate = await askUse(view, { site, certificates: data.certificates, first });
    if (certificate === null) return;
    await run(`Serving ${site.names?.[0] ?? site.name} over HTTPS`, [httpsStep(site.name, certificate)], { onDone: reread });
  }

  async function turnOff(site) {
    const { askOff } = await import('./site-dialogs.js');
    if (await askOff(view, { site, sites: data.read.sites })) await run(`Turning HTTPS off for ${site.names?.[0] ?? site.name}`, [offStep(site.name)], { onDone: reread });
  }

  async function remove(site) {
    const { askRemove } = await import('./site-dialogs.js');
    const chosen = await askRemove(view, { site, sites: data.read.sites });
    if (chosen === null) return;
    await run(`Removing ${site.names?.[0] ?? site.name}`, [removeStep(site.name, chosen.files)], {
      onDone: async () => {
        fill(body, banner('ok', 'check', `${site.names?.[0] ?? site.name} is removed. ${chosen.files ? 'Its folder is removed too.' : (site.folder ? 'Its folder is kept.' : '')}${site.https && site.certificate ? ' Its certificate stays in certd.' : ''}`,
          h('div', { class: 'btns' }, h('a', { class: 'btn default', href: '/websites' }, 'Back to the websites'))));
      },
    });
  }

  // ---- what is drawn ----

  const button = (words, onclick, kind = 'small') => {
    const one = h('button', { class: `btn ${kind}`.trim(), type: 'button' }, words);
    one.addEventListener('click', () => onclick(one));
    return one;
  };

  /** The HTTPS row: what the site has, and what an administrator may do about it. */
  function httpsRow(site) {
    const now = httpsOf(site, data.certificates, dateOf(view));
    const locked = httpsLocked(site);
    const options = certificateChoices({ names: site.names ?? [site.name], list: data.certificates });
    const can = (id) => options.options.some((one) => one.id === id && !one.off);
    const buttons = [];
    // While certd tries to get the certificate there is nothing to ask for: the words stand in place of the button.
    const getting = gettingOne(site, data.wanted);
    if (admin && locked === '') {
      if (site.https) buttons.push(button('Turn HTTPS off…', () => turnOff(site)));
      else {
        if (can('letsencrypt') && !getting) buttons.push(button('Get a certificate', () => getCertificate(site), 'small default'));
        if (can('machine')) buttons.push(button('Use this machine\'s certificate', () => use(site, 'this-machine')));
        if (options.here.length > 0) buttons.push(button('Use one that is here…', () => use(site, options.here[0].name)));
      }
    }
    return h('dd', { 'data-row': 'https' },
      h('span', { class: now.tone === 'warn' ? 'web-tone-warn' : null }, now.words),
      now.certificate && [' · ', certificateLink(now.certificate)],
      now.ends && h('small', {}, `Ends ${now.ends}${now.renews ? `, certd renews it from ${now.renews}` : ''}`),
      admin && locked !== '' && h('small', { class: 'muted' }, locked),
      getting && h('p', { class: 'small', 'data-row': 'getting' }, GETTING_WORDS),
      buttons.length > 0 && h('div', { class: 'btns' }, buttons));
  }

  /** The database row: those a read of the databases found for this site, else the way to make one. */
  function databaseRow(site) {
    const made = (remembered(view.session.name)?.databases ?? []).filter((one) => one.site === site.name);
    const make = admin && hasPage('/databases/add') && h('a', { class: 'btn small', href: `/databases/add?site=${encodeURIComponent(site.name)}` }, 'Make its database');
    return h('dd', { 'data-row': 'database' },
      made.length > 0 ? made.flatMap((one, at) => [at > 0 && ', ', h('a', { href: databaseHref(one.name) }, one.name)]) : h('span', { class: 'muted' }, 'None that this page knows of'),
      make && h('div', { class: 'btns' }, make));
  }

  /** The file row: its path, a hand edit said in words, and its text on request. */
  function fileRow(site) {
    const edit = handEditWords(site);
    const text = h('pre', { class: 'cmdtext web-file-text', tabindex: '0', hidden: true, 'aria-label': `Text of ${site.file}` }, site.text ?? '');
    const show = button('Show the file', (one) => {
      text.hidden = !text.hidden;
      one.textContent = text.hidden ? 'Show the file' : 'Hide the file';
    });
    return h('dd', { 'data-row': 'file', class: 'web-file' },
      h('span', { class: 'mono' }, site.file),
      h('small', { class: edit.tone === 'warn' ? 'web-tone-warn' : null }, edit.words),
      edit.by && hasPage('/reliability') && h('small', {}, 'It is also a row of ', h('a', { href: '/reliability' }, 'What changed'), ', with Keep it and Put back.'),
      h('div', { class: 'btns' }, show), text);
  }

  /** The card of the visits: the chart of the 30 days once the site has been counted, which is read when it is drawn, else what is waited for. */
  function visitsCard(site) {
    const slot = h('div', { class: 'web-visits' });
    if (site.visits) {
      import('./visits.js').then(({ drawVisits }) => drawVisits(slot, site.visits))
        .catch(() => fill(slot, h('p', { class: 'small muted' }, 'The chart could not be drawn.')));
    }
    return h('section', { class: 'card', 'aria-labelledby': 'web-visits-h', 'data-card': 'visits' },
      h('h2', { id: 'web-visits-h' }, 'Visits'),
      site.visits ? [slot, h('p', { class: 'small muted' }, COUNTED_WORDS)] : h('p', { class: 'muted' }, NOT_COUNTED_WORDS));
  }

  function draw() {
    if (data?.read === null || data === null) {
      fill(body, h('p', { class: 'muted' }, data === null ? 'Reading the website…' : 'The websites could not be read.'));
      return;
    }
    const site = siteOf();
    drawHead(site);
    if (site === null) {
      fill(body, h('p', { class: 'muted' }, `There is no website called ${name}.`), h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/websites' }, 'Back to the websites')));
      return;
    }
    const scheme = site.https ? 'https' : 'http';
    const found = (check?.problems()[site.name] ?? []);
    const state = stateOf(site, { nginx: data.read.nginx, phpFpm: data.read.phpFpm });
    const problems = [...(site.problems ?? []), ...found].filter((one, at, all) => all.findIndex((other) => other.words === one.words) === at);
    fill(body,
      !admin && h('p', { class: 'muted' }, READER_WORDS),
      state.tone === 'bad' && banner('bad', 'warn', state.words),
      problems.map((one) => banner('warn', 'warn', one.words, one.id && h('p', { class: 'small mono' }, one.id))),
      check?.element,
      h('section', { class: 'card', 'aria-label': 'The website' },
        h('dl', { class: 'rows' },
          h('dt', {}, 'Names'),
          h('dd', { 'data-row': 'names' }, h('ul', { class: 'web-names' }, (site.names?.length > 0 ? site.names : [site.name]).map((one) => h('li', {}, h('a', { href: `${scheme}://${one}/`, target: '_blank', rel: 'noopener' }, one))))),
          h('dt', {}, 'Serves'),
          h('dd', { 'data-row': 'serves' }, serveWords(site), admin && view.session.terminal === true && hasPage('/terminal') && [' · ', h('a', { href: '/terminal' }, 'Open the Terminal')]),
          h('dt', {}, 'HTTPS'), httpsRow(site),
          h('dt', {}, 'Its database'), databaseRow(site),
          h('dt', {}, 'Its file'), fileRow(site))),
      visitsCard(site),
      admin && h('div', { class: 'btns' }, button('Remove…', () => remove(site), 'danger')),
      h('div', { class: 'web-foot' }, view.api.inline('websites.list', {})));
  }

  draw();
  await reread();
}
