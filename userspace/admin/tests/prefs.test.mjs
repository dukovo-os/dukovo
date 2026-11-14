// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How a person's display choices become the root's classes (theme.js) and
// how the shell turns the server's preferences into changes to post
// (shell/prefs-model.js).

import assert from 'node:assert/strict';
import test from 'node:test';

import { changeFor, displayOf, THEME_CHOICES, TEXT_CHOICES, MOTION_CHOICES, withTerminalDefaults } from '../shell/prefs-model.js';
import { browser } from './display.mjs';

test('with nothing kept the page is dark, at 100% and with full motion', () => {
  const page = browser();
  assert.deepEqual(page.classes(), ['dark', 'dk']);
});

test('each theme is its own colours, and the sizes and less motion add classes', () => {
  const { display } = browser();
  const classesFor = (shown, system = {}) => display.classesFor(shown, system);
  assert.deepEqual(classesFor({ theme: 'dark' }), ['dark']);
  assert.deepEqual(classesFor({ theme: 'light' }), ['light']);
  assert.deepEqual(classesFor({ theme: 'high-contrast' }), ['hc']);
  assert.deepEqual(classesFor({ theme: 'light', text: 112 }), ['light', 't112']);
  assert.deepEqual(classesFor({ theme: 'light', text: 125 }), ['light', 't125']);
  assert.deepEqual(classesFor({ theme: 'light', text: 150, motion: 'less' }), ['light', 't150', 'calm']);
  assert.deepEqual(classesFor({ motion: 'less' }), ['dark', 'calm']);
});

test('"Same as this device" follows what the device asks for and nothing else does', () => {
  const { display } = browser();
  const device = { theme: 'device' };
  assert.deepEqual(display.classesFor(device, { dark: true }), ['dark']);
  assert.deepEqual(display.classesFor(device, { dark: false }), ['light']);
  assert.deepEqual(display.classesFor(device, {}), ['light']);
  assert.deepEqual(display.classesFor(device, { contrast: true, dark: true }), ['hc']);
  assert.deepEqual(display.classesFor(device, { contrast: true, dark: false }), ['hc']);
  assert.deepEqual(display.classesFor(device, { forced: true }), ['hc'], 'a forced-colour theme of the system');
  assert.deepEqual(display.classesFor({ theme: 'dark' }, { contrast: true, forced: true }), ['dark'], 'a chosen theme stays');
  assert.deepEqual(display.classesFor({ theme: 'light' }, { dark: true }), ['light']);
});

test('a value the console does not have is read as the default, one at a time', () => {
  const { display } = browser();
  assert.deepEqual(display.clean({ theme: 'sepia', text: 112, motion: 'full' }), { theme: 'dark', text: 112, motion: 'full' });
  assert.deepEqual(display.clean({ theme: 'light', text: 111, motion: 'full' }), { theme: 'light', text: 100, motion: 'full' });
  assert.deepEqual(display.clean({ theme: 'light', text: '125', motion: 'none' }), { theme: 'light', text: 100, motion: 'full' });
  assert.deepEqual(display.clean(null), { theme: 'dark', text: 100, motion: 'full' });
  assert.deepEqual(display.clean('light'), { theme: 'dark', text: 100, motion: 'full' });
});

test('the copy this browser kept is what the first paint uses', () => {
  const kept = JSON.stringify({ theme: 'light', text: 125, motion: 'less' });
  assert.deepEqual(browser({ kept }).classes(), ['calm', 'dk', 'light', 't125']);
  const device = JSON.stringify({ theme: 'device', text: 100, motion: 'full' });
  assert.deepEqual(browser({ kept: device, system: { dark: true } }).classes(), ['dark', 'dk']);
});

test('a kept copy that is damaged, or a browser that refuses storage, gives the defaults and does not throw', () => {
  assert.deepEqual(browser({ kept: '{not json' }).classes(), ['dark', 'dk']);
  assert.deepEqual(browser({ kept: '"light"' }).classes(), ['dark', 'dk']);
  assert.deepEqual(browser({ kept: 'null' }).classes(), ['dark', 'dk']);
  const blocked = browser({ storage: 'throws' });
  assert.deepEqual(blocked.classes(), ['dark', 'dk']);
  assert.doesNotThrow(() => blocked.display.keep({ theme: 'light' }));
  assert.deepEqual(blocked.display.read(), blocked.display.DEFAULTS);
});

test('applying a choice swaps the classes and keeps the rest of the root\'s', () => {
  const page = browser({ kept: JSON.stringify({ theme: 'light', text: 150, motion: 'less' }) });
  assert.deepEqual(page.classes(), ['calm', 'dk', 'light', 't150']);
  page.display.apply({ theme: 'dark', text: 100, motion: 'full' });
  assert.deepEqual(page.classes(), ['dark', 'dk']);
  page.display.apply({ theme: 'high-contrast', text: 112, motion: 'full' });
  assert.deepEqual(page.classes(), ['dk', 'hc', 't112']);
});

test('what is kept reads back the same, clean', () => {
  const page = browser();
  page.display.keep({ theme: 'light', text: 112, motion: 'less', zone: 'Europe/Prague' });
  assert.deepEqual(JSON.parse(page.stored.get('dukovo-admin.display.v1')), { theme: 'light', text: 112, motion: 'less' });
  assert.deepEqual(page.display.read(), { theme: 'light', text: 112, motion: 'less' });
});

test('"Same as this device" is followed while the device changes, and other themes are not', () => {
  const page = browser({ kept: JSON.stringify({ theme: 'device', text: 100, motion: 'full' }), system: { dark: false } });
  assert.deepEqual(page.classes(), ['dk', 'light']);
  page.systemChanges('(prefers-color-scheme: dark)', true);
  assert.deepEqual(page.classes(), ['dark', 'dk']);
  page.systemChanges('(prefers-contrast: more)', true);
  assert.deepEqual(page.classes(), ['dk', 'hc']);
  page.systemChanges('(forced-colors: active)', true);
  page.systemChanges('(prefers-contrast: more)', false);
  assert.deepEqual(page.classes(), ['dk', 'hc']);
  page.display.apply({ theme: 'light' });
  page.systemChanges('(prefers-color-scheme: dark)', false);
  assert.deepEqual(page.classes(), ['dk', 'light']);
});

test('the display of the server\'s preferences is the three choices and no more', () => {
  const prefs = { theme: 'light', text: 112, motion: 'less', zone: 'Europe/Prague', commands: 'shown', terminal: { size: 16 } };
  assert.deepEqual(displayOf(prefs), { theme: 'light', text: 112, motion: 'less' });
});

test('a change to post holds only the keys that differ from what the person has', () => {
  const prefs = { theme: 'dark', text: 100, motion: 'full', zone: 'UTC', commands: 'hidden' };
  assert.deepEqual(changeFor(prefs, { theme: 'light' }), { theme: 'light' });
  assert.deepEqual(changeFor(prefs, { theme: 'dark', text: 125 }), { text: 125 });
  assert.deepEqual(changeFor(prefs, { theme: 'dark' }), {});
  assert.deepEqual(changeFor(prefs, { zone: 'Europe/Prague', commands: 'shown' }), { zone: 'Europe/Prague', commands: 'shown' });
  assert.deepEqual(changeFor(prefs, { terminal: { size: 16 } }), { terminal: { size: 16 } });
});

test('each choice the account menu and the pages offer is named in words, and has a value the server takes', () => {
  assert.deepEqual(THEME_CHOICES.map((choice) => choice.value), ['dark', 'light', 'high-contrast', 'device']);
  assert.deepEqual(THEME_CHOICES.map((choice) => choice.label), ['Dark', 'Light', 'High contrast', 'Same as this device']);
  assert.deepEqual(TEXT_CHOICES.map((choice) => [choice.value, choice.label]), [[100, '100%'], [112, '112%'], [125, '125%'], [150, '150%']]);
  assert.deepEqual(MOTION_CHOICES.map((choice) => [choice.value, choice.label]), [['full', 'Full'], ['less', 'Less']]);
});

test('preferences from a server that does not have the newer keys still have them', () => {
  const old = { theme: 'dark', text: 100, motion: 'full', zone: '' };
  const filled = withTerminalDefaults(old);
  assert.equal(filled.commands, 'hidden');
  assert.deepEqual(filled.terminal, {
    size: 14, scrollback: 10000, 'copy-on-select': false, 'keep-days': 7, announce: false, shortcuts: true,
  });
  assert.equal(filled.zone, '');
  const given = withTerminalDefaults({ ...old, commands: 'shown', terminal: { size: 18 } });
  assert.equal(given.commands, 'shown');
  assert.equal(given.terminal.size, 18);
  assert.equal(given.terminal.scrollback, 10000, 'a key it left out is the default');
});
