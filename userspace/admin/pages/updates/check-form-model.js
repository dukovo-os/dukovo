// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The values of the "Add a check" question and the arguments of `update
// checks add` they make, as plain values. Read when the question is first
// asked for.

/** The check the board offers ready made: this console answering, which is not on by default. */
export const CONSOLE_CHECK = Object.freeze({ name: 'console', what: 'url', target: 'https://localhost:9090/', insecure: true, required: false });

/** A name of a check as `updates.check-add` takes it. */
const NAME = /^[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}$/;

/**
 * The arguments of `updates.check-add` from the dialog's `form` (`{name, what,
 * target, insecure, required, timeout}`), or `{problem, field}` in words when
 * the dialog's values would be refused.
 */
export function addArgs(form) {
  const name = String(form.name ?? '').trim();
  const target = String(form.target ?? '').trim();
  const timeout = form.timeout === '' || form.timeout == null ? 30 : Number(form.timeout);
  if (!NAME.test(name)) return { problem: 'Give the check a name of letters, numbers, spaces, dots, dashes and underscores, up to 64.', field: 'name' };
  if (form.what === 'url' && !(/^https?:\/\/\S+$/.test(target) && target.length <= 2048)) return { problem: 'Write a web address that starts with http:// or https://.', field: 'target' };
  if (form.what === 'service' && !/^[A-Za-z0-9][A-Za-z0-9_.@:-]*$/.test(target)) return { problem: 'Write the name of a service, such as nginx.', field: 'target' };
  if (!Number.isInteger(timeout) || timeout < 1 || timeout > 600) return { problem: 'The time a check may take is from 1 to 600 seconds.', field: 'timeout' };
  const args = form.what === 'url' ? { name, url: target } : { name, service: target };
  if (form.what === 'url' && form.insecure) args.insecure = true;
  return { args: { ...args, required: form.required === true, timeout } };
}
