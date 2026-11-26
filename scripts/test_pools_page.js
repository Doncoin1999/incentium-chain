// Checks the decoder on incentium.org/pools against the PoolSummary layout in the
// technical specification, Section 7 (SubnetPoolsApi). Run: node scripts/test_pools_page.js
const fs = require("fs");
const path = require("path");
const assert = require("assert");

const page = fs.readFileSync(path.join(__dirname, "..", "site", "pools", "index.html"), "utf8");
const lib = page.slice(page.indexOf("/*ic:lib*/"), page.indexOf("/*ic:endlib*/"));
const IC = new Function(lib + "\nreturn IC;")();

const le = (v, n) => { let s = ""; v = BigInt(v); for (let i = 0; i < n; i++) { s += Number(v & 255n).toString(16).padStart(2, "0"); v >>= 8n; } return s; };
const compact = (n) => le(n << 2, 1);                       // single-byte mode, n < 64
const str = (t) => compact(Buffer.byteLength(t)) + Buffer.from(t).toString("hex");
const U = 1000000000n;
const pool = (netuid, name, open, f) =>
  le(netuid, 2) + str(name) + (open ? "01" : "00") +
  [f.price, f.ict, f.alpha, f.issued, f.staked, f.bonusToday, f.bonusMinted, f.stock, f.sold].map((x) => le(x, 16)).join("") +
  le(5000, 4);

const hex = "0x" + compact(2) +
  pool(1, "Compute", true, { price: 12n * U / 1000n, ict: 4200n * U, alpha: 350000n * U, issued: 900000n * U, staked: 120000n * U,
    bonusToday: 3600n * U, bonusMinted: 540000n * U, stock: 1200n * U, sold: 2400n * U }) +
  pool(2, "Søk", false, { price: 2n * U, ict: 1n, alpha: 2n, issued: 3n, staked: 4n, bonusToday: 0n, bonusMinted: 6n, stock: 0n, sold: 0n });

const p = IC.decode.pools(hex);
assert.strictEqual(p.length, 2);
assert.strictEqual(p[0].netuid, 1);
assert.strictEqual(p[0].name, "Compute");
assert.strictEqual(p[0].open, true);
assert.strictEqual(p[0].price, 12000000n);
assert.strictEqual(p[0].ictReserve, 4200n * U);
assert.strictEqual(p[0].alphaReserve, 350000n * U);
assert.strictEqual(p[0].alphaIssued, 900000n * U);
assert.strictEqual(p[0].alphaStaked, 120000n * U);
assert.strictEqual(p[0].bonusToday, 3600n * U);
assert.strictEqual(p[0].bonusMinted, 540000n * U);
assert.strictEqual(p[0].stock, 1200n * U);
assert.strictEqual(p[0].releasedToday, 2400n * U);
assert.strictEqual(p[0].feePpm, 5000);
assert.strictEqual(p[1].name, "Søk");
assert.strictEqual(p[1].open, false);
assert.strictEqual(p[1].bonusMinted, 6n);
assert.strictEqual(IC.decode.pools("0x00").length, 0);

// The quote must match ict_economics::alpha::AlphaPool::swap exactly (fee at least 1 base unit,
// out = reserveOut * net / (reserveIn + net), rounded down). Cases mirrored in Rust by
// alpha::tests::page_quotes_match (same numbers).
const q = IC.decode.quote;
assert.strictEqual(q(500n * U, 12500n * U, 100n * U, 5000), 2074645537948n);   // stake 100 ICT into 500 / 12,500
assert.strictEqual(q(12500n * U, 500n * U, 1000n * U, 5000), 36865505742n);    // sell 1,000 Alpha
assert.strictEqual(q(10n, 10n, 1n, 5000), 0n);                                   // the minimum fee eats a 1-unit trade
assert.strictEqual(q(0n, 10n, 5n, 5000), 0n);                                    // closed pool
console.log("pools page decoder and quote: ok");
