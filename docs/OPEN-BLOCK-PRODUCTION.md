# Open block production (nominated Proof of Stake)

Approved by the owner on 26 Nov 2026 (proposal "Incentium: åpen blokkproduksjon – forslag").
Branch `feat/npos`, on top of runtime 10/11.

## What changes

Today four genesis keys on server A and B produce and finalise every block (Aura + GRANDPA), and no
one else can join. After this upgrade anyone can stand as a block producer or back one with ICT,
and the chain elects the set every day.

| Item | Value |
| --- | --- |
| Session | 1,800 blocks (6 h) |
| Era | 4 sessions = 7,200 blocks (1 day) |
| Elected slots | 20 while the genesis keys keep their reserve slots, then 24; Root may change it (max 100) |
| Candidate minimum bond | 100 ICT |
| Nominator minimum bond | 10 ICT, up to 16 candidates each |
| Election | on-chain sequential Phragmén, up to 1,000 voters (bags list orders them by stake) |
| Unbonding | 7 eras |
| Slash deferral | 6 eras; a Root or Emergency referendum can cancel a deferred slash |
| History kept for payouts | 84 eras |

## Pallets

- `pallet_authorship` (who wrote the block, era points),
- `pallet_staking` (bonding, nominating, election input, exposures, rewards, slashing),
- `pallet_session` + `historical` (validator set and session keys; Aura and GRANDPA keys rotate through it),
- `pallet_offences` (GRANDPA equivocation reports reach staking),
- `pallet_bags_list` (voters sorted by stake for the bounded election),
- `pallet_block_producers` (this repo): genesis-key reserve, offline rule, reward pot.

Aura stays as block engine. GRANDPA equivocation reporting is switched on
(`KeyOwnerProof = sp_session::MembershipProof`, report system over `Offences` and `Historical`).

## Rewards: 2 % of issuance from the fund, plus all fees

- `ict-emission` mints the development-fund share as before, but `ValidatorShare` of issuance
  (20,000 ppm = 2 %) goes to the reward pot account instead of the fund account. Total issuance
  and the subnets' share are unchanged; the fund falls from 10 % to 8 %.
- Transaction fees, burned until now, go to the same pot.
- At the end of each era, `EraPayout` burns the pot's balance and reports it as the era's validator
  payout; `payout_stakers` mints it again to validators and nominators by era points, after each
  validator's commission. Supply is therefore unchanged except for rewards nobody claims within 84
  eras, which stay burned.

## Slashing and offline

- **Equivocation in GRANDPA.** Reported by anyone with a membership proof. The slash is at least 10 %
  of the offender's and its nominators' exposure (a wrapper raises the offence's own fraction to
  10 % when lower), deferred 6 eras, then burned. The offender is disabled for the rest of the era.
- **Offline.** A validator elected for an era that wrote no block in it gets no reward (no era
  points) and is removed as a candidate at the start of the next era. It can stand again with
  `validate`. No slash.

## The genesis keys' reserve

- The four genesis authorities are written into `pallet_block_producers::GenesisValidators`. While
  `ReserveActive`, the session manager adds them to every elected set.
- At each new era the pallet counts elected validators that are not genesis keys. When that count
  has been at least 7 in each of 30 consecutive eras, `ReserveActive` turns off for good,
  `ValidatorCount` rises by 4, and the genesis keys leave at the next era. They can still stand as
  ordinary candidates.
- Root may end the reserve earlier, never restart it.

## Live-chain migration

- Session storage is seeded from the current authorities: `Aura::authorities()` and
  `Grandpa::grandpa_authorities()` are paired by index (genesis built both from one list); each
  pair's validator id is the account whose bytes equal the Aura key. `Validators`, `QueuedKeys`,
  `NextKeys` and `KeyOwner` are written; session 0 starts at the upgrade.
- Staking starts empty with the minimum bonds and 20 slots. Until candidates exist, the election
  returns nothing and the genesis keys keep producing.
- The genesis keys need no funds and are never slashed (they have no exposure).

## Status (26 Nov 2026)

Runtime 12 on `feat/npos`, on top of runtime 10/11 (mainnet line). Tests: 10 runtime tests for
election, reserve, the 30-era rule, offline, era payout from the pot, minimum slash and the
migration; all earlier tests still pass; the node builds.

## Open after this branch

- `session.set_keys` needs an ownership proof signed by the new keys: the installer must create
  keys and proof on the node (rotate keys with owner) and submit them with `bond` and `validate`.
- Validators tab on incentium.org, one-line installer, more bootnodes.
- Testnet rehearsal with external candidates; the testnet runs its own branch (with Revive), so
  this is merged there first.
- Benchmarked weights for the new pallets (the defaults are used for now).
