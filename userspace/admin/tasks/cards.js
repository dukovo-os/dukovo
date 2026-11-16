// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The drawer's list of tasks (design 8): the three groups with the Tasks
// board's cards, a card's Stop and its log. tasks/drawer.js reads this file
// the first time the drawer opens, so the shell's start does not carry it.

import { fill, h, icon, sheet } from '../dom.js';
import { needsLook, titleOf } from '../lib/tasks-model.js';
import {
  canStop, endWords, groupsOf, logDue, percentOf, startedWords, stepWords,
} from '../lib/tasks-words.js';

/**
 * `model` is lib/tasks-model.js's; `changed()` tells the shell the list
 * changed; the rest is as in `createTasks`. Gives `{build, render, showLog}`
 * once the sheet of the progress bars has loaded.
 */
export async function makeCards({ model, get, post, session, time, notice, problem, commands, hasPage, changed }) {
  await sheet('/dialogs.css');
  const logs = new Map();
  let view = null;
  let signature = '';

  async function refreshLog(id) {
    const entry = logs.get(id) ?? { open: true, text: '', at: null };
    if (!logDue(entry.at, Date.now())) return;
    entry.at = Date.now();
    logs.set(id, entry);
    try {
      const task = await get(`/api/1/tasks/${id}`);
      entry.text = (task.log ?? []).join('\n') || 'The log is empty.';
      for (const box of view?.body.querySelectorAll(`[data-log="${id}"]`) ?? []) box.textContent = entry.text;
    } catch (error) {
      entry.at = null;
      for (const box of view?.body.querySelectorAll(`[data-log="${id}"]`) ?? []) box.textContent = error.title ?? 'The log could not be read.';
    }
  }

  async function stop(task, button) {
    const { ask } = await import('../shell/dialogs.js');
    const choice = await ask({
      title: `Stop task ${task.id}?`,
      words: 'Its command is ended; what it did so far stays.',
      iconName: 'warn',
      choices: [{ label: 'Stop the task', value: 'stop', kind: 'danger' }, { label: 'Keep it running', value: 'keep', kind: 'default', safe: true }],
    });
    if (choice !== 'stop') return;
    try {
      const answer = await post(`/api/1/tasks/${task.id}/stop`);
      if (answer?.task) model.put(answer.task);
      notice(`Stopping task ${task.id}.`);
      changed();
      // The card has moved to another group and its Stop is gone, so the
      // place focus went back to is too: it goes to the drawer's heading.
      document.getElementById('drawer-title')?.focus();
    } catch (error) {
      const slot = button.closest('.task')?.querySelector('.task-slot');
      problem(error, { slot: slot ?? undefined });
    }
  }

  function card(task) {
    const t = time();
    const failed = task.state === 'failed';
    const title = titleOf(task);
    const heading = h('b', { id: `task-${task.id}-title` }, title);
    const open = logs.get(task.id)?.open === true;
    const box = h('pre', {
      class: 'log', tabindex: '0', hidden: !open, 'data-log': task.id, 'aria-label': `Log of task ${task.id}`,
    }, logs.get(task.id)?.text ?? '');
    const logButton = h('button', {
      class: 'btn small', type: 'button', 'aria-expanded': String(open), 'data-key': `log-${task.id}`,
    }, open ? 'Hide the log' : 'Show the log');
    logButton.addEventListener('click', () => {
      const entry = logs.get(task.id) ?? { open: false, text: '', at: null };
      entry.open = !entry.open;
      logs.set(task.id, entry);
      box.hidden = !entry.open;
      logButton.setAttribute('aria-expanded', String(entry.open));
      logButton.textContent = entry.open ? 'Hide the log' : 'Show the log';
      if (entry.open) refreshLog(task.id);
    });
    const buttons = [];
    if (canStop(task, session())) {
      const stopButton = h('button', { class: 'btn small', type: 'button', 'data-key': `stop-${task.id}`, 'data-changes': '' }, 'Stop');
      stopButton.addEventListener('click', () => stop(task, stopButton));
      buttons.push(stopButton);
    }
    buttons.push(logButton);

    const percent = percentOf(task);
    const running = task.state === 'waiting' || task.state === 'running';
    const state = running
      ? h('span', { class: 'pill info' }, task.steps > 0 ? `Step ${task.step} of ${task.steps}` : percent === null ? 'Running' : `${percent}%`)
      : needsLook(task) && h('span', { class: `pill ${failed ? 'bad' : 'warn'}` }, failed ? 'Failed' : 'Stopped');
    return h('section', { class: `task${failed ? ' failed' : ''}`, 'aria-labelledby': `task-${task.id}-title` },
      h('div', { class: 'task-head' },
        heading, state,
        h('span', { class: 'small muted task-by' }, startedWords(task, (seconds) => t.when(seconds)))),
      running
        ? [
          h('p', { class: 'small' }, stepWords(task)),
          meter(task, title, percent),
        ]
        : h('p', {}, endWords(task), task.ended && ` at ${t.when(task.ended)}`),
      h('div', { class: 'task-slot' }),
      h('div', { class: 'btns' }, ...buttons),
      box,
      task.action && commands.inline(task.action, task.args ?? {}, { always: true }));
  }

  function meter(task, title, percent) {
    const bar = h('i', {});
    if (percent !== null) bar.style.setProperty('--v', String(percent));
    return h('div', {
      class: percent === null ? 'meter wait' : 'meter', role: 'progressbar', 'aria-label': title, 'aria-valuemin': '0', 'aria-valuemax': '100',
      'aria-valuenow': percent === null ? null : String(percent),
    }, bar);
  }

  function group(title, tasks, draw) {
    if (tasks.length === 0) return null;
    return [
      h('h3', {}, title, ' ', h('span', { class: 'small' }, String(tasks.length))),
      draw(tasks),
    ];
  }

  function render() {
    if (!view) return;
    const t = time();
    const groups = groupsOf(model.all(), t.midnight());
    const now = JSON.stringify([groups, session().name, [...logs].map(([id, entry]) => [id, entry.open])]);
    if (now === signature) return;
    signature = now;
    const focused = document.activeElement?.closest?.('[data-key]')?.dataset.key;
    const done = (tasks) => h('ul', { class: 'done-list' }, ...tasks.map((task) => h('li', {},
      h('span', { class: 'tick done' }, icon('check')),
      h('span', { class: 'what' }, titleOf(task), h('br'), h('span', { class: 'small muted' }, task.who)),
      h('time', { datetime: new Date((task.ended ?? task.started) * 1000).toISOString(), title: t.full(task.ended ?? task.started) },
        t.when(task.ended ?? task.started)))));
    const empty = groups.running.length + groups.needsLook.length + groups.finishedToday.length === 0;
    fill(view.body,
      group('Running', groups.running, (tasks) => tasks.map(card)),
      group('Needs a look', groups.needsLook, (tasks) => tasks.map(card)),
      group('Finished today', groups.finishedToday, done),
      empty && h('p', { class: 'muted' }, 'No tasks have run today.'),
      h('p', { class: 'small muted' },
        'Tasks keep running when you close the browser or lose the connection, and you see them from any device you sign in on. Every step also goes into the journal.',
        hasPage('reliability') && [' ', h('a', { href: '/reliability' }, 'Open the journal')]));
    if (focused) view.body.querySelector(`[data-key="${focused}"]`)?.focus();
    for (const [id, entry] of logs) if (entry.open) refreshLog(id);
  }

  return {
    /** Draws the list into `body`, and gives what takes it away. */
    build(body) {
      view = { body };
      signature = '';
      render();
      return () => { view = null; };
    },
    render,
    /** Marks the log of task `id` as one to show open. */
    showLog(id) {
      logs.set(id, { open: true, text: logs.get(id)?.text ?? '', at: null });
    },
  };
}
