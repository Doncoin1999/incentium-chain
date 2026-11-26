// Checks incentium.org/governance against raw storage from a real referendum, produced by the
// runtime test `governance_page_fixture` (runtime/src/tests.rs). Run: node scripts/test_governance_page.js
const fs = require("fs");
const path = require("path");
const assert = require("assert");
const page = fs.readFileSync(path.join(__dirname, "..", "site", "governance", "index.html"), "utf8");
const lib = page.slice(page.indexOf("/*ic:lib*/"), page.indexOf("/*ic:endlib*/"));
const IC = new Function(lib + "\nreturn IC;")();
const K = IC.keys, D = IC.decode, U = 1000000000n;

// storage keys
assert.strictEqual(K.storageKey("System", "Number"), "0x26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac");
assert.strictEqual(K.storageKey("Sudo", "Key"), "0x5c0d1176a568c1f92944340dbfed9e9c530ebca703c85910e7164cb7d1c9e47b");
assert.strictEqual(
  K.storageKey("Referenda", "ReferendumInfoFor", K.blake2_128concat(K.u32le(0))),
  "0x0f6738a0ee80c8e74cd2c7417c1e25569613e9bbc07e304aa9a1af9b85898e5a11d2df4e979aa105cf552e9544ebd2b500000000");
const fund = new Uint8Array(32); fund.set(new TextEncoder().encode("modlict/fund"));
assert.strictEqual(
  K.storageKey("System", "Account", K.blake2_128concat(fund)),
  "0x26aa394eea5630e07c48ae0c9558cef7b99d880ec681799c0cf30e8886371da92e66bdb9032d9d2dff597df6234eeaf96d6f646c6963742f66756e640000000000000000000000000000000000000000");

// a Root referendum: System.authorize_upgrade(0x07..07), enactment after 100 blocks,
// decision deposit placed, 5,000 ICT aye
const raw = "0x0000000000018800090707070707070707070707070707070707070707070707070707070707070707016400000001000000d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d00ca9a3b000000000000000000000000018eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a4800e8764817000000000000000000000000005039278c040000000000000000000000000000000000000000000000000000005039278c04000000000000000000000001020000000200000000000000";
const r = D.referendum(raw);
assert.strictEqual(r.state, "Ongoing");
assert.strictEqual(r.trackName, "Root");
assert.strictEqual(r.origin, "Root");
assert.strictEqual(r.proposal.name, "System.authorize_upgrade");
assert.strictEqual(r.proposal.arg, "0x" + "07".repeat(32));
assert.deepStrictEqual(r.enactment, { at: false, n: 100 });
assert.strictEqual(r.submitted, 1);
assert.strictEqual(r.submissionDeposit.amount, U);
assert.strictEqual(r.decisionDeposit.amount, 100n * U);
assert.strictEqual(r.deciding, null);
assert.deepStrictEqual(r.tally, { ayes: 5000n * U, nays: 0n, support: 5000n * U });
assert.strictEqual(r.inQueue, false);
assert.strictEqual(r.alarm.at, 2);
// decided referenda: Approved(block, ..)
assert.deepStrictEqual(D.referendum("0x01" + "40e20100" + "00" + "00"), { state: "Approved", at: 123456 });
assert.deepStrictEqual(D.referendum("0x05" + "0a000000"), { state: "Killed", at: 10 });
console.log("governance page: ok");
