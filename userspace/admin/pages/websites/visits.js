// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The chart of a site's visits and errors over 30 days, with the picture's own
// numbers a click away. A site's page reads this file, and the chart it draws
// with, when the site has been counted; a list of sites reads neither.

import { fill, h } from '../../dom.js';
import { makeChart } from '../../lib/chart.js';
import { dayTime, PARTIAL_WORDS, visitsModel } from './visits-model.js';

/** Draws the chart of `visits` (a site's `visits`) in `slot`; `now` is the time in milliseconds since 1970. */
export function drawVisits(slot, visits, now = Date.now()) {
  const chart = makeChart('visits');
  chart.update(visitsModel(visits, now), dayTime(now));
  fill(slot, chart.element, visits.complete === false && h('p', { class: 'small muted' }, PARTIAL_WORDS));
}
