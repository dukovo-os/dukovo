// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The manifest that lets a phone add the console to its home screen, and the
// icons it names: valid, named as the console is, with colours from the
// light theme, icons that are what they say, and linked from index.html.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { ROOT } from './graph.mjs';

const manifest = JSON.parse(readFileSync(`${ROOT}manifest.webmanifest`, 'utf8'));
const consoleCss = readFileSync(`${ROOT}console.css`, 'utf8');

/** The width and height that a PNG file says it has, from its header. */
function pngSize(file) {
  const bytes = readFileSync(file);
  assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', `${file} is a PNG`);
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

test('the manifest names the console and opens it as an app', () => {
  assert.equal(manifest.name, 'Dukovo Admin');
  assert.equal(manifest.short_name, 'Dukovo');
  assert.equal(manifest.start_url, '/');
  assert.equal(manifest.scope, '/');
  assert.equal(manifest.display, 'standalone');
});

test('its colours are tokens of the light theme', () => {
  const light = consoleCss.slice(0, consoleCss.indexOf('.dark'));
  for (const colour of [manifest.background_color, manifest.theme_color]) {
    assert.match(colour, /^#[0-9A-F]{6}$/);
    assert.ok(light.toUpperCase().includes(`: ${colour};`) || light.toUpperCase().includes(`: ${colour} `), `${colour} is in the light theme's tokens`);
  }
});

test('its icons are the two sizes of PNG, which may be masked, and the mark', () => {
  const [small, large, mark] = manifest.icons;
  for (const [icon, side] of [[small, 192], [large, 512]]) {
    assert.equal(icon.type, 'image/png');
    assert.equal(icon.sizes, `${side}x${side}`);
    assert.equal(icon.purpose, 'any maskable');
    assert.deepEqual(pngSize(`${ROOT}${icon.src.slice(1)}`), [side, side]);
  }
  assert.equal(small.src, '/icons/app-192.png');
  assert.equal(large.src, '/icons/app-512.png');
  assert.equal(mark.type, 'image/svg+xml');
  assert.ok(existsSync(`${ROOT}${mark.src.slice(1)}`));
});

test('index.html links it, and nothing inline', () => {
  const html = readFileSync(`${ROOT}index.html`, 'utf8');
  assert.match(html, /<link rel="manifest" href="\/manifest\.webmanifest">/);
  assert.doesNotMatch(html, /<style|\sstyle=|\son[a-z]+=/);
});
