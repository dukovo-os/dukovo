// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// New output read aloud (design 13.10, the `announce` preference): the
// words of announce-model.js go into a polite live region, at most every
// 500 ms. Read when the preference is on and a pane has output.

import { Announcer } from './announce-model.js';

export class Voice {
  #announcer = new Announcer();

  #decoder = new TextDecoder();

  #timer = null;

  /** `region` is the live region; `now()` the clock. */
  constructor(region) {
    this.region = region;
  }

  /** Takes output bytes, and says them when it is time. */
  push(bytes) {
    this.#announcer.push(this.#decoder.decode(bytes, { stream: true }));
    if (this.#timer !== null) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      const words = this.#announcer.take(performance.now());
      if (words !== null) this.region.textContent = words;
      if (this.#announcer.waiting) this.push(new Uint8Array(0));
    }, this.#announcer.wait(performance.now()));
  }

  stop() {
    clearTimeout(this.#timer);
    this.#timer = null;
  }
}
