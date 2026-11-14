// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// WebAuthn between the console's API and the browser. The API sends and
// takes binary values as base64url without padding; `navigator.credentials`
// wants ArrayBuffers and gives them back.

/** Base64url, without padding, of an ArrayBuffer or a typed array. */
export function toBase64url(buffer) {
  const bytes = ArrayBuffer.isView(buffer)
    ? new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
    : new Uint8Array(buffer);
  return btoa(String.fromCharCode(...bytes))
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/, '');
}

/** The ArrayBuffer of a base64url string, with or without padding. */
export function fromBase64url(text) {
  const binary = atob(text.replaceAll('-', '+').replaceAll('_', '/'));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0)).buffer;
}

const withBinaryId = (credential) => ({ ...credential, id: fromBase64url(credential.id) });

/**
 * The options of `navigator.credentials.get` from the API's `publicKey`:
 * the challenge and the ids of the credentials that may answer become
 * ArrayBuffers. Everything else goes on as it is.
 */
export function requestOptions(options) {
  return {
    ...options,
    challenge: fromBase64url(options.challenge),
    allowCredentials: (options.allowCredentials ?? []).map(withBinaryId),
  };
}

/**
 * The options of `navigator.credentials.create` from the API's `publicKey`:
 * the challenge, the user's id and the ids of the credentials to leave out
 * become ArrayBuffers.
 */
export function creationOptions(options) {
  return {
    ...options,
    challenge: fromBase64url(options.challenge),
    user: { ...options.user, id: fromBase64url(options.user.id) },
    excludeCredentials: (options.excludeCredentials ?? []).map(withBinaryId),
  };
}

/**
 * The JSON the sign-in and unlock routes take for a credential the browser
 * gave for `get`. The user handle is left out when the passkey gave none.
 */
export function assertionJson(credential) {
  const { clientDataJSON, authenticatorData, signature, userHandle } = credential.response;
  const response = {
    clientDataJSON: toBase64url(clientDataJSON),
    authenticatorData: toBase64url(authenticatorData),
    signature: toBase64url(signature),
  };
  if (userHandle?.byteLength > 0) response.userHandle = toBase64url(userHandle);
  return { id: toBase64url(credential.rawId), response };
}

/** The JSON the route that adds a passkey takes for a credential the browser gave for `create`. */
export function attestationJson(credential) {
  const { clientDataJSON, attestationObject } = credential.response;
  return {
    response: {
      clientDataJSON: toBase64url(clientDataJSON),
      attestationObject: toBase64url(attestationObject),
    },
  };
}

/**
 * Asks the person's passkey to answer `options` (the API's `publicKey`) and
 * returns the credential as JSON. `signal` ends the prompt early.
 */
export async function getAssertion(options, signal) {
  const credential = await navigator.credentials.get({ publicKey: requestOptions(options), signal });
  return assertionJson(credential);
}

/**
 * Asks the browser to make a passkey from `options` (the API's `publicKey`)
 * and returns the credential as JSON.
 */
export async function createCredential(options, signal) {
  const credential = await navigator.credentials.create({ publicKey: creationOptions(options), signal });
  return attestationJson(credential);
}

/** Whether `hostname` is an IP address, which browsers refuse as a passkey's relying party. */
export function isAddress(hostname) {
  return hostname.startsWith('[') || /^\d{1,3}(\.\d{1,3}){3}$/.test(hostname);
}

/** Whether passkeys can be used on this page: the browser has WebAuthn, and the page is opened by a name. */
export function canUsePasskeys() {
  return 'PublicKeyCredential' in window && !isAddress(location.hostname);
}

/**
 * What to show for a failure of a passkey call, as an object with a
 * `title`, `detail` and `fix` as the API's errors have. The browser's
 * errors say little by design, so that a page cannot tell a refusal from a
 * timeout; an error that already has a title, an ApiError, passes through.
 */
export function passkeyProblem(error) {
  if (error.title) return error;
  switch (error.name) {
    case 'InvalidStateError':
      return {
        title: 'This passkey is already added',
        detail: 'That key or device already holds a passkey for this account.',
        fix: 'Use another key, or remove the old passkey first.',
      };
    case 'SecurityError':
      return {
        title: 'The browser will not use a passkey on this address',
        detail: 'Passkeys work only when the console is opened by the server\'s name, not by its address.',
        fix: 'Open the console by the server\'s name.',
      };
    case 'NotSupportedError':
      return {
        title: 'This device cannot make a passkey of a kind the console accepts',
        detail: 'The console accepts ES256, EdDSA and RS256 keys.',
        fix: 'Use another key or device.',
      };
    default:
      return {
        title: 'The passkey was not used',
        detail: 'The prompt was closed, it timed out, or the passkey did not answer.',
        fix: 'Try again and follow the browser\'s prompt.',
      };
  }
}
