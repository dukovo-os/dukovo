// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Alerts page gives the Overview: the step of Finish setting up that
// sets up the alert email, and the things to look at when no alert leaves this
// server or a channel keeps failing. Where alerts go is read only when an
// administrator's console is unlocked, and never otherwise: a locked console
// leaves the step not done, and says to unlock.

import { h } from '../../dom.js';
import { isUnlocked } from '../../unlock.js';

const PATH = '/api/1/alerts/channels';

/** The channels, as `alerts status --json` lists them, or null when this session may not read them now or the read failed. */
async function channelsOf(cx) {
  if (cx.session.admin !== true || !isUnlocked()) return null;
  try {
    const found = await cx.cache(PATH, 5000, () => cx.api.get(PATH));
    return Array.isArray(found) ? found : null;
  } catch {
    return null;
  }
}

/** Whether the channel's last try failed: it has an error and no success after it. */
const failing = (channel) => Array.isArray(channel?.last_error) && (!Number.isFinite(channel.last_ok) || channel.last_error[0] > channel.last_ok);

/** The things to look at, from the channels `channels` (null for none read). */
export function needsOf(channels) {
  if (channels === null) return [];
  if (channels.length === 0) {
    return [{
      id: 'alerts-none', severity: 'info', title: 'No alert leaves this server: set up email or push', detail: 'Alerts show on this server\'s screens only.', href: '/alerts', icon: 'bell', at: null,
    }];
  }
  return channels.filter(failing).map((channel) => ({
    id: `alerts-failing-${channel.kind}-${channel.number ?? channel.id ?? ''}`,
    severity: 'warn',
    title: `Alerts to ${channel.kind === 'email' ? 'email' : channel.channel} are failing`,
    detail: typeof channel.last_error[1] === 'string' ? channel.last_error[1] : 'The last try failed.',
    href: '/alerts#channels',
    icon: 'bell',
    at: channel.last_error[0],
  }));
}

/** The things to look at, for the Overview. */
export async function needs(cx) {
  return needsOf(await channelsOf(cx));
}

/** The words that say how the alert email stands, from the channels `channels` (null when they are not read now). */
export const summaryOf = (channels) => (channels === null ? 'Unlock to see if it is set up' : channels.some((one) => one.kind === 'email') ? 'Email is set up' : 'No email yet');

export const steps = [{
  id: 'alert-email',
  title: 'Get an email when something needs you',
  order: 60,
  points: 0,
  action: 'alerts.email',
  available: async (cx) => cx.session.admin === true,
  done: async (cx) => (await channelsOf(cx))?.some((one) => one.kind === 'email') === true,
  summary: async (cx) => summaryOf(await channelsOf(cx)),

  /** The fields of the email, made when the code that draws them has loaded; a secret is cleared when the step is left. */
  form(view) {
    const holder = h('div', {}, h('p', { class: 'muted' }, 'Loading the fields…'));
    const said = h('p', { class: 'small', role: 'status' });
    let fields = null;
    import('./email-fields.js').then(({ makeEmailFields }) => {
      if (view.signal.aborted) return;
      fields = makeEmailFields(view, { told: false });
      holder.replaceChildren(fields.element);
    });
    const element = h('section', { class: 'card', 'aria-labelledby': 'alert-email-h' },
      h('h2', { id: 'alert-email-h' }, 'Get an email when something needs you'),
      h('p', {}, 'An alert also shows on this server\'s screens, which nobody may be looking at. An email waits for you. Send it through your mail provider, and a test alert follows to show that it arrives.'),
      holder,
      said);
    const wait = () => Object.assign(new Error('loading'), { words: 'The fields are still loading: try again in a moment.' });
    return {
      element,
      /** The arguments, which read the password once; a field that is not right is named. */
      args() {
        if (fields === null) throw wait();
        const mistake = fields.check();
        if (mistake) {
          mistake.input?.focus();
          throw Object.assign(new Error(mistake.words), { words: mistake.words });
        }
        return fields.take();
      },
      /** The arguments that the command under the fields shows: a typed password is `''`, and nothing is read. */
      shown() {
        if (fields === null || fields.check()) throw wait();
        return fields.shown();
      },
      /** Sends a test alert and says whether it reached email. */
      async after() {
        fields?.clear();
        said.textContent = 'Sending a test alert…';
        const sent = await view.api.run('alerts.test', {}, { where: { slot: said }, words: { unlock: 'send a test alert', done: 'The test alert was sent.', busy: 'Sending…' } });
        if (!sent.ok) {
          said.textContent = 'The email is saved. The test alert did not go: send one from Alerts.';
          return;
        }
        const { deliveriesOf } = await import('./test-model.js');
        const mail = (deliveriesOf(sent.result) ?? []).find((one) => String(one.channel).startsWith('email'));
        const why = typeof mail?.error === 'string' && mail.error !== '' ? `: ${mail.error}` : '';
        said.textContent = mail === undefined ? 'Sent a test alert.' : mail.ok === true ? 'Sent a test alert: it reached email.' : `Sent a test alert: it did not reach email${why}.`;
      },
      /** Takes the password out of the field, when the step is left. */
      clear: () => fields?.clear(),
    };
  },
}];
