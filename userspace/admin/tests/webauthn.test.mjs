// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// webauthn.js: the conversions between the API's base64url and what
// `navigator.credentials` takes and gives. The browser's own calls are
// replaced by ones that record what they were given.

import assert from 'node:assert/strict';
import test, { afterEach } from 'node:test';

import {
  assertionJson,
  attestationJson,
  createCredential,
  creationOptions,
  fromBase64url,
  getAssertion,
  isAddress,
  passkeyProblem,
  requestOptions,
  toBase64url,
} from '../webauthn.js';

const bytes = (...values) => new Uint8Array(values).buffer;

test('base64url has no padding and uses - and _ for the 62nd and 63rd characters', () => {
  // RFC 4648 section 10 and section 5.
  assert.equal(toBase64url(new TextEncoder().encode('f')), 'Zg');
  assert.equal(toBase64url(new TextEncoder().encode('fo')), 'Zm8');
  assert.equal(toBase64url(new TextEncoder().encode('foobar')), 'Zm9vYmFy');
  assert.equal(toBase64url(bytes(0xfb, 0xff, 0xbe)), '-_--');
  assert.equal(toBase64url(bytes()), '');
});

test('base64url reads back what it wrote, with or without padding', () => {
  const original = bytes(0, 1, 2, 250, 251, 252, 253, 254, 255);
  assert.deepEqual(new Uint8Array(fromBase64url(toBase64url(original))), new Uint8Array(original));
  assert.deepEqual(new Uint8Array(fromBase64url('Zg==')), new Uint8Array([0x66]));
  assert.deepEqual(new Uint8Array(fromBase64url('Zg')), new Uint8Array([0x66]));
  assert.deepEqual(new Uint8Array(fromBase64url('-_--')), new Uint8Array([0xfb, 0xff, 0xbe]));
});

test('a typed array that is a view of a larger buffer is encoded by its own bytes', () => {
  const whole = new Uint8Array([9, 9, 1, 2, 3, 9]);
  assert.equal(toBase64url(whole.subarray(2, 5)), toBase64url(bytes(1, 2, 3)));
});

test('the request options get ArrayBuffers for the challenge and the ids, and keep the rest', () => {
  const options = requestOptions({
    challenge: 'AQID',
    rpId: 'duke-nas.local',
    allowCredentials: [{ type: 'public-key', id: 'BAUG' }, { type: 'public-key', id: 'Bwg' }],
    userVerification: 'required',
    timeout: 120000,
  });
  assert.deepEqual(new Uint8Array(options.challenge), new Uint8Array([1, 2, 3]));
  assert.deepEqual(options.allowCredentials.map((c) => [c.type, [...new Uint8Array(c.id)]]), [
    ['public-key', [4, 5, 6]],
    ['public-key', [7, 8]],
  ]);
  assert.equal(options.rpId, 'duke-nas.local');
  assert.equal(options.userVerification, 'required');
  assert.equal(options.timeout, 120000);
});

test('request options without credentials to allow are a sign-in with a passkey alone', () => {
  const options = requestOptions({ challenge: 'AQID', rpId: 'duke-nas.local', allowCredentials: [], userVerification: 'required' });
  assert.deepEqual(options.allowCredentials, []);
  assert.deepEqual(requestOptions({ challenge: 'AQID' }).allowCredentials, []);
});

test('the creation options get ArrayBuffers for the challenge, the user and the ids to leave out', () => {
  const options = creationOptions({
    challenge: 'AQID',
    rp: { id: 'duke-nas.local', name: 'Dukovo Admin' },
    user: { id: 'BAUG', name: 'chase', displayName: 'Chase' },
    pubKeyCredParams: [{ type: 'public-key', alg: -7 }, { type: 'public-key', alg: -8 }, { type: 'public-key', alg: -257 }],
    excludeCredentials: [{ type: 'public-key', id: 'Bwg' }],
    authenticatorSelection: { residentKey: 'required', userVerification: 'required' },
    attestation: 'none',
    timeout: 120000,
  });
  assert.deepEqual([...new Uint8Array(options.challenge)], [1, 2, 3]);
  assert.deepEqual([...new Uint8Array(options.user.id)], [4, 5, 6]);
  assert.deepEqual([options.user.name, options.user.displayName], ['chase', 'Chase']);
  assert.deepEqual([...new Uint8Array(options.excludeCredentials[0].id)], [7, 8]);
  assert.deepEqual(options.pubKeyCredParams.map((p) => p.alg), [-7, -8, -257]);
  assert.equal(options.attestation, 'none');
  assert.equal(options.authenticatorSelection.residentKey, 'required');
});

test('an assertion becomes the JSON of the sign-in routes, with the user handle when there is one', () => {
  const credential = {
    id: 'ignored in favour of rawId',
    rawId: bytes(1, 2, 3),
    response: {
      clientDataJSON: bytes(4, 5),
      authenticatorData: bytes(6, 7, 8),
      signature: bytes(9),
      userHandle: bytes(10, 11),
    },
  };
  assert.deepEqual(assertionJson(credential), {
    id: 'AQID',
    response: { clientDataJSON: 'BAU', authenticatorData: 'BgcI', signature: 'CQ', userHandle: 'Cgs' },
  });
});

test('an assertion with no user handle, or an empty one, has none in its JSON', () => {
  const made = (userHandle) => assertionJson({
    rawId: bytes(1),
    response: { clientDataJSON: bytes(1), authenticatorData: bytes(1), signature: bytes(1), userHandle },
  });
  assert.equal('userHandle' in made(null).response, false);
  assert.equal('userHandle' in made(bytes()).response, false);
  assert.equal('userHandle' in made(undefined).response, false);
});

test('a new credential becomes the JSON of the route that adds a passkey', () => {
  const credential = {
    id: 'not sent',
    rawId: bytes(1),
    response: { clientDataJSON: bytes(4, 5), attestationObject: bytes(6, 7, 8), getTransports: () => ['usb'] },
  };
  assert.deepEqual(attestationJson(credential), {
    response: { clientDataJSON: 'BAU', attestationObject: 'BgcI' },
  });
});

function browserWith(credentials) {
  Object.defineProperty(globalThis, 'navigator', { value: { credentials }, configurable: true, writable: true });
}

afterEach(() => {
  delete globalThis.navigator;
});

test('getAssertion hands the browser converted options and the signal, and gives JSON back', async () => {
  const seen = [];
  browserWith({
    get: async (arguments_) => {
      seen.push(arguments_);
      return {
        rawId: bytes(1, 2, 3),
        response: { clientDataJSON: bytes(4), authenticatorData: bytes(5), signature: bytes(6), userHandle: null },
      };
    },
  });
  const abort = new AbortController();
  const json = await getAssertion({ challenge: 'AQID', rpId: 'duke-nas.local', allowCredentials: [{ type: 'public-key', id: 'BAUG' }] }, abort.signal);
  assert.equal(seen.length, 1);
  assert.equal(seen[0].signal, abort.signal);
  assert.deepEqual([...new Uint8Array(seen[0].publicKey.challenge)], [1, 2, 3]);
  assert.deepEqual([...new Uint8Array(seen[0].publicKey.allowCredentials[0].id)], [4, 5, 6]);
  assert.deepEqual(json, { id: 'AQID', response: { clientDataJSON: 'BA', authenticatorData: 'BQ', signature: 'Bg' } });
});

test('createCredential hands the browser converted options and gives JSON back', async () => {
  const seen = [];
  browserWith({
    create: async (arguments_) => {
      seen.push(arguments_);
      return { response: { clientDataJSON: bytes(4), attestationObject: bytes(5) } };
    },
  });
  const json = await createCredential({
    challenge: 'AQID',
    rp: { id: 'duke-nas.local', name: 'Dukovo Admin' },
    user: { id: 'BAUG', name: 'chase', displayName: 'Chase' },
    pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
  });
  assert.deepEqual([...new Uint8Array(seen[0].publicKey.user.id)], [4, 5, 6]);
  assert.deepEqual(json, { response: { clientDataJSON: 'BA', attestationObject: 'BQ' } });
});

test('a browser that refuses is not hidden: its error goes on to the caller', async () => {
  browserWith({ get: async () => { throw new DOMException('no', 'NotAllowedError'); } });
  await assert.rejects(getAssertion({ challenge: 'AQID' }), { name: 'NotAllowedError' });
});

test('an IP address is not a name a passkey can be made for', () => {
  for (const address of ['192.168.1.20', '10.0.0.1', '[::1]', '[fd00::10]']) assert.equal(isAddress(address), true, address);
  for (const name of ['duke-nas', 'duke-nas.local', 'localhost', 'admin.example.org', '1.example.org']) assert.equal(isAddress(name), false, name);
});

test('a failure of a passkey call is put in words, and an API error is left as it is', () => {
  const named = (name) => passkeyProblem(new DOMException('x', name));
  assert.match(named('InvalidStateError').title, /already added/);
  assert.match(named('SecurityError').title, /address/);
  assert.match(named('NotSupportedError').title, /cannot make a passkey/);
  for (const name of ['NotAllowedError', 'AbortError', 'UnknownError']) {
    assert.equal(named(name).title, 'The passkey was not used', name);
  }
  const fromApi = { title: 'Passkey refused', detail: 'It was not one of yours.', fix: '' };
  assert.equal(passkeyProblem(fromApi), fromApi);
});
