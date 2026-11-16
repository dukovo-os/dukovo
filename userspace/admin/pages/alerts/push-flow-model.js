// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of getting alerts in this browser as plain values: the name it is
// given, the machine's key as the bytes the browser takes, the subscription as
// the one line that alertd takes, what a refusal means and the words each
// refusal says.

/** What is said when the person, or the browser, did not allow notices. */
export const DENIED_WORDS = 'This browser said no. Allow notifications for this site in the browser\'s settings, then try again.';

/** The words when the browser does not trust this server's certificate: a lead, two ways, and what reaches a phone meanwhile. */
export const CERTIFICATE_WORDS = Object.freeze({
  lead: 'This browser does not trust this server\'s certificate, so it cannot get push notices. Two ways:',
  letsEncrypt: ['Give the console a certificate from Let\'s Encrypt', ' for a name such as duke-nas.example.org (Certificates). On a private network, prove the name by DNS: the name can lead to a private address, and port 80 need not be open. Then use it for this console and open the console by that name.'],
  own: ['Trust this server\'s own certificate', ' on this device: save it (Certificates, this-machine, Save the certificate) and add it to the device\'s trusted certificates, as the device\'s maker describes.'],
  meanwhile: 'Until then, email and webhooks such as ntfy reach your phone.',
});

/**
 * Whether the browser refused the worker because it does not trust the page's
 * certificate: a `SecurityError`, or words that say so. The browsers word it
 * differently, and a page cannot ask whether its certificate is trusted.
 */
export const certificateRefusal = (error) => error?.name === 'SecurityError' || /certificate|ssl|insecure|untrusted/i.test(String(error?.message ?? ''));

/** The most characters a browser's name may have (alertd's own limit). */
export const NAME_MOST = 64;

/** The name offered for this browser, from what the browser says of itself in `userAgentData` (null when it says nothing): "Google Chrome on a phone". */
export function suggestedName(data) {
  const brands = Array.isArray(data?.brands) ? data.brands.map((one) => one.brand) : [];
  const brand = brands.find((word) => !/not.?a.?brand/i.test(word) && word !== 'Chromium') ?? brands.find((word) => word === 'Chromium');
  if (!brand) return 'This browser';
  return `${brand} on ${data.mobile === true ? 'a phone' : 'a computer'}`;
}

/** What is wrong with a name, or null. */
export function nameProblem(name) {
  if (name.trim() === '') return 'Give this browser a name, so that you know it in the list.';
  return name.trim().length > NAME_MOST ? `A name has at most ${NAME_MOST} characters.` : null;
}

/** The machine's key, base64url of the 65 bytes of an uncompressed point, as the bytes `applicationServerKey` takes; null when it is not one. */
export function keyBytes(text) {
  if (typeof text !== 'string' || !/^[A-Za-z0-9_-]{87}=?$/.test(text)) return null;
  const standard = text.replace(/=$/, '').replace(/-/g, '+').replace(/_/g, '/');
  const bytes = Uint8Array.from(atob(`${standard}=`), (letter) => letter.charCodeAt(0));
  return bytes.length === 65 && bytes[0] === 4 ? bytes : null;
}

/** The subscription as the one line of JSON that `alerts.push-add` takes, from `subscription.toJSON()`; null when it is not one. */
export function subscriptionLine(json) {
  const { endpoint, keys } = json ?? {};
  if (typeof endpoint !== 'string' || typeof keys?.p256dh !== 'string' || typeof keys?.auth !== 'string') return null;
  const line = JSON.stringify({ endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } });
  return line.length <= 2048 ? line : null;
}

/** The ID in the answer of `alerts.push-add`, a line of its output, `{"format":1,"id"}`; null when none is. */
export function addedId(answer) {
  for (const line of Array.isArray(answer?.output) ? answer.output : []) {
    try {
      const { id } = JSON.parse(line);
      if (/^[0-9a-f]{16}$/.test(id)) return id;
    } catch {
      // A line that is not JSON is not the ID.
    }
  }
  return null;
}

/** Whether `a` and `b`, the bytes of two keys, are the same. */
export const sameKey = (a, b) => a?.byteLength === b?.byteLength && new Uint8Array(a).every((value, at) => value === new Uint8Array(b)[at]);
