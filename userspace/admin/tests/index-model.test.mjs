// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// palette/index-model.js (design 11.1, 11.3): the list built from the pages,
// a fixture of `GET /api/1/actions` (made from admind's registry, as
// tests/registry.mjs reads it) and what the pages' palette parts give; what a
// query finds in it, the groups it is shown in, and the pills each row wears.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  ACTIONS_GROUP, actionRows, applyRecent, argsReady, buildIndex, cleanRecent, countWords, find, flatten, groupResults, PAGES_GROUP,
  pageRows, phraseFor, pillsFor, reaches, RECENT_GROUP, RECENT_MAX, recentName, remember, runWords, stateOf, thingRows,
} from '../palette/index-model.js';
import { PAGES } from '../pages.js';
import { listedActions } from './registry.mjs';

const ACTIONS = listedActions();
const admin = { name: 'chase', admin: true, terminal: true };
const reader = { name: 'robin', admin: false, terminal: false };

const entry = (id, title, icon, extra = {}) => ({ id, title, icon, path: `/${id}`, load: null, needs: 'person', keywords: [], children: [], sections: [], ...extra });
const SERVICES = entry('services', 'Services', 'services');
const CERTIFICATES = entry('certificates', 'Certificates', 'certs');

const services = (...items) => ({
  phrases: {
    'service.start': 'Start {name}', 'service.restart': 'Restart {name}', 'service.explain': 'Explain {name}', 'service.nowhere': 'Nowhere {name}',
  },
  groups: [{
    group: 'Services', kind: 'service', filters: { state: ['failed', 'running', 'stopped'] },
    items: items.map(([name, state, detail]) => ({
      key: name, title: name, detail, state, href: `/services/${name}`, action: 'service.explain', args: { name },
    })),
  }],
});

const certificates = {
  phrases: { 'certificate.renew': 'Renew {name}' },
  groups: [{
    group: 'Certificates', kind: 'certificate', filters: {},
    items: [{ key: 'this-machine', title: 'this-machine', detail: 'Expires in 59 days', state: 'valid', href: '/certificates/this-machine', action: 'certificate.show', args: { name: 'this-machine' } }],
  }],
};

const COLLECTED = [
  { entry: SERVICES, value: services(['nginx', 'running', 'Web server'], ['diskhealth', 'running', 'Watches the disks'], ['backup-sync', 'failed', 'Copies to the NAS'], ['valkey', 'stopped', '']) },
  { entry: CERTIFICATES, value: certificates },
];

const index = (session = admin, collected = COLLECTED) => buildIndex({ pages: PAGES, actions: ACTIONS, collected, session });
const titles = (results) => results.map((one) => one.row.title);

test('the pages a person may see are rows, with their children and sections, and the pages that are not shown are not', () => {
  const rows = pageRows(PAGES, admin);
  assert.deepEqual(rows.map((one) => one.title), [
    'Overview', 'Logs', 'Reliability', 'Storage', 'Snapshots and backups', 'Snapshots and backups › Backups', 'Move to new hardware',
    'Networking', 'Firewall', 'Accounts',
    'Set up this server',
    'Your account', 'Your account › Display', 'Your account › Time zone', 'Your account › Second step', 'Your account › API tokens',
    'Terminal', 'Terminal › Sessions', 'Terminal › Terminal settings', 'Terminal › Terminal keys',
    'Services', 'Services › Scheduled tasks',
    'Apps', 'Certificates', 'Security', 'Hardware', 'Updates', 'Report a problem', 'Alerts',
  ]);
  const terminal = rows.find((one) => one.title === 'Terminal › Terminal keys');
  assert.deepEqual(terminal, {
    key: 'page:/terminal/settings#keys', kind: 'page', group: PAGES_GROUP, title: 'Terminal › Terminal keys', detail: '',
    keywords: ['shell', 'command line', 'console', 'ssh'], facts: { kind: ['page'] }, rank: 0, icon: 'terminal', state: null,
    target: { path: '/terminal/settings#keys' }, change: false, elevated: false, recent: false,
  });
  const shown = new Set(rows.map((one) => one.target.path.split('#')[0]));
  assert.equal(shown.has('/welcome'), false, 'the welcome page is shown once and not found');
  assert.equal(shown.has('/services'), true);
  assert.equal(shown.has('/certificates'), true);
  const folder = { id: 'folder', title: 'Folder', path: '/folder', load: null, needs: 'person' };
  assert.deepEqual(pageRows([folder], admin), [], 'a folder of parts only has no page to go to');
});

test('a reader does not find the administrators\' pages, and a person with no terminal does not find the terminal', () => {
  const rows = pageRows(PAGES, reader).map((one) => one.title);
  assert.ok(rows.includes('Overview'));
  assert.ok(rows.includes('Your account'));
  assert.equal(rows.some((title) => title.startsWith('Set up')), false);
  assert.equal(rows.some((title) => title.startsWith('Terminal')), false);
  assert.ok(pageRows(PAGES, { admin: false, terminal: true }).some((one) => one.title === 'Terminal'));
});

test('an entry that leaves out children and sections is one row', () => {
  const rows = pageRows([{ id: 'x', title: 'X', path: '/x', load: () => {}, needs: 'person' }], reader);
  assert.deepEqual(rows.map((one) => [one.title, one.icon, one.keywords]), [['X', 'code', []]]);
});

test('whether an address is a page that is registered and that this person may see', () => {
  assert.equal(reaches(PAGES, admin, '/terminal/sessions'), true);
  assert.equal(reaches(PAGES, admin, '/account#display'), true);
  assert.equal(reaches(PAGES, admin, '/?x=1'), true);
  assert.equal(reaches(PAGES, admin, '/services/nginx'), true);
  assert.equal(reaches(PAGES, admin, '/certificates'), true);
  assert.equal(reaches([{ id: 'folder', path: '/folder', load: null, needs: 'person' }], admin, '/folder'), false, 'a folder of parts only is no page');
  assert.equal(reaches(PAGES, admin, '/setup'), true);
  assert.equal(reaches(PAGES, reader, '/setup'), false);
  assert.equal(reaches(PAGES, reader, '/terminal'), false);
  assert.equal(reaches(PAGES, admin, '/nowhere'), false);
  assert.equal(reaches(PAGES, admin, ''), false);
  assert.equal(reaches(PAGES, admin, null), false);
});

test('each action admind lists is a row, with who runs it and an icon of its area', () => {
  const rows = actionRows(ACTIONS);
  assert.equal(rows.length, 148);
  const restart = rows.find((one) => one.key === 'action:service.restart');
  assert.deepEqual(restart, {
    key: 'action:service.restart', kind: 'action', group: ACTIONS_GROUP, title: 'Restart a service', detail: '',
    keywords: ['services', 'service restart'], facts: { kind: ['action'] }, rank: 1, icon: 'services', state: null,
    target: { action: 'service.restart', args: {} }, change: true, elevated: true, recent: false,
  });
  const list = rows.find((one) => one.key === 'action:service.list');
  assert.deepEqual([list.change, list.elevated], [false, false]);
  assert.equal(rows.find((one) => one.key === 'action:certificate.list').icon, 'certs');
  assert.equal(rows.find((one) => one.key === 'action:checks.last').icon, 'security');
  assert.equal(rows.find((one) => one.key === 'action:console.about').icon, 'info');
  assert.equal(rows.find((one) => one.key === 'action:metrics.now').icon, 'reliability');
  const all = rows.find((one) => one.key === 'action:logs.read-all');
  assert.deepEqual([all.icon, all.change, all.elevated], ['logs', false, true]);
  const set = rows.find((one) => one.key === 'action:network.set');
  assert.deepEqual([set.icon, set.change, set.elevated], ['network', true, true]);
  const dropped = rows.find((one) => one.key === 'action:firewall.dropped');
  assert.deepEqual([dropped.icon, dropped.change, dropped.elevated], ['firewall', false, true]);
  const health = rows.find((one) => one.key === 'action:storage.health');
  assert.deepEqual([health.icon, health.change, health.elevated], ['storage', false, true]);
  const disable = rows.find((one) => one.key === 'action:accounts.disable');
  assert.deepEqual([disable.icon, disable.change, disable.elevated], ['accounts', true, true]);
  const name = rows.find((one) => one.key === 'action:account.name');
  assert.deepEqual([name.icon, name.change, name.elevated], ['accounts', true, false], 'Your account is the person\'s own, done as them');
  const off = rows.find((one) => one.key === 'action:ssh.off');
  assert.deepEqual([off.icon, off.change, off.elevated], ['security', true, true], 'SSH is on the Security page');
  assert.equal(rows.find((one) => one.key === 'action:private.add').icon, 'security');
  const take = rows.find((one) => one.key === 'action:snapshots.take');
  assert.deepEqual([take.icon, take.change], ['snapshots', true]);
  const run = rows.find((one) => one.key === 'action:backups.run');
  assert.deepEqual([run.icon, run.change, run.elevated], ['snapshots', true, true]);
  assert.equal(actionRows([{ id: 'lab.fail', title: 'Fail', area: 'lab', changes: true, runs: 'as-the-person' }])[0].icon, 'code');
  assert.deepEqual(actionRows([{ id: 5, title: 'x' }, { id: 'a' }, null]), [], 'a list that is not right gives no rows');
});

test('a thing is a row, and each phrase makes one more for each thing that has arguments', () => {
  const { rows, groups, filters, kinds } = thingRows({ collected: COLLECTED, actions: ACTIONS, pages: PAGES, session: admin });
  assert.deepEqual(groups, ['Services', 'Certificates']);
  assert.deepEqual(kinds, ['service', 'certificate']);
  assert.deepEqual(filters, { state: ['failed', 'running', 'stopped'] });
  const nginx = rows.find((one) => one.key === 'thing:service:nginx');
  assert.deepEqual(nginx, {
    key: 'thing:service:nginx', kind: 'thing', group: 'Services', title: 'nginx', detail: 'Web server', keywords: ['Web server'],
    facts: { kind: ['service'], state: ['running'] }, rank: 2, icon: 'services', state: { word: 'running', label: 'Running', tone: 'ok' },
    target: { path: '/services/nginx' }, change: false, elevated: false, recent: false,
  });
  const restart = rows.find((one) => one.key === 'phrase:service.restart:service:nginx');
  assert.deepEqual(restart, {
    key: 'phrase:service.restart:service:nginx', kind: 'phrase', group: ACTIONS_GROUP, title: 'Restart nginx', detail: '', keywords: ['Web server'],
    facts: { kind: ['action'], state: ['running'] }, rank: 1, icon: 'services', state: null,
    target: { action: 'service.restart', args: { name: 'nginx' } }, change: true, elevated: true, recent: false,
  });
  // Three phrases are of actions that are in the list, for four services; the fourth names no action.
  assert.equal(rows.filter((one) => one.kind === 'phrase' && one.key.startsWith('phrase:service.')).length, 12);
  assert.equal(rows.some((one) => one.key.includes('nowhere')), false, 'a phrase of an action that is not listed makes no row');
  const certificate = rows.find((one) => one.key === 'phrase:certificate.renew:certificate:this-machine');
  assert.deepEqual([certificate.title, certificate.icon, certificate.elevated], ['Renew this-machine', 'certs', true]);
  assert.equal(rows.find((one) => one.key === 'thing:certificate:this-machine').state.tone, 'ok');
});

test('a thing goes to its page when that page is registered, and to its action when it is not', () => {
  const collected = [{
    entry: SERVICES,
    value: {
      phrases: {},
      groups: [{
        group: 'Pages', kind: 'page-thing', items: [
          { key: 'a', title: 'Account', href: '/account', action: 'service.explain', args: { name: 'a' } },
          { key: 'b', title: 'Nowhere', href: '/nowhere', action: 'service.explain', args: { name: 'b' } },
          { key: 'c', title: 'Nothing yet', href: '/nowhere/c' },
          { key: 'd', title: 'Only an action', action: 'service.restart' },
        ],
      }],
    },
  }];
  const { rows } = thingRows({ collected, actions: ACTIONS, pages: PAGES, session: admin });
  assert.deepEqual(rows.map((one) => one.target), [
    { path: '/account' },
    { action: 'service.explain', args: { name: 'b' } },
    null,
    { action: 'service.restart', args: {} },
  ]);
  assert.deepEqual(rows.map((one) => [one.change, one.elevated]), [[false, false], [false, false], [false, false], [true, true]],
    'a thing that goes to an action says who runs it');
});

test('what a part gives that is not right is left out and nothing breaks', () => {
  const collected = [
    { entry: SERVICES, value: null },
    { entry: SERVICES, value: { phrases: 'Restart {name}', groups: 'none' } },
    {
      entry: SERVICES,
      value: {
        phrases: { 'service.restart': 'Restart {name}', 'service.start': 5 },
        groups: [
          null,
          { items: [
            { key: 'one', title: 'One', args: { name: 'one' } },
            { key: 'one', title: 'Same key', args: { name: 'one' } },
            { key: '', title: 'No key' },
            { key: 'x', title: '' },
            null,
            { key: 'two', title: 'Two', args: { other: 'two' } },
            { key: 'three', title: 'Three', args: 'x' },
          ] },
        ],
      },
    },
  ];
  const { rows, groups, kinds } = thingRows({ collected, actions: ACTIONS, pages: PAGES, session: admin });
  assert.deepEqual(groups, ['Services'], 'a provider with no group takes the page\'s title');
  assert.deepEqual(kinds, ['services'], 'and its kind is the page\'s id');
  assert.deepEqual(rows.map((one) => one.title), ['One', 'Restart one', 'Two', 'Three']);
});

test('a phrase is no row when its action refuses the arguments, as "Restart admind" is refused', () => {
  const collected = [{ entry: SERVICES, value: services(['admind', 'running', 'The console'], ['nginx', 'running', '']) }];
  const { rows } = thingRows({ collected, actions: ACTIONS, pages: PAGES, session: admin });
  const keys = rows.map((one) => one.key);
  assert.ok(keys.includes('phrase:service.start:service:admind'), 'starting it is allowed');
  assert.ok(keys.includes('phrase:service.explain:service:admind'));
  assert.equal(keys.includes('phrase:service.restart:service:admind'), false, 'the action refuses the console\'s own service');
  assert.ok(keys.includes('phrase:service.restart:service:nginx'));
  // An action the list gives no schema for takes what it is given.
  const bare = ACTIONS.map(({ input, ...rest }) => rest);
  assert.ok(thingRows({ collected, actions: bare, pages: PAGES, session: admin }).rows.some((one) => one.key === 'phrase:service.restart:service:admind'));
});

test('a phrase takes its words from the arguments, and is nothing when one is missing', () => {
  assert.equal(phraseFor('Restart {name}', { name: 'nginx' }), 'Restart nginx');
  assert.equal(phraseFor('Move {from} to {to}', { from: 'a', to: 'b' }), 'Move a to b');
  assert.equal(phraseFor('Restart {name}', {}), null);
  assert.equal(phraseFor('Restart {name}', { name: null }), null);
  assert.equal(phraseFor('Restart it', {}), 'Restart it');
  assert.equal(phraseFor('Wait {n}', { n: 0 }), 'Wait 0');
});

test('states are words with the board\'s tones', () => {
  assert.deepEqual(stateOf('failed'), { word: 'failed', label: 'Failed', tone: 'bad' });
  assert.equal(stateOf('expiring').tone, 'warn');
  assert.equal(stateOf('running').tone, 'ok');
  assert.equal(stateOf('stopped').tone, 'off');
  assert.equal(stateOf('done').tone, 'ok');
  assert.equal(stateOf('waiting').tone, 'warn');
  assert.equal(stateOf('somewhere-else').tone, 'off');
  assert.equal(stateOf(''), null);
  assert.equal(stateOf(undefined), null);
});

test('the index lists the groups in order, the filters, and two to name in the foot', () => {
  const made = index();
  assert.deepEqual(made.groups, ['Actions', 'Services', 'Certificates', 'Settings and pages']);
  assert.deepEqual(made.filterKeys, ['kind', 'state']);
  assert.deepEqual(made.filters.kind, ['page', 'action', 'service', 'certificate']);
  assert.deepEqual(made.hints, ['kind:service', 'state:failed']);
  assert.deepEqual(made.rows.slice(0, 2).map((one) => one.kind), ['page', 'page'], 'pages come first, then actions, then things');
  const bare = buildIndex({ pages: PAGES, actions: [], collected: [], session: reader });
  assert.deepEqual(bare.groups, ['Actions', 'Settings and pages']);
  assert.deepEqual(bare.hints, ['kind:page']);
  assert.deepEqual(bare.filterKeys, ['kind']);
});

test('"rest ngi" finds Restart nginx first', () => {
  const found = find(index(), 'rest ngi');
  assert.equal(found[0].row.title, 'Restart nginx');
  assert.deepEqual(found[0].marks.slice(0, 4), [0, 1, 2, 3]);
  assert.ok(found.every((one) => one.row.title !== 'Restart a service'), 'the action alone has no "ngi"');
});

test('a phrase is a row only when something is typed that is not a filter', () => {
  const made = index();
  assert.equal(find(made, '').some((one) => one.row.kind === 'phrase'), false);
  assert.equal(find(made, 'state:failed').some((one) => one.row.kind === 'phrase'), false);
  assert.equal(find(made, 'kind:service').some((one) => one.row.kind === 'phrase'), false);
  assert.equal(find(made, 'restart').some((one) => one.row.kind === 'phrase'), true);
  // A phrase that was used lately is in the list at once.
  applyRecent(made.rows, ['phrase:service.restart:service:nginx']);
  assert.deepEqual(find(made, '').filter((one) => one.row.kind === 'phrase').map((one) => one.row.title), ['Restart nginx']);
});

test('filters keep what a provider says, and a phrase is an action that has the state of its thing', () => {
  const made = index();
  assert.deepEqual(titles(find(made, 'state:failed')), ['backup-sync']);
  assert.deepEqual(titles(find(made, 'state:fail')), ['backup-sync'], 'a value is the start of a word');
  assert.deepEqual(titles(find(made, 'restart state:failed')), ['Restart backup-sync']);
  assert.deepEqual(titles(find(made, 'state:running')).sort(), ['diskhealth', 'nginx']);
  assert.deepEqual(titles(find(made, 'kind:service')).sort(), ['backup-sync', 'diskhealth', 'nginx', 'valkey']);
  assert.deepEqual(titles(find(made, 'kind:service nginx')), ['nginx'], 'a phrase is not a service');
  assert.ok(titles(find(made, 'kind:action nginx')).includes('Restart nginx'));
  assert.deepEqual(titles(find(made, 'kind:certificate')), ['this-machine']);
  assert.deepEqual(titles(find(made, 'kind:page overview')), ['Overview']);
  assert.deepEqual(titles(find(made, 'kind:action renew')), ['Renew this-machine', 'Renew a certificate']);
  assert.deepEqual(find(made, 'state:').length, find(made, '').length, 'a filter with no value filters nothing');
  assert.equal(find(made, 'state:failed overview').length, 0, 'a page has no state');
});

test('a query that is no filter of this index is matched as text', () => {
  const made = buildIndex({ pages: PAGES, actions: ACTIONS, collected: [], session: admin });
  assert.deepEqual(made.filterKeys, ['kind']);
  assert.equal(find(made, 'state:failed').length, 0, 'state: is not a key here, so it is a word that nothing has');
});

test('an action is found by its title, by its area and by its id\'s words', () => {
  const made = index();
  assert.equal(find(made, 'restart a serv')[0].row.title, 'Restart a service');
  assert.ok(titles(find(made, 'certificates')).includes('Renew a certificate'), 'by its area');
  assert.ok(titles(find(made, 'service stop')).includes('Stop a service'));
  assert.ok(titles(find(made, 'copies nas')).includes('backup-sync'), 'a thing by its detail');
});

test('the recent list puts a row first on a tie, and is made safe, short and unique', () => {
  assert.equal(recentName('chase'), 'recent.v1.chase');
  assert.deepEqual(cleanRecent(null), []);
  assert.deepEqual(cleanRecent('x'), []);
  assert.deepEqual(cleanRecent(['a', 3, '', 'b', null]), ['a', 'b']);
  assert.equal(cleanRecent(Array.from({ length: 20 }, (_, n) => `k${n}`)).length, RECENT_MAX);
  let list = [];
  for (let n = 0; n < 10; n += 1) list = remember(list, `k${n}`);
  assert.deepEqual(list, ['k9', 'k8', 'k7', 'k6', 'k5', 'k4', 'k3', 'k2']);
  assert.deepEqual(remember(list, 'k5'), ['k5', 'k9', 'k8', 'k7', 'k6', 'k4', 'k3', 'k2']);
  const made = index();
  applyRecent(made.rows, ['action:service.stop', 'gone']);
  assert.equal(made.rows.find((one) => one.key === 'action:service.stop').recent, 0);
  assert.equal(made.rows.find((one) => one.key === 'action:service.start').recent, false);
  // These two score the same for "a service"; the shorter title leads until the longer one is recent.
  const same = (rows) => titles(rows).filter((title) => ['Stop a service', 'Enable a service'].includes(title));
  assert.deepEqual(same(find(made, 'a service')), ['Stop a service', 'Enable a service']);
  applyRecent(made.rows, ['action:service.enable']);
  assert.deepEqual(same(find(made, 'a service')), ['Enable a service', 'Stop a service']);
});

test('with nothing typed the recent rows are a group of their own, first; with something typed they are not', () => {
  const made = index();
  applyRecent(made.rows, ['thing:service:nginx', 'page:/account']);
  // The list is cut at fifty rows, and the actions are more than that.
  const empty = groupResults(made, find(made, '', 200), '');
  assert.deepEqual(empty.map((group) => group.label), [RECENT_GROUP, 'Actions', 'Services', 'Certificates', 'Settings and pages']);
  assert.deepEqual(titles(empty[0].results), ['nginx', 'Your account'], 'in the order they were used');
  assert.equal(empty.find((group) => group.label === 'Services').results.some((one) => one.row.title === 'nginx'), false, 'and not twice');
  const typed = groupResults(made, find(made, 'ngi'), 'ngi');
  assert.equal(typed.some((group) => group.label === RECENT_GROUP), false);
  assert.equal(groupResults(made, find(made, 'state:running'), 'state:running').some((group) => group.label === RECENT_GROUP), false);
});

test('groups come in the order of the index with no empty group, and the arrow keys walk them in that order', () => {
  const made = index();
  // "nginx" in full: a shorter word is found in the words of other actions too ("ngi" in "connection against the firewall").
  const groups = groupResults(made, find(made, 'nginx'), 'nginx');
  assert.deepEqual(groups.map((group) => group.label), ['Actions', 'Services']);
  assert.deepEqual(flatten(groups).map((one) => one.row.title), ['Start nginx', 'Restart nginx', 'Explain nginx', 'nginx']);
  assert.deepEqual(groupResults(made, [], 'zzzz'), []);
  assert.deepEqual(flatten([]), []);
});

test('at most fifty rows come back, whatever is typed', () => {
  const many = Array.from({ length: 80 }, (_, n) => [`web-${n}`, 'running', '']);
  const made = index(admin, [{ entry: SERVICES, value: services(...many) }]);
  assert.equal(find(made, 'web').length, 50);
  assert.equal(find(made, '').length, 50);
  assert.equal(find(made, 'web', 5).length, 5);
});

test('a row\'s pills: Needs Unlock for an administrator\'s change while locked, Administrators only for a reader', () => {
  const made = index();
  const stop = made.rows.find((one) => one.key === 'action:service.stop');
  const list = made.rows.find((one) => one.key === 'action:service.list');
  const phrase = made.rows.find((one) => one.key === 'phrase:service.restart:service:nginx');
  assert.deepEqual(pillsFor(stop, { admin: true, unlocked: false }), [{ id: 'unlock', label: 'Needs Unlock', tone: 'warn' }]);
  assert.deepEqual(pillsFor(phrase, { admin: true, unlocked: false }).map((pill) => pill.label), ['Needs Unlock']);
  assert.deepEqual(pillsFor(stop, { admin: true, unlocked: true }), []);
  assert.deepEqual(pillsFor(stop, { admin: false, unlocked: false }), [{ id: 'admin', label: 'Administrators only', tone: '' }]);
  assert.deepEqual(pillsFor(stop, { admin: false, unlocked: true }).map((pill) => pill.id), ['admin']);
  for (const who of [{ admin: true, unlocked: false }, { admin: false, unlocked: false }]) {
    assert.deepEqual(pillsFor(list, who), [], 'a read has no pill');
    assert.deepEqual(pillsFor(made.rows[0], who), [], 'a page has none');
  }
});

test('the status line says how many rows there are', () => {
  assert.equal(countWords(0), 'No results');
  assert.equal(countWords(1), '1 result');
  assert.equal(countWords(12), '12 results');
});

test('the words of a run from the palette are the row\'s own title', () => {
  assert.deepEqual(runWords('Restart nginx'), {
    unlock: 'restart nginx', running: 'Restart nginx', done: 'Restart nginx: done.', unchanged: 'Restart nginx: nothing changed.', busy: 'Restart nginx…',
  });
});

test('arguments are ready when the action\'s schema says they are all it needs', () => {
  const byId = (id) => ACTIONS.find((one) => one.id === id);
  assert.equal(argsReady(byId('service.list'), {}), true);
  assert.equal(argsReady(byId('service.explain'), {}), false);
  assert.equal(argsReady(byId('service.explain'), { name: 'nginx' }), true);
  assert.equal(argsReady(byId('service.restart'), { name: 'admind' }), false);
  assert.equal(argsReady({ id: 'x' }, {}), false, 'with no schema it cannot be said');
  assert.equal(argsReady(undefined, {}), false);
});
