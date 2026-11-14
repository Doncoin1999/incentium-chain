# Mainnet launch period (90 days)

The first 90 days (648,000 blocks) are a launch period designed to get the network running
quickly: the participation rules meant for a mature, open network (registration fees, minimum
validator stake, per-key work cap) start on day 90. The launch operator builds up the first
nodes, validators and miners. All rewards for launch-period work are locked in an on-chain
vault and released over 22 months. In the code the period is called "bootstrap".

| Rule | Value | Where |
| --- | --- | --- |
| Length | 648,000 blocks (90 days), fixed at genesis | `Registry.BootstrapUntil` |
| Who registers neurons | Only the bootstrap operator, without fee | `Registry.register_neuron` |
| Validator minimum stake | Waived | `Consensus.stake` |
| Genesis allocation | 100 IPM to the operator (fees and first stake), counted in the 21M cap | `Emission.TotalIssued` at genesis |
| End | Automatic at block 648,000; the operator or governance can end it earlier, never extend it | `Registry.end_bootstrap` |
| Reward lock | All launch-period rewards go to the launch vault: locked 2 months after day 90, then 10 % every 2 months (100 % after 22 months); claim with `Consensus.claim_launch_rewards` | `Consensus.LaunchLocked` |
| Expected amounts | About 583,200 IPM into the launch vault (90 % of 648,000), plus 64,800 IPM to the on-chain fund | |

After block 648,000 anyone can register as miner or validator, with the normal fee and
minimum stake.

## Launch steps

1. Generate keys offline (`ipm-node key generate`): one operator account, and an Aura
   (sr25519) and Grandpa (ed25519) key for each of the three nodes.
2. Write `keys.json` with the public addresses and run
   `python3 scripts/make_bootstrap_spec.py keys.json --node ./target/release/ipm-node`.
3. On each node: `ipm-node key insert --chain ipm-mainnet-raw.json --scheme sr25519 --key-type aura ...`
   and the same for `gran` with ed25519; then start with `--chain ipm-mainnet-raw.json --validator`.
4. From the operator account: register your validator and miner hotkeys in SN0
   (`Registry.register_neuron`), send each hotkey a little IPM for fees, stake the validators
   (`Consensus.stake`), publish miner endpoints (`Registry.set_endpoint`), and start the
   reference miners and validators from `neuron/`.
5. Publish the operator address and all hotkey addresses, so anyone can follow what the
   bootstrap period pays out.
