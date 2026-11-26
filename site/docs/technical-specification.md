# Incentium Technical Specification v0.2

Nov 25, 2026 · **Ichiro Pakeshi Miyamoto**

## 1. Scope and conventions

Incentium (incentium.org) is the name of the network; its native coin is ICT. This specification turns the Incentium whitepaper (v0.2) into rules precise enough to build and test. It is the output of step 1 of the build plan and the input to step 2 (the chain). It defines each on-chain module's state, calls and rules, and records what the economic simulation says about the proposed parameters.

- **Status:** Draft. Parameters marked *proposed* may change after further simulation; parameters marked *protected* can only be changed by a 75% supermajority and a 30-day time-lock.
- **Units:** ICT and Alpha amounts are fixed-point integers with 9 decimals (1 ICT = 10⁹ base units). All percentages are stored as parts per million. Addresses use the standard Substrate SS58 format (prefix 42), so they start with 5.
- **Time:** Blocks are 12 seconds; 1 day = 7,200 blocks; an epoch is 360 blocks (72 minutes).
- **Rounding:** Every division rounds down; the remainder stays with the protocol and is burned at the end of the epoch, so issuance never exceeds the schedule.
- **Code:** The simulation that backs Section 2 is in the `incentium-sim` package (Python, with unit tests). Its results are reproducible with `python3 run_all.py --seeds 8`.

## 2. Simulation findings

The simulation confirms the issuance schedule and the cap, but shows that three whitepaper rules need to change: Alpha injection must be price-aware, “net ICT locked” needs a different definition, and the fee weight must be set against the fee rate. The model runs 12 subnets for 2 years in daily steps, averaged over 8 random seeds (attack tests over 4 seeds).

### 2.1 Issuance schedule

The schedule behaves as specified: 10,506,000 ICT are issued by the end of year 4, when the block reward halves to 0.5 ICT, and 19,690,500 ICT by year 16. Supply never exceeds 21,000,000 (unit-tested over 60 years).

### 2.2 Alpha injection: a fixed schedule dilutes stakers

Injecting a fixed 7,200 Alpha per day into each pool outpaces the buyback. Stakers end up with 32% less ICT value than they put in. A price-aware rule — inject Alpha only up to what that day's buyback absorbs at the current price, and leave the rest of the schedule unminted — reverses this.

| Injection rule | Staker value after 2 years (ICT out per ICT in) | Alpha burned (share of minted) | Allocation vs quality (rank correlation) |
| --- | --- | --- | --- |
| Fixed 7,200 Alpha/day | 0.68 | 4.4% | 0.67 |
| Price-aware (paired) | 1.46 | 24.9% | 0.98 |

Metric used: market value, Phase 2.

### 2.3 Which “locked ICT” metric to use

“Net ICT staked” (inflows minus outflows) turns negative in every subnet once stakers take profits, so all subnets fall to the same floor and the allocation becomes an equal split. It cannot be used. The pool's ICT reserve resists pumping but follows quality poorly; market value follows quality well but rewards a pump with 1.6 ICT of extra issuance per ICT pumped. Their geometric mean sits in between.

| Metric (Phase 2, price-aware injection) | Allocation vs quality (rank correlation) | Extra issuance from a 20,000 ICT pump held 30 days |
| --- | --- | --- |
| Pool ICT reserve | 0.42 | −3,576 ICT |
| Net ICT staked | 0.63 (degenerate: equal split) | not tested |
| Market value (price × supply) | 0.98 | 31,889 ICT |
| Geometric mean of reserve and market value | 0.88 | 13,827 ICT |

A 100,000 ICT pump loses money under every metric, because the round trip costs more than the extra issuance. A 20,000 ICT pump still paid off for an attacker controlling half the subnet's issuance under every metric except the pool reserve (+7,521 ICT with the geometric mean, +17,203 ICT with market value), partly because momentum-chasing stakers in the model buy into the pump.

### 2.4 Wash trading: the fee rate matters, not the metric

When every trade pays the same fee rate, burned fees are proportional to volume, so a fee-based metric and a volume-based metric gave the same results. What protects the network is the fee rate relative to the metric's weight. With colluding miners and validators (attacker captures 50% of the subnet's issuance), one year of wash trading gives:

| Daily wash volume | Profit at 0.1% fee | Profit at 0.5% fee |
| --- | --- | --- |
| 5,000 ICT | +17,046 ICT | +2,844 ICT |
| 20,000 ICT | +47,172 ICT | −8,399 ICT |
| 100,000 ICT | +31,023 ICT | −225,102 ICT |

A subnet owner acting alone (7.5% capture) loses money in every case.

### 2.5 Concentration

With the 18% cap, the largest subnet's share stayed at or below 17.1% and the Herfindahl index around 0.10 in every Phase 2 and Phase 3 run.

### 2.6 Recommended changes to the whitepaper

1. **Alpha injection:** replace the fixed schedule with price-aware injection (Section 7).
2. **Phase 2 allocation metric:** use the geometric mean of the pool's ICT reserve and Alpha market value, both as 30-day averages. Drop “net ICT staked”.
3. **Phase 3 fee metric:** count fees only on each account's net daily flow, so a same-day round trip counts as zero; keep the fee at 0.5% and lower the fee weight from 0.2 to 0.1 until organic volume is known.

### 2.7 Limitations

- New stake arrives as an outside stream (20,000 ICT/day at a 20% yield, scaled 0.2–2× by actual yield); real demand may differ a lot.
- Stakers partly chase 7-day price momentum, which makes short pumps look cheaper than they may be.
- Organic trading volume in the model is low, which overstates what wash trading can achieve.
- In Phase 3, user revenue is modelled as proportional to subnet quality, so the near-perfect quality tracking there (0.98–0.99) is partly built in.
- The ICT price itself is not modelled.

## 3. Module `ict-emission`

Issues ICT every block and splits it between the development fund and subnets; the halving is triggered by the amount issued.

**State**

| Item | Type | Meaning |
| --- | --- | --- |
| `TotalIssued` | u128 | ICT issued since genesis |
| `Halvings` | u8 | Halvings so far (k) |
| `TotalBurned` | u128 | ICT burned since genesis |
| `FundAccount` | AccountId | Development fund, controlled by governance |

**Rules (every block, in `on_initialize`)**

1. Reward r = R₀ / 2^k, with R₀ = 1 ICT (*protected*).
2. Next threshold T = 21,000,000 × (1 − 2^−(k+1)). If `TotalIssued` + r would pass T, issue only up to T and increment k; the rest of the block uses the new reward.
3. 10% of r goes to `FundAccount` (*proposed*); 90% goes to the subnet pot for `utility-metrics` to allocate.
4. In Phase 1 the subnet pot goes entirely to SN0.

**Invariants**

- `TotalIssued` ≤ 21,000,000 ICT at all times (*protected*).
- No call can mint ICT outside this module.

**Calls:** none for users. Governance can change the fund share and the fund account.

## 4. Module `subnet-registry`

Sells subnet slots by Dutch auction once registration has unlocked, registers miner and validator keys, and closes subnets. Rules decided 25 Nov 2026 (subnet registration specification); runtime 10, on the testnet since 25 Nov 2026 and planned on mainnet before day 90.

**State**

| Item | Type | Meaning |
| --- | --- | --- |
| `Subnets` | map netuid → SubnetInfo | Owner, created block, price paid |
| `Metadata` | map netuid → SubnetMetadata | Name (≤ 64 bytes), description (≤ 512 bytes), hash of the incentive code |
| `Phase2Enabled` | bool | Registration has unlocked; never locks again |
| `AuctionStart`, `Sales`, `LastSalePrice` | BlockNumber, u32, u128 | Current auction and price basis |
| `MaxSubnets` | u16 | 8 until governance raises it; SN0 does not count |
| `MinSpacing` | u32 | 60 days from a sale to the next auction; governance may lower it to 30 |
| `MinerHistory`, `MinerDay` | 30 daily averages, running tally | Active SN0 miners for the unlock criterion |
| `SwapAttested` | contract, audit hash | Governance's attestation of the audited, immutable Base swap contract |
| `Neurons` | map (netuid, uid) → hotkey | Registered miners and validators |

**Unlock.** Registration unlocks by itself in the first daily check (every 7,200 blocks) where all of these hold; anyone may also call `try_unlock`:

1. Day 180 after genesis (block 1,296,000; mainnet: 17 May 2027).
2. At least 300 active SN0 miners as a 30-day average. An active miner is one paid in an epoch: not penalised by a failed spot check and not banned.
3. Sudo removed and at least 3 approved referenda.
4. `attest_swap` passed by referendum: the Base swap contract is audited and has no pause guardian.

**Auction price.** The slot on sale is priced start × 0.75^(blocks since start / 7,200), smoothly per block, never below its floor. SN1 starts at 50,000 ICT; every later slot at 4 × the previous sale price. Floors by sale number: 1st 1,000 ICT, 2nd–4th 500, 5th–8th 300, 9th and later 200. The first buyer at the current price wins.

**Payment.** Not refunded, not even on deregistration: 50 % to the treasury, 50 % burned. When `alpha-assets` and `amm` arrive, the split becomes 50 % pool seed, 25 % treasury, 25 % burned.

**Pace.** One slot at a time. After a sale the next auction starts `MinSpacing` later, and only while fewer than `MaxSubnets` exist. A full cap does not push anyone out: the auction waits until a slot frees up or the cap rises, and a waiting auction then starts at once.

**Removal.** No subnet is voted out for being weak. A subnet can leave in only two ways, and nothing is refunded in either:

1. **Dead** (`remove_dead_subnet`, anyone): past its 180-day immunity, and no epoch in the last 30 days paid at least 3 distinct miners.
2. **Emergency brake** (`deregister_subnet`, Incentium governance on the Root track: 75 % approval, one week before it takes effect): against abuse; works during the immunity too.

Stake on a removed subnet moves to the unbonding queue.

**Cap ladder.** The cap doubles by itself, 8 → 16 → 32 → 64 → 128 → 256, at the daily check when all of these hold: every slot has been taken for 90 days; at least 75 % of the subnets paid at least 10 miners per epoch on average over 30 days; 180 days since the last raise (by the ladder or by governance); and not before day 775 (1 January 2029 on mainnet). Governance can also raise the cap at any time, up to 256. Settlement is spread over the epoch (Section 6), so 256 subnets need no further code.

**Calls**

| Call | Who | Effect |
| --- | --- | --- |
| `register_subnet(max_price, name, description, code_hash)` | Anyone, while an auction runs | Pays the current price, creates the subnet and publishes its metadata; fails if the price exceeds `max_price` |
| `try_unlock()` | Anyone | Unlocks now if every criterion holds |
| `deregister_subnet(netuid)` | Governance | Emergency brake; nothing is refunded |
| `remove_dead_subnet(netuid)` | Anyone | Removes a dead subnet (above); nothing is refunded |
| `attest_swap(contract, audit_hash)` | Governance, once | Unlock criterion 4 |
| `set_min_spacing(blocks)`, `set_max_subnets(max)` | Governance | Pace and cap, within bounds |
| `register_neuron(netuid, hotkey)` | Anyone | Burns the subnet's registration fee and assigns a uid |
| `transfer_ownership`, `set_multisig`, `set_dao`, `set_params`, `set_metadata` | Subnet controller | Subnet control, above |
| `propose`, `vote`, `close_proposal` | Subnet stakers (close: anyone) | Subnet DAO votes |
| `report_failed_work(netuid, epoch, uid, evidence)` | Subnet validators | Penalties in subnets |

**Subnet control (SN1 and later).** Control only moves forward:

1. **One key** (at registration): the owner signs.
2. **Multisig** (`set_multisig(signatories, threshold)`, 2–20 signatories, threshold ≥ 2): the owner becomes the multisig account, derived as in `pallet-multisig`, and acts through it.
3. **Subnet DAO** (`set_dao`): the owner becomes the subnet's DAO account, which receives the 12 % owner share. Any staker of the subnet may `propose` one action at a time: change settings, change metadata, or spend from the DAO account. Votes run 5 days (shorter than the 7-day unbonding) and are weighed by each voter's stake when the vote closes. `close_proposal` (anyone) carries the action out at once if aye stake exceeds nay stake and is at least 1/3 of the subnet's stake.

A subnet can be sold or handed to another single key (`transfer_ownership`) while one key or a multisig controls it; the new owner starts again as one key. A DAO subnet stays with its stakers.

The controller (owner key, multisig, or the DAO by vote) sets the subnet's settings within the protocol's bounds, and its metadata (`set_metadata`, e.g. a new incentive-code hash for a new release):

| Setting | Default | Bounds |
| --- | --- | --- |
| `neuron_fee` (burned per neuron registration) | 1 ICT | 1–1,000 ICT |
| `max_neurons` | 256 | 16–256, and not below the neurons already registered |
| `penalties` | off | on / off |
| `max_failures` | 3 | 1–10 |
| `failure_window` (epochs) | 600 (30 days) | 20–1,800 (1–90 days) |

**Penalties in subnets.** With penalties on, a validator of the subnet reports a miner's failed work with `report_failed_work(netuid, epoch, uid, evidence)`. When reporters holding at least half of the subnet's active stake (the stake of validators whose weights counted in the last settled epoch) agree, the miner gets no incentive for that epoch; `max_failures` penalties inside `failure_window` ban the hotkey from the subnet. The subnet decides what counts as failed work; the protocol fixes the quorum and the consequences. There is no bond to slash yet (open point; slashing will need 2/3 of active stake). SN0 keeps its own spot checks and bonds (Section 5), and a hotkey banned there is paid in no subnet.

**Runtime API and subnet page.** `SubnetRegistryApi` answers `unlock_status`, `auction`, `price_at(block)` and `subnets` (with each subnet's current share of issuance), so any client reads the same figures the chain uses. incentium.org/subnets shows them live: progress on the four unlock criteria, the auction price now and in 1, 3 and 7 days, and every subnet.

**Alpha (decided 25 Nov 2026):** every subnet except SN0 gets an Alpha token with ICT's tokenomics (7,200 a day from its own first day, halving, 21 million); half of each day's Alpha is a bonus 50/38/12 on top of ICT (owner's part vested 6 months), half goes to the pool's stock, sold to buyers at most one day's issuance per day; staking on a subnet is buying its Alpha, so the pool holds the stakers' ICT; the pool starts at sale price ÷ 1,000,000; issuance never buys Alpha (Section 7).

**Open points:** simulate the Alpha rules together with the Phase 3 metrics; an optional miner bond per subnet that penalties can slash (2/3 of active stake), with a call for miners to leave a subnet.

## 5. Module `work-verify` (SN0)

Credits SN0 miners for verified work units, slashes failed spot checks and pays pool sub-workers on-chain.

**Work units:** W = T × Q × A × G per miner per epoch, where T is accepted tokens from all verified tasks, whatever their source (output tokens count in full; input and embedding tokens at a lower factor, to be set on testnet), Q the quality factor (0–1), A availability (0–1; for real-time tiers it may include latency) and G the tier weight. Tasks are assigned at random in proportion to each miner's measured capacity (see Task sources below), so throughput counts within a tier and equal capacity earns equal W. The reference validator today gives every miner the same number of tasks per epoch (8 tasks of at most 32 tokens) and must be changed. A key's credited W per epoch is capped at 2% of the SN0 total (*proposed*).

**Task tiers (proposed, to be calibrated on testnet)**

| Tier | Recommended model (Apache 2.0) | Typical hardware | Inference engine | Weight G |
| --- | --- | --- | --- | --- |
| 0 | Qwen3.5-4B, 4-bit quantized (entry tier; the only tier in the launch period), plus a small embedding model to be pinned on testnet | Ordinary PC with 16 GB memory or CPU server, no GPU | llama.cpp | 0.15 |
| 1 | Qwen3.5-9B, 4-bit quantized | Gaming GPU or Apple Silicon laptop | llama.cpp | 1.0 |
| 2 | Qwen3.5-27B | One 48–80 GB data-center GPU | vLLM | about 3 |
| 3 | Qwen3.5-122B-A10B | Multi-GPU server | vLLM | about 5 |

One model family across all tiers keeps the tokenizer and output format identical, which simplifies spot checks. Tiers 0, 1 and 2 use dense models, whose outputs vary less between machines than mixture-of-experts models; tier 3 gets a wider tolerance. The exact weight files are pinned by hash at genesis, together with the inference engine version. Alternative if a non-Qwen family is preferred: gpt-oss-20b (fits in 16 GB) and gpt-oss-120b (one 80 GB GPU), also Apache 2.0. *Decided 14 Nov 2026.*

G values are relative starting points set by governance. They are calibrated from the measured cost per token on each tier (hardware depreciation, power and what the machine could earn elsewhere) plus a premium for larger tiers, so that running a larger, more useful model earns somewhat more per unit of cost. Governance recalibrates G every quarter, with a limit on each step (*proposed*: ±25%). An optional demand adjustment, up to +20% for a tier with unserved tasks, can be added later. Model names and versions are fixed at genesis and changed only by governance.

**Tier 0 and the launch period.** During the 90-day launch period SN0 runs only tier 0, which runs on ordinary CPU servers and keeps launch costs low. Tier 0 moves from Qwen3.5-2B to Qwen3.5-4B (*proposed* 20 Nov 2026): the 4B model scores 27 against 16 for the 2B on the Artificial Analysis index and still fits an ordinary PC. The switch is a governance call on the tier table after a test on testnet with mixed Intel, AMD and Apple processors. Tier 1 (Qwen3.5-9B) is enabled by a governance vote at a time the launch operator chooses, preferably a few weeks before day 90, so the switch is tested before outside miners join. After that, tier 0 remains the entry tier for miners without a GPU.

**At most a quarter of memory (decided 25 Nov 2026).** The miner software picks the largest open tier whose model, with its context (two slots of 4,096 tokens), takes at most 25% of the system or Apple unified memory, or fits entirely in a separate NVIDIA graphics card, which the miner uses alone. Memory a tier then needs: tier 0 (2B) 8 GB; tier 1 (9B) 32 GB, or an 8 GB card; tier 2 (27B) 96 GB, or a 24 GB card. Without a card a machine runs tier 0 only, since larger models are too slow on a processor for the validators' deadlines. The Mac and Windows node and the Linux installer (`setup_miner choose`, Vulkan build of llama.cpp on NVIDIA) apply the same rule, and the miner declares its tier on chain.

**Declared tier and verification.** Each miner declares its tier on chain when it registers. Validators keep a reference model for every enabled tier and spot-check each miner against the model of its declared tier. A miner that declares tier 1 but runs the 4B model fails the check, because the two models' top-k logits differ, and is reported and slashed as described under Spot checks below.

**Adding or switching a model.** A governance call adds a tier (model name, weight hash, engine version, G and tolerance). A new tier runs alongside existing ones for a transition window (proposed: 14 days) so miners can upgrade without losing rewards.

**Spot checks**

1. Each epoch, validators pick 1–5% of requests at random, seeded by the drand beacon so miners cannot predict which.
2. The validator scores the miner's output with the reference model in one forward pass over prompt and output (prefill) and compares the top-k logits at every position; a check passes if the distance is within the tier's tolerance. Prefill is many times cheaper than generation, so checking 5% of tasks needs well under 1% of the tier's capacity. The reference validator still regenerates answers and must be changed; vLLM returns prompt log-probabilities directly, and the method for llama.cpp on tier 0 is still to be settled.
3. When reporters with at least half of the active validator stake (stake whose weights counted in the last settled epoch) report the same failed check, the miner's W for the epoch is zeroed. When they hold at least 2/3, 10% of the miner's bond (or, for a pool, the pool's bond) is also slashed. Three penalties in 30 days ban the key. *Thresholds decided 25 Nov 2026 (previously 1/3 for both).*
4. Slashed ICT: 50% burned, 50% to the insurance fund.

**Validator tiers (proposed).** Each validator declares on chain which tiers it checks and must hold the pinned model for each (new storage in `subnet-registry`). A tier opens only when at least three validators have declared it. Later, a piece of work can be sent to two miners of the same tier, and the validator checks only when their answers disagree.

**Task sources (proposed).** Validators fill miners' capacity from three sources in fixed priority:

1. Paid jobs from the gateway (Section 13).
2. An open public-goods queue, for example structuring and translating public texts, synthetic training data for small models and embeddings of public archives; results are published openly.
3. Hidden control tasks with known answers, indistinguishable from the others.

**Assignment.** All three sources count in W in the same way. Tasks are assigned by the protocol, not chosen by a gateway or a miner: each piece goes to a miner drawn at random among the eligible miners of its tier, weighted by measured capacity, with the draw seeded by the drand beacon so anyone can check it. Real-time jobs may be limited to miners within a latency bound, with the draw random inside that set. The public-goods queue fills whatever capacity paid jobs leave, so W reflects capacity made available rather than demand a miner happened to receive. Equal capacity, quality and availability therefore give equal ICT; within one epoch the draw causes small differences that even out within days. Tasks for open tiers carry no sensitive data.

**Pools**

| Item | Rule |
| --- | --- |
| Registration | A pool registers one SN0 key and posts a bond (proposed: 5,000 ICT) |
| Sub-workers | Sign each work item with their own key; the pool forwards the signed receipt with the result |
| Verification | A failed spot check is traced to the sub-worker through its receipt; the pool's bond covers the slash, and the pool may pass it on |
| Payout | Each epoch, `work-verify` pays the pool's ICT to sub-workers pro rata to verified W, minus the pool's declared fee (max 5%) |
| Transparency | Receipts are published to the data-availability layer; anyone can recompute the payout |

**Calls:** `register_pool(fee, bond)`, `set_pool_fee(fee)` (takes effect after 7 days), `submit_work_root(epoch, merkle_root)` (pool or miner), `report_spot_check(epoch, uid, result, proof)` (validator).

## 6. Modules `weights` and `consensus`

Validators commit their weights in encrypted form; the weights open by time-lock, and Yuma Consensus is computed off-chain and verified on-chain by proof.

**Commit-reveal (`weights`)**

1. A validator submits `commit_weights(netuid, ciphertext)`: its weight vector encrypted to the drand round that falls 1 epoch later.
2. When that round is published, anyone can decrypt; the chain accepts the decrypted vector through `reveal_weights` only if it matches the commitment.
3. A validator that does not reveal within 1 epoch gets zero dividends for that epoch.
4. One commit per validator per epoch; a later commit in the same epoch replaces the earlier one.

**Validator weight in consensus**

Effective stake = min(√(stake), 10% of the subnet's total effective stake) × audit factor. The audit factor starts at 1.0, drops by 0.1 for each failed audit (floor 0.3) and recovers by 0.02 per clean epoch.

**Off-chain Yuma with proof (`consensus`)**

| Step | Where | What |
| --- | --- | --- |
| 1 | Off-chain prover | Reads revealed weights and effective stakes for the epoch; computes consensus, clipping, bonds and incentives |
| 2 | Off-chain prover | Produces a zkVM proof (SP1 or RISC Zero) that the output follows the published Yuma program |
| 3 | Chain | `submit_epoch_result(netuid, epoch, result_root, proof)` verifies the proof and stores per-uid incentives and dividends |
| 4 | Chain | Pays out; if no valid proof arrives within 2 epochs, falls back to the last valid result and pays 50% |

Anyone may run a prover; the first valid proof earns a small fee from the epoch's issuance (proposed: 0.1%).

**Audits:** Each epoch a drand-seeded draw picks validators whose published evaluations (prompts, outputs, scores on IPFS or Celestia) are re-scored by other validators. A score deviation beyond the subnet's tolerance counts as a failed audit.

**Sharing issuance between subnets (decided 25 Nov 2026; runtime 10, on the testnet since 25 Nov 2026, on mainnet before day 90)**

The development fund takes its share of each block first (10 %); the rest goes to the subnet pot, which consensus shares out every epoch.

| | Before the SN1 unlock | After the SN1 unlock |
| --- | --- | --- |
| Pot to SN0 | 100 % | At least 50 % of all issuance (0.5 / (1 − fund share) of the pot: 55.6 % at a 10 % fund) |
| Pot to SN1 and later | 0 % | Rises linearly from 0 to the rest over 180 days, shared by the ICT staked on each subnet; a subnet without stake gets nothing |
| SN0 split | Miners 66.7 %, validators 33.3 % of SN0's slice (60 % / 30 % of issuance) | Miners 50 / 88, validators 38 / 88 of SN0's slice; SN0 has no owner |
| SN1 and later split | – | Miners 50 %, validators 38 %, owner 12 % of the subnet's slice |

Settlement is spread over the epoch: the epoch's first block shares the pot and settles SN0, and each following block settles one further subnet (an epoch has 360 blocks, so 256 subnets fit). Validators of SN1 and later stake on their own subnet (`stake_subnet`, `unstake_subnet`; same minimum and unbonding as SN0). From the Alpha upgrade, staking on these subnets means buying Alpha in their pool, and the ICT in the pool is their stake (Section 7). Closing a subnet moves its stake to the unbonding queue. A subnet with no usable weights in an epoch is not paid; its slice stays in the pot. Spot checks (`work-verify`) penalise SN0 miners only; other subnets verify work with their own incentive mechanism. The 50 % floor for SN0 holds through 2028 and can only be changed by a runtime upgrade.

## 7. Modules `alpha-assets`, `amm` and `dividends`

*Rules decided 25 and 26 Nov 2026; not in runtime 10. Simulated in `sim/alpha_valuation.py` and `sim/alpha_decided.py` (results in `sim/VALUATION.md`, `sim/DECIDED.md`).* Each subnet other than SN0 has one Alpha asset with ICT's tokenomics and one pool in which staking and trading are the same thing. Alpha pays its holders ICT; neither issuance nor revenue buys Alpha back.

**Alpha asset (`alpha-assets`)**

- Issuance exactly like ICT, per subnet: 0 at the block the subnet is registered (`opened`), then 7,200 Alpha per day (300 per epoch of 360 blocks), halving on ICT's curve counted from `opened`, maximum 21,000,000 (*protected per subnet once created*).
- Daily split `BonusPpm`, fixed per subnet at registration (500,000 for subnets registered under the current rule; a Root referendum may set it between 250,000 and 750,000 for subnets registered later).
- **Bonus.** At each settlement, the epoch's bonus is minted and split like its ICT: 50% to miners by incentive, 38% to validators by dividends, 12% to the owner (DAO account for a DAO subnet). Miners' and validators' Alpha is credited as stake on their own hotkey; the owner's part vests linearly over 180 days. An epoch that pays no ICT sends its bonus to the stock.
- **Stock.** The rest of each epoch's issuance is minted to the pool's waiting stock `W`, outside the reserves, and released to the pool's Alpha reserve as below.
- Alpha is held as stake on a (coldkey, hotkey, netuid) position and can be moved between hotkeys of the same subnet and transferred to another coldkey.

**Dividends (`dividends`)**

- Each validator hotkey sets `take` in 0–180,000 ppm (default 180,000). Lowering takes effect at once; raising is rate-limited to once per 30 days.
- At each settlement the validators' 38% of the subnet's ICT is paid per validator by its consensus dividends; of each validator's amount, `take` goes to the validator and the rest to all Alpha staked on that hotkey, pro rata, in ICT. Alpha in the pool or in `W` receives nothing.
- Phase 3: settled user revenue for the subnet is split 50/38/12 the same way and its validators' part paid by the same rule (Section 8).

**Pool (`amm`)**

- Pool account derived from the netuid (`PalletId` + netuid); no private key; no call moves funds out except a swap. Changing the rules takes a runtime upgrade.
- **Closed period.** For `OpenAfter` = 72,000 blocks (10 days) after `opened`: no swaps; `W` collects; bonus Alpha is credited but cannot be unstaked; the treasury half of the sale price is held in the pool account as `reserved_ict` and counts as the subnet's stake for the issuance share.
- **Opening.** At block `opened + OpenAfter`, compute p₀ on chain (`ict_economics::alpha::start_price`): the present value of the ICT an Alpha is expected to pay, from the opening day over 20 years in monthly steps, each month's ICT divided over all Alpha issued to the subnet by the middle of that month (fully diluted):

  p₀ = Σ over months m = 0..239 of  h · E · 30 · 2^(−halvings by then) / S(10 + 30m + 15) · (1 + r/12)^(−m)

  with h = 0.38 × (1 − 0.18), E = the ICT a day the average subnet other than SN0 receives now (all issuance less the fund, less SN0's half, divided by the number of such subnets), S(d) = Alpha issued by day d, r = `RequiredReturn` (200,000 ppm; Parameters track). With 10 subnets this gives about 0.054 ICT per Alpha. Then ICT reserve x = min(`reserved_ict`, 36,000 Alpha × p₀, waiting stock × p₀), Alpha reserve y = x / p₀ moved from `W`; `reserved_ict − x` goes to the treasury. A pool with no issued Alpha yet stays closed.
- Constant product x · y = k on real reserves only; price = x / y ICT per Alpha. No virtual reserves.
- **Staking is buying.** `stake(netuid, hotkey, ict_in, min_alpha_out)` swaps ICT for Alpha and credits it as stake on `hotkey`; `unstake(netuid, hotkey, alpha_in, min_ict_out)` sells it back. A swap that would return less than its minimum fails; there are no partial fills.
- **Stock release.** Each block after opening, release from `W` to the Alpha reserve up to the block's share of daily issuance plus at most the same again from what was held back, unless the price is below 80% of its 30-day average (`HoldBackPpm` = 200,000), in which case nothing is released that block. The 30-day average is the mean of one price sample a day over the last 30 days.
- Fee 0.5% on every swap, in the input asset; half burned, half left in the pool.
- **Removed subnet:** issuance stops; `W` is burned; the owner's unvested Alpha is burned; `unstake` keeps working.

**What changes in runtime 10.** When the module is installed (by referendum, before registration unlocks), ICT staking on subnets other than SN0 is replaced; there is no such stake before the unlock, so nothing is migrated:

| Runtime 10 uses | From the Alpha upgrade |
| --- | --- |
| `SubnetStake` for the issuance share between subnets | real ICT reserve of the subnet's pool (`reserved_ict` while closed) |
| ICT staked with a validator, for weights, active stake, penalty and slashing thresholds | Alpha staked with the validator's hotkey on that subnet |
| Stake at close, for subnet DAO votes | Alpha staked at close |
| `stake_subnet` / `unstake_subnet` | `amm.stake` / `amm.unstake` |
| Validators' 38% paid to the validator | paid through `dividends` to the validator (take) and its stakers |

SN0 has no Alpha and keeps ICT staking.

**Runtime API for clients (`SubnetPoolsApi`).** incentium.org/pools reads only this API. All amounts are base units (9 decimals); prices are ICT base units per whole Alpha.

| Call | Returns |
| --- | --- |
| `pools()` | `Vec<PoolSummary>` in netuid order: `netuid: u16`, `name: Vec<u8>`, `open: bool` (false while closed or once removed), `price: u128`, `ict_reserve: u128`, `alpha_reserve: u128`, `alpha_issued: u128`, `alpha_staked: u128`, `bonus_today: u128`, `bonus_minted: u128`, `stock: u128` (waiting stock `W`), `released_today: u128`, `fee_ppm: u32` |
| `quote_stake(netuid: u16, ict_in: u128)` | `Option<u128>`: Alpha out after fee, or none if the pool is not open |
| `quote_unstake(netuid: u16, alpha_in: u128)` | `Option<u128>`: ICT out after fee, or none if the pool is not open |

The page fills `min_alpha_out` / `min_ict_out` from the quote and a slippage limit the user chooses (default 1%). The field names `stock` and `released_today` keep the positions of the earlier `stock` and `sold_today`, so the page's decoder is unchanged.

**Invariants:** k never decreases during a swap; Alpha issued per subnet follows the schedule exactly and never exceeds 21,000,000; nothing enters the pool's reserves that has not been issued; the pool's ICT reserve never goes negative; no call moves funds out of the pool except a swap; dividends paid per settlement equal the validators' share less takes.


## 8. Modules `utility-metrics`, `payments` and `phase-control`

`utility-metrics` decides each subnet's share of issuance in Phase 3 from on-chain data only (in Phase 2 the share follows ICT staked on each subnet, Section 6); `payments` pays user revenue out 50/38/12 like issuance; `phase-control` switches phases on.

**Metrics, updated once per epoch as 30-day exponential moving averages (90-day for L once the marketplace opens, Section 10)**

| Symbol | Definition | Used in |
| --- | --- | --- |
| L | √(pool ICT reserve × Alpha price × Alpha supply), using the epoch's average price; real ICT in the pool and Alpha in circulation count; the auction ICT the pool opened with does not | Phase 3 |
| R | User revenue routed to the subnet through `payments`, in ICT | Phase 3 |
| F | Burned fees, counted only on each account's net daily flow per subnet | Phase 3 |

**Allocation**

```latex
\text{Phase 3:}\quad S_s = 0.45\,\frac{\bar L_s}{\sum_j \bar L_j} + 0.45\,\frac{\bar R_s}{\sum_j \bar R_j} + 0.10\,\frac{\bar F_s}{\sum_j \bar F_j}
```

Then floors (1% for a subnet's first 30 days) and the cap (18%) are applied, and any excess is redistributed pro rata to the uncapped subnets. The Phase 3 weights (0.45 / 0.45 / 0.10) are the recommendation from Section 2.6 and replace the whitepaper's 0.4 / 0.4 / 0.2 if adopted.

**Revenue cap (*proposed* 20 Nov 2026).** The issuance a subnet gains through R in an epoch may not exceed k × the ICT value burned from its revenue in that epoch (half of the Alpha bought with revenue, valued at the epoch's average price), with k = 0.8. Self-paid revenue then costs more than the issuance it returns, whoever pays it. R weight left unused by the cap is redistributed to L and F pro rata. Not simulated yet (Section 9). The addendum of 20 Nov 2026 also proposed replacing L with net stake inflow; that is not adopted, because Section 2.3 found that net ICT staked degenerates to an equal split.

**Payments (`payments`)**

- Gateway operators register and post a bond. Users pay in ICT (stablecoin payments are swapped to ICT by the gateway before settlement).
- For each batch of requests, the serving validator co-signs a usage receipt; the gateway settles receipts per epoch with `settle(netuid, receipts_root, amount)`.
- Settled revenue is paid out in the next settlement 50/38/12 like issuance, the validators' part through `dividends` (Section 7). Neither revenue nor issuance buys Alpha.
- Receipts are published to the data-availability layer; a fraudulent receipt slashes the gateway bond.

**SN0 payments (Phase 1, *proposed*).** SN0 has no Alpha token, so payments for SN0 work settle into an epoch pot: 70% goes to miners pro rata to their W for the epoch, exactly like issuance; 10% to validators pro rata to their consensus weight; 20% buys ICT and burns it. Who served a paid job does not change what anyone is paid. Paying for fake jobs therefore spreads the payment over all miners, burns 20% and returns to the payer only its own capacity share. Users may pay in ICT, at a discount, or in USDC, which the gateway converts through the ICT⇄USDC atomic swap or another venue before settlement.

**Phase control (`phase-control`)**

| Switch | Turns on | Requires |
| --- | --- | --- |
| `Phase2` | `register_subnet`, Alpha, AMM | `register_subnet`: the four unlock criteria of Section 4, checked on chain, no vote needed. Alpha and AMM: installed by a runtime upgrade passed by referendum after an audit, no earlier than the unlock |
| `Phase3` | Phase 3 allocation formula, revenue paid out | Gate 2 criteria (whitepaper Section 11) and a passed referendum |

Switches can only move forward. Each switch takes effect at the start of the epoch after a 14-day enactment delay.

## 9. Open questions and next step

**Decisions needed before step 2**

- [x] Done: the changes in Sections 2.6 and 10.5 are in the whitepaper (v0.1, 14 Nov 2026).
- [x] Decided: the ICT side of the deposit is returned as it stands in the pool (Section 4).
- [x] Decided: Alpha is a transferable token traded in canonical pools through a marketplace (Section 10).
- [x] Decided: the chain is built on the Polkadot SDK solochain template.
- [x] Decided: no legal entity or chosen jurisdiction; the protocol, the marketplace front end and the reference software are open source and run by no one in particular. The development fund is an on-chain treasury governed by vote from genesis.
- [x] Decided: SN0 uses the Qwen3.5 family (Section 5).
- [x] Decided: the network is named Incentium (incentium.org) and its coin ICT; both documents use these names.
- [x] Done: whitepaper references to a foundation are replaced with core contributors and on-chain governance.

**Simulation work still to do**

- The recommended Phase 3 setup (geometric-mean L, weights 0.45 / 0.45 / 0.10, net-flow fee metric) has not been simulated as a whole yet; Section 2 tested its parts separately.
- Add an ICT price model and miner/validator selling pressure.
- Stress tests: a market crash (inflow drops 90%), a bank run on one subnet, and 32 subnets instead of 12.
- Calibrate SN0 tier weights against real hardware costs once testnet data exists.

Simulate the revenue cap (Section 8) with self-paid revenue at several sizes, and calibrate f, k and the pooled SN0 payment pot before Phase 3 is switched on.

**Step 2: chain build (done; mainnet live since 18 Nov 2026)**

1. Set up the repository: Polkadot SDK node, CI, reproducible builds.
2. Implement `ict-emission`, `subnet-registry` (SN0 only) and `work-verify` with unit tests that mirror the simulation's emission tests.
3. Implement `weights` with drand commit-reveal, and a first version of `consensus` that computes Yuma on-chain for SN0, to be replaced by the proof-based version.
4. Launch a closed devnet with SN0 and a reference miner, validator and pool.

**Status 19 Nov 2026 (repository `incentium-chain`, Polkadot SDK stable2610):**

| Component | Status | Tests |
| --- | --- | --- |
| `ict-economics` (issuance, AMM, buyback with price-aware injection, allocation, auction price, Yuma-lite) | Done | 27 passing; matches the simulation (10,506,000 ICT after 4 years) |
| `pallet-ict-emission` | Done | Passing |
| `pallet-subnet-registry` (SN0, automatic unlock, Dutch auction, subnet control and DAO, penalties, dead-subnet removal, cap ladder, on-chain endpoints) | Done | 28 passing |
| `pallet-weights` (hash commit-reveal per epoch) | Done | Passing, including a hash test vector shared with the Python validator |
| `pallet-work-verify` (bonds, spot-check quorum, slashing, three-strike ban, pools) | Done | Passing |
| `pallet-ict-consensus` (validator staking, Yuma-lite, epoch payout) | Done | Passing |
| Runtime (Aura, Grandpa, all Incentium pallets, devnet presets, `fast-epochs` feature) | Done | End-to-end Phase 1 test: issuance, stake, pool, commit, reveal, settlement and pool payout |
| Node (from the Polkadot SDK solochain template) | Runs mainnet | Built in CI (artifact incentium-node); mainnet on two servers with four genesis authorities |
| Reference miner and validator (Python) | Done | Passing against mock models, including a cheating miner that gets reported |
| Devnet launcher, SN0 bootstrap, runbook, CI with devnet smoke test | Done | CI not yet run |
| Governance (treasury, bounties, child bounties, multisig, referenda, conviction voting, launch-end hook) | Done | Runtime tests, including that the launch vault and the fund cannot vote |
| Mainnet scripts (keys, genesis, node, SN0, miner, watchdog, runtime upgrade) and runbook docs/MAINNET.md | Done | Used for the 18 Nov 2026 launch |

Mainnet has run since the evening of 18 November 2026, which started the 90-day launch period. Not yet in the runtime: Frontier EVM and the Hyperlane bridge (Section 14), the `access` module (Section 13) and the Phase 2–3 modules (`amm`, `buyback`, `alpha-assets`, `utility-metrics`, `payments`).

The status on 20 Nov 2026 and the changes that follow from the SN0 review are in Section 16.

One finding from the Rust port: price-aware injection keeps the Alpha side of a pool level, so the ICT side grows by the buyback budget and the Alpha price rises over time. This matches the simulation and is the intended effect: price-aware injection prevents dilution; it does not hold the price constant.

## 10. Alpha marketplace (simulation round 2)

An open marketplace where each subnet's own token (Alpha1 for SN1, Alpha2 for SN2, and so on) is swapped against ICT works, provided four safeguards are in place. Under them, Phase 3 allocation still follows subnet quality closely (rank correlation 0.98), and none of the tested wash-trading strategies paid off.

### 10.1 What was tested

The model from Section 2 was extended with an open market:

- **Transferable Alpha:** each AlphaN is a token that can be bought, held and sold without staking. Only staked Alpha receives the buyback distribution (from 25 Nov 2026: revenue buybacks only).
- **Traders:** speculative traders with 200,000 ICT of capital follow 7-day momentum, pull back toward the 90-day average price, and trade 1% (light) or 3% (heavy) of each pool's ICT reserve per day.
- **Cross-subnet swaps:** 30% of trader sales are routed straight into another subnet's Alpha through ICT, paying the fee twice.
- **Third-party liquidity providers (LPs):** outside LPs keep 50% of each pool's liquidity.
- **Attacks:** pumps, wash trading (same-day round trips and buy-one-day, sell-the-next) and a new liquidity-inflation attack, where an attacker parks liquidity in its own subnet's pool for 30 days.

Phase 2 results average 8 random seeds; attack results average 4 seeds and are noisy.

### 10.2 Effect of an open market on Phase 2

| Setup | Allocation vs quality (rank correlation) | Daily price volatility | Staker value (ICT out per ICT in) | Trader value |
| --- | --- | --- | --- | --- |
| Stake-only, no market | 0.88 | 1.5% | 1.46 | – |
| Market, light trading (1%/day) | 0.92 | 2.7% | 1.48 | 0.92 |
| Market, heavy trading (3%/day) | 0.50 | 6.9% | 1.46 | 1.01 |
| Heavy trading + third-party LPs | 0.71 | 6.1% | 1.44 | 0.93 |
| Heavy trading + LPs, LP liquidity excluded from L | 0.71 | 6.2% | 1.44 | 0.94 |
| Heavy trading, 90-day average instead of 30-day | 0.65 | 6.6% | 1.46 | 0.96 |

Light trading does no harm. Heavy speculation makes prices noisy, and because Phase 2 allocation is based on pool size and price, the noise reaches issuance. Deeper liquidity and a longer average both reduce this. Stakers do equally well in every setup; traders as a group lose money.

### 10.3 Phase 3 holds up

With user revenue in the formula, heavy trading barely matters. With 3% daily trading, the whitepaper weights (0.4 / 0.4 / 0.2, gross fees) gave a rank correlation of 0.93; the recommended setup (0.45 / 0.45 / 0.10, net-flow fees) gave 0.98, and 0.99 with light trading.

### 10.4 Attacks in an open market

Profit below is for an attacker controlling 50% of its subnet's issuance (owner, miners and validators colluding), in ICT.

| Attack | Unprotected | With safeguard | Safeguard |
| --- | --- | --- | --- |
| Wash trading, buy one day and sell the next, 20,000 ICT/day | +3,056 | −10,351 | Net-flow fee metric, fee weight 0.10 |
| Wash trading, same-day round trip, 20,000 ICT/day | −28,296 | −69,714 | Net-flow fee metric (the round trip counts as zero) |
| Pump 20,000 ICT, hold 30 days | +12,809 | +2,925 | 90-day average |
| Pump 20,000 ICT, hold 90 days | +22,151 | −1,525 | 90-day average |
| Park 100,000 ICT of liquidity for 30 days | +29,842 | −11,996 | LP liquidity excluded from L |

An owner acting alone (7.5% capture) lost money in almost every case; the largest exception was +4,803 ICT from a 90-day pump under the 30-day average.

### 10.5 Recommended marketplace design

1. **One canonical pool per subnet.** AlphaN trades against ICT in the protocol's own constant-product pool. The marketplace is a front end and router over these pools; a swap from AlphaN to AlphaM is routed through ICT and pays the fee on both legs.
2. **Alpha as a transferable token, with yield only for staked Alpha.** Holders can trade freely, but only Alpha that is staked receives the buyback distribution. This separates long-term stakers from traders.
3. **Third-party LPs welcome, but not counted in L.** Outside liquidity makes prices steadier and allocation more accurate, but it must not raise a subnet's share of issuance. Only protocol-owned liquidity and staked positions count.
4. **Use a 90-day average for L once the market opens.** It cuts the gain from pumps to around break-even, and with heavy trading it also tracked quality better than 30 days (0.65 vs 0.50).
5. **Keep the Phase 3 changes from Section 2.6:** net-flow fee metric and weights 0.45 / 0.45 / 0.10.

### 10.6 Not yet covered

- Trading on outside venues (other DEXs, exchanges) once Alpha is transferable. The metrics read only canonical pools, and arbitrage should keep prices aligned, but this is not simulated.
- Regulation: a marketplace front end for trading tokens may count as a crypto-asset service under MiCA, even if the pools themselves are non-custodial protocol code. This needs legal review before the marketplace opens.

## 11. Launch period (decided 14 Nov 2026, reward split updated 19 Nov 2026)

The first 90 days of mainnet are a launch period in which the network can be built up quickly. Mainnet started on 18 November 2026; the period ends automatically at block 648,000. The participation rules designed for a mature, open network with many independent operators — registration fees, minimum validator stake and the per-key work cap — take effect after 90 days, so they do not slow down the start. In return, only 25% of the ICT the network pays out for work done in the launch period is paid at once; the other 75% is locked and released gradually.

**What applies in the launch period**

| Item | Launch period (first 90 days) | From day 90 |
| --- | --- | --- |
| Registration of miners and validators | By the launch operator, who builds up SN0's first capacity; no fee | Open to everyone, with the normal fee |
| Validator minimum stake | None | 1,000 ICT |
| Genesis allocation | 100 ICT to the launch operator for transaction fees and first stake, counted in the 21M cap | – |
| Rewards | 25% paid at once; 75% locked per account in an on-chain launch vault | Paid directly |
| End | Automatic at block 648,000; can be ended earlier, never extended | – |

**Lock on launch-period rewards**

For work done in an epoch that started during the launch period, 25% of each reward is paid at once and 75% goes to the launch vault, recorded per account. The locked part is released as follows, counted from the end of the launch period (day 90):

| Time after day 90 | Day | Share of the locked part unlocked |
| --- | --- | --- |
| 0–2 months | 90–149 | 0% |
| 2 months | 150 | 10% |
| Every further 2 months | +60 days | +10% |
| 20 months | 690 | 100% |

Accounts collect their unlocked share with `claim_launch_rewards`. The schedule is enforced by the runtime; nobody can release vault funds faster. The development fund's 10% share of issuance is not locked: it sits in the on-chain treasury and can only be spent by governance vote.

**Expected amounts:** 648,000 ICT are issued in the launch period. Miners and validators earn about 583,200 ICT: about 145,800 are paid during the period and about 437,400 (2.1% of maximum supply) go into the launch vault. The other 64,800 ICT go to the development fund.

**Implementation (in `incentium-chain`, tested):**

| Item | Module | Rule |
| --- | --- | --- |
| `BootstrapUntil`, `BootstrapOperator` | `subnet-registry` | Set in genesis: 648,000 blocks and the launch operator |
| `register_neuron` | `subnet-registry` | In the launch period only the launch operator registers, without fee |
| `end_bootstrap` | `subnet-registry` | Ends the launch period early; it cannot be extended or restarted |
| `stake` | `ict-consensus` | No minimum stake in the launch period |
| Launch vault, `LaunchLocked`, `claim_launch_rewards` | `ict-consensus` | Pays 25% of launch-period rewards at once (LaunchImmediatePpm = 250,000) and locks 75%; 2-month lock, then 10% every 2 months |
| `pre_issued` | `ict-emission` | The 100 ICT genesis allocation counts against the 21M cap |
| `bootstrap` preset, `make_bootstrap_spec.py` | runtime, scripts | Mainnet template; the script inserts the real keys and rejects development keys |

A runtime test runs the whole period: registration and stake from the 100 ICT allocation, payouts into the vault, open registration at block 648,000, 25% paid at once, nothing of the locked part claimable before day 150, 10% on day 150 and everything on day 690.

**End of the launch period (runtime, `governance`):** in the block where the launch period ends, the runtime itself carries out three disclosed steps. Nobody has to act.

1. It removes the sudo key; from then on every change goes through on-chain governance.
2. It creates and approves two bounties from the development fund: 25,000 ICT for marketing (curated bounties) and up to 50,000 ICT for listing ICT on a recognised exchange. The fund holds about 64,800 ICT at day 90, so the marketing bounty is funded at once and the listing bounty around day 104.
3. It appoints the curator: a 2-of-3 multisig created at genesis from three separately stored keys, held at first by the launch operator. Governance can replace the curator at any time.

**Governance from genesis:**

| Item | Rule |
| --- | --- |
| Development fund | `pallet-treasury` on the fund account; spends paid once a day; unspent funds are not burned |
| Referendum tracks | Root (runtime changes and all protocol settings: 75 % approval at the end of a 7-day vote, 7 days before it takes effect), Treasurer (large spends), Small spender (up to 1,000 ICT), Emergency (runtime 11, installed on the testnet by referendum as a rehearsal without sudo, then on mainnet before day 90: authorizes a runtime upgrade only; 1,000 ICT decision deposit, 1 h prepare, up to 2 days deciding, 3 h confirm, 1 h before it takes effect; approval 100 % falling to 90 %, support 50 % falling to 20 %; about 5 hours with broad, fast support) |
| Runtime upgrades | Decided by hash: a referendum on `authorize_upgrade(blake2_256(wasm))` (Root or Emergency track) needs no preimage. Once enacted, anyone uploads the code with `system.apply_authorized_upgrade`; the chain takes it only if the hash matches, the spec name is unchanged and the spec version is higher, and charges no fee. Tool: `scripts/governance.py` (propose-upgrade, vote, status, apply-upgrade) |
| Voting | Conviction voting; vote locking period 7 days; submission deposit 1 ICT; undecided proposals time out after 14 days |
| Turnout | Counts only ICT that can vote; the launch vault, the fund, the subnet pot and stake and work accounts are left out |
| Bounties | Child bounties under each bounty; 3-day payout delay, during which governance can stop an award; the curator must show activity every 90 days |
| Portal | governance.incentium.org, voting with Talisman or SubWallet |

## 12. Miner onboarding

A new miner should earn their first ICT within 15 minutes, without opening ports, running a node or holding ICT first. Onboarding offers three entry points that share one technical base.

### 12.1 Entry points

| Entry point | For | How it works |
| --- | --- | --- |
| Pool | Anyone, no hardware | A web page: connect a wallet and stake ICT in a pool run by a professional miner; payout is pro rata (pools in `work-verify`, Section 5). |
| Incentium Miner app | Mac or Windows PC with a GPU or Apple Silicon | Install and press Start. The app detects the hardware and picks the tier (2B, 9B, 27B or 122B), downloads the pinned model and checks its hash, creates a wallet with a recovery phrase, registers the miner and shows earnings. |
| One-line install | GPU servers and cloud | `install.sh` or a Docker image (`incentium-miner start`); ready-made templates on GPU rental services. |

### 12.2 Technical requirements

1. **Pull-based tasks.** The miner opens an outbound connection to validators or a relay and fetches its tasks. No inbound port is needed, so it works behind any home router. This replaces the inbound endpoint in the current reference neuron.
2. **No own node.** The miner talks to public RPC endpoints (at least two, run by different operators). An embedded light client (smoldot) can replace them later.
3. **No ICT needed to start.** On testnet a faucet funds registration. On mainnet the first registration is sponsored and repaid from the miner's first payouts.
4. **Pinned models and automatic updates.** Model hashes are stored on chain, and the app updates itself when the protocol changes.

### 12.3 Delivery order

| Step | Deliverable | Ready by |
| --- | --- | --- |
| 1 | Pull-based task protocol in the reference neuron | Testnet |
| 2 | Install script and Docker image | Testnet |
| 3 | Public RPC endpoints and testnet faucet | Testnet |
| 4 | Desktop app for Mac and Windows (Tauri, llama.cpp built in) | Day 90 of mainnet |
| 5 | Pool web page over the existing pallets | Day 90 of mainnet |
| 6 | Onboarding test with outside users (a mainnet readiness criterion) | Before mainnet |

During the 90-day launch period only the launch operator registers miners, so the app and the pool page must be ready when registration opens on day 90.

## 13. Public access and stake-for-access

The public reaches the network's models through a gateway, not through individual miners. The first gateway is a free public demo run by the launch operator; once ICT circulates, larger shares of capacity are reserved for wallets that lock ICT for access.

### 13.1 Request path

A user or app calls the gateway's OpenAI-compatible API (for example `chat.incentium.org`). The protocol's draw picks a miner of the requested tier (Section 5, Assignment), limited to miners within the latency bound, and the gateway returns the answer. Validators spot-check a share of gateway traffic in the same way as their own tasks. Miners see the content of requests, so the terms state that sensitive data must not be sent; trusted execution (TEE) can remove this limit later.

**Streaming, failover and batch (*proposed*).** Miners keep an outbound WebSocket open to the gateway, so answers stream token by token without inbound ports; if a miner drops mid-answer, the gateway continues on another miner. Checks run after delivery. Besides real-time chat the gateway offers a batch API: jobs that can wait up to 24 hours are split into pieces of one to two minutes and served by tier-0 PCs at a lower price. A paid tier (per million tokens) sits beside stake-for-access, the API is listed on OpenRouter, and a private tier routes confidential work only to approved operators or TEE machines.

### 13.2 Access tiers

| Tier | Requirement | Capacity |
| --- | --- | --- |
| Free | Bot check, no wallet | A few messages per day per user and IP |
| Holder | Wallet signed in (Talisman, SubWallet, Polkadot.js) and ICT locked for access | Pro rata to the account's share of all ICT locked for access |
| Developer | Larger lock, API key bound to the wallet | Higher rate limits and API access |

Sign-in uses a signed message (no transaction, no fee): the gateway checks the sr25519 signature and reads the account's locked amount from chain.

### 13.3 Module `access` (new)

- `lock_for_access(amount)` locks ICT in the caller's own account; the tokens never leave the wallet.
- `unlock(amount)` starts an unbonding period (proposed: 7 days), after which `withdraw` releases the tokens. Locked balance cannot be moved or used twice, which prevents the same ICT from granting access to many wallets.
- Storage: locked amount per account and the network total, so a gateway can compute each account's share.

### 13.4 Rules and timing

- **Gateway traffic** counts towards miner rewards like any other task, because the protocol, not the gateway, assigns it (Section 5, Assignment): a gateway operator cannot route traffic to its own miners, and payments are pooled per epoch (Section 8).
- **Abuse protection:** per-user and per-IP limits, a bot check (for example Cloudflare Turnstile) in front of the free tier, and a queue with a clear message when capacity is full.
- **Timing:** the gateway is built and tested against the devnet now. The public demo opens when Tier 1 (Qwen3.5-9B on GPU) is enabled, before day 90, because the CPU-only 2B model is too slow for a public showcase. Holder and Developer tiers open once ICT can be traded, since almost all ICT issued before day 90 sits in the launch vault.
- **Legal:** terms of use, AI disclosure and data retention (preferably none) are set before the public opening; how stake-for-access affects the token's assessment under MiCA is part of the pre-launch legal review.

## 14. EVM compatibility and bridge to Base

ICT reaches EVM wallets and exchanges through a Hyperlane Warp Route between the Incentium chain and Base, which the project deploys and operates itself without anyone's permission. The bridge is built and tested on testnet ahead of time, but is not switched on for mainnet until day 90; it then opens with caps.

### 14.1 Design

1. **EVM on the Incentium chain.** The runtime adds Frontier (`pallet-evm`, `pallet-ethereum`), so MetaMask and Solidity contracts work directly on the chain. Each EVM address maps to a native account; ICT keeps 9 decimals natively and is shown with 18 decimals in the EVM through Frontier's balance adapter.
2. **Hyperlane core** contracts are deployed on the Incentium EVM with Hyperlane's own tooling (`hyperlane core deploy`). They are Hyperlane's standard, widely used contracts; no bridge code is written in-house.
3. **Warp Route Incentium ↔ Base** (`hyperlane warp deploy`): native ICT is locked as collateral on the Incentium chain, and an equal amount of ICT is minted as an ERC-20 on Base. Moving back burns the ERC-20 and releases the locked ICT. Locked ICT always equals the ERC-20 supply, so the 21,000,000 cap is unchanged.
4. **Agents run by the project:** Hyperlane validators sign outgoing messages, and a relayer delivers them. The interchain security module (ISM) is a multisig of these validators (proposed: 5 of 7).
5. **Bridge page** (`bridge.incentium.org`): connect Talisman and MetaMask, enter an amount, confirm; the page shows fee and progress.

### 14.2 Security

- Caps per transfer, per day and in total, raised step by step; a pause switch held by governance.
- Continuous monitoring that locked ICT equals the ERC-20 supply on Base, with an alert on any difference.
- Validator keys in hardware security modules. At launch the project runs the validators, which is disclosed; the signer set then moves to independent operators.
- The ISM can be replaced later without redeploying the route, so the bridge can move to a trust-minimized model (light client or ZK proofs).
- External audit of the EVM integration and the Hyperlane configuration before mainnet use.

### 14.3 Liquidity and delivery

An ICT/USDC pool on a Base DEX is needed before trading can start. If the development fund seeds it, that is decided by an on-chain vote and disclosed.

| Step | Deliverable | Ready by |
| --- | --- | --- |
| 1 | Frontier EVM in the runtime, address mapping and decimals | Testnet |
| 2 | Hyperlane core and Warp Route against Base Sepolia | Testnet |
| 3 | Bridge page with Talisman and MetaMask | Before day 90 |
| 4 | External audit; validator keys in HSMs | Before day 90 |
| 5 | Mainnet route with caps; DEX pool | Switched on at day 90 |

## 15. Subnet categories (*proposed* 20 Nov 2026)

Six subnet categories are defined before Phase 2, each with a standard unit, a standard verification module and a standard on-chain payment contract. Categories carry no issuance share: Section 8 applies to every subnet alike. They exist so that revenue is measured the same way everywhere and new teams start from working code.

| Category | Unit metered by `payments` | Verification module | Template |
| --- | --- | --- | --- |
| 1. Compute rental | GPU-hour | Hardware attestation, uptime probes, test jobs | To be written |
| 2. AI inference (incl. verifiable and confidential) | Tokens, requests | Prefill spot checks (Section 5) | SN0 |
| 3. Data, search and storage | Queries, GB-month | Retrieval checks, storage proofs | To be written |
| 4. Model building | Delivered model or adapter | Hidden, rotated test sets | To be written |
| 5. Applied AI for a sector | Completed task | Reference answers, customer rating | To be written |
| 6. Forecasts and signals | Signal subscription | Scored against outcomes afterwards | To be written |

A seventh, open category covers anything else and uses the generic receipt format of Section 8. A subnet declares its category in `set_subnet_identity`; the category selects the metering unit its gateway receipts must use. Order: category 2 runs today as SN0; category 1 comes next because it reuses SN0 hardware; categories 3–6 open to outside teams with Phase 2.

## 16. Implementation status and required changes

On 25 November 2026 the testnet runs runtime 10 (subnet registration and issuance sharing), and a referendum is installing runtime 11 (Emergency track) on it without sudo. Mainnet runs the Phase 1 core and gets runtimes 10 and 11 before day 90, once the testnet rehearsal is complete. Most changes from the SN0 review are in the reference neuron, not the runtime. The table lists, per area, what the code does today, what has to change, where, and what kind of step it is.

| Area | In the code today | Change or action | Where | Kind of step |
| --- | --- | --- | --- | --- |
| Issuance | Done; mainnet since 18 Nov 2026 | None | `ict-emission` | – |
| Launch period and vault | Done; 25% paid, 75% locked, release from day 150 to day 690 | None | `subnet-registry`, `ict-consensus` | – |
| Governance | Done: treasury, bounties, multisig, referenda, conviction voting, launch-end hook; Emergency track and upgrades by hash (runtime 11); `scripts/governance.py` | Rehearse on the testnet without sudo; a parameter track; a governance web page | runtime `governance` | Referendum on testnet, runtime upgrade |
| Subnets (Phase 2) | Runtime 10 on the testnet: automatic unlock, Dutch auction, issuance sharing, staking per subnet, subnet control and DAO, penalties, dead-subnet removal, cap ladder, spread settlement, `SubnetRegistryApi`; incentium.org/subnets | Mainnet upgrade before day 90; a miner bond per subnet | `subnet-registry`, `ict-consensus` | Runtime upgrade |
| Tier table | Four tiers on chain; tier 0 (Qwen3.5-2B) open, tiers 1–3 closed | Tier 0 to Qwen3.5-4B with pinned file hash; embedding job type; validator tier declarations; three-validator rule for opening a tier | `subnet-registry` | Runtime upgrade, then governance call |
| Task source | Synthetic prompts (`SN0 task <epoch>:<uid>:<i>: <nonce>`), 8 per miner per epoch, at most 32 tokens | Public-goods queue and hidden control tasks; later paid jobs from the gateway | `neuron/validator.py` | Software release |
| Task fetching | Miner polls validators every 5 s and gets a fixed task count | Protocol assignment by capacity with a drand-seeded draw; WebSocket streaming for real-time tiers | `neuron/miner.py`, `tasks.py` | Software release |
| Work units | W = T × Q × A × G; T = output tokens capped at the task's limit | T by token type; G from measured cost per token, quarterly recalibration | `neuron/scoring.py`, tier table | Software release, governance |
| Spot checks | Regenerates the answer; default share 25% (devnet value); local random draw | Prefill scoring; 1–5% on mainnet; drand-seeded draw | `neuron/validator.py`, `scoring.py` | Software release |
| Slashing and pools | Done: quorum reports, 10% slash, three-strike ban, pool bonds and pro-rata payouts | None | `work-verify` | – |
| Weights | Hash commit-reveal per epoch | drand time-lock (Section 6) | `weights` | Runtime upgrade |
| Consensus | Yuma-lite computed on chain for SN0 | Proof-based Yuma when subnets outgrow on-chain computation (Section 6) | `ict-consensus` | Runtime upgrade |
| ICT⇄USDC swap | Pallet `ict-swap` (index 15) on testnet since 20 Nov 2026; `IncentiumSwap` at 0x24836E90fEA706DA855D0f3cD7772D7282fD962D on Base Sepolia; keeper and trading page on testnet; 19 pallet, 16 contract and 4 keeper end-to-end tests pass | Full testnet run including failure paths; benchmarked weights; replace `ValidateUnsigned` before April 2027; external audit; mainnet upgrade and Base mainnet contract | `pallets/ict-swap`, `swap/` | Audit, runtime upgrade, contract deployment |
| Explorer | Live at explorer.incentium.org; swap events classified in code but not yet deployed | Deploy swap events; from Phase 2 show allocation inputs per subnet and epoch | `explorer/` | Software release |
| Alpha and Phase 3 economics | Alpha rules decided 25–26 Nov 2026 and simulated (`sim/DECIDED.md`); first module on `feat/alpha-pools` | Rebuild `amm` to the 26 Nov rules (real reserves, opening after 10 days, hold-back, price limits), build `dividends`, connect to the registry and consensus, then `utility-metrics`, `payments`; add the revenue cap and the pooled SN0 payment pot; simulate the Phase 3 setup as a whole | New pallets | Build, simulation, audit, referendum |
| Gateway and access | Not built | Gateway with streaming, failover, batch and payment receipts; `access` module (Section 13) | New service, new pallet | Build, legal review |
| Miner onboarding | Pull-based miner; node for Mac and Windows; `install-miner.sh` picks the tier on Linux; the model takes at most a quarter of memory | Test the Linux installer on an NVIDIA machine; pool web page, Docker image (Section 12) | `neuron/`, `scripts/` | Software release |
| EVM and bridge | Not in the runtime | Frontier and Hyperlane route (Section 14), or rely on the atomic swap until a bridge is audited | Runtime, contracts | Decision, then build and audit |
| Legal | No legal entity | Review of the contracting party for paid users, GDPR for the gateway, MiCA for trading and marketing, EU AI Act Article 50 | – | External advice |

None of the proposed changes touches protected parameters (maximum supply and halving, for ICT and every Alpha token). The 50% issuance-funded buyback floor was dropped on 25 Nov 2026, before any Alpha module was built.
