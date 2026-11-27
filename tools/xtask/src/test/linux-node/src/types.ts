// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What SWC turns from TypeScript into JavaScript in the linux-node boot
// test.
interface Point {
	x: number;
	y: number;
}

enum Axis {
	X,
	Y,
}

export class Shape {
	constructor(private readonly points: Point[]) {}

	extent(axis: Axis): number {
		const values = this.points.map((point) => (axis === Axis.X ? point.x : point.y));
		return Math.max(...values) - Math.min(...values);
	}
}

export const square = new Shape([
	{ x: 0, y: 0 },
	{ x: 3, y: 0 },
	{ x: 3, y: 3 },
	{ x: 0, y: 3 },
]);
