// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The console's calls to admind's API under /api/1/: JSON in and out, the
// session cookie sent by the browser, and the session's CSRF token on every
// POST. An answer that is not a success becomes an ApiError, so that each
// page shows an error in the same words.

// The error that says the session is gone: admind has restarted, or the
// session ended where the page could not see it.
const SESSION_ENDED = 'ADM-107';

/**
 * An answer that is not a success. `title` and `detail` are admind's
 * words, `fix` what to do (empty when it says nothing), `id` the error's ID
 * (null for errors the page makes itself), `status` the HTTP status (0
 * when no answer came) and `retryAfter` the seconds to wait that a 429
 * asks for, or null. `field` names the argument a value that cannot work
 * was given for, and `command` is `{argv, status, stderr}` of the program
 * whose failure it is, each null when admind says none.
 */
export class ApiError extends Error {
  constructor({
    id = null, title, detail = '', fix = '', status = 0, retryAfter = null, field = null, command = null,
  }) {
    super(title);
    this.name = 'ApiError';
    Object.assign(this, { id, title, detail, fix, status, retryAfter, field, command });
  }
}

let csrfToken = null;
let sessionEnded = null;
let unreachable = null;

/** Sets the token the session's POSTs carry, from `GET /api/1/session`. */
export function setCsrf(token) {
  csrfToken = token;
}

/** Calls `handler` with the error each time an answer says the session ended. */
export function whenSessionEnds(handler) {
  sessionEnded = handler;
}

/** Calls `handler` each time a request gets no answer from the server at all. */
export function whenUnreachable(handler) {
  unreachable = handler;
}

function failure(response, body) {
  const retryAfter = Number.parseInt(response.headers.get('Retry-After'), 10);
  const given = body?.error;
  const rest = { status: response.status, retryAfter: Number.isFinite(retryAfter) ? retryAfter : null };
  if (given?.title) return new ApiError({ ...given, ...rest });
  return new ApiError({
    title: 'The console got an answer it cannot read',
    detail: `The server answered with status ${response.status} and no explanation.`,
    fix: 'Try again. If it keeps happening, look at the server\'s log.',
    ...rest,
  });
}

async function exchange(method, path, body) {
  const headers = { Accept: 'application/json' };
  const options = { method, headers, credentials: 'same-origin' };
  if (method !== 'GET') {
    headers['Content-Type'] = 'application/json';
    if (csrfToken) headers['X-Dukovo-CSRF'] = csrfToken;
    options.body = JSON.stringify(body ?? {});
  }
  let response;
  try {
    response = await fetch(path, options);
  } catch {
    const error = new ApiError({
      title: 'The console cannot reach the server',
      detail: 'The browser got no answer from it.',
      fix: 'Check that the server is on and that this network reaches it, then try again.',
    });
    unreachable?.(error);
    throw error;
  }
  const answer = await response.json().catch(() => null);
  if (response.ok) return answer;
  const error = failure(response, answer);
  if (error.id === SESSION_ENDED) sessionEnded?.(error);
  throw error;
}

/**
 * Sends `body` as JSON with `method` to `path` and returns the answer's
 * JSON, or null for an answer with no body. `signal` ends the wait when
 * the page has gone, with the browser's AbortError, and not the call: an
 * aborted fetch closes its connection (ADR 0517).
 *
 * @throws {ApiError} when the server answers with an error or not at all.
 */
export function request(method, path, body, { signal } = {}) {
  if (!signal) return exchange(method, path, body);
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const stop = () => reject(signal.reason);
    signal.addEventListener('abort', stop, { once: true });
    exchange(method, path, body).then(resolve, reject).finally(() => signal.removeEventListener('abort', stop));
  });
}

export const get = (path, options) => request('GET', path, undefined, options);
export const post = (path, body, options) => request('POST', path, body, options);
