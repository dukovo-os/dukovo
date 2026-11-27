// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What src/main.js imports, and a function it does not, which a bundler
// leaves out.
export function greet(names) {
	return `Hello, ${names.join(" and ")}`;
}

export function sum(values) {
	return values.reduce((total, value) => total + value, 0);
}

export function unused() {
	return "left out of the bundle";
}
