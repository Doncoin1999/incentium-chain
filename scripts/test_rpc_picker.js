// Checks site/assets/rpc.js: reads the endpoint register (pallet `endpoints`, spec 19.2), keeps
// only active RPC and archive endpoints, orders by latency, and falls back to the seeds.
// Run: node scripts/test_rpc_picker.js
const assert = require("assert");
const le = (v, n) => { let s = ""; v = BigInt(v); for (let i = 0; i < n; i++) { s += Number(v & 255n).toString(16).padStart(2, "0"); v >>= 8n; } return s; };
const str = (t) => le(t.length << 2, 1) + Buffer.from(t).toString("hex");
const entry = (kind, url, active, score) => "0x" + "11".repeat(32) + le(kind, 1) + str(url) + str("eu") + le(100n * 10n ** 9n, 16) + le(5, 4) + (active ? "01" : "00") + "00" + le(score, 1);

// A fake network: per URL, a delay (ms) or null for unreachable, and what the register holds.
let net = {}, register = [];
global.WebSocket = class {
  constructor(url) {
    this.url = url;
    const d = net[url];
    setTimeout(() => (d === undefined || d === null ? this.onerror && this.onerror() : this.onopen && this.onopen()), d == null ? 1 : d);
  }
  send(m) {
    const q = JSON.parse(m);
    let result = {};
    if (q.method === "state_getKeysPaged") result = register.map((_, k) => "0xkey" + k);
    if (q.method === "state_queryStorageAt") result = [{ block: "0x", changes: register.map((e, k) => ["0xkey" + k, e]) }];
    setTimeout(() => this.onmessage && this.onmessage({ data: JSON.stringify({ id: q.id, result }) }), 1);
  }
  close() {}
};
global.localStorage = { m: {}, getItem(k) { return this.m[k] ?? null; }, setItem(k, v) { this.m[k] = v; } };
const R = require("../site/assets/rpc.js");

(async () => {
  // the decoder
  const e = R._decodeEntry(entry(1, "wss://archive.example", true, 97));
  assert.deepStrictEqual(e, { kind: "Archive", url: "wss://archive.example", region: "eu", active: true, score: 97 });

  // no register on the chain: the seeds, reachable first
  net = { "wss://rpc.incentium.org": null, "wss://rpc2.incentium.org": 20 };
  register = [];
  let list = await R.refresh(false);
  assert.deepStrictEqual(list, ["wss://rpc2.incentium.org", "wss://rpc.incentium.org"]);

  // a register: inactive, bootnode and plain-http entries are skipped; fastest first; seeds kept
  localStorage.m = {};
  net = { "wss://rpc.incentium.org": 40, "wss://rpc2.incentium.org": 60, "wss://a.example": 10, "wss://b.example": 30, "wss://down.example": null };
  register = [
    entry(0, "wss://a.example", true, 100),
    entry(1, "wss://b.example", true, 95),
    entry(0, "wss://off.example", false, 10),
    entry(2, "/dns/boot.example/tcp/30333/p2p/x", true, 100),
    entry(0, "wss://down.example", true, 99),
  ];
  const into = ["wss://rpc.incentium.org"];
  list = await R.refresh(false, into);
  assert.deepStrictEqual(list, ["wss://a.example", "wss://b.example", "wss://rpc.incentium.org", "wss://rpc2.incentium.org"]);
  assert.deepStrictEqual(into, list, "the page's array is reordered in place");
  // the next visit starts from the cache
  assert.deepStrictEqual(R.list(false), list);
  assert.deepStrictEqual(R.list(true), ["wss://testnet-rpc.incentium.org"]);
  console.log("rpc picker: ok");
})().catch((e) => { console.error(e); process.exit(1); });
