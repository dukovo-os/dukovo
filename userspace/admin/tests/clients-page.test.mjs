// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/clients (ADR 0690): the AI clients page. What it reads locked and
// unlocked, the cards of what waits with Approve and Refuse, the clients
// and Remove, what they did, the dialog that adds a client and the one
// that says how it connects, and that a token is gone from the page once
// its dialog closes.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  about, areas, AT, client, every, FINGERPRINT, history, NOW, request, requests, TOKEN,
} from './clients-fixtures.mjs';
import {
  buttonOf, holdings, openClients, refusal, settle, text,
} from './clients-view.mjs';

const ADD = '/api/1/account/clients/add';
const APPROVE = '/api/1/clients/requests/12/approve';
const REFUSE = '/api/1/clients/requests/12/refuse';

/** How often each path of `calls` was read. */
const count = (calls, path) => calls.filter((one) => one === path).length;

/** The pressed button of the group of `root` named `label`. */
const pressedIn = (root, label) => text(root.querySelectorAll('[role="group"]').find((group) => group.getAttribute('aria-label') === label)
  .querySelectorAll('button').find((button) => button.getAttribute('aria-pressed') === 'true'));

/** Presses the level `words` of the area `title` in the Add dialog. */
async function level(page, title, words) {
  const group = page.dialog().querySelectorAll('[role="group"]').find((one) => one.getAttribute('aria-label') === title);
  await page.press(group, words);
}

/** The text of the box of `root` that is named `label`. */
const boxOf = (root, label) => root.querySelectorAll('[role="group"]').find((group) => group.getAttribute('aria-label') === label)?.querySelector('pre').textContent;

/** Opens the Add dialog of `page` and types `name`. */
async function adding(page, name = 'desk') {
  page.view.headed.buttons[0].click();
  await settle();
  const dialog = page.dialog();
  await page.type(dialog.querySelector('input.mono'), name);
  return dialog;
}

test('importing the page does nothing and needs no page', async () => {
  assert.equal(typeof globalThis.document, 'undefined');
  const module = await import('../pages/clients/page.js');
  assert.deepEqual([typeof module.show, typeof module.reader], ['function', 'function']);
});

test('the page is headed by what a client is and may do, with Show commands and Add a client', async (context) => {
  const page = await openClients(context);
  const { headed } = page.view;
  assert.equal(headed.title, 'AI clients');
  assert.equal(headed.commands, true);
  assert.match(headed.lead, /^An AI assistant of your choice can look after this server through the Model Context Protocol \(MCP\)\./);
  assert.match(headed.lead, /each change it asks for waits here for an administrator's yes\.$/);
  assert.deepEqual(headed.buttons.map((button) => [text(button), button.disabled]), [['Add a client…', false]]);
  assert.match(text(page.view.main.querySelector('.banner')), /^What a client reads is sent to the AI service behind it\. Add a client only for a service you trust/);
  assert.deepEqual(page.view.main.querySelectorAll('section').map((card) => text(card.querySelector('h2'))), [
    'Waiting for a yes 1', 'Clients', 'What they did Open the journal', 'What no client can do',
  ]);
  assert.equal(page.view.main.querySelectorAll('.chk-list li').length, 4);
});

test('a person who is no administrator is told whose page it is, and nothing is read', async (context) => {
  const page = await openClients(context, { session: { name: 'bob', admin: false }, entry: 'reader' });
  assert.match(text(page.view.main), /^AI clients are for administrators\. bob's account can look but not change/);
  assert.equal(page.view.main.querySelector('a').getAttribute('href'), '/accounts');
  assert.deepEqual(page.calls, []);
});

test('an unlocked console lists every administrator\'s clients, with the command that does', async (context) => {
  const page = await openClients(context);
  assert.equal(count(page.calls, '/api/1/console/clients'), 1);
  assert.equal(count(page.calls, '/api/1/account/clients'), 0);
  const card = page.card('cl-clients');
  assert.equal(text(card.querySelector('caption')), 'Every administrator\'s AI clients');
  assert.deepEqual(card.querySelectorAll('th').map(text), ['Client', 'Connects over', 'Reads', 'May ask to change', 'Last used', 'Ends', 'Action']);
  assert.deepEqual(card.querySelectorAll('tbody tr').map((row) => row.querySelectorAll('td').slice(0, 6).map(text)), [
    ['workstation-assistantchase', 'SSH', 'Every area', 'ServicesCertificatesUpdates', '2 minutes agofrom 192.168.1.20', 'Feb 22, 2027'],
    ['night-watchchase', 'HTTPS, token', 'ServicesLogsStorage', 'Nothing', '6 hours agofrom 192.168.1.31', 'Dec 24'],
    ['alice-laptopalice', 'SSH', 'ServicesLogs', 'Nothing', 'Never', 'Ended Nov 20'],
  ]);
  assert.deepEqual(card.querySelectorAll('tbody button').map((button) => button.getAttribute('aria-label')), [
    'Remove the client workstation-assistant', 'Remove the client night-watch', 'Remove the client alice\'s alice-laptop',
  ]);
  assert.equal(card.querySelector('[data-inline]').getAttribute('data-inline'), 'console.clients');
  assert.equal(buttonOf(card, 'Unlock'), undefined);
});

test('a locked console lists the person\'s own clients and offers to unlock for the rest', async (context) => {
  const page = await openClients(context, { unlocked: false });
  assert.equal(count(page.calls, '/api/1/console/clients'), 0);
  assert.equal(count(page.calls, '/api/1/account/clients'), 1);
  const card = page.card('cl-clients');
  assert.equal(text(card.querySelector('caption')), 'Your AI clients');
  assert.equal(card.querySelectorAll('tbody tr').length, 2);
  assert.match(text(card.querySelector('.cl-own')), /^These are your own clients\. Unlock to see every administrator's\. Unlock$/);
  assert.equal(card.querySelector('[data-inline]'), null);
  const line = card.querySelector('.inline-cmd');
  assert.deepEqual([text(line), line.hidden], ['admin mcp list', false]);
  // The line follows the Show commands switch.
  page.setCommands('hidden');
  assert.equal(line.hidden, true);
  assert.equal(page.card('cl-did').querySelector('.inline-cmd').hidden, true);
  page.setCommands('shown');
  assert.equal(line.hidden, false);
  // Unlocked, the page reads every administrator's.
  page.setUnlocked(true);
  await page.emit('unlock', { unlock_seconds: 900 });
  assert.equal(count(page.calls, '/api/1/console/clients'), 1);
  assert.equal(page.card('cl-clients').querySelectorAll('tbody tr').length, 3);
});

test('an unlock that ran out between the look and the read leaves the person\'s own clients', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/console/clients': refusal('ADM-106', 'Unlock first', 403) } } });
  assert.equal(count(page.calls, '/api/1/account/clients'), 1);
  assert.equal(page.card('cl-clients').querySelectorAll('tbody tr').length, 2);
  assert.notEqual(buttonOf(page.card('cl-clients'), 'Unlock'), undefined);
});

test('no client yet is said so, with what adding one is for', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/console/clients': { clients: [] } } } });
  assert.match(text(page.card('cl-clients')), /No client is let in\. Add one, and an AI assistant of your choice can look after this server\./);
  assert.equal(page.card('cl-clients').querySelector('table'), null);
});

test('what waits is a card with who asked, the reason, the command and what a yes would change', async (context) => {
  const page = await openClients(context);
  const card = page.card('cl-waiting');
  const pill = card.querySelector('h2 .pill');
  assert.deepEqual([text(pill), pill.hidden], ['1', false]);
  const ask = card.querySelector('.ask');
  assert.equal(ask.getAttribute('aria-label'), 'Request 12: Restart a service: nginx');
  assert.equal(text(ask.querySelector('.ask-head b')), 'Restart a service: nginx');
  assert.equal(text(ask.querySelector('.pill')), 'Waiting 2 minutes');
  assert.equal(text(ask.querySelector('.ask-who')), 'Asked by workstation-assistant, chase\'s client, at 9:40 AM · request 12 · lapses at 10:40 AM if nobody answers');
  assert.equal(text(ask.querySelector('.said')), 'The client\'s reason, in its own wordsnginx stopped answering after last night\'s certificate renewal. A restart loads the new certificate.');
  assert.equal(boxOf(ask, 'The command this runs'), 'elevate service nginx restart');
  assert.deepEqual(ask.querySelectorAll('dl dt').map(text), ['Changes anything', 'Files it writes']);
  assert.deepEqual(ask.querySelectorAll('dl dd').map(text), ['Yes.', 'None']);
  assert.deepEqual(page.posted('/api/1/check'), [{ action: 'service.restart', args: { name: 'nginx' } }]);
  assert.match(text(ask), /Judge the command, not the reason: the reason is the AI's\./);
});

test('the check of a request is asked once, however often the page reads', async (context) => {
  const page = await openClients(context);
  await page.emit('clients', { waiting: 1 });
  await page.emit('clients', { waiting: 1 });
  assert.equal(count(page.calls, '/api/1/clients/requests'), 3);
  assert.equal(page.posted('/api/1/check').length, 1);
  assert.equal(text(page.card('cl-waiting').querySelector('dl dd')), 'Yes.');
});

test('how long a request has waited is said again each minute', async (context) => {
  const page = await openClients(context);
  Date.now.mock.mockImplementation(() => NOW + 3 * 60_000);
  await page.tick(60_000);
  assert.equal(text(page.card('cl-waiting').querySelector('.ask .pill')), 'Waiting 5 minutes');
});

test('nothing that waits is said so, with no count', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/clients/requests': { requests: [] } } } });
  const card = page.card('cl-waiting');
  assert.equal(card.querySelector('h2 .pill').hidden, true);
  assert.match(text(card), /Nothing waits\. A change that a client asks for is shown here, with the command a yes would run\./);
  assert.deepEqual(page.allPosted(), []);
});

test('Approve runs the change as the person and says what came of it', async (context) => {
  const page = await openClients(context);
  await page.press(page.card('cl-waiting'), 'Approve');
  assert.deepEqual(page.posted(APPROVE), [{}]);
  assert.deepEqual(page.notices, [['Restart a service: nginx: done.', { kind: 'ok' }]]);
  assert.equal(count(page.calls, '/api/1/clients/requests'), 2, 'the page reads again');
});

test('a change that runs as a task says so, with the way to it', async (context) => {
  const page = await openClients(context, { world: { posts: { [APPROVE]: { task: 1841 } } } });
  await page.press(page.card('cl-waiting'), 'Approve');
  const [[words, options]] = page.notices;
  assert.equal(words, 'Restart a service: nginx: running as task 1841');
  assert.equal(options.action.label, 'Open Tasks');
  options.action.run();
  assert.deepEqual(page.tasks, [1841]);
});

test('a change that was already so says that nothing changed', async (context) => {
  const page = await openClients(context, { world: { posts: { [APPROVE]: { done: true, changed: false, output: [] } } } });
  await page.press(page.card('cl-waiting'), 'Approve');
  assert.deepEqual(page.notices, [['Restart a service: nginx: nothing changed.', { kind: 'ok' }]]);
});

test('a yes that is refused says why on the card, and the page reads again', async (context) => {
  const gone = refusal('ADM-145', 'The client may no longer ask for this', 409);
  const page = await openClients(context, { world: { posts: { [APPROVE]: gone } } });
  await page.press(page.card('cl-waiting'), 'Approve');
  assert.equal(page.problems.length, 1);
  assert.equal(page.problems[0][0], gone);
  assert.match(text(page.card('cl-waiting')), /The client may no longer ask for this/);
  assert.deepEqual(page.notices, []);
  assert.equal(count(page.calls, '/api/1/clients/requests'), 2);
});

test('Approve on a locked console asks to unlock first, with what is about to be done', async (context) => {
  const page = await openClients(context, { unlocked: false });
  await page.press(page.card('cl-waiting'), 'Approve');
  assert.match(text(page.dialog()), /You are about to approve request 12\./);
  assert.deepEqual(page.posted(APPROVE), []);
});

test('Refuse needs no unlock and changes nothing', async (context) => {
  const page = await openClients(context, { unlocked: false });
  await page.press(page.card('cl-waiting'), 'Refuse');
  assert.equal(page.dialog(), null);
  assert.deepEqual(page.posted(REFUSE), [{}]);
  assert.deepEqual(page.notices, [['Request 12 is refused. Nothing was changed.', { kind: 'ok' }]]);
});

test('a request whose command cannot be worked out can be refused and not approved', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/clients/requests': requests(request({ command: null, files: null })) } } });
  const card = page.card('cl-waiting');
  assert.equal(buttonOf(card, 'Approve').disabled, true);
  assert.match(text(card.querySelector('[role="alert"]')), /^The console could not work out what this would run, so it cannot be approved\./);
  await page.press(card, 'Approve');
  assert.deepEqual(page.posted(APPROVE), []);
  await page.press(card, 'Refuse');
  assert.deepEqual(page.posted(REFUSE), [{}]);
});

test('a check that fails says that the console could not find out', async (context) => {
  const page = await openClients(context, { world: { posts: { '/api/1/check': refusal('ADM-500', 'It broke', 500) } } });
  assert.equal(text(page.card('cl-waiting').querySelector('dl dd')), 'The console could not find out.');
});

test('the stream\'s clients event reads the page again', async (context) => {
  const page = await openClients(context);
  page.reads['/api/1/clients/requests'] = requests(request(), request({ request: 13, title: 'Update an app: valkey', action: 'app.update', args: { id: 'valkey' } }));
  await page.emit('clients', { waiting: 2 });
  const card = page.card('cl-waiting');
  assert.equal(text(card.querySelector('h2 .pill')), '2');
  assert.deepEqual(card.querySelectorAll('.ask-head b').map(text), ['Restart a service: nginx', 'Update an app: valkey']);
});

test('a card whose read failed says so and reads again, and the others are drawn', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/clients/history': refusal('ADM-500', 'It broke', 500) } } });
  const did = page.card('cl-did');
  assert.match(text(did), /What the clients did could not be read: It broke\./);
  assert.equal(page.card('cl-clients').querySelectorAll('tbody tr').length, 3);
  page.reads['/api/1/clients/history'] = history();
  await page.press(did, 'Try again');
  assert.equal(page.card('cl-did').querySelectorAll('.tl-item').length, 8);
});

test('without the areas no row says what a client reaches, and none can be added', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/clients/areas': refusal('ADM-500', 'It broke', 500) } } });
  assert.match(text(page.card('cl-clients')), /The clients could not be read: It broke\./);
  assert.match(text(page.card('cl-did')), /What the clients did could not be read: It broke\./);
  assert.equal(page.view.headed.buttons[0].disabled, true);
  assert.equal(page.card('cl-waiting').querySelectorAll('.ask').length, 1, 'what waits is still shown');
});

test('what they did lists the newest twelve, with the way to a task, the journal and the log', async (context) => {
  const many = { entries: Array.from({ length: 15 }, (_, at) => ({
    at: AT - at * 60, owner: 'chase', client: 'desk', kind: 'read', since: AT - at * 60, count: at + 1, areas: ['logs'],
  })) };
  const page = await openClients(context, { world: { reads: { '/api/1/clients/history': many } } });
  assert.equal(page.card('cl-did').querySelectorAll('.tl-item').length, 12);
  page.reads['/api/1/clients/history'] = history();
  await page.emit('clients', { waiting: 1 });
  const did = page.card('cl-did');
  assert.deepEqual(did.querySelectorAll('.tl-rail i').map((mark) => mark.getAttribute('class')), ['info', null, 'ok', null, 'ok', 'bad', 'warn', null]);
  await page.press(did, 'Open the task');
  assert.deepEqual(page.tasks, [1841]);
  assert.equal(did.querySelector('h2 a').getAttribute('href'), '/reliability/changes');
  assert.equal(text(did.querySelector('.inline-cmd')), 'elevate log --service admind --grep "AI client"');
  const none = await openClients(context, { world: { reads: { '/api/1/clients/history': { entries: [] } } } });
  assert.match(text(none.card('cl-did')), /No client has done anything yet\./);
});

test('Remove asks first, then removes the person\'s own client by their own route', async (context) => {
  const page = await openClients(context);
  await page.press(page.card('cl-clients').querySelectorAll('tbody tr')[0], 'Remove');
  const dialog = page.dialog();
  assert.equal(text(dialog.querySelector('h2')), 'Remove workstation-assistant?');
  assert.match(text(dialog), /It stops at once: its token or its SSH command lets nothing in from its next message on/);
  assert.equal(dialog.querySelector('[data-panel]'), null, 'no action removes one\'s own');
  assert.deepEqual(page.posted('/api/1/account/clients/remove'), []);
  await page.press(dialog, 'Remove');
  assert.deepEqual(page.posted('/api/1/account/clients/remove'), [{ name: 'workstation-assistant' }]);
  assert.deepEqual(page.notices, [['workstation-assistant is removed.', { kind: 'ok' }]]);
  assert.equal(page.dialog(), null);
  assert.equal(count(page.calls, '/api/1/console/clients'), 2);
});

test('Cancel leaves the client, and a removal that fails says why and keeps the dialog', async (context) => {
  const page = await openClients(context, { world: { posts: { '/api/1/account/clients/remove': refusal('ADM-123', 'There is no such client', 404) } } });
  const row = page.card('cl-clients').querySelectorAll('tbody tr')[1];
  await page.press(row, 'Remove');
  await page.press(page.dialog(), 'Cancel');
  assert.equal(page.dialog(), null);
  assert.deepEqual(page.posted('/api/1/account/clients/remove'), []);
  await page.press(row, 'Remove');
  await page.press(page.dialog(), 'Remove');
  assert.match(text(page.dialog()), /There is no such client/);
  assert.equal(count(page.calls, '/api/1/console/clients'), 1, 'nothing was removed, so nothing is read again');
});

test('another administrator\'s client is removed by the action, which is shown', async (context) => {
  const page = await openClients(context);
  await page.press(page.card('cl-clients').querySelectorAll('tbody tr')[2], 'Remove');
  const dialog = page.dialog();
  assert.equal(text(dialog.querySelector('h2')), 'Remove alice\'s alice-laptop?');
  assert.equal(dialog.querySelector('[data-panel]').getAttribute('data-panel'), 'console.client-remove');
  await page.press(dialog, 'Remove');
  assert.deepEqual(page.ran('console.client-remove'), [{ owner: 'alice', name: 'alice-laptop' }]);
  assert.deepEqual(page.posted('/api/1/account/clients/remove'), []);
  assert.equal(page.dialog(), null);
});

test('the Add dialog starts from reading everything, with the name first and SSH chosen', async (context) => {
  const page = await openClients(context);
  page.view.headed.buttons[0].click();
  await settle();
  const dialog = page.dialog();
  assert.equal(text(dialog.querySelector('h2')), 'Add an AI client');
  assert.equal(page.document.activeElement, dialog.querySelector('input.mono'));
  assert.equal(pressedIn(dialog, 'Start from'), 'Read everything');
  assert.equal(pressedIn(dialog, 'Services'), 'Read');
  assert.equal(pressedIn(dialog, 'Restart and shut down'), 'Off');
  const [ssh, https] = dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
  assert.deepEqual([ssh.checked, https.checked], [true, false]);
  assert.match(text(dialog.querySelector('.radio-card')), /^Over SSH Safest/);
  assert.equal(dialog.querySelectorAll('.area').length, areas().areas.length);
  // The areas that decide who gets in carry a lock, and offer nothing to ask for.
  assert.deepEqual(dialog.querySelectorAll('.area').filter((row) => row.querySelector('.ic-lock')).map((row) => text(row.querySelector('span'))), ['Accounts', 'The console\'s settings']);
  assert.deepEqual(dialog.querySelectorAll('option').map((option) => [option.getAttribute('value'), option.hasAttribute('selected')]), [
    ['30', false], ['90', true], ['180', false], ['366', false],
  ]);
  assert.equal(boxOf(dialog, 'The command that adds this client'), 'admin mcp add NAME --scope \'*:read\' --days 90');
  assert.match(text(dialog), /Adding it is your approval, as chase\. Nothing connects until you add it\./);
});

test('what is chosen in the Add dialog is what the command under it says', async (context) => {
  const page = await openClients(context);
  const dialog = await adding(page, 'desk');
  assert.equal(boxOf(dialog, 'The command that adds this client'), 'admin mcp add desk --scope \'*:read\' --days 90');
  await level(page, 'Services', 'Read all and ask');
  assert.equal(pressedIn(dialog, 'Start from'), 'Choose');
  assert.equal(boxOf(dialog, 'The command that adds this client'), 'admin mcp add desk --scope \'*:read\' --scope services --days 90');
  await level(page, 'Logs', 'Off');
  await page.choose(dialog, 'https');
  await page.pick(dialog.querySelector('select'), '30');
  assert.equal(
    boxOf(dialog, 'The command that adds this client'),
    'admin mcp add desk --https \\\n    --scope services,storage:read,certificates:read,updates:read,accounts:read,console:read \\\n    --days 30',
  );
  await page.press(dialog, 'Read all and ask, everywhere');
  assert.equal(pressedIn(dialog, 'Restart and shut down'), 'Ask');
  assert.equal(boxOf(dialog, 'The command that adds this client'), 'admin mcp add desk --https --scope \'*\' --days 30');
  await page.press(dialog, 'Choose');
  assert.equal(pressedIn(dialog, 'Services'), 'Read all and ask', 'Choose keeps what is chosen');
});

test('the command is left out of the Add dialog while commands are not shown', async (context) => {
  const page = await openClients(context, { prefs: { commands: 'hidden' } });
  const dialog = await adding(page);
  assert.equal(boxOf(dialog, 'The command that adds this client'), undefined);
});

test('a name that will not do is said in the dialog, and nothing is sent', async (context) => {
  const page = await openClients(context);
  const dialog = await adding(page, 'night-watch');
  await page.press(dialog, 'Add the client');
  const input = dialog.querySelector('input.mono');
  assert.match(text(dialog.querySelector('[role="alert"]')), /^You already have a client named night-watch\./);
  assert.equal(input.getAttribute('aria-invalid'), 'true');
  assert.equal(page.document.activeElement, input);
  await page.type(input, 'Desk');
  await page.press(dialog, 'Add the client');
  assert.match(text(dialog.querySelector('[role="alert"]')), /^A name is 1 to 32 lowercase letters, digits and hyphens\./);
  // Another administrator's name is free: clients are named within an account.
  await page.type(input, 'alice-laptop');
  for (const row of dialog.querySelectorAll('.area')) await page.press(row, 'Off');
  await page.press(dialog, 'Add the client');
  assert.equal(text(dialog.querySelector('[role="alert"]')), 'Choose at least one area for it to reach.');
  assert.deepEqual(page.posted(ADD), []);
  assert.notEqual(page.dialog(), null);
});

test('a client added over SSH is told the command its settings run and a check to try', async (context) => {
  const page = await openClients(context);
  const dialog = await adding(page, 'desk');
  await page.press(dialog, 'Add the client');
  assert.deepEqual(page.posted(ADD), [{ proof: '', name: 'desk', way: 'ssh', scopes: ['*:read'], days: 90 }]);
  assert.equal(count(page.calls, '/api/1/console/clients'), 2, 'the page reads again');
  const connect = page.dialog();
  assert.equal(text(connect.querySelector('h2')), 'desk is added');
  assert.match(text(connect.querySelector('.banner')), /^It connects over SSH and signs in as you, with your SSH key\./);
  assert.deepEqual(JSON.parse(boxOf(connect, 'MCP settings for the AI client')), {
    mcpServers: { 'duke-nas': { command: 'ssh', args: ['chase@duke-nas.local', 'admin', 'mcp', 'desk'] } },
  });
  assert.equal(boxOf(connect, 'A check from a terminal'), 'ssh chase@duke-nas.local admin mcp desk --check');
  assert.equal(connect.querySelector('a').getAttribute('href'), '/accounts/chase#keys');
  await page.press(connect, 'Done');
  assert.equal(page.dialog(), null);
});

test('a client added over HTTPS is shown its token once, and nothing of it stays in the page', async (context) => {
  const page = await openClients(context, { world: { posts: { [ADD]: { token: TOKEN } } } });
  const dialog = await adding(page, 'night-desk');
  await page.choose(dialog, 'https');
  await level(page, 'Services', 'Read all and ask');
  await page.press(dialog, 'Add the client');
  assert.deepEqual(page.posted(ADD), [{ proof: '', name: 'night-desk', way: 'https', scopes: ['*:read', 'services'], days: 90 }]);
  const connect = page.dialog();
  assert.equal(text(connect.querySelector('h2')), 'night-desk is added');
  assert.match(text(connect.querySelector('.banner.warn')), /^The token is shown once\. Copy it now, and keep it as you keep a password\./);
  assert.deepEqual(JSON.parse(connect.querySelector('pre').textContent), {
    mcpServers: { 'duke-nas': { type: 'http', url: 'https://duke-nas.local:9090/mcp', headers: { Authorization: `Bearer ${TOKEN}` } } },
  });
  // The machine's own certificate: the client has to be told to trust it.
  const note = connect.querySelector('.banner.info');
  assert.match(text(note), /^This server made its own certificate, so the AI client has to be told to trust it\. cert show this-machine --pem prints the certificate/);
  assert.equal(note.querySelector('.cmdtext').textContent.replace(/[:\n]/g, '').toLowerCase(), FINGERPRINT);
  assert.equal(holdings(note).some((held) => held.includes(TOKEN)), false);
  await page.press(connect, 'Copy');
  assert.match(page.copied.at(-1), new RegExp(`Bearer ${TOKEN}`));
  await page.press(connect, 'I have copied the token');
  assert.equal(page.dialog(), null);
  assert.equal(holdings(page.document.body).some((held) => held.includes(TOKEN)), false, 'the token is nowhere in the page');
});

test('closing the token unkept is asked about first', async (context) => {
  const page = await openClients(context, { world: { posts: { [ADD]: { token: TOKEN } } } });
  const dialog = await adding(page, 'night-desk');
  await page.choose(dialog, 'https');
  await page.press(dialog, 'Add the client');
  const connect = page.dialog();
  assert.equal(connect.fire('cancel').defaultPrevented, true);
  await settle();
  assert.match(text(page.dialog()), /The token is shown only now\. Without it the client cannot connect/);
  await page.press(page.dialog(), 'Go back');
  assert.equal(page.dialog(), connect);
});

test('what the token\'s dialog says of the certificate follows what the console serves', async (context) => {
  const added = async (reads) => {
    const page = await openClients(context, { world: { reads, posts: { [ADD]: { token: TOKEN } } } });
    const dialog = await adding(page, 'night-desk');
    await page.choose(dialog, 'https');
    await page.press(dialog, 'Add the client');
    return page.dialog().querySelector('.banner.info');
  };
  // Another's certificate vouches for the console: nothing to say.
  assert.equal(await added({ '/api/1/console/about': about('00'.repeat(32)) }), null);
  // The console cannot tell: it says how to get the certificate, should the client need it.
  const unknown = await added({ '/api/1/certificates/this-machine': refusal('ADM-500', 'It broke', 500) });
  assert.match(text(unknown), /^If the AI client does not trust this server's certificate, cert show this-machine --pem prints the certificate to give it\./);
});

test('a client that should have a token and got none is not shown as connected', async (context) => {
  const page = await openClients(context);
  const dialog = await adding(page, 'night-desk');
  await page.choose(dialog, 'https');
  await page.press(dialog, 'Add the client');
  assert.equal(page.dialog(), dialog);
  assert.match(text(dialog.querySelector('[role="alert"]')), /^The server added the client and gave no token for it\. Remove it, and add it again\./);
});

test('a refusal to add is said in the dialog, which stays', async (context) => {
  const refused = refusal('ADM-143', 'authd took no proof', 403);
  const page = await openClients(context, { world: { posts: { [ADD]: refused } } });
  const dialog = await adding(page, 'desk');
  await page.press(dialog, 'Add the client');
  assert.equal(page.dialog(), dialog);
  assert.match(text(dialog), /authd took no proof/);
  assert.equal(count(page.calls, '/api/1/console/clients'), 1);
});

test('adding on a locked console asks to unlock first, and sends nothing before', async (context) => {
  const page = await openClients(context, { unlocked: false });
  const dialog = await adding(page, 'desk');
  await page.press(dialog, 'Add the client');
  assert.match(text(page.dialog()), /You are about to add the AI client desk\./);
  assert.deepEqual(page.posted(ADD), []);
});

test('Cancel adds nothing, and a page that is left closes the dialog', async (context) => {
  const page = await openClients(context);
  const dialog = await adding(page, 'desk');
  await page.press(dialog, 'Cancel');
  assert.equal(page.dialog(), null);
  assert.deepEqual(page.posted(ADD), []);
  await adding(page, 'desk');
  page.leave();
  await settle();
  assert.equal(page.dialog(), null);
});

test('the names the dialog holds taken are the person\'s own, as the last read listed them', async (context) => {
  const page = await openClients(context, { world: { reads: { '/api/1/console/clients': { clients: [...every().clients, client({ owner: 'alice', name: 'desk' })] } } } });
  const dialog = await adding(page, 'desk');
  await page.press(dialog, 'Add the client');
  assert.equal(page.posted(ADD).length, 1, 'alice has a client named desk, and chase may have one too');
});
