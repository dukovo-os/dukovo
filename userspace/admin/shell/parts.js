// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The parts that pages give the shell (design 3.5): the Overview's needs, the
// palette's providers and the navigation's badges. Each call has 5 s; a part
// that throws, or runs out of time, is left out and named in the browser's
// console with its page's id, so that the Overview, the palette and the
// navigation never break for one part. A part that is not there is skipped.

/** How long a part has, in milliseconds. */
export const PART_MS = 5000;

/**
 * `pages` is the list of entries; `get(path, {signal})` is api.js's;
 * `session()` and `time()` give the current ones. Gives `collect(name,
 * call)`: for each entry that has the part `name`, `call(module, cx)`'s
 * answer, as `[{entry, value}]`; and `forget()`, which drops every answer the
 * cache holds.
 */
export function makeParts({
  pages, get, session, time, now = () => Date.now(), limit = PART_MS,
  warn = (...words) => console.warn(...words), note = (...words) => console.debug(...words),
}) {
  const kept = new Map();
  // One answer for as long as `ms`, shared by every part that asks the same
  // thing, so that three parts that read the services make one call.
  const cache = (key, ms, make) => {
    const hit = kept.get(key);
    if (hit && now() - hit.at < ms) return hit.promise;
    const promise = Promise.resolve().then(make);
    kept.set(key, { at: now(), promise });
    promise.catch(() => kept.delete(key));
    return promise;
  };

  async function one(entry, name, call) {
    const controller = new AbortController();
    let module;
    try {
      module = await entry.parts[name]();
    } catch (error) {
      note(`The page ${entry.id} has no ${name} part yet:`, error);
      return null;
    }
    let timer;
    const cx = { api: { get: (path) => get(path, { signal: controller.signal }) }, session: session(), time: time(), cache };
    try {
      const value = await Promise.race([
        Promise.resolve().then(() => call(module, cx)),
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error(`it took more than ${limit / 1000} s`)), limit);
        }),
      ]);
      return { entry, value };
    } catch (error) {
      controller.abort();
      warn(`The ${name} part of the page ${entry.id} was left out:`, error);
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  return {
    async collect(name, call) {
      const found = await Promise.all(pages.filter((entry) => entry.parts?.[name]).map((entry) => one(entry, name, call)));
      return found.filter(Boolean);
    },
    // A task that moved or a change that was made can make any kept answer
    // old, so the shell forgets them all on those events: a page that reads
    // again after one is given what is true now.
    forget() {
      kept.clear();
    },
  };
}
