// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page of one service (design 4.4): which buttons it offers a reader, an
// administrator and for the console's own services, the words of what they
// run, the banner that explains a service that is not running, and the rows
// of the cards from a fixture of `service NAME status --json`, among them
// what #390 adds: its use, limits and sockets.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  bannerOf, BOOT_HINT, complaintOf, consoleCommand, consoleWords, controlsOf, explainable, healthOf, holdsRows, needsRows, nowMsOf, READER_LINE,
  readyWords, restartWords, runsOf, triedOf, unknownService, unknownWords, wordsFor,
} from '../pages/services/detail-model.js';
import { fileOf, limitsRows, socketsRows, tryCommand } from '../pages/services/limits-model.js';
import {
  chartsOf, memoryLimitOf, modelsOf, noHistory, NO_HISTORY, USE_FOOT, useRows,
} from '../pages/services/use-model.js';
import {
  explanation, NOW, NOW_MS, service, status, usageAnswer, withLimits,
} from './services-fixtures.mjs';

const time = makeTime('UTC', () => NOW * 1000);
const ids = (controls) => controls.buttons.map((button) => button.id);

test('a reader has no buttons and no switch, and is told who has', () => {
  const controls = controlsOf(service('nginx', '', 'running', 1), false);
  assert.deepEqual(controls, { buttons: [], boot: null, note: { words: 'Administrators can start and stop services.', command: null } });
  assert.equal(READER_LINE, controls.note.words);
});

test('an administrator may run a service that is not running, and stop or restart one that is', () => {
  const admin = (state) => ids(controlsOf(service('nginx', '', state, 1), true));
  for (const state of ['stopped', 'disabled', 'failed', 'done', 'waiting', 'pending']) assert.deepEqual(admin(state), ['start'], state);
  for (const state of ['running', 'starting', 'listening']) assert.deepEqual(admin(state), ['stop', 'restart'], state);
  assert.deepEqual(admin('stopping'), [], 'one that is stopping has nothing to press');
});

test('each button runs the action of its name', () => {
  const [start] = controlsOf(service('a', '', 'failed', 1), true).buttons;
  assert.deepEqual(start, { id: 'start', action: 'service.start', label: 'Run now' });
  const [stop, restart] = controlsOf(service('a', '', 'running', 1), true).buttons;
  assert.deepEqual([stop.action, stop.label, restart.action, restart.label], ['service.stop', 'Stop', 'service.restart', 'Restart']);
});

test('the switch Starts at boot turns it on or off by what the file says, and names the action it runs', () => {
  assert.deepEqual(controlsOf(service('a', '', 'running', 1), true).boot, { on: true, action: 'service.disable', fixed: false });
  assert.deepEqual(controlsOf(service('a', '', 'running', 1, { enabled: false }), true).boot, { on: false, action: 'service.enable', fixed: false });
  assert.equal(controlsOf(service('a', '', 'stopped', 1, { enabled: false }), true).note, null);
});

test('admind and authd cannot be stopped, restarted or turned off from the page, and the page says why before it is tried', () => {
  for (const name of ['admind', 'authd']) {
    const controls = controlsOf(service(name, 'The console', 'running', 1), true);
    assert.deepEqual(controls.buttons, [], name);
    assert.equal(controls.boot.fixed, true, name);
    assert.deepEqual(controls.note, { words: consoleWords(name), command: consoleCommand(name) });
  }
  assert.equal(consoleCommand('admind'), 'elevate service admind restart');
  assert.match(consoleWords('authd'), /^The console runs on authd, so this page does not stop it, restart it or turn off its start at boot: that is done at a terminal\.$/);
});

test('a console service that is not running may be run, and one that is off may be turned on', () => {
  const down = controlsOf(service('authd', '', 'failed', 1, { enabled: false }), true);
  assert.deepEqual(ids(down), ['start']);
  assert.deepEqual(down.boot, { on: false, action: 'service.enable', fixed: false });
  assert.equal(down.note, null, 'nothing is refused, so nothing is said');
});

test('each action has the words of what it does, with the name in them', () => {
  for (const action of ['service.start', 'service.stop', 'service.restart', 'service.enable', 'service.disable']) {
    const words = wordsFor(action, 'nginx');
    assert.deepEqual(Object.keys(words).sort(), ['busy', 'done', 'running', 'unchanged', 'unlock'], action);
    for (const key of ['unlock', 'running', 'done', 'unchanged']) assert.match(words[key], /nginx/, `${action} ${key}`);
    assert.match(words.busy, /…$/);
  }
  assert.equal(wordsFor('service.start', 'nginx').unlock, 'start nginx');
  assert.equal(wordsFor('service.stop', 'nginx').running, 'Stopping nginx');
  assert.equal(wordsFor('service.enable', 'nginx').done, 'nginx starts at boot, and runs now.');
  assert.equal(wordsFor('service.disable', 'nginx').done, 'nginx no longer starts at boot, and is stopped.');
  assert.deepEqual(wordsFor('service.explain', 'nginx'), {});
  assert.match(BOOT_HINT, /also starts the service now.*also stops it/);
});

test('a service that is not running or done is explained', () => {
  const yes = ['failed', 'stopped', 'disabled', 'pending', 'waiting', 'stopping'];
  for (const state of ['running', 'starting', 'listening', 'done', ...yes]) assert.equal(explainable({ state }), yes.includes(state), state);
  assert.equal(explainable(undefined), false);
});

test('the banner is the explanation: the sentence, the fix and the service behind it', () => {
  const failed = service('backup-sync', '', 'failed', 1);
  assert.deepEqual(bannerOf(failed, explanation), { kind: 'bad', sentence: explanation.sentence, fix: explanation.fix, root: null });
  const behind = { sentence: 'db is stopped: it requires disk, which failed.', chain: ['db', 'disk'], root: 'disk', fix: null };
  assert.deepEqual(bannerOf(service('db', '', 'stopped', 1), behind), { kind: 'warn', sentence: behind.sentence, fix: null, root: 'disk' });
  assert.equal(bannerOf(service('db', '', 'stopped', 1), { ...behind, fix: '' }).fix, null);
});

test('when the explanation cannot be read, Spark\'s own words of the service are the banner', () => {
  const failed = service('backup-sync', '', 'failed', 1, { failures: 3, last_end: 'exited with code 1', last_line: 'the NAS did not answer' });
  assert.deepEqual(bannerOf(failed, null), {
    kind: 'bad',
    sentence: 'backup-sync is not running. It gave up after 3 starts that failed in a row. Its last run exited with code 1. It last said: the NAS did not answer.',
    fix: null,
    root: null,
  });
  assert.equal(bannerOf(service('printer', '', 'disabled', 1, { why: 'it is off until it is started' }), {}).sentence,
    'printer is not running. It is off until it is started.');
  assert.equal(bannerOf(service('x', '', 'stopped', 1), { sentence: '' }).sentence, 'x is not running. It is not running.');
});

test('a failed health check says how often it failed and when Spark restarts the service', () => {
  const health = (more) => healthOf({ health: { state: 'failing', failures: 3, restart_after: 5, last_failure: 'GET / answered 502', ...more } });
  assert.equal(health({}), 'Its health check failed 3 times in a row: GET / answered 502. Spark restarts it after 5 failed checks in a row.');
  assert.equal(health({ last_failure: null, restart_after: 0, failures: 1 }), 'Its health check failed 1 time in a row.');
  assert.equal(health({ last_failure: 'it hung.', restart_after: 1 }), 'Its health check failed 3 times in a row: it hung. Spark restarts it after 1 failed check in a row.');
  assert.equal(healthOf({ health: { state: 'passing', failures: 0 } }), null);
  assert.equal(healthOf({ health: null }), null);
  assert.equal(healthOf({}), null);
  assert.equal(healthOf(undefined), null);
});

test('what is wrong with the service\'s file is a sentence, or nothing', () => {
  assert.equal(complaintOf({ complaint: 'line 4: no such key' }), 'Line 4: no such key.');
  assert.equal(complaintOf({ complaint: '' }), null);
  assert.equal(complaintOf({}), null);
});

test('the clock of the document is read beside the service or inside it', () => {
  assert.equal(nowMsOf({ now_ms: 5, service: {} }), 5);
  assert.equal(nowMsOf({ service: { now_ms: 7 } }), 7);
  assert.equal(nowMsOf({ now_ms: 'x', service: {} }), null);
  assert.equal(nowMsOf(undefined), null);
});

test('the time to be ready is said in the shortest honest words', () => {
  assert.equal(readyWords(0), '0 ms');
  assert.equal(readyWords(412.4), '412 ms');
  assert.equal(readyWords(1000), '1.0 s');
  assert.equal(readyWords(2500), '2.5 s');
  assert.equal(readyWords(12_000), '12 s');
  assert.equal(readyWords(150_000), '3 min');
});

test('the restart rule says when it starts again and when Spark gives up', () => {
  assert.equal(restartWords({ restart: 'always' }), 'Starts again whenever it ends');
  assert.equal(restartWords({ restart: 'on-failure', give_up_after: 5 }), 'Starts again when it fails, and gives up after 5 starts that fail in a row');
  assert.equal(restartWords({ restart: 'never', give_up_after: 1 }), 'Does not start again when it ends, and gives up after 1 start that fails in a row');
  assert.equal(restartWords({ restart: 'sometimes' }), 'Sometimes');
  assert.equal(restartWords({ restart: '' }), null);
  assert.equal(restartWords({}), null);
});

test('the Runs card says since when, the processes, where it listens, restarts, what started it, when it was ready and its rule', () => {
  assert.deepEqual(runsOf(status, NOW, time), [
    { label: 'Since', value: '9:12 AM' },
    { label: 'Processes', items: ['312 /system/commands/nginx as www', '313 /system/commands/nginx as www'] },
    { label: 'Listening on', items: ['port 443 (TCP)'] },
    { label: 'Restarts', value: '2 since the machine started' },
    { label: 'Started by', value: 'The machine starting' },
    { label: 'Ready in', value: '1.0 s' },
    { label: 'Restart rule', value: 'Starts again when it fails, and gives up after 5 starts that fail in a row' },
  ]);
});

test('a row Spark has nothing for is left out, and so is the time when the document has no clock', () => {
  const bare = { service: service('x', '', 'stopped', 1, { restarts: undefined, since_ms: undefined }), processes: [], listening: [] };
  assert.deepEqual(runsOf(bare, NOW, time), []);
  const noClock = { ...status, now_ms: undefined };
  assert.equal(runsOf(noClock, NOW, time).some((row) => row.label === 'Since'), false);
  const inside = { ...noClock, service: { ...status.service, now_ms: NOW_MS } };
  assert.equal(runsOf(inside, NOW, time)[0].label, 'Since');
  assert.equal(runsOf({ service: service('x', '', 'running', 1, { restarts: 0, started_by: 'a request' }) }, NOW, time).at(-1).value, 'A request');
  assert.equal(runsOf({ service: { restarts: 0 } }, NOW, time)[0].value, 'None since the machine started');
});

test('the Runs card lists the service\'s own process first and up to five more, and says how many it left out', () => {
  const many = Array.from({ length: 9 }, (_, index) => ({ pid: 100 + index, user: 'www', program: `/p${index}` }));
  const rows = runsOf({ now_ms: NOW_MS, service: service('x', '', 'running', 1, { pid: 104 }), processes: many }, NOW, time);
  const items = rows.find((row) => row.label === 'Processes').items;
  assert.equal(items.length, 7);
  assert.equal(items[0], '104 /p4 as www');
  assert.deepEqual(items.slice(1, 6), ['100 /p0 as www', '101 /p1 as www', '102 /p2 as www', '103 /p3 as www', '105 /p5 as www']);
  assert.equal(items[6], 'and 3 more');
});

test('the process of a service that the list leaves out is still named, and a process with no user has no "as"', () => {
  const rows = runsOf({ service: service('x', '', 'running', 1, { pid: 7 }), processes: [{ pid: 8, program: '/bin/y' }] }, NOW, time);
  assert.deepEqual(rows.find((row) => row.label === 'Processes').items, ['7', '8 /bin/y']);
  const none = runsOf({ service: service('x', '', 'running', 1, { pid: 7 }) }, NOW, time);
  assert.deepEqual(none.find((row) => row.label === 'Processes').items, ['7']);
});

test('a socket that is not TCP says so', () => {
  const rows = runsOf({ service: {}, listening: [{ port: 53, tcp: false }, { port: 53, tcp: true }, { tcp: true }, null] }, NOW, time);
  assert.deepEqual(rows.find((row) => row.label === 'Listening on').items, ['port 53 (UDP)', 'port 53 (TCP)']);
});

test('the Needs card lists what the service requires, wants, starts after and waits for, and leaves out the empty', () => {
  assert.deepEqual(needsRows(status.service), [
    { label: 'Requires', names: ['netd'] },
    { label: 'Wants', names: ['logd'] },
    { label: 'Starts after', names: ['netd', 'logd'] },
  ]);
  assert.deepEqual(needsRows({ waiting_for: ['disk', '', 7] }), [{ label: 'Waiting for', names: ['disk'] }]);
  assert.deepEqual(needsRows({ requires: null }), []);
});

test('the Holds card says what the file gives the service, in words', () => {
  const holds = {
    whole_tree: false, account: 'www', reads: ['/system/settings/web'], changes: ['/var/web'], mounts: [], devices: 'disks', devices_write: false,
    root_subvolumes: ['home'], every_process: true, secrets: ['tls-key'], listen: ['80', '8000-8010'], outbound: true, admin: ['network'],
    names: true, cards: ['0000:00:03.0'],
  };
  assert.deepEqual(holdsRows({ holds }), [
    { label: 'Acts as', value: 'www' },
    { label: 'May read', items: ['/system/settings/web'] },
    { label: 'May change', items: ['/var/web'] },
    { label: 'Devices', value: 'disks, to read only' },
    { label: 'Volumes', items: ['home'] },
    { label: 'Processes', value: 'Sees every process on the machine' },
    { label: 'Secrets', items: ['tls-key'] },
    { label: 'May listen on', items: ['80', '8000-8010'] },
    { label: 'Connects out', value: 'Yes' },
    { label: 'Settings it may change', items: ['network'] },
    { label: 'Names', value: 'May give processes and files another user\'s name' },
    { label: 'Cards', items: ['0000:00:03.0'] },
  ]);
});

test('a service with nothing special holds what every service does, and one with the whole tree says so', () => {
  assert.deepEqual(holdsRows({ holds: {} }), [{ label: 'Acts as', value: 'root' }, { label: 'Connects out', value: 'No' }]);
  assert.equal(holdsRows({ holds: { whole_tree: true, devices: 'disks', devices_write: true } })[0].value, 'The whole tree, with the machine\'s own access');
  assert.equal(holdsRows({ holds: { devices: 'disks', devices_write: true } }).find((row) => row.label === 'Devices').value, 'disks, to read and write');
  assert.deepEqual(holdsRows({}), []);
  assert.deepEqual(holdsRows({ holds: null }), []);
});

test('a trial that service try began is a card with the time it is undone and the commands to keep or undo it', () => {
  const tried = { ...status, trial: { id: 'service.nginx', left_ms: 41_000 } };
  assert.deepEqual(triedOf(tried, NOW, time), {
    words: 'A change to this service\'s file is being tried. It is undone at 10:12 AM unless it is kept.',
    keep: 'elevate service keep nginx',
    undo: 'elevate service undo nginx',
  });
  assert.equal(triedOf({ ...tried, trial: { id: 'service.nginx' } }, NOW, time).words, 'A change to this service\'s file is being tried. It is undone unless it is kept.');
  assert.equal(triedOf(status, NOW, time), null);
  assert.equal(triedOf({ trial: 'yes' }, NOW, time), null);
  assert.equal(triedOf(undefined, NOW, time), null);
});

test('the words of a name that is no service say so and keep what the command said', () => {
  const error = { id: 'ADM-123', status: 500, command: { argv: ['service', 'nope', 'status', '--json'], status: 4, stderr: 'no service is called nope' } };
  assert.equal(unknownService(error), true);
  assert.equal(unknownService({ status: 404 }), true);
  assert.equal(unknownService({ id: 'ADM-123', command: { status: 1 } }), false);
  assert.equal(unknownService({ id: 'ADM-111', status: 502 }), false);
  assert.equal(unknownService(null), false);
  assert.deepEqual(unknownWords('nope', error), {
    id: 'ADM-123',
    title: 'There is no service called nope',
    detail: 'Spark does not know a service by that name. It may have been removed, or the address may have a mistake in it.',
    fix: 'Open the list to see the services this server has.',
    status: 500,
    command: error.command,
  });
  assert.deepEqual(unknownWords('a b', null), {
    id: null, title: 'There is no service called a b', detail: unknownWords('x', null).detail, fix: unknownWords('x', null).fix, status: 404, command: null,
  });
});

// ---- what #390 adds: use, limits and sockets ----------------------------------------------

test('the limits are what the file sets, in words, with no limit said where it sets none', () => {
  assert.deepEqual(limitsRows(withLimits), [
    { label: 'Memory', value: '268 MB' },
    { label: 'CPU weight', value: '100' },
    { label: 'Threads', value: 'At most 4,096' },
    { label: 'Open files', value: '65,536 for each process' },
    { label: 'Disk weight', value: '200' },
    { label: 'Disk reads', value: '52 MB a second' },
    { label: 'Disk writes', value: 'No limit' },
    { label: 'Read requests', value: 'No limit' },
    { label: 'Write requests', value: '500 requests a second' },
    { label: 'Stop signal', value: 'SIGTERM' },
    { label: 'Stop timeout', value: '5 s, then Spark ends it' },
    { label: 'Its file', value: '/system/settings/spark/services/nginx.toml' },
  ]);
  const plain = limitsRows({ name: 'a', holds: { memory_bytes: null, cpu_weight: 100, io_weight: 100, threads: 4096, open_files: null } });
  assert.deepEqual(plain.filter((row) => ['Memory', 'Open files'].includes(row.label)), [
    { label: 'Memory', value: 'No limit' },
    { label: 'Open files', value: 'The usual 1,024 for each process' },
  ]);
  assert.equal(plain.some((row) => row.label === 'Stop signal' || row.label === 'Stop timeout'), false, 'a document that does not say how it stops is not made to');
  assert.deepEqual(limitsRows({ name: 'a' }), []);
  assert.deepEqual(limitsRows(undefined), []);
});

test('the limits are changed with service try, which edits the file and undoes the change unless it is kept', () => {
  assert.equal(tryCommand('nginx'), 'elevate service try nginx');
  assert.equal(fileOf('Notes_2'), '/system/settings/spark/services/Notes_2.toml');
});

test('the sockets are those Spark holds, and say whether the service takes the connections now', () => {
  const rows = (state) => socketsRows({ ...withLimits, state });
  assert.deepEqual(rows('listening'), [
    { label: 'Sockets', items: ['port 80', 'port 443'] },
    { label: 'Now', value: 'Waiting for a connection. The first one starts the service.' },
    { label: 'Also in the groups', items: ['web'] },
  ]);
  assert.equal(rows('running')[1].value, 'Running, and takes the connections');
  assert.equal(rows('starting')[1].value, 'Running, and takes the connections');
  assert.equal(rows('stopped')[1].value, 'Not running');
  assert.deepEqual(socketsRows({ holds: { sockets: [], groups: ['web'] }, state: 'running' }), [{ label: 'Also in the groups', items: ['web'] }]);
  assert.equal(socketsRows({ holds: { sockets: [], groups: [] }, state: 'running' }), null);
  assert.equal(socketsRows({ holds: {}, state: 'running' }), null);
  assert.equal(socketsRows(undefined), null);
});

test('the use is what the domain has used and been held to, and the memory limit is said only once it has done something', () => {
  assert.deepEqual(useRows(withLimits.usage), [
    { label: 'Memory now', value: '148 MB, at most 212 MB so far' },
    { label: 'Memory limit', value: 'It refused 3 requests for more memory' },
    { label: 'Last end', value: 'Stopped at its memory limit' },
    { label: 'Threads', value: '14' },
    { label: 'Read from disks', value: '9.8 GB' },
    { label: 'Written to disks', value: '1.2 MB' },
    { label: 'Waited for a disk', value: '850 ms' },
    { label: 'Held back by its rates', value: '12 s' },
  ]);
  const quiet = useRows({ ...withLimits.usage, memory_refused: 1, stopped_at_limit: false });
  assert.deepEqual(quiet.map((row) => row.label), ['Memory now', 'Memory limit', 'Threads', 'Read from disks', 'Written to disks', 'Waited for a disk', 'Held back by its rates']);
  assert.equal(quiet[1].value, 'It refused 1 request for more memory');
  assert.equal(useRows({ ...withLimits.usage, memory_refused: 0 }).some((row) => row.label === 'Memory limit'), false);
  assert.deepEqual(useRows(null), []);
  assert.deepEqual(useRows({ memory_bytes: 5_000_000 }), [{ label: 'Memory now', value: '5 MB' }]);
  assert.match(USE_FOOT, /^Counted since/);
});

test('the charts are the processor and memory of the service, memory drawn up to its limit when the file sets one', () => {
  const [cpu, memory] = chartsOf(withLimits);
  assert.deepEqual(cpu.chart.lines, [{ series: 'service.cpu', label: 'CPU' }]);
  assert.equal(cpu.chart.scale, 'peak');
  assert.deepEqual([memory.chart.scale, memory.total], ['total', 268_435_456]);
  assert.equal(memoryLimitOf(withLimits), 268_435_456);
  const free = chartsOf({ holds: { memory_bytes: null } });
  assert.deepEqual([free[1].chart.scale, free[1].total], ['peak', null]);
  assert.equal(memoryLimitOf({ holds: { memory_bytes: 0 } }), null);
  assert.equal(memoryLimitOf(undefined), null);
});

test('the history of a service is its two series over the range, found by their names with the service as the instance', () => {
  const [cpu, memory] = modelsOf(withLimits, usageAnswer, '24h', NOW);
  assert.equal(cpu.range.id, '24h');
  assert.deepEqual(cpu.lines[0].points.map((point) => point.value), [3.5, 12, 6.2]);
  assert.deepEqual(memory.lines[0].points.map((point) => point.value), [140_000_000, 212_000_000, 148_000_000]);
  assert.equal(memory.ceiling, 268_435_456, 'up to the memory limit');
  assert.equal(noHistory([cpu, memory]), false);
});

test('a machine that has kept nothing of the service says so, however the nothing comes', () => {
  assert.equal(noHistory(modelsOf(withLimits, [], '24h', NOW)), true);
  assert.equal(noHistory(modelsOf(withLimits, [{ series: 'service.cpu{nginx}', points: [] }], '1h', NOW)), true);
  assert.equal(noHistory(modelsOf(withLimits, [{ series: 'service.cpu{nginx}', points: [[NOW, null]] }], '1h', NOW)), true);
  assert.equal(noHistory(modelsOf(withLimits, undefined, '1h', NOW)), true);
  assert.equal(NO_HISTORY, 'No history yet: the machine keeps it from its first 10 minutes on.');
});
