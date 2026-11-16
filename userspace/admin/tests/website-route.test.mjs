// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The addresses of the Websites pages, and the sites that use a certificate.

import assert from 'node:assert/strict';
import test from 'node:test';

import { hrefOf, routeOf, sitesOf, sitesUsing } from '../lib/website-route.js';

test('a site\'s page is at its name, and one called add is written so that Add a website is not taken for it', () => {
  assert.equal(hrefOf('shop.example.org'), '/websites/shop.example.org');
  assert.equal(hrefOf('add'), '/websites/%61dd');
  assert.equal(hrefOf('a b'), '/websites/a%20b');
});

test('the part of the address after /websites/ asks for the list, Add a website or a site', () => {
  assert.deepEqual(routeOf(''), { kind: 'list' });
  assert.deepEqual(routeOf(undefined), { kind: 'list' });
  assert.deepEqual(routeOf('/'), { kind: 'list' });
  assert.deepEqual(routeOf('add'), { kind: 'add' });
  assert.deepEqual(routeOf('add/'), { kind: 'add' });
  assert.deepEqual(routeOf('shop.example.org'), { kind: 'site', name: 'shop.example.org' });
  assert.deepEqual(routeOf('%61dd'), { kind: 'site', name: 'add' });
  assert.deepEqual(routeOf('%E0%A4%A'), { kind: 'site', name: '%E0%A4%A' });
});

test('the sites that use a certificate are named with a link, from the list of sites or the answer that holds it', () => {
  const answer = {
    websites: [
      { name: 'shop.example.org', names: ['shop.example.org', 'www.shop.example.org'], certificate: 'shop.example.org' },
      { name: 'blog', names: [], certificate: 'shop.example.org' },
      { name: 'plain', names: ['plain.example.org'], certificate: null },
      { name: 'add', names: ['add.example.org'], certificate: 'add' },
      { nothing: true },
    ],
  };
  assert.deepEqual(sitesUsing('shop.example.org', answer), [
    { name: 'shop.example.org', first: 'shop.example.org', href: '/websites/shop.example.org' },
    { name: 'blog', first: 'blog', href: '/websites/blog' },
  ]);
  assert.deepEqual(sitesUsing('add', answer), [{ name: 'add', first: 'add.example.org', href: '/websites/%61dd' }]);
  assert.deepEqual(sitesUsing('shop.example.org', answer.websites).length, 2);
  assert.deepEqual(sitesUsing('none', answer), []);
  assert.deepEqual(sitesUsing('x', null), []);
  assert.deepEqual(sitesOf({ websites: 'nope' }), []);
});
