// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The entry that esbuild and Rollup bundle in the linux-node boot test.
import { greet, sum } from "./util.js";

export function main(names) {
	return `${greet(names)}: ${sum(names.map((name) => name.length))} letters`;
}

console.log(main(["Ada", "Grace", "Linus"]));
