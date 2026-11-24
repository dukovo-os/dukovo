// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Who asked for a change that an administrator approved (ADR 0690). The
// journal and a task keep how a change was made in `how`, and one that an
// MCP client asked for says `client NAME of OWNER`; its `who` approved it.

/**
 * What follows the name of `who`, who approved: ", asked for by the AI
 * client desk", with whose client it is when it is another's, or nothing
 * when no client asked.
 */
export function askedBy(how, who) {
  const [, name, owner] = /^client ([a-z0-9-]+) of (\S+)$/.exec(how ?? '') ?? [];
  if (name === undefined) return '';
  return `, asked for by ${owner === who ? 'the' : `${owner}'s`} AI client ${name}`;
}
