/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The support center's script: search, the menus and the contents filter.
 * search-index.js, written by `ob docs site`, sets DOCS to one entry per
 * page and per other name of a page: [title, kind, link from the site's root, summary].
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

  // The version picker: the support center's copies on dukovo.org, one per
  // release and one for the main branch, which /support/versions.js lists
  // as [path, name]. It opens the same page in the chosen copy, or that
  // copy's home when the page is not in it. A copy opened anywhere else has
  // no list, and shows its version without a picker.
  var versions = window.DOCS_VERSIONS || [];
  var base = new URL(root || './', location.href).pathname;
  if (versions.length > 1) {
    document.querySelectorAll('select[data-versions]').forEach(function (select) {
      versions.forEach(function (v) {
        var option = document.createElement('option');
        option.value = v[0];
        option.textContent = v[1];
        option.selected = v[0] === base;
        select.appendChild(option);
      });
      select.classList.add('ready');
      select.addEventListener('change', function () {
        var page = location.pathname.slice(base.length);
        fetch(select.value + page, { method: 'HEAD' })
          .then(function (r) { location.href = r.ok ? select.value + page + location.hash : select.value; })
          .catch(function () { location.href = select.value; });
      });
    });
    var badge = document.querySelector('.dochead > .badge');
    if (badge) badge.hidden = true;
  }

  // Supported hardware: status pills in the tables, their filter, and
  // "Check your machine", which reads `lspci -nn` in the browser alone.
  var STATUS = [['Works', 'ok'], ['Partly', 'part'], ['Planned', 'plan'], ['Not yet', 'no']];
  function statusOf(text) {
    for (var i = 0; i < STATUS.length; i++) if (text.indexOf(STATUS[i][0]) === 0) return STATUS[i][1];
    return '';
  }
  function pill(kind, text) {
    return '<span class="hwpill hw-' + kind + '">' + esc(text) + '</span>';
  }
  var hwTables = [];
  document.querySelectorAll('.doc table').forEach(function (table) {
    var head = table.querySelectorAll('th');
    if (head.length < 2 || head[1].textContent.trim() !== 'Status') return;
    hwTables.push(table);
    table.classList.add('hwtable');
    table.querySelectorAll('tbody tr').forEach(function (row) {
      var cell = row.cells[1], text = cell.textContent.trim(), kind = statusOf(text);
      if (!kind) return;
      row.setAttribute('data-hw', kind);
      cell.innerHTML = pill(kind, text.replace(/^Planned, /, 'Planned · '));
    });
  });

  var filter = document.querySelector('[data-hwfilter]');
  if (filter && hwTables.length) {
    filter.innerHTML = '<label class="search"><input type="search" placeholder="Search: RTL8125, NVMe, Wi-Fi, Hyper-V…" aria-label="Search the hardware list" autocomplete="off"></label>' +
      '<div class="hwchips" role="group" aria-label="Show by status"><button type="button" aria-pressed="true" data-s="">All</button>' +
      STATUS.map(function (s) { return '<button type="button" aria-pressed="false" data-s="' + s[1] + '">' + s[0] + '</button>'; }).join('') + '</div>';
    var query = '', want = '';
    var apply = function () {
      hwTables.forEach(function (table) {
        var shown = 0;
        table.querySelectorAll('tbody tr').forEach(function (row) {
          var section = table.previousElementSibling ? table.previousElementSibling.textContent : '';
          var ok = (!want || row.getAttribute('data-hw') === want) &&
            (!query || (section + ' ' + row.textContent).toLowerCase().indexOf(query) >= 0);
          row.hidden = !ok;
          if (ok) shown++;
        });
        table.hidden = !shown;
        var title = table.previousElementSibling;
        if (title && title.tagName === 'H3') title.hidden = !shown;
      });
    };
    filter.querySelector('input').addEventListener('input', function (e) {
      query = e.target.value.trim().toLowerCase();
      apply();
    });
    filter.querySelector('.hwchips').addEventListener('click', function (e) {
      var b = e.target.closest('button');
      if (!b) return;
      want = b.getAttribute('data-s');
      filter.querySelectorAll('.hwchips button').forEach(function (x) { x.setAttribute('aria-pressed', x === b ? 'true' : 'false'); });
      apply();
    });
  }

  var check = document.querySelector('[data-hwcheck]');
  if (check) {
    var ids = document.createElement('script');
    ids.src = root + 'hardware-ids.js';
    ids.onload = function () { hwCheck(check, window.HW_IDS || {}); };
    document.body.appendChild(ids);
  }

  function hwCheck(box, data) {
    function known(rows, ven, dev) {
      return (rows || []).some(function (r) { return r[1] === ven && (' ' + r[2] + ' ').indexOf(' ' + dev + ' ') >= 0; });
    }
    function classify(cls, ven, dev, name) {
      var c2 = cls.slice(0, 2);
      if (cls === '0108') return ['ok', 'NVMe driver'];
      if (cls === '0106') return ['ok', 'AHCI driver'];
      if (cls === '0101') return ['ok', 'IDE driver'];
      if (cls === '0104') return known(data.vmd, ven, dev) ? ['ok', 'Intel VMD: the disks behind it work as they are'] : ['plan', 'Hardware RAID controllers come in 1.12', '1.12'];
      if (cls === '0107') return ['plan', 'SAS HBAs come in 1.12', '1.12'];
      if (cls === '0200') return known(data.network, ven, dev) ? ['ok', 'Wired network driver'] : ['no', 'No driver for this network chip yet. A USB Ethernet adapter works meanwhile.'];
      if (cls === '0280') return ['plan', 'Wi-Fi comes in 1.6. Use Ethernet, a USB adapter or USB tethering until then.', '1.6'];
      if (c2 === '03') return ['part', 'A picture through the firmware’s framebuffer; acceleration comes in 1.6'];
      if (cls === '0403') return ['part', 'Stereo output; microphones and HDMI audio come in 1.7'];
      if (cls === '0c03') return ['ok', 'USB driver'];
      if (cls === '0c05') return ['ok', 'SMBus driver'];
      if (cls === '0805') return ['ok', 'SD card reader driver'];
      if (known(data.cardReader, ven, dev)) return ['ok', 'Realtek card reader driver'];
      if (cls === '0700') return ['ok', 'Serial port driver'];
      if (cls === '0c0a' || cls === '0c0b' || /thunderbolt|usb4/i.test(name)) return ['plan', 'USB4 and Thunderbolt come in 1.6', '1.6'];
      if (cls === '1180' || cls === '0c80') return /i2c|serial io/i.test(name) ? ['ok', 'I2C driver; a touchpad on it works with the desktop in 1.1'] : ['ok', 'Needs no driver'];
      if (cls === '1200') return ['no', 'Accelerators such as NPUs have no driver yet'];
      if (c2 === '05' || c2 === '06' || c2 === '08' || c2 === '11') return ['ok', 'Part of the system; needs no driver'];
      return ['no', 'No driver known yet'];
    }
    box.innerHTML = '<label for="hwpaste">Output of <code>lspci -nn</code></label>' +
      '<textarea id="hwpaste" spellcheck="false" placeholder="00:17.0 SATA controller [0106]: Intel Corporation … [8086:9d03]"></textarea>' +
      '<div class="hwrow"><button type="button" class="primary">Check</button><button type="button">Try an example</button><span class="hwcount"></span></div>' +
      '<div class="hwverdict" aria-live="polite"></div><ul class="hwres"></ul>';
    var text = box.querySelector('textarea'), btns = box.querySelectorAll('.hwrow button');
    var LINE = /^\s*(?:[0-9a-f]{4,5}:)?[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]\s+(.+?)\s*\[([0-9a-f]{4})\]:\s*(.+?)\s*\[([0-9a-f]{4}):([0-9a-f]{4})\]/i;
    var EXAMPLE = '00:00.0 Host bridge [0600]: Intel Corporation Xeon E3-1200 v6/7th Gen Core Processor Host Bridge/DRAM Registers [8086:5914] (rev 08)\n' +
      '00:02.0 VGA compatible controller [0300]: Intel Corporation UHD Graphics 620 [8086:5917] (rev 07)\n' +
      '00:14.0 USB controller [0c03]: Intel Corporation Sunrise Point-LP USB 3.0 xHCI Controller [8086:9d2f] (rev 21)\n' +
      '00:15.0 Signal processing controller [1180]: Intel Corporation Sunrise Point-LP Serial IO I2C Controller #0 [8086:9d60] (rev 21)\n' +
      '00:17.0 SATA controller [0106]: Intel Corporation Sunrise Point-LP SATA Controller [AHCI mode] [8086:9d03] (rev 21)\n' +
      '00:1f.3 Audio device [0403]: Intel Corporation Sunrise Point-LP HD Audio [8086:9d71] (rev 21)\n' +
      '00:1f.4 SMBus [0c05]: Intel Corporation Sunrise Point-LP SMBus [8086:9d23] (rev 21)\n' +
      '01:00.0 Ethernet controller [0200]: Realtek Semiconductor Co., Ltd. RTL810xE PCI Express Fast Ethernet controller [10ec:8136] (rev 07)\n' +
      '02:00.0 Network controller [0280]: Qualcomm Atheros QCA9377 802.11ac Wireless Network Adapter [168c:0042] (rev 31)';
    function run() {
      var found = [];
      text.value.split('\n').forEach(function (line) {
        var m = LINE.exec(line);
        if (!m) return;
        var r = classify(m[2].toLowerCase(), m[4].toLowerCase(), m[5].toLowerCase(), m[3]);
        found.push({ kind: m[1], name: m[3].replace(/\s*\(rev [0-9a-f]+\)$/i, ''), id: (m[4] + ':' + m[5]).toLowerCase(), s: r[0], why: r[1], rel: r[2] });
      });
      var verdict = box.querySelector('.hwverdict'), list = box.querySelector('.hwres');
      box.querySelector('.hwcount').textContent = found.length ? found.length + ' parts found' : '';
      if (!found.length) {
        verdict.innerHTML = text.value.trim() ? '<b>No parts found.</b> Paste what <code>lspci -nn</code> prints: each line ends with IDs in brackets, such as [10ec:8168].' : '';
        list.innerHTML = '';
        return;
      }
      var order = { no: 0, plan: 1, part: 2, ok: 3 };
      found.sort(function (a, b) { return order[a.s] - order[b.s]; });
      var count = function (s) { return found.filter(function (f) { return f.s === s; }).length; };
      var blocking = found.some(function (f) { return f.s === 'no' && /ethernet|sata|non-volatile|usb/i.test(f.kind); });
      var wired = found.some(function (f) { return /ethernet/i.test(f.kind) && f.s === 'ok'; });
      var wifi = found.some(function (f) { return /network controller/i.test(f.kind); });
      verdict.innerHTML = blocking ? '<b>Some essential parts have no driver yet.</b> They are listed first. A USB Ethernet adapter can stand in for a network chip.'
        : !wired && wifi ? '<b>Dukovo runs here with a USB Ethernet adapter.</b> Disks and USB work; this machine has Wi-Fi only, which comes in 1.6.'
        : '<b>Dukovo runs on this machine.</b> ' + count('ok') + ' parts work; ' + (count('part') + count('plan')) + ' get more in later releases.';
      list.innerHTML = found.map(function (f) {
        var label = { ok: 'Works', part: 'Partly', plan: 'Planned · ' + f.rel, no: 'Not yet' }[f.s];
        return '<li><span>' + esc(f.name) + '<small>' + esc(f.kind) + ' · ' + f.id + '</small></span>' + pill(f.s, label) + '<em>' + esc(f.why) + '</em></li>';
      }).join('');
    }
    btns[0].addEventListener('click', run);
    btns[1].addEventListener('click', function () { text.value = EXAMPLE; run(); });
  }

  document.documentElement.classList.add('js');
})();
