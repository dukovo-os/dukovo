/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The support center's script: search, the menus and the contents filter.
 * search-index.js, written by `ob docs site`, sets DOCS to one entry per
 * page and heading: [title, kind, link from the site's root, summary].
 * The page's <body data-root> says how to reach the root from the page.
 */
(function () {
  'use strict';
  var root = document.body.getAttribute('data-root') || '';
  var docs = window.DOCS || [];

  function esc(text) {
    return String(text).replace(/[&<>"]/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c];
    });
  }

  // Exact names first, then names that start with the query, then the rest.
  function rank(entry, query) {
    var title = entry[0].toLowerCase();
    if (title === query || title.indexOf(query + '(') === 0) return 0;
    if (title.indexOf(query) === 0) return 1;
    if (title.indexOf(query) >= 0) return 2;
    return 3;
  }

  function wireSearch(input, box) {
    var selected = -1, items = [];
    function render() {
      var query = input.value.trim().toLowerCase();
      if (!query) { box.classList.remove('open'); return; }
      items = docs.filter(function (d) {
        return (d[0] + ' ' + d[1] + ' ' + d[3]).toLowerCase().indexOf(query) >= 0;
      });
      items.sort(function (a, b) { return rank(a, query) - rank(b, query); });
      items = items.slice(0, 10);
      box.innerHTML = items.length ? items.map(function (d, i) {
        return '<a href="' + esc(root + d[2]) + '"' + (i === selected ? ' class="sel"' : '') +
          '><em>' + esc(d[1]) + '</em><span>' + esc(d[0]) + '</span><small>' + esc(d[3]) + '</small></a>';
      }).join('') : '<a><span>Nothing found. Try a command name, like <code>service</code>.</span></a>';
      box.classList.add('open');
    }
    input.addEventListener('input', function () { selected = -1; render(); });
    input.addEventListener('keydown', function (e) {
      if (e.key === 'ArrowDown') { selected = Math.min(selected + 1, items.length - 1); render(); e.preventDefault(); }
      else if (e.key === 'ArrowUp') { selected = Math.max(selected - 1, 0); render(); e.preventDefault(); }
      else if (e.key === 'Enter' && items[Math.max(selected, 0)]) { location.href = root + items[Math.max(selected, 0)][2]; }
      else if (e.key === 'Escape') { input.value = ''; render(); }
    });
    document.addEventListener('click', function (e) {
      if (!box.contains(e.target) && e.target !== input) box.classList.remove('open');
    });
  }

  var fields = document.querySelectorAll('[data-search]');
  fields.forEach(function (w) { wireSearch(w.querySelector('input'), w.querySelector('.results')); });

  // "/" jumps to the first search field that is showing.
  document.addEventListener('keydown', function (e) {
    var active = document.activeElement;
    if (e.key !== '/' || /INPUT|TEXTAREA|SELECT/.test(active.tagName)) return;
    for (var i = 0; i < fields.length; i++) {
      var input = fields[i].querySelector('input');
      if (input.offsetParent !== null) { input.focus(); e.preventDefault(); return; }
    }
  });

  // The site menu, the contents on small screens, and the search button
  // that opens the search there.
  function toggle(button, target, cls) {
    if (!button || !target) return;
    button.addEventListener('click', function (e) {
      e.stopPropagation();
      var on = !target.classList.contains(cls);
      target.classList.toggle(cls, on);
      button.setAttribute('aria-expanded', on ? 'true' : 'false');
      if (on && cls === 'searching') target.querySelector('.search input').focus();
    });
  }
  var nav = document.getElementById('site-nav');
  toggle(document.querySelector('.menu-btn[aria-controls]'), nav, 'open');
  toggle(document.querySelector('.menu-docs'), document.querySelector('.side'), 'open');
  toggle(document.querySelector('.search-btn'), document.querySelector('.dochead'), 'searching');
  document.addEventListener('click', function (e) { if (nav && !nav.contains(e.target)) nav.classList.remove('open'); });
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape' && nav) nav.classList.remove('open'); });

  // Filter the contents by name.
  var filter = document.querySelector('.side input');
  if (filter) filter.addEventListener('input', function () {
    var query = filter.value.toLowerCase();
    document.querySelectorAll('.side a').forEach(function (a) {
      a.style.display = a.textContent.toLowerCase().indexOf(query) >= 0 ? '' : 'none';
    });
  });

  // The version picker goes to the same page in the chosen version.
  document.querySelectorAll('select[data-versions]').forEach(function (select) {
    select.addEventListener('change', function () {
      if (select.value) location.href = select.value;
    });
  });

  document.documentElement.classList.add('js');
})();
