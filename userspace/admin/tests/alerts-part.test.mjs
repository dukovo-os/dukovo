// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Alerts page gives the shell: the Overview's things to look at, the
// step of Finish setting up that sets up the alert email (its fields, its
// secret and its test alert), and the palette's rows. Each is read with a fake
// `cx` that answers the reads and counts them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { needs, needsOf, steps, summaryOf } from '../pages/alerts/overview.js';
import * as palette from '../pages/alerts/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  at, emailChannel, ntfyChannel, pushChannel, tested,
} from './alerts-fixtures.mjs';
import { h } from '../dom.js';
import { usePage } from './fake-dom.mjs';
import { listedActions } from './registry.mjs';

const settle = async () => {
  for (let turn = 0; turn < 30; turn += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** A `cx` that answers `GET` of each path in `answers`, and counts the calls. */
function fakeCx(answers, session = { admin: true }) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    session,
    api: { get: async (path) => { calls.push(path); if (answers[path] instanceof Error) throw answers[path]; return answers[path]; } },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

const unlocked = (context, on = true) => {
  setUnlockSeconds(on ? 900 : null);
  context.after(() => setUnlockSeconds(null));
};
const PATH = '/api/1/alerts/channels';
const STEP = steps[0];

// ---- the Overview's things to look at ----

test('no channel is a thing to look at that is not scored, and says where to go', () => {
  assert.deepEqual(needsOf([]), [{
    id: 'alerts-none', severity: 'info', title: 'No alert leaves this server: set up email or push', detail: 'Alerts show on this server\'s screens only.', href: '/alerts', icon: 'bell', at: null,
  }]);
  assert.deepEqual(needsOf(null), []);
});

test('a channel whose last try failed is a warning, and one that took a message since is not', () => {
  const ntfy = ntfyChannel();
  const found = needsOf([emailChannel(), ntfy, pushChannel({ last_ok: at(14, 9), last_error: [at(14, 8), 'gone'] })]);
  assert.deepEqual(found, [{
    id: 'alerts-failing-ntfy-2', severity: 'warn', title: 'Alerts to ntfy (ntfy.sh) are failing', detail: 'ntfy answered 403 Forbidden: forbidden', href: '/alerts#channels', icon: 'bell', at: at(14, 2),
  }]);
  const both = needsOf([emailChannel({ last_ok: null, last_error: [at(14, 3), 'refused'] }), ntfy]);
  assert.deepEqual(both.map((one) => [one.id, one.title]), [
    ['alerts-failing-email-', 'Alerts to email are failing'],
    ['alerts-failing-ntfy-2', 'Alerts to ntfy (ntfy.sh) are failing'],
  ]);
});

test('the part reads where alerts go once, as an unlocked administrator, and for nobody else', async (context) => {
  unlocked(context);
  const cx = fakeCx({ [PATH]: [] });
  assert.deepEqual((await needs(cx)).map((one) => one.id), ['alerts-none']);
  assert.deepEqual(cx.calls, [PATH]);
  await STEP.done(cx);
  await STEP.summary(cx);
  assert.deepEqual(cx.calls, [PATH], 'the step shares the answer');
  unlocked(context, false);
  const locked = fakeCx({ [PATH]: [] });
  assert.deepEqual(await needs(locked), []);
  assert.deepEqual(locked.calls, []);
  unlocked(context);
  const reader = fakeCx({ [PATH]: [] }, { admin: false });
  assert.deepEqual(await needs(reader), []);
  assert.deepEqual(reader.calls, []);
});

test('a read that fails, or answers what is not a list, gives nothing to look at', async (context) => {
  unlocked(context);
  assert.deepEqual(await needs(fakeCx({ [PATH]: new Error('alertd is not running') })), []);
  assert.deepEqual(await needs(fakeCx({ [PATH]: { not: 'a list' } })), []);
});

// ---- the step ----

test('the alert email step is the sixth, gives no points and runs alerts.email', () => {
  assert.deepEqual([STEP.id, STEP.title, STEP.order, STEP.points, STEP.action], ['alert-email', 'Get an email when something needs you', 60, 0, 'alerts.email']);
});

test('the step is for administrators, and done once a channel of kind email is listed', async (context) => {
  unlocked(context);
  assert.equal(await STEP.available(fakeCx({}, { admin: true })), true);
  assert.equal(await STEP.available(fakeCx({}, { admin: false })), false);
  assert.equal(await STEP.done(fakeCx({ [PATH]: [ntfyChannel()] })), false);
  assert.equal(await STEP.done(fakeCx({ [PATH]: [ntfyChannel(), emailChannel()] })), true);
  assert.equal(await STEP.summary(fakeCx({ [PATH]: [] })), 'No email yet');
  assert.equal(await STEP.summary(fakeCx({ [PATH]: [emailChannel()] })), 'Email is set up');
  assert.equal(summaryOf(null), 'Unlock to see if it is set up');
});

test('a locked console leaves the step not done, and says to unlock', async (context) => {
  unlocked(context, false);
  const cx = fakeCx({ [PATH]: [emailChannel()] });
  assert.equal(await STEP.done(cx), false);
  assert.equal(await STEP.summary(cx), 'Unlock to see if it is set up');
  assert.deepEqual(cx.calls, []);
});

/** The step's form on a page of its own, with the actions it runs and the commands it shows. */
async function openForm(context, answers = {}) {
  const page = usePage(context);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const ran = [];
  const view = {
    host: 'duke-nas',
    signal: controller.signal,
    api: {
      run: async (action, args, options) => {
        ran.push([action, args, options]);
        return answers[action] ?? { ok: true, result: {} };
      },
    },
  };
  const form = STEP.form(view);
  page.document.body.append(form.element);
  await settle();
  const field = (name) => ['input', 'textarea', 'select'].flatMap((tag) => form.element.querySelectorAll(tag)).find((one) => one.id.endsWith(`-${name}`));
  const secret = form.element.querySelectorAll('input').find((one) => /^secret-\d+$/.test(one.id));
  const fill = (values = {}) => {
    field('to').value = values.to ?? 'ops@example.org';
    field('server').value = values.server ?? 'mail.example.org';
    field('port').value = values.port ?? '587';
    field('user').value = values.user ?? '';
    field('from').value = values.from ?? '';
    if (values.password !== undefined) secret.value = values.password;
  };
  return {
    ...page, form, view, ran, field, secret, fill, controller,
  };
}

const wordsOf = (node) => (node.children ? node.children.map(wordsOf).join(' ') : node.text ?? '');
const said = (world) => wordsOf(world.form.element.querySelector('[role="status"]')).replace(/\s+/g, ' ').trim();

test('the step\'s fields are the email\'s, without how bad an alert has to be or the kinds', async (context) => {
  const world = await openForm(context);
  for (const name of ['to', 'server', 'port', 'user', 'from']) assert.ok(world.field(name), name);
  assert.ok(world.secret, 'the password is a secret field');
  assert.equal(world.form.element.querySelectorAll('fieldset').length, 0);
  assert.equal(typeof world.form.shown, 'function');
  assert.equal(typeof world.form.clear, 'function');
});

test('the arguments read the password once, and a field that is not right is named', async (context) => {
  const world = await openForm(context);
  world.fill({ to: 'nobody' });
  assert.throws(() => world.form.args(), (error) => /nobody is not an email address/.test(error.words));
  world.fill({ user: 'ops', password: 'hunter2-secret' });
  assert.equal(world.form.args().password, 'hunter2-secret');
  assert.equal(world.secret.value, '', 'the password is gone from the field once taken');
  assert.throws(() => world.form.args(), (error) => /Type the password/.test(error.words));
});

test('the arguments are the email action\'s, with every alert and every kind', async (context) => {
  const world = await openForm(context);
  world.fill({ to: 'ops@example.org, admin@example.org', user: 'ops', password: 'x', port: '465' });
  assert.deepEqual(world.form.args(), {
    to: 'ops@example.org\nadmin@example.org', server: 'mail.example.org', port: '465', user: 'ops', severity: 'warning', cleared: true, password: 'x',
  });
});

test('the command that is shown holds an empty password and reads nothing', async (context) => {
  const world = await openForm(context);
  world.fill({ user: 'ops', password: 'hunter2-secret' });
  const shown = world.form.shown();
  assert.equal(shown.password, '');
  assert.doesNotMatch(JSON.stringify(shown), /hunter2-secret/);
  assert.equal(world.secret.value, 'hunter2-secret', 'the password stays where it was typed');
  world.fill({ to: 'nobody' });
  assert.throws(() => world.form.shown(), /loading/, 'a field that is not right shows no command');
});

test('before the fields have loaded the form asks to try again, and sends nothing', async (context) => {
  const page = usePage(context);
  const view = { host: 'duke-nas', signal: new AbortController().signal, api: { run: async () => { throw new Error('nothing is run'); } } };
  const form = STEP.form(view);
  page.document.body.append(form.element);
  assert.throws(() => form.args(), (error) => /still loading/.test(error.words));
  assert.throws(() => form.shown());
  await settle();
  assert.throws(() => form.args(), (error) => /at least one/.test(error.words), 'once they have, the form says what is missing');
});

test('after the email is saved a test alert goes, and the step says that it reached email', async (context) => {
  const world = await openForm(context, {
    'alerts.test': { ok: true, result: tested([{ channel: 'email', ok: true, error: null, error_id: null }, { channel: 'ntfy (ntfy.sh)', ok: false, error: 'refused', error_id: 'ALR-102' }]) },
  });
  world.fill({ user: 'ops', password: 'hunter2-secret' });
  world.form.args();
  await world.form.after({ done: true });
  assert.deepEqual(world.ran.map((call) => [call[0], call[1]]), [['alerts.test', {}]]);
  assert.equal(said(world), 'Sent a test alert: it reached email.');
  assert.equal(world.secret.value, '');
});

test('a test that email refused says why, and one that gives no word of email says only that it was sent', async (context) => {
  const refused = await openForm(context, { 'alerts.test': { ok: true, result: tested([{ channel: 'email', ok: false, error: 'the mail server said no', error_id: 'ALR-101' }]) } });
  await refused.form.after({});
  assert.equal(said(refused), 'Sent a test alert: it did not reach email: the mail server said no.');
  const silent = await openForm(context, { 'alerts.test': { ok: true, result: { done: true, output: ['sent'] } } });
  await silent.form.after({});
  assert.equal(said(silent), 'Sent a test alert.');
});

test('a test that could not be sent leaves the email saved and says to send one from Alerts', async (context) => {
  const world = await openForm(context, { 'alerts.test': { ok: false } });
  await world.form.after({});
  assert.match(said(world), /The email is saved\. The test alert did not go: send one from Alerts\./);
});

test('the password is cleared when the step is left, and when the page is', async (context) => {
  const left = await openForm(context);
  left.fill({ user: 'ops', password: 'hunter2-secret' });
  left.form.clear();
  assert.equal(left.secret.value, '');
  const gone = await openForm(context);
  gone.fill({ user: 'ops', password: 'hunter2-secret' });
  gone.controller.abort();
  assert.equal(gone.secret.value, '');
});

// ---- the palette ----

test('the palette finds the cards of the page for an administrator, and nothing for anyone else', async () => {
  assert.deepEqual(palette.itemsOf(true).map((one) => [one.key, one.title, one.href]), [
    ['where', 'Where alerts go', '/alerts#channels'],
    ['browser', 'Get alerts in this browser', '/alerts#browser'],
    ['phone', 'Set up a phone', '/alerts#phone'],
    ['recent', 'Alerts now and lately', '/alerts#active'],
  ]);
  assert.deepEqual(palette.itemsOf(false), []);
  const [provider] = palette.providers;
  assert.deepEqual([provider.group, provider.kind], ['Alerts', 'alerts']);
  assert.equal((await provider.items({ session: { admin: true } })).length, 4);
  assert.equal((await provider.items({ session: { admin: false } })).length, 0);
});

test('the palette goes to the card that is found', async () => {
  const [provider] = palette.providers;
  const collected = [{
    entry: PAGES.find((one) => one.id === 'alerts'),
    value: { phrases: palette.phrases ?? {}, groups: [{ ...provider, items: await provider.items({ session: { admin: true } }) }] },
  }];
  const made = buildIndex({ pages: PAGES, actions: listedActions(), collected, session: { admin: true } });
  const row = find(made, 'get alerts in this browser').find((one) => one.row.title === 'Get alerts in this browser').row;
  assert.deepEqual(row.target, { path: '/alerts#browser' });
  assert.deepEqual(find(made, 'kind:alerts').map((one) => one.row.title).sort(), ['Alerts now and lately', 'Get alerts in this browser', 'Set up a phone', 'Where alerts go']);
});

test('the Alerts page registers its Overview and palette parts, and both load', async () => {
  const entry = PAGES.find((one) => one.id === 'alerts');
  assert.equal(entry.needs, 'admin');
  assert.equal((await entry.parts.overview()).steps[0].id, 'alert-email');
  assert.equal((await entry.parts.palette()).providers.length, 1);
});

// ---- the step on /setup ----

/** /setup with the step of the alert email, on a page of its own: what the command panel was given and what each action ran with. */
async function openSetup(context) {
  const page = usePage(context);
  globalThis.history = { replaceState() {} };
  context.after(() => { delete globalThis.history; });
  const controller = new AbortController();
  context.after(() => controller.abort());
  const ran = [];
  const panels = [];
  // What the page listens for, and a read of the steps that waits until `held` is let go.
  const handlers = {};
  let held = null;
  const hold = () => {
    let release;
    held = new Promise((resolve) => { release = resolve; });
    return () => { held = null; release(); };
  };
  const view = {
    main: page.document.createElement('main'),
    signal: controller.signal,
    session: { name: 'chase', admin: true },
    host: 'duke-nas',
    query: new URLSearchParams('step=alert-email'),
    head() {},
    on(name, handler) { handlers[name] = handler; },
    go() {},
    parts: async (name, call) => [{
      value: await (held ?? Promise.resolve()).then(() => call({
        steps: [...steps, {
          id: 'other', title: 'Another step', order: 70, points: 0, action: null, done: async () => false, summary: async () => '', form: () => ({ element: h('section', { class: 'card' }, h('h2', {}, 'Another step')), args: () => ({}) }),
        }],
      }, fakeCx({ [PATH]: [] }))),
    }],
    api: {
      get: async () => { throw new Error('no answer'); },
      run: async (action, args) => { ran.push([action, args]); return { ok: true, result: action === 'alerts.test' ? tested([{ channel: 'email', ok: true, error: null, error_id: null }]) : {} }; },
      panel: (action, args) => { panels.push([action, args]); return page.document.createElement('div'); },
    },
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/setup/page.js');
  await show(view);
  await settle();
  const input = (name) => ['input', 'textarea', 'select'].flatMap((tag) => view.main.querySelectorAll(tag)).find((one) => one.id.endsWith(`-${name}`));
  const secret = view.main.querySelectorAll('input').find((one) => /^secret-\d+$/.test(one.id));
  const named = (words) => view.main.querySelectorAll('button').find((one) => one.textContent.trim() === words);
  return {
    ...page, view, ran, panels, input, secret, named, controller, handlers, hold,
  };
}

test('/setup shows the command with an empty password while the person types, and keeps what is typed', async (context) => {
  unlocked(context);
  const app = await openSetup(context);
  app.input('to').value = 'ops@example.org';
  app.input('server').value = 'mail.example.org';
  app.input('user').value = 'ops';
  app.secret.value = 'hunter2-secret';
  app.view.main.querySelector('.step-form section').fire('change');
  await new Promise((resolve) => setTimeout(resolve, 500));
  assert.ok(app.panels.length > 0, 'the command is shown');
  const [action, args] = app.panels.at(-1);
  assert.equal(action, 'alerts.email');
  assert.equal(args.password, '');
  assert.doesNotMatch(JSON.stringify(app.panels), /hunter2-secret/);
  assert.equal(app.secret.value, 'hunter2-secret', 'drawing the command takes nothing out of the field');
});

test('/setup runs the email with the password, then a test alert, and empties the field', async (context) => {
  unlocked(context);
  const app = await openSetup(context);
  app.input('to').value = 'ops@example.org';
  app.input('server').value = 'mail.example.org';
  app.input('user').value = 'ops';
  app.secret.value = 'hunter2-secret';
  app.named('Set up').click();
  await settle();
  assert.deepEqual(app.ran.map((call) => call[0]), ['alerts.email', 'alerts.test']);
  assert.equal(app.ran[0][1].password, 'hunter2-secret');
  assert.equal(app.secret.value, '');
  assert.match(wordsOf(app.view.main).replace(/\s+/g, ' '), /Sent a test alert: it reached email\./);
});

test('/setup names the field that is wrong and runs nothing', async (context) => {
  unlocked(context);
  const app = await openSetup(context);
  app.input('to').value = 'nobody';
  app.named('Set up').click();
  await settle();
  assert.deepEqual(app.ran, []);
  assert.match(wordsOf(app.view.main), /nobody is not an email address/);
});

test('/setup empties the password when the person goes to another step, and when the page is left', async (context) => {
  unlocked(context);
  const app = await openSetup(context);
  app.secret.value = 'hunter2-secret';
  app.view.main.querySelectorAll('button').find((one) => one.textContent.includes('Another step')).click();
  assert.equal(app.secret.value, '');
  const left = await openSetup(context);
  left.secret.value = 'hunter2-secret';
  left.controller.abort();
  assert.equal(left.secret.value, '');
});

test('/setup keeps the step the person chose while the steps were read again', async (context) => {
  unlocked(context);
  const app = await openSetup(context);
  const release = app.hold();
  // A task ends: the page reads the steps again, and the read takes a while.
  app.handlers.task();
  app.view.main.querySelectorAll('button').find((one) => one.textContent.includes('Another step')).click();
  release();
  await settle();
  const chosen = app.view.main.querySelector('[aria-current="step"]');
  assert.match(chosen.textContent, /Another step/);
  const run = app.view.main.querySelectorAll('button').find((one) => (one.getAttribute('aria-label') ?? '').startsWith('Set up'));
  assert.equal(run.getAttribute('aria-label'), 'Set up: Another step');
});
