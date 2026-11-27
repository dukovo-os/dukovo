// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The work of the linux-node boot test, once `npm ci` has installed the
// project's packages: esbuild bundles src/main.js through its command,
// which `npx` finds and a shell starts; Rollup bundles it too, with its
// parser in its native module; SWC turns src/types.ts into JavaScript; and
// sharp, with libvips, resizes a picture it makes. Each step writes what
// it made to out/ and says its SHA-256.
//
//   node workload.mjs                  says `linux-node: STEP SHA-256` for
//                                      each step, as ob takes them on the
//                                      build machine's Linux
//   node workload.mjs --expect FILE    checks each against FILE's line,
//                                      says `linux-node: STEP ok` or what
//                                      differs, then `linux-node: all N
//                                      passed` or how many failed
//
// The bundles of esbuild and Rollup run too, and must print what main.js
// prints.

import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const GREETING = "Hello, Ada and Grace and Linus: 13 letters";

// The SHA-256 of `bytes`, in hex.
function sha256(bytes) {
	return createHash("sha256").update(bytes).digest("hex");
}

// Runs `command` through the shell, as npm's scripts run, and gives what
// it printed.
function shell(command) {
	return execSync(command, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
}

// esbuild's command, which npx finds in node_modules/.bin, bundles and
// minifies main.js; the bundle runs.
function esbuild() {
	shell("npx --offline esbuild src/main.js --bundle --minify --format=esm --log-level=warning --outfile=out/esbuild.mjs");
	const printed = shell("node out/esbuild.mjs").trim();
	if (printed !== GREETING) throw new Error(`the bundle printed ${JSON.stringify(printed)}`);
	return readFileSync("out/esbuild.mjs");
}

// Rollup's API bundles main.js as an ES module, leaving out what it does
// not use; the bundle runs.
async function rollup() {
	const { rollup } = require("rollup");
	const bundle = await rollup({ input: "src/main.js", logLevel: "silent" });
	const { output } = await bundle.generate({ format: "es" });
	await bundle.close();
	writeFileSync("out/rollup.mjs", output[0].code);
	if (output[0].code.includes("left out of the bundle")) throw new Error("unused code was kept");
	const printed = shell("node out/rollup.mjs").trim();
	if (printed !== GREETING) throw new Error(`the bundle printed ${JSON.stringify(printed)}`);
	return output[0].code;
}

// SWC turns the TypeScript into JavaScript of ES2020.
function swc() {
	const { transformSync } = require("@swc/core");
	const { code } = transformSync(readFileSync("src/types.ts", "utf8"), {
		jsc: { parser: { syntax: "typescript" }, target: "es2020" },
		module: { type: "es6" },
		sourceMaps: false,
	});
	writeFileSync("out/swc.mjs", code);
	return code;
}

// sharp makes a picture of 256 by 192 pixels, a gradient, resizes it to
// 64 by 48 with libvips's threads, and writes it as a PNG; the pixels are
// what is compared, which no encoder's choices change.
async function sharp() {
	const sharp = require("sharp");
	const [width, height] = [256, 192];
	const pixels = Buffer.alloc(width * height * 3);
	for (let y = 0; y < height; y++) {
		for (let x = 0; x < width; x++) {
			const at = (y * width + x) * 3;
			pixels[at] = x;
			pixels[at + 1] = y;
			pixels[at + 2] = (x * y) & 0xff;
		}
	}
	const raw = { raw: { width, height, channels: 3 } };
	const small = sharp(pixels, raw).resize(64, 48, { kernel: "lanczos3" });
	await small.clone().png().toFile("out/small.png");
	const { data, info } = await small.raw().toBuffer({ resolveWithObject: true });
	if (info.width !== 64 || info.height !== 48) throw new Error(`it is ${info.width} by ${info.height}`);
	return data;
}

const STEPS = [
	["esbuild", esbuild],
	["rollup", rollup],
	["swc", swc],
	["sharp", sharp],
];

async function main() {
	mkdirSync("out", { recursive: true });
	const expectAt = process.argv.indexOf("--expect");
	const expected = new Map();
	if (expectAt > 0) {
		for (const line of readFileSync(process.argv[expectAt + 1], "utf8").split("\n")) {
			const [, step, sum] = line.split(" ");
			if (step) expected.set(step, sum);
		}
	}
	console.log(`linux-node: node ${process.version} on ${process.platform}-${process.arch}`);
	let failed = 0;
	for (const [name, step] of STEPS) {
		let sum;
		try {
			sum = sha256(await step());
		} catch (error) {
			console.log(`linux-node: ${name} FAILED: ${error.stack || error}`);
			failed++;
			continue;
		}
		if (expectAt < 0) {
			console.log(`linux-node: ${name} ${sum}`);
		} else if (expected.get(name) === sum) {
			console.log(`linux-node: ${name} ok`);
		} else {
			console.log(`linux-node: ${name} FAILED: made ${sum}, not ${expected.get(name)}`);
			failed++;
		}
	}
	if (expectAt > 0) {
		console.log(failed ? `linux-node: ${failed} failed` : `linux-node: all ${STEPS.length} passed`);
	}
	process.exitCode = failed ? 1 : 0;
}

await main();
