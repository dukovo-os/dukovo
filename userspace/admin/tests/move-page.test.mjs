// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Move to new hardware page, drawn on a page of the test's own with a view
// that answers as admind does (design 6): the four steps and the one that is
// open, the key as a file or typed, that a typed key is in the body of `move.plan`
// and `move.stage` and in no other call, no attribute and no text, the cards'
// choices and when they read the plan again, Stage it as a task, a restore that
// is staged already, and the states of 6.5. What the page looks like is the lab's
// web-console/move test.

import assert from 'node:assert/strict';
import test from 'node:test';

import { setUnlockSeconds } from '../unlock.js';
import { usePage } from './fake-dom.mjs';
import {
  NEW0, OLD0, found, freshBackups, nasBackups, plan, planFull, planOne, source, stagedBackups, time,
} from './move-fixtures.mjs';

const TYPED = 'AGE-SECRET-KEY-1TYPED-MARKER-7731';
const FOLDER = '/volumes/BACKUP/dukovo-backup/duke-nas';
const KEY_FILE = '/volumes/KEYS/duke-nas-recovery.key';

const settle = async () => {
  for (let at = 0; at < 12; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** Every attribute and text under `node`, for what a secret must not be left in. */
const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/**
 * The page of Move on a fresh document. `world` is what admind answers and
 * changes as the test goes: `{backups, found, tasks, plan, stage, cancel}`;
 * `calls` holds what was run, `panels` what the Show command panel was given.
 */
async function open(context, { query = '', admin = false, world: given = {} } = {}) {
  const page = usePage(context);
  globalThis.location = { port: '9090' };
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    delete globalThis.location;
  });
  const world = {
    backups: freshBackups(), found: found(), tasks: { tasks: [] }, plan: plan(), stage: { ok: true, task: 41, result: { task: 41 } }, cancel: { ok: true, result: {} }, ...given,
  };
  const calls = [];
  const panels = [];
  const listeners = {};
  const opened = [];
  const expected = [];
  const view = {
    main: page.document.createElement('main'),
    signal: controller.signal,
    session: { name: 'chase', admin },
    time,
    query: new URLSearchParams(query),
    host: 'new-box',
    prefs: { commands: 'hidden' },
    slot: page.document.createElement('div'),
    notice: () => {},
    expectRestart: (words, options) => expected.push([words, options]),
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    openTask: (id) => opened.push(id),
    api: {
      get: async (path) => {
        calls.push(['GET', path]);
        if (path === '/api/1/backups') return world.backups;
        if (path === '/api/1/tasks') return world.tasks;
        if (path === '/api/1/power') return world.power ?? { shutdown: null };
        throw new Error(`no answer for ${path}`);
      },
      run: async (action, args) => {
        calls.push([action, args]);
        const answers = {
          'move.find': { ok: true, result: world.found },
          'move.plan': world.plan?.ok === false ? world.plan : { ok: true, result: world.plan },
          'move.stage': world.stage,
          'move.cancel': world.cancel,
          'power.restart': { ok: true, result: { done: true, changed: true } },
        };
        const answer = answers[action];
        return typeof answer === 'function' ? answer(args) : answer;
      },
      panel: (action, args) => {
        panels.push([action, args]);
        return page.document.createElement('div');
      },
    },
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/move/page.js');
  await show(view);
  const panel = (id) => view.main.querySelector(`[data-step="${id}"]`);
  const emit = (event, data) => (listeners[event] ?? []).forEach((listener) => listener(data));
  /** The ones of `calls` that ran `action`. */
  const ran = (action) => calls.filter((call) => call[0] === action).map((call) => call[1]);
  const shown = () => ['find', 'unlock', 'check', 'restart'].filter((id) => !panel(id).hidden);
  return {
    ...page, view, world, calls, panels, opened, expected, panel, emit, ran, shown, listeners,
  };
}

/** The input of the recovery key in `root`: the one that is not a radio. */
const keyInput = (root) => root.querySelectorAll('input').find((input) => input.getAttribute('type') !== 'radio');

/** Types `text` as the browser would: the input has it, and the event reaches the field's wrapper. */
function type(root, text) {
  const input = keyInput(root);
  input.value = text;
  input.fire('input');
  root.querySelector('.secret-field').fire('input');
}

/** Chooses the backup of the first step by its id, and goes on. */
async function chooseDisk(app, id = `disk:${FOLDER}`) {
  const radio = app.panel('find').querySelectorAll('input').find((input) => input.getAttribute('value') === id);
  radio.fire('change');
  buttonOf(app.panel('find'), 'Continue').click();
  await settle();
}

/** Unlocks with what the key step has, and waits for the plan. */
async function unlock(app) {
  buttonOf(app.panel('unlock'), 'Unlock it').click();
  await settle();
}

// ---- step 1 ----

test('the first step lists what was found, the backup places of this machine and a folder to type, and goes on to the key', async (context) => {
  const app = await open(context, { world: { backups: nasBackups() } });
  assert.deepEqual(app.shown(), ['find']);
  const cards = app.panel('find').querySelectorAll('.radio-card').map((card) => card.textContent);
  assert.deepEqual(cards, [
    'USB disk BACKUPduke-nas, today 1:30 AM',
    'Disk DATAold-pi, Nov 12 10:05 PM',
    'Backup place nasssh://backup@nas.local/srv/dukovo',
    'A folder…A folder of a machine\'s backups, typed',
  ]);
  assert.equal(app.view.headed.title, 'Move to new hardware');
  assert.deepEqual(app.view.headed.crumbs, [{ title: 'Snapshots and backups', path: '/snapshots' }]);
  assert.deepEqual(app.ran('move.find'), [{}]);

  buttonOf(app.panel('find'), 'Continue').click();
  assert.deepEqual(app.shown(), ['find'], 'nothing is chosen yet');
  assert.ok(app.panel('find').textContent.includes('Choose the backup to move from.'));

  await chooseDisk(app);
  assert.deepEqual(app.shown(), ['unlock']);
  const chosen = app.panel('unlock').querySelectorAll('.radio-card').filter((card) => card.classes.has('on'));
  assert.deepEqual(chosen.map((card) => card.textContent), [`Recovery key from the KEYS stick${KEY_FILE}`], 'the key named after the machine is chosen');
});

test('nothing found says so, with Look again and a folder to type', async (context) => {
  const app = await open(context, { world: { found: { sources: [], keys: [] } } });
  assert.ok(app.panel('find').textContent.includes('No backups found on the plugged-in disks'));
  assert.ok(buttonOf(app.panel('find'), 'Look again'));
  assert.ok(app.panel('find').textContent.includes('A folder…'));
  buttonOf(app.panel('find'), 'Look again').click();
  await settle();
  assert.equal(app.ran('move.find').length, 2);
});

test('an address with a folder, or a place, chooses the backup and opens the key step; via=setup is the crumb', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}&via=setup` });
  assert.deepEqual(app.shown(), ['unlock']);
  assert.deepEqual(app.view.headed.crumbs, [{ title: 'Finish setting up', path: '/setup' }]);
  const place = await open(context, { query: 'from=nas', world: { backups: nasBackups() } });
  assert.deepEqual(place.shown(), ['unlock']);
});

test('a folder that is typed must be a path, and is a backup that goes on to the key', async (context) => {
  const app = await open(context);
  const find = app.panel('find');
  find.querySelectorAll('input').find((input) => input.getAttribute('value') === 'folder').fire('change');
  const text = find.querySelectorAll('input').find((input) => input.getAttribute('type') === 'text');
  text.value = 'backups/duke-nas';
  buttonOf(find, 'Continue').click();
  assert.deepEqual(app.shown(), ['find']);
  assert.ok(find.textContent.includes('Write the whole path, starting with /.'));
  text.value = '/media/x/backups/duke-nas';
  buttonOf(find, 'Continue').click();
  await settle();
  assert.deepEqual(app.shown(), ['unlock']);
  buttonOf(app.panel('unlock'), 'Unlock it').click();
  await settle();
  assert.equal(app.ran('move.plan').length, 0, 'a folder is no machine, so no key file is chosen for it, and nothing typed reads nothing');
  assert.ok(app.panel('unlock').textContent.includes('Choose a key file or type the recovery key'));
});

test('a console that is locked is asked to unlock before the disks are read, and reads them once it is', async (context) => {
  context.after(() => setUnlockSeconds(null));
  const app = await open(context, { admin: true });
  assert.equal(app.ran('move.find').length, 0);
  assert.ok(app.panel('find').textContent.includes('Unlock the console to look for backups on the plugged-in disks.'));
  setUnlockSeconds(900);
  app.emit('unlock');
  await settle();
  assert.equal(app.ran('move.find').length, 1);
  assert.equal(app.panel('find').querySelectorAll('.radio-card').length, 3);
});

// ---- step 2: the key ----

test('a key file is sent as the key file, with no key, and the page goes to the check', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  assert.deepEqual(app.ran('move.plan'), [{ source: FOLDER, key_file: KEY_FILE }]);
  assert.deepEqual(app.shown(), ['check']);
  assert.ok(app.panel('check').textContent.includes('Name, accounts and passwords'));
});

test('a typed key is sent in the body of move.plan alone, and is gone from the page', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  const key = app.panel('unlock');
  type(key, TYPED);
  assert.equal(key.querySelectorAll('.radio-card').filter((card) => card.classes.has('on')).length, 0, 'typing took the file\'s choice away');
  await unlock(app);
  assert.deepEqual(app.ran('move.plan'), [{ source: FOLDER, key: TYPED }], 'a key and no file');
  assert.equal(keyInput(key).value, '', 'the field is empty after Unlock it');
  assert.deepEqual(app.shown(), ['check']);
  assert.equal(holdings(app.view.main).some((held) => held.includes(TYPED)), false, 'the key is in no attribute and no text');
  assert.ok(app.panel('unlock').textContent.includes('The key is not kept'), 'the page says so');
});

test('picking a key file empties what was typed, and the key file goes alone', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  const key = app.panel('unlock');
  type(key, TYPED);
  const radio = key.querySelectorAll('input').find((input) => input.getAttribute('value') === KEY_FILE);
  radio.fire('change');
  assert.equal(keyInput(key).value, '');
  await unlock(app);
  assert.deepEqual(app.ran('move.plan'), [{ source: FOLDER, key_file: KEY_FILE }]);
});

test('with no key file and nothing typed, Unlock it asks for a key and runs nothing', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  const key = app.panel('unlock');
  type(key, 'x');
  keyInput(key).value = '';
  await unlock(app);
  assert.equal(app.ran('move.plan').length, 0);
  assert.ok(key.textContent.includes('Choose a key file or type the recovery key'));
  assert.deepEqual(app.shown(), ['unlock']);
});

test('a key that does not open the backups stays on the key step with what the action said, and the field is ready again', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}`, world: { plan: { ok: false, error: { id: 'BAK-103', title: 'This key does not open these backups' } } } });
  type(app.panel('unlock'), TYPED);
  await unlock(app);
  assert.equal(app.ran('move.plan').length, 1);
  assert.deepEqual(app.shown(), ['unlock']);
  assert.equal(keyInput(app.panel('unlock')).value, '', 'a key that was refused is not kept');
});

// ---- step 3: what changes ----

test('what changes is what comes across, the hardware\'s differences and what happens after the restart', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  const text = app.panel('check').textContent;
  for (const words of [
    'What comes across', 'Name, accounts and passwords', 'duke-nas · owner and helper', '2 apps at the same versions', '/users and /shared, 502 GB', 'Older snapshots',
    'Differences in the hardware', 'Network ports', '1 choice', 'No mirror', 'Plug it in', 'Processor and graphics',
    'The firewall rule "deny ping on net1" names a port this machine does not have, so it does nothing here.',
    'After the restart', 'Turn the old duke-nas off before this machine restarts.',
    'After the restart, open https://duke-nas.local:9090/ or https://192.168.1.12:9090/ and sign in as one of duke-nas\'s administrators.',
  ]) assert.ok(text.includes(words), words);
  const choices = app.panel('check').querySelectorAll('input').filter((input) => input.getAttribute('data-card') === OLD0);
  assert.deepEqual(choices.map((input) => input.getAttribute('value')), [NEW0, 'automatic']);
});

test('the command panel of Stage it is drawn once the choices stop changing, with the key as the file\'s path, and the restart\'s command', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  assert.equal(app.panels.length, 0, 'not at once');
  context.mock.timers.tick(400);
  assert.deepEqual(app.panels, [['move.stage', { source: FOLDER, key_file: KEY_FILE, cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000' }]]);
  assert.ok(app.panel('check').textContent.includes('elevate reboot'));
});

test('a typed key is never given to the Show command panel: its key is an empty text', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const app = await open(context, { query: `source=${FOLDER}` });
  type(app.panel('unlock'), TYPED);
  await unlock(app);
  context.mock.timers.tick(400);
  assert.deepEqual(app.panels, [['move.stage', { source: FOLDER, key: '', cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000' }]]);
  assert.equal(JSON.stringify([app.panels, app.calls.filter((call) => call[0] !== 'move.plan')]).includes(TYPED), false);
});

test('a card\'s choice reads the plan again with the cards when a key file was used, and the firewall words follow it', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  app.world.plan = plan({ firewall_on_missing: [] });
  const automatic = app.panel('check').querySelectorAll('input').find((input) => input.getAttribute('data-card') === OLD0 && input.getAttribute('value') === 'automatic');
  automatic.fire('change');
  await settle();
  assert.deepEqual(app.ran('move.plan'), [{ source: FOLDER, key_file: KEY_FILE }, { source: FOLDER, key_file: KEY_FILE, cards: `${OLD0}=automatic` }]);
  assert.equal(app.panel('check').textContent.includes('deny ping on net1'), false, 'the plan that came back is drawn');
  const banner = app.panel('check').querySelector('.mv-after-box .banner');
  assert.ok(banner.classes.has('warn') && !banner.classes.has('bad'), 'no fixed address is kept, so the old machine\'s is not what two machines share');
});

test('a card\'s choice with a typed key keeps the plan, reads nothing again, and goes to Stage it as the choices are', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  type(app.panel('unlock'), TYPED);
  await unlock(app);
  const automatic = app.panel('check').querySelectorAll('input').find((input) => input.getAttribute('data-card') === OLD0 && input.getAttribute('value') === 'automatic');
  automatic.fire('change');
  await settle();
  assert.equal(app.ran('move.plan').length, 1, 'the key is not there to read it again');
  const banner = app.panel('check').querySelector('.mv-after-box .banner');
  assert.ok(banner.classes.has('warn'));
  buttonOf(app.panel('check'), 'Continue').click();
  type(app.panel('restart'), TYPED);
  buttonOf(app.panel('restart'), 'Stage it').click();
  await settle();
  assert.deepEqual(app.ran('move.stage'), [{ source: FOLDER, key: TYPED, cards: `${OLD0}=automatic`, at: '2026-11-14-013000' }]);
});

test('a record of format 1 says why the hardware is not shown, and still offers the cards', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}`, world: { plan: planOne() } });
  await unlock(app);
  const text = app.panel('check').textContent;
  assert.ok(text.includes('These backups were made before Dukovo kept the machine\'s hardware with them, so the differences cannot be shown.'));
  assert.equal(text.includes('Plug it in') || text.includes('No mirror'), false);
  assert.ok(text.includes('Network ports'));
});

// ---- step 4: Stage it ----

test('with a key file Stage it sends it with the cards and the time that were checked, and the task is followed to Staged', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  const restart = app.panel('restart');
  assert.deepEqual(app.shown(), ['restart']);
  assert.equal(restart.querySelectorAll('input').length, 0, 'no key is asked for again');
  buttonOf(restart, 'Stage it').click();
  await settle();
  assert.deepEqual(app.ran('move.stage'), [{ source: FOLDER, key_file: KEY_FILE, cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000' }]);
  assert.equal(buttonOf(restart, 'Stage it'), undefined, 'it is being staged');
  app.emit('task', {
    id: 41, action: 'move.stage', state: 'running', step: 2, steps: 5, words: 'receiving /users', percent: 41,
  });
  assert.ok(restart.textContent.includes('Step 2 of 5: receiving /users'));
  app.world.backups = stagedBackups();
  app.emit('task', { id: 41, action: 'move.stage', state: 'done' });
  await settle();
  assert.ok(restart.textContent.includes('Staged from nas.'));
  assert.ok(restart.textContent.includes('Nothing changes until this machine restarts, and you can cancel until then.'));
  assert.ok(restart.textContent.includes('elevate reboot'));
  assert.ok(buttonOf(restart, 'Cancel'));
  assert.equal(buttonOf(restart, 'Stage it'), undefined);
  assert.ok(buttonOf(restart, 'Restart now…'), 'a restore that is staged can be finished from here');
  assert.equal(buttonOf(restart, 'Restart'), undefined, 'and Restart now is the only button that restarts');
});

test('a typed key is asked for again at Stage it, and goes in the body of move.stage alone', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  type(app.panel('unlock'), TYPED);
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  const restart = app.panel('restart');
  assert.ok(keyInput(restart), 'the field is there');
  buttonOf(restart, 'Stage it').click();
  await settle();
  assert.equal(app.ran('move.stage').length, 0, 'with no key typed, nothing is staged');
  assert.ok(restart.textContent.includes('Type the recovery key'));
  type(restart, TYPED);
  buttonOf(restart, 'Stage it').click();
  await settle();
  assert.deepEqual(app.ran('move.stage'), [{ source: FOLDER, key: TYPED, cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000' }]);
  assert.equal(holdings(app.view.main).some((held) => held.includes(TYPED)), false);
  const others = app.calls.filter((call) => call[0] !== 'move.plan' && call[0] !== 'move.stage');
  assert.equal(JSON.stringify([others, app.panels]).includes(TYPED), false);
});

test('the files that do not fit turn Stage it off, with the sentence', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}`, world: { plan: planFull() } });
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  const stage = buttonOf(app.panel('restart'), 'Stage it');
  assert.equal(stage.getAttribute('aria-disabled'), 'true');
  stage.click();
  await settle();
  assert.equal(app.ran('move.stage').length, 0);
  assert.ok(app.panel('restart').textContent.includes('The files need 1.24 TB and this machine has 900 GB free.'));
});

test('a restore that is staged already is shown as staged, with Cancel and no Stage it', async (context) => {
  const app = await open(context, { world: { backups: stagedBackups() } });
  assert.deepEqual(app.shown(), ['restart']);
  const restart = app.panel('restart');
  assert.ok(restart.textContent.includes('Staged from nas.'));
  assert.ok(restart.textContent.includes('elevate reboot'));
  assert.equal(buttonOf(restart, 'Stage it'), undefined);
  assert.equal(app.ran('move.plan').length, 0);
  buttonOf(restart, 'Cancel').click();
  await settle();
  assert.deepEqual(app.ran('move.cancel'), [{}]);
});

test('Restart now asks first, then restarts the machine in a minute and counts the minute on the card', async (context) => {
  context.after(() => setUnlockSeconds(null));
  setUnlockSeconds(900);
  const realObserver = globalThis.MutationObserver;
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  context.after(() => {
    if (realObserver === undefined) delete globalThis.MutationObserver;
    else globalThis.MutationObserver = realObserver;
  });
  const app = await open(context, { admin: true, world: { backups: stagedBackups() } });
  const restart = app.panel('restart');
  assert.ok(restart.textContent.includes('press Restart now, or type this at a terminal'));
  assert.ok(restart.textContent.includes('elevate reboot'), 'the command stays for a terminal');
  app.world.power = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: Date.now() + 60_000 } };
  buttonOf(restart, 'Restart now…').click();
  await settle();
  const dialog = app.document.body.querySelector('dialog');
  assert.equal(dialog.querySelector('h2').textContent, 'Restart new-box?');
  assert.ok(dialog.textContent.includes('To finish the restore from nas.'));
  assert.equal(app.ran('power.restart').length, 0, 'nothing is set before the question is answered');
  dialog.querySelector('form').fire('submit');
  await settle();
  assert.deepEqual(app.ran('power.restart'), [{ at: null }]);
  assert.ok(restart.textContent.includes('new-box restarts in 60 seconds'), 'the card counts the minute');
  assert.ok(buttonOf(restart, 'Cancel'), 'and Cancel stays');
});

test('a restart that is set when the page opens shows its line on the staged card', async (context) => {
  const app = await open(context, {
    admin: true, world: { backups: stagedBackups(), power: { shutdown: { end: 'reboot', in_seconds: 5400, at_ms: Date.now() + 5_400_000 } } },
  });
  await settle();
  assert.match(app.panel('restart').textContent, /new-box restarts at/);
});

test('a card that is not staged has no Restart now', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}`, admin: true });
  assert.equal(buttonOf(app.panel('restart'), 'Restart now…'), undefined);
});

test('a task that fails because a restore was staged first (BAK-104) shows that one Staged', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  buttonOf(app.panel('restart'), 'Stage it').click();
  await settle();
  // Another tab staged one while this page showed Stage it.
  app.world.backups = stagedBackups();
  app.emit('task', {
    id: 41,
    action: 'move.stage',
    state: 'failed',
    status: 1,
    note: 'backup: a restore of a whole machine is staged already: restart to carry it out, or end it with `backup restore --machine --cancel` (BAK-104)',
  });
  await settle();
  const restart = app.panel('restart');
  assert.ok(buttonOf(restart, 'Cancel'));
  assert.equal(buttonOf(restart, 'Stage it'), undefined);
  assert.ok(!restart.textContent.includes('Nothing is staged'));
});

test('a plan that says a restore is staged is shown as staged', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}`, world: { plan: plan({ staged: true }) } });
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  assert.ok(buttonOf(app.panel('restart'), 'Cancel'));
  assert.equal(buttonOf(app.panel('restart'), 'Stage it'), undefined);
});

test('a task that fails says so, that nothing is staged, and Stage it is there again', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  buttonOf(app.panel('check'), 'Continue').click();
  buttonOf(app.panel('restart'), 'Stage it').click();
  await settle();
  app.emit('task', {
    id: 41, action: 'move.stage', state: 'failed', status: 1, note: 'the disk was unplugged',
  });
  const restart = app.panel('restart');
  assert.ok(restart.textContent.includes('Failed: the disk was unplugged. Nothing is staged.'));
  assert.ok(buttonOf(restart, 'Stage it'));
});

test('a task that is not this page\'s is not followed, and one that runs already is found at the start', async (context) => {
  const app = await open(context, {
    world: { backups: stagedBackups(), tasks: { tasks: [{ id: 7, action: 'move.stage', state: 'running', step: 1, steps: 3, words: 'receiving /shared', percent: null }] } },
  });
  app.emit('task', { id: 99, action: 'other', state: 'done' });
  assert.deepEqual(app.shown(), ['restart']);
  const idle = await open(context, {
    query: `source=${FOLDER}`, world: { tasks: { tasks: [{ id: 7, action: 'move.stage', state: 'running', step: 1, steps: 3, words: 'receiving /shared', percent: null }] } },
  });
  await unlock(idle);
  buttonOf(idle.panel('check'), 'Continue').click();
  assert.ok(idle.panel('restart').textContent.includes('Step 1 of 3: receiving /shared'));
  buttonOf(idle.panel('restart'), 'Open the task').click();
  assert.deepEqual(idle.opened, [7]);
});

test('the steps are a list, and a step that is done can be gone back to', async (context) => {
  const app = await open(context, { query: `source=${FOLDER}` });
  await unlock(app);
  const list = app.view.main.querySelector('.mv-list');
  const steps = list.querySelectorAll('button');
  assert.deepEqual(steps.map((button) => button.getAttribute('aria-current')), [null, null, 'step', null]);
  assert.deepEqual(steps.map((button) => button.hasAttribute('disabled')), [false, false, false, true], 'the last step is not there to go to yet');
  assert.ok(steps[1].textContent.includes('Done: '));
  steps[0].click();
  assert.deepEqual(app.shown(), ['find']);
  assert.equal(app.ran('move.plan').length, 1, 'going back reads nothing');
});
