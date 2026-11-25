// Checks the decoder on incentium.org/subnets against SCALE output from the runtime.
//
//   mkdir -p /tmp/apidump
//   INCENTIUM_API_DUMP=/tmp/apidump SKIP_WASM_BUILD=1 cargo test -p incentium-runtime subnet_api_output
//   node scripts/test_subnets_page.js /tmp/apidump
const fs = require("fs");
const path = require("path");
const assert = require("assert");

const page = fs.readFileSync(path.join(__dirname, "..", "site", "subnets", "index.html"), "utf8");
const lib = page.slice(page.indexOf("/*ic:lib*/"), page.indexOf("/*ic:endlib*/"));
const IC = new Function(lib + "\nreturn IC;")();
const dir = process.argv[2];
const read = (n) => fs.readFileSync(path.join(dir, n + ".hex"), "utf8").trim();
const UNIT = 1000000000n;

const st = IC.decode.unlockStatus(read("unlock_status"));
assert.strictEqual(st.unlocked, true);
assert.strictEqual(st.block, 200 * 7200);
assert.strictEqual(st.minBlock, 180 * 7200);
assert.strictEqual(st.minMiners, 300);
assert.strictEqual(st.minReferenda, 3);
assert.strictEqual(st.minersAvg, null);

const a = IC.decode.auction(read("auction"));
assert.strictEqual(a.now, 200 * 7200);
assert.strictEqual(a.price, null); // the next auction waits 60 days
assert.strictEqual(a.startBlock, 260 * 7200);
assert.strictEqual(a.startPrice, 200000n * UNIT); // 4 x 50,000
assert.strictEqual(a.floor, 500n * UNIT);
assert.strictEqual(a.nextSale, 2);
assert.strictEqual(a.subnetCount, 1);
assert.strictEqual(a.maxSubnets, 8);

assert.strictEqual(IC.decode.optionU128(read("price_at")), 150000n * UNIT); // one day in

const subs = IC.decode.subnets(read("subnets"));
assert.strictEqual(subs.length, 2);
assert.strictEqual(subs[0].netuid, 0);
assert.strictEqual(subs[0].owner, null);
assert.strictEqual(subs[1].netuid, 1);
assert.strictEqual(subs[1].name, "Translate");
assert.strictEqual(subs[1].description, "Open translation");
assert.strictEqual(subs[1].control, "Key");
assert.strictEqual(subs[1].pricePaid, 50000n * UNIT);
assert.strictEqual(subs[1].immuneUntil, 380 * 7200);
assert.strictEqual(IC.ss58(subs[1].owner), "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY");
assert.strictEqual(subs[1].dead, false);
console.log("subnets page decoder: ok");
