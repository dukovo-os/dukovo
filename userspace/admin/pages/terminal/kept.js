// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What this browser tab shows of the terminals, for the Sessions page to say
// "in this tab": the ids of the panes the workspace has on its socket. The
// workspace keeps them for a while after the person leaves the Terminal.

export const kept = { panes: new Set() };
