// Maker secrets for Incentium Swap (spec Section 4). Works in browsers and in Node 20+ (WebCrypto).
//   secret i = HMAC-SHA256(seed, [i]),  hash i = SHA-256(secret i),  i = 0..15
// A seed comes from a deterministic Ethereum signature, or is random and kept in an encrypted
// backup (Polkadot accounts sign with randomised signatures).
"use strict";
(function (root) {
  const subtle = (root.crypto || globalThis.crypto).subtle;
  const enc = new TextEncoder();
  const toHex = (u8) => "0x" + Array.from(u8, (b) => b.toString(16).padStart(2, "0")).join("");
  const fromHex = (h) => { h = h.replace(/^0x/, ""); return new Uint8Array(h.match(/../g).map((x) => parseInt(x, 16))); };

  async function sha256(u8) { return new Uint8Array(await subtle.digest("SHA-256", u8)); }

  async function secretAt(seed, i) {
    const key = await subtle.importKey("raw", seed, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return new Uint8Array(await subtle.sign("HMAC", key, new Uint8Array([i])));
  }

  async function hashesFor(seed) {
    const out = [];
    for (let i = 0; i < 16; i++) out.push(toHex(await sha256(await secretAt(seed, i))));
    return out;
  }

  function seedMessage(chain, nonce) {
    return `Incentium Swap secret seed v1\nchain: ${chain}\norder: ${nonce}\n\nSigning this creates the secrets for your order. It is not a transaction and costs nothing.`;
  }

  async function seedFromSignature(sigHex) { return sha256(fromHex(sigHex)); }

  function randomSeed() { return (root.crypto || globalThis.crypto).getRandomValues(new Uint8Array(32)); }

  async function passwordKey(password, salt) {
    const base = await subtle.importKey("raw", enc.encode(password), "PBKDF2", false, ["deriveKey"]);
    return subtle.deriveKey({ name: "PBKDF2", salt, iterations: 310000, hash: "SHA-256" }, base,
      { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  }

  // Encrypted backup of a seed: {v, chain, order, salt, iv, data}
  async function encryptSeed(seed, password, chain, order) {
    const salt = (root.crypto || globalThis.crypto).getRandomValues(new Uint8Array(16));
    const iv = (root.crypto || globalThis.crypto).getRandomValues(new Uint8Array(12));
    const data = new Uint8Array(await subtle.encrypt({ name: "AES-GCM", iv, additionalData: enc.encode(order) },
      await passwordKey(password, salt), seed));
    return { v: 1, kind: "incentium-swap-seed", chain, order, salt: toHex(salt), iv: toHex(iv), data: toHex(data) };
  }

  async function decryptSeed(backup, password) {
    const plain = await subtle.decrypt({ name: "AES-GCM", iv: fromHex(backup.iv), additionalData: enc.encode(backup.order) },
      await passwordKey(password, fromHex(backup.salt)), fromHex(backup.data));
    return new Uint8Array(plain);
  }

  const api = { toHex, fromHex, sha256, secretAt, hashesFor, seedMessage, seedFromSignature, randomSeed, encryptSeed, decryptSeed };
  if (typeof module !== "undefined") module.exports = api; else root.SwapSecrets = api;
})(typeof window !== "undefined" ? window : globalThis);
