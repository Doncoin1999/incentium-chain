// Checks the decoders on incentium.org/validators against BlockProducersApi (runtime 12,
// pallets/block-producers/src/lib.rs runtime_api). Run: node scripts/test_validators_page.js
const fs = require("fs");
const path = require("path");
const assert = require("assert");
const page = fs.readFileSync(path.join(__dirname, "..", "site", "validators", "index.html"), "utf8");
const lib = page.slice(page.indexOf("/*ic:lib*/"), page.indexOf("/*ic:endlib*/"));
const IC = new Function(lib + "\nreturn IC;")();
const le = (v, n) => { let s = ""; v = BigInt(v); for (let i = 0; i < n; i++) { s += Number(v & 255n).toString(16).padStart(2, "0"); v >>= 8n; } return s; };
const b = (x) => (x ? "01" : "00");
const U = 1000000000n;

const ov = "0x" + [le(12, 4), le(20, 4), le(3, 4), le(7, 4), le(5, 4), le(40, 4)].join("") +
  [le(5000n * U, 16), le(100n * U, 16), le(10n * U, 16)].join("") + b(true) + le(4, 4) + le(30, 4) + le(7, 4) +
  le(144n * U, 16) + le(150n * U, 16) + le(2, 4);
const o = IC.decode.bpOverview(ov);
assert.deepStrictEqual(
  [o.era, o.slots, o.elected, o.producing, o.candidates, o.nominators, o.reserveActive, o.streak, o.streakNeeded, o.minIndependent, o.toHalt],
  [12, 20, 3, 7, 5, 40, true, 4, 30, 7, 2]);
assert.strictEqual(o.totalBacking, 5000n * U);
assert.strictEqual(o.minValidatorBond, 100n * U);
assert.strictEqual(o.pot, 144n * U);
assert.strictEqual(o.lastEraPayout, 150n * U);

const stash = "d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"; // Alice
const v = (g) => stash + b(!g) + b(true) + b(g) + le(1000n * U, 16) + le(1500n * U, 16) + le(3, 4) + le(50_000_000, 4) + b(false) + le(400, 4) + le(60, 4);
const vs = IC.decode.bpValidators("0x08" + v(false) + v(true));
assert.strictEqual(vs.length, 2);
assert.strictEqual(IC.ss58(vs[0].stash), "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY");
assert.deepStrictEqual([vs[0].candidate, vs[0].producing, vs[0].genesis], [true, true, false]);
assert.deepStrictEqual([vs[1].candidate, vs[1].genesis], [false, true]);
assert.strictEqual(vs[0].selfBond, 1000n * U);
assert.strictEqual(vs[0].backing, 1500n * U);
assert.deepStrictEqual([vs[0].nominators, vs[0].commissionPpb, vs[0].blocked, vs[0].pointsLastEra, vs[0].pointsThisEra], [3, 50_000_000, false, 400, 60]);
// the same values as encoded by the runtime (runtime/src/tests_npos.rs, read_api_encoding_...)
const fromRust = { ov: "0x0c0000001400000003000000070000000500000028000000005039278c040000000000000000000000e8764817000000000000000000000000e40b5402000000000000000000000001040000001e0000000700000000a01187210000000000000000000000005cb2ec22000000000000000000000002000000", vs: "0x08d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d0101000010a5d4e800000000000000000000000098f73e5d01000000000000000000000300000080f0fa0200900100003c000000d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d0001010010a5d4e800000000000000000000000098f73e5d01000000000000000000000300000080f0fa0200900100003c000000" };
assert.strictEqual(ov, fromRust.ov, "overview layout matches the runtime");
assert.strictEqual("0x08" + v(false) + v(true), fromRust.vs, "validator layout matches the runtime");
console.log("validators page decoders: ok");
