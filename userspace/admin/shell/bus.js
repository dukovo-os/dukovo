// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the shell tells a page: 'task', 'change', 'unlock', 'session' and
// 'online'. A page listens with `view.on`, which ends with its signal.

/** A small bus of named events. */
export function makeBus() {
  const listeners = new Map();
  return {
    /** Calls `listener` for each `name`; gives the function that stops it. */
    on(name, listener) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
      return () => listeners.get(name)?.delete(listener);
    },
    /** Tells each listener of `name`. One that throws does not stop the others. */
    emit(name, data) {
      for (const listener of [...(listeners.get(name) ?? [])]) {
        try {
          listener(data);
        } catch (error) {
          console.error(`A listener of ${name} failed:`, error);
        }
      }
    },
  };
}
