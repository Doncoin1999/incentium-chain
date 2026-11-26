// Checks that the governance page's wallet panel builds exactly the calls the runtime
// expects. First run (on testnet-spec12):
//   SKIP_WASM_BUILD=1 cargo test -p incentium-runtime wallet_calls_fixture
// then: cd tools/wallet-bundle && npm ci && node test_calls.js
const { TypeRegistry, Metadata } = require('@polkadot/types');
const { expandMetadata } = require('@polkadot/types/metadata');
const fs = require('fs');
const reg = new TypeRegistry();
const md = new Metadata(reg, new Uint8Array(fs.readFileSync('/tmp/ict_metadata.bin')));
reg.setMetadata(md);
const tx = expandMetadata(reg, md).tx;
const UNIT = 1000000000n;
// exactly what the page builds
const got = [
  tx.convictionVoting.vote(3, { Standard: { vote: { aye: true, conviction: 'Locked2x' }, balance: 100n * UNIT + 5n } }),
  tx.convictionVoting.vote(3, { Standard: { vote: { aye: false, conviction: 'None' }, balance: UNIT } }),
  tx.convictionVoting.removeVote(null, 3),
  tx.referenda.placeDecisionDeposit(3),
  tx.parameters.set(tx.staking.setValidatorCount(33)),
].map((c) => c.toHex().slice(2));
const want = fs.readFileSync('/tmp/ict_calls.txt', 'utf8').trim().split('\n');
let ok = true;
got.forEach((g, i) => { const m = g === want[i]; ok &&= m; console.log(m ? 'same' : 'DIFF', g, want[i]); });
process.exit(ok ? 0 : 1);
