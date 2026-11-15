// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Which view an address and a session call for (design 3.4), as plain
// values so that a test can check each choice.

// The paths the shell answers itself, apart from the list of pages.
export const SECOND_STEP_PATH = '/second-step';
export const WELCOME_PATH = '/welcome';
export const HELP_PATH = '/help';

/** The entry whose `path` is the longest prefix of `path` at a `/` boundary (`/` owns only itself), with `rest`, the path after it; or null. */
export function matchPage(pages, path) {
  let best = null;
  for (const entry of pages) {
    const own = entry.path;
    const owns = own === '/' ? path === '/' : path === own || path.startsWith(`${own}/`);
    if (owns && (best === null || own.length > best.path.length)) best = entry;
  }
  if (best === null) return null;
  return { entry: best, rest: best.path === '/' ? '' : path.slice(best.path.length).replace(/^\//, '') };
}

const under = (path, base) => path === base || path.startsWith(`${base}/`);

/** `path` without a trailing slash, except `/`. */
export function normal(path) {
  return path.length > 1 ? path.replace(/\/+$/, '') : path;
}

/**
 * The view for `path`, given `session` (null when nobody is signed in) and
 * the person's `prefs` (null until read):
 *
 * - `sign-in`: no session; `second-step`: the sign-in rules limit the
 *   session to adding a second step, whatever the path;
 * - `welcome` with `then`: no time zone yet, so `/welcome` first and the
 *   path asked after; `redirect` with `to`: a better address;
 * - `second-step-page`, `help` with `rest`: pages the shell has itself;
 * - `page` with `entry` and `rest`, or `admins-only` for a reader who asks
 *   for an administrators' page; `none`: no page has that path.
 */
export function chooseView({ pages, session, prefs, path }) {
  if (!session) return { kind: 'sign-in' };
  if (session.needs_step) return { kind: 'second-step' };
  const wanted = normal(path);
  if (wanted !== path) return { kind: 'redirect', to: wanted };
  const first = prefs !== null && prefs.zone === '';
  if (first && wanted !== WELCOME_PATH && !under(wanted, HELP_PATH) && wanted !== SECOND_STEP_PATH) {
    return { kind: 'welcome', then: wanted };
  }
  if (wanted === WELCOME_PATH && prefs !== null && !first) return { kind: 'redirect', to: '/' };
  if (wanted === SECOND_STEP_PATH) return { kind: 'second-step-page' };
  if (under(wanted, HELP_PATH)) return { kind: 'help', rest: wanted.slice(HELP_PATH.length + 1) };
  const found = matchPage(pages, wanted);
  if (found === null) return { kind: 'none' };
  if (found.entry.needs === 'admin' && !session.admin) return { kind: 'admins-only', ...found };
  return { kind: 'page', ...found };
}

/** The tab's title: "PAGE · HOST · Dukovo Admin". */
export function titleFor(page, host) {
  return [page, host, 'Dukovo Admin'].filter(Boolean).join(' · ');
}

/**
 * Whether a click on `link` (`{href, target, download}`) with the event's
 * `{button, ctrlKey, metaKey, shiftKey, altKey}` is for the shell to follow
 * without a page load. A file, another origin, a new tab and a download
 * are the browser's.
 */
export function followsInPage(link, click, origin) {
  if (click.button !== 0 || click.ctrlKey || click.metaKey || click.shiftKey || click.altKey) return false;
  if (link.target && link.target !== '_self') return false;
  if (link.download) return false;
  let to;
  try {
    to = new URL(link.href, origin);
  } catch {
    return false;
  }
  if (to.origin !== origin) return false;
  const last = to.pathname.split('/').pop();
  return !last.includes('.');
}
