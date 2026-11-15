// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Move to new hardware page gives Finish setting up and /setup
// (design 5.11): the step "Bring everything from your old machine", which a
// machine with no backup place and one person's account is offered and is
// never done, with a form that is a link to /move; and /setup showing that
// link as "Continue on its page" in place of Set up, while a step that has
// fields, whose `after` shows a recovery key, works as it did.

import assert from 'node:assert/strict';
import test from 'node:test';

import { h } from '../dom.js';
import { readSteps } from '../pages/overview/steps-read.js';
import { isNewInstall, peopleCount } from '../pages/snapshots/overview-model.js';
import { steps } from '../pages/snapshots/overview.js';
import { fakeCx } from './overview-fixtures.mjs';
import { usePage } from './fake-dom.mjs';
import { freshBackups, nasBackups } from './move-fixtures.mjs';

const owner = { name: 'owner', uid: 1000, full_name: '', home: '/users/owner', shell: '/system/commands/nukesh', admin: true, console: true, groups: ['admins', 'owner'], disabled: null, password: null };
const helper = { ...owner, name: 'helper', uid: 1001, admin: false, groups: ['helper'] };

const move = steps.find((step) => step.id === 'move');

/** The context of a part, with the answers the step reads. */
const cxOf = (backups, accounts) => fakeCx({ ...(backups === undefined ? {} : { '/api/1/backups': backups }), ...(accounts === undefined ? {} : { '/api/1/accounts': accounts }) });

// ---- the step ----

test('the step is Bring everything from your old machine, first, worth no points, and runs no action', () => {
  assert.equal(move.id, 'move');
  assert.equal(move.title, 'Bring everything from your old machine');
  assert.equal(move.order, 5);
  assert.equal(move.points, 0);
  assert.equal(move.action, null);
  assert.equal(typeof move.form, 'function');
  assert.ok(move.order < steps.find((step) => step.id === 'backups').order);
});

test('a machine with no backup place and one person\'s account is offered it', async () => {
  assert.equal(await move.available(cxOf(freshBackups(), [owner])), true);
  assert.equal(await move.available(cxOf(freshBackups(), { accounts: [owner] })), true, 'the object that holds the list is read as the list');
});

test('a machine that has a backup place, or other accounts, or none that can be counted, is not offered it', async () => {
  assert.equal(await move.available(cxOf(nasBackups(), [owner])), false, 'it has a backup place');
  assert.equal(await move.available(cxOf(freshBackups(), [owner, helper])), false, 'it has two accounts');
  assert.equal(await move.available(cxOf(freshBackups(), [])), false);
  assert.equal(await move.available(cxOf(freshBackups(), { error: 'x' })), false, 'an answer that is not a list');
  assert.equal(await move.available(cxOf(freshBackups())), false, 'the accounts could not be read');
  assert.equal(await move.available(cxOf(undefined, [owner])), false, 'the backup places could not be read');
});

test('the step is never done, and says what it is for', async () => {
  const cx = cxOf(freshBackups(), [owner]);
  assert.equal(await move.done(cx), false);
  assert.equal(await move.done(cxOf(nasBackups(), [owner])), false);
  assert.equal(await move.summary(cx), 'Start from the backups of your old machine, not from nothing');
});

test('the form is a link to the page, and has no arguments', (context) => {
  usePage(context);
  const form = move.form({});
  assert.equal(form.args, null);
  assert.equal(form.href, '/move?via=setup');
  assert.ok(form.element.textContent.includes('Bring everything from your old machine'));
});

test('Finish setting up reads it first on a new machine, and leaves it out of any other', async () => {
  const read = (cx) => readSteps({ parts: async (name, call) => [{ value: await call({ steps }, cx) }] });
  const fresh = await read(fakeCx({ '/api/1/backups': freshBackups(), '/api/1/accounts': [owner] }));
  assert.deepEqual(fresh.map((step) => [step.id, step.done]), [['move', false], ['backups', false]]);
  assert.equal(fresh[0].form !== null, true);
  const used = await read(fakeCx({ '/api/1/backups': nasBackups(), '/api/1/accounts': [owner, helper] }));
  assert.deepEqual(used.map((step) => step.id), ['backups']);
});

test('how many people there are is counted from the list, or the object that holds it, and is not told otherwise', () => {
  assert.equal(peopleCount([owner, helper]), 2);
  assert.equal(peopleCount({ accounts: [owner] }), 1);
  assert.equal(peopleCount({}), null);
  assert.equal(peopleCount(null), null);
  assert.equal(isNewInstall(freshBackups(), [owner]), true);
  assert.equal(isNewInstall(null, [owner]), false, 'a read that failed is not a machine with no backup place');
  assert.equal(isNewInstall(freshBackups(), null), false);
});

// ---- /setup ----

const tick = () => new Promise((resolve) => setImmediate(resolve));

/** /setup with `list` of steps, on a page of the test's own. */
async function setup(context, list, { query = '' } = {}) {
  const page = usePage(context);
  globalThis.history = { replaceState() {} };
  context.after(() => { delete globalThis.history; });
  const controller = new AbortController();
  context.after(() => controller.abort());
  const runs = [];
  const view = {
    main: page.document.createElement('main'),
    signal: controller.signal,
    session: { name: 'chase', admin: true },
    host: 'new-box',
    query: new URLSearchParams(query),
    head() {},
    on() {},
    go() {},
    parts: async (name, call) => [{ value: await call({ steps: list }, fakeCx({})) }],
    api: {
      get: async () => { throw new Error('no answer'); },
      run: async (action, args) => { runs.push([action, args]); return { ok: true, result: { key: 'the-answer' } }; },
      panel: () => page.document.createElement('div'),
    },
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/setup/page.js');
  await show(view);
  const buttons = () => view.main.querySelectorAll('.btns .btn');
  const named = (words) => [...view.main.querySelectorAll('a'), ...view.main.querySelectorAll('button')].find((one) => one.textContent.trim() === words);
  return {
    ...page, view, runs, buttons, named,
  };
}

const MOVE = {
  id: 'move', title: 'Bring everything from your old machine', order: 5, points: 0, action: null, done: async () => false, summary: async () => 'Start from the backups', form: move.form,
};

/** A step with fields, as the backups step has them. */
const withFields = (more = {}) => ({
  id: 'backups',
  title: 'Copy your data off this server',
  order: 30,
  points: 8,
  action: 'backups.add',
  done: async () => false,
  summary: async () => 'Nothing is copied',
  form: () => ({ element: h('section', { class: 'card' }, h('h2', {}, 'Copy your data off this server')), args: () => ({ name: 'nas' }) }),
  ...more,
});

test('/setup shows Continue on its page for a step whose form has no arguments, and no Set up', async (context) => {
  const app = await setup(context, [MOVE], { query: 'step=move' });
  const link = app.named('Continue on its page');
  assert.ok(link, 'the link is there');
  assert.equal(link.tag, 'a');
  assert.equal(link.getAttribute('href'), '/move?via=setup');
  assert.equal(link.hidden, false);
  assert.equal(app.named('Set up').hidden, true, 'Set up is not offered');
  assert.equal(app.named('Skip for now').hidden, false);
});

test('/setup shows Set up, and no link, for a step that has fields, and calls its `after` with the answer', async (context) => {
  const after = [];
  const BACKUPS = withFields({
    form: () => ({ element: h('section', { class: 'card' }, h('h2', {}, 'Copy your data off this server')), args: () => ({ name: 'nas' }), after: async (answer) => { after.push(answer); } }),
  });
  const app = await setup(context, [MOVE, BACKUPS], { query: 'step=backups' });
  assert.equal(app.named('Set up').hidden, false);
  assert.equal(app.named('Continue on its page').hidden, true, 'a step with fields has no link');
  app.named('Set up').click();
  await tick();
  await tick();
  assert.deepEqual(app.runs, [['backups.add', { name: 'nas' }]]);
  assert.deepEqual(after, [{ key: 'the-answer' }], 'the key that the answer holds is shown once, as it was');
});

test('moving from one step to another changes the buttons with it', async (context) => {
  const app = await setup(context, [MOVE, withFields()], { query: 'step=move' });
  assert.equal(app.named('Continue on its page').hidden, false);
  app.view.main.querySelectorAll('button').find((one) => one.textContent.includes('Copy your data off this server')).click();
  assert.equal(app.named('Continue on its page').hidden, true);
  assert.equal(app.named('Set up').hidden, false);
  app.view.main.querySelectorAll('button').find((one) => one.textContent.includes('Bring everything from your old machine')).click();
  assert.equal(app.named('Continue on its page').hidden, false);
  assert.equal(app.named('Set up').hidden, true);
});
