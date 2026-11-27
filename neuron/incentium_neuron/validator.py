"""SN0 reference validator.

Each epoch:
 1. Reveal the weights committed in the previous epoch.
 2. Read miners, their declared models and the open model list from chain (`Registry.Models`;
    older runtimes: the tier table). A validator checks the model its reference serves
    (`--tier`, the model id); miners of other models are left to validators that hold those.
    A model that is not open or retiring gets no tasks, nor does a model other than the
    launch model (id 0) that fewer than three validators declare on chain.
 3. Publish tasks (prompts) for each miner on its task server; miners fetch them, answer and
    post signed results before the deadline (pull mode: miners need no open port).
 4. Count tokens and answered tasks, spot-check a random share of answers by re-running them
    on the validator's own reference backend and comparing top-k log-probabilities.
 5. Report failed checks and commit the weights hash. The salt and weights wait on disk
    (`state_file`) for the reveal, so a restart does not lose an epoch.

The chain is abstracted behind `Chain` so the loop is testable without a node.
"""
from __future__ import annotations

import hashlib
import json
import os
import random
import secrets
import time
from dataclasses import asdict, dataclass, field
from typing import NamedTuple, Protocol

from .scale import commit_hash
from .scoring import MinerStats, passes_spot_check, to_weights
from .tasks import Task, TaskBoard

SN0 = 0
PPM = 1_000_000
# A model other than the launch model gets tasks only while this many validators declare it.
MIN_MODEL_HOLDERS = 3
LAUNCH_MODEL = 0


class TierInfo(NamedTuple):
    """One entry of the model list (the id is the former tier number)."""
    g: float  # value factor G
    tolerance: float  # max spot-check distance
    model: str
    model_hash: str  # hex, all zero until pinned
    open: bool  # miners may declare it now
    status: str = "Open"  # Proposed | Open | Retiring | Closed
    mem_mb: int = 0
    engine: str = ""

    @property
    def paid(self) -> bool:
        """Miners that declared it are tasked and paid (open, or retiring in its transition)."""
        return self.status in ("Open", "Retiring")


class Chain(Protocol):
    def current_epoch(self) -> int: ...
    def miners(self, netuid: int) -> list[tuple]: ...  # (uid, hotkey, endpoint[, tier])
    def tiers(self) -> dict[int, TierInfo]: ...
    def commit_weights(self, netuid: int, h: bytes) -> None: ...
    def reveal_weights(self, netuid: int, uids: list[int], weights: list[int], salt: bytes) -> None: ...
    def report_failed_check(self, epoch: int, uid: int, evidence: bytes) -> None: ...


@dataclass
class PendingReveal:
    epoch: int
    uids: list[int]
    weights: list[int]
    salt: bytes


@dataclass
class Validator:
    chain: Chain
    reference: object  # backend with .complete(prompt, max_tokens, seed)
    own_uid: int
    board: TaskBoard = field(default_factory=TaskBoard)
    tier: int = 0  # the tier `reference` serves
    prompts_per_miner: int = 8
    spot_check_share: float = 0.25  # high on devnet; 1-5 % on mainnet
    max_tokens: int = 32
    collect_seconds: float = 60.0  # how long miners have to answer each epoch's tasks
    state_file: str | None = None  # where the pending reveal is kept across restarts
    pending: PendingReveal | None = None
    log: list[str] = field(default_factory=list)
    sleep: object = time.sleep

    def __post_init__(self):
        self.pending = self.load_pending()

    # -- pending reveal on disk ------------------------------------------------------------
    def load_pending(self) -> PendingReveal | None:
        if not self.state_file or not os.path.exists(self.state_file):
            return self.pending
        try:
            with open(self.state_file) as f:
                d = json.load(f)
            return PendingReveal(d["epoch"], d["uids"], d["weights"], bytes.fromhex(d["salt"]))
        except (OSError, ValueError, KeyError):
            return None

    def save_pending(self) -> None:
        if not self.state_file:
            return
        if self.pending is None:
            if os.path.exists(self.state_file):
                os.remove(self.state_file)
            return
        d = asdict(self.pending)
        d["salt"] = self.pending.salt.hex()
        os.makedirs(os.path.dirname(os.path.abspath(self.state_file)), exist_ok=True)
        tmp = self.state_file + ".tmp"
        with open(tmp, "w") as f:
            json.dump(d, f)
        os.replace(tmp, self.state_file)

    # -- work --------------------------------------------------------------------------------
    def publish(self, epoch: int) -> tuple[list[tuple[int, str]], float | None, float | None]:
        """Put this epoch's tasks on the board. Returns the miners to score and G, tolerance."""
        tiers = self.chain.tiers() if hasattr(self.chain, "tiers") else {}
        info = tiers.get(self.tier)
        miners, tasks = [], []
        if info is not None and not info.paid:
            self.log.append(f"epoch {epoch}: model {self.tier} ({info.model}) is {info.status}; no tasks")
            self.board.publish(epoch, time.time() + self.collect_seconds, [])
            return [], None, None
        if self.tier != LAUNCH_MODEL and hasattr(self.chain, "model_holders"):
            holders = self.chain.model_holders(self.tier)
            if holders < MIN_MODEL_HOLDERS:
                self.log.append(f"epoch {epoch}: model {self.tier} has {holders} declaring validators "
                                f"(needs {MIN_MODEL_HOLDERS}); no tasks")
                self.board.publish(epoch, time.time() + self.collect_seconds, [])
                return [], None, None
        for m in self.chain.miners(SN0):
            uid, hotkey = m[0], m[1]
            miner_tier = m[3] if len(m) > 3 else 0
            if uid == self.own_uid or hotkey in getattr(self.chain, "validator_hotkeys", lambda: set())():
                continue
            if miner_tier != self.tier:
                self.log.append(f"epoch {epoch} uid {uid}: model {miner_tier}, this validator checks "
                                f"model {self.tier}; skipped")
                continue
            miners.append((uid, hotkey))
            for i in range(self.prompts_per_miner):
                nonce = secrets.token_hex(8)  # unpredictable prompts: no precomputed answers
                tasks.append(Task(task_id=f"{epoch}-{self.own_uid}-{uid}-{i}-{nonce}", hotkey=hotkey, uid=uid,
                                  prompt=f"SN0 task {epoch}:{uid}:{i}: {nonce}", max_tokens=self.max_tokens,
                                  seed=secrets.randbelow(2**31)))
        self.board.publish(epoch, time.time() + self.collect_seconds, tasks)
        return miners, (info.g if info else None), (info.tolerance if info else None)

    def evaluate(self, epoch: int, miners, g, tol) -> list[MinerStats]:
        rng = random.Random(secrets.randbits(64))
        by_uid = {uid: MinerStats(uid=uid, tier=self.tier, g=g) for uid, _ in miners}
        for t in self.board.snapshot():
            s = by_uid.get(t.uid)
            if s is None:
                continue
            s.requests += 1
            if t.result is None:
                continue
            s.answered += 1
            s.tokens += min(int(t.result["tokens"]), t.max_tokens)
            if rng.random() < self.spot_check_share:
                s.checks += 1
                ref = self.reference.complete(t.prompt, t.max_tokens, t.seed)
                if passes_spot_check(s.tier, t.result["top_logprobs"], ref.top_logprobs, tol):
                    s.checks_passed += 1
                else:
                    ev = hashlib.blake2b(json.dumps([t.prompt, t.seed, t.result]).encode(), digest_size=32).digest()
                    s.failed_check_evidence.append(ev.hex())
        return [by_uid[uid] for uid, _ in miners]

    def measure(self, epoch: int) -> list[MinerStats]:
        miners, g, tol = self.publish(epoch)
        if miners:
            self.sleep(self.collect_seconds)
        return self.evaluate(epoch, miners, g, tol)

    def step(self) -> None:
        """Run once per epoch."""
        epoch = self.chain.current_epoch()
        if self.pending and self.pending.epoch == epoch - 1:
            p = self.pending
            try:
                self.chain.reveal_weights(SN0, p.uids, p.weights, p.salt)
                self.log.append(f"revealed epoch {p.epoch}")
            except RuntimeError as e:
                self.log.append(f"reveal for epoch {p.epoch} failed: {e}")
            self.pending = None
            self.save_pending()
        elif self.pending and self.pending.epoch < epoch - 1:
            self.log.append(f"pending reveal for epoch {self.pending.epoch} is too old; dropped")
            self.pending = None
            self.save_pending()
        stats = self.measure(epoch)
        for s in stats:
            self.log.append(f"epoch {epoch} uid {s.uid}: answered {s.answered}/{s.requests}, "
                            f"tokens {s.tokens}, spot checks passed {s.checks_passed}/{s.checks}")
        for s in stats:
            for ev in s.failed_check_evidence[:1]:
                self.chain.report_failed_check(epoch, s.uid, bytes.fromhex(ev))
                self.log.append(f"reported uid {s.uid} in epoch {epoch}")
        uids, weights = to_weights(stats)
        if uids:
            salt = os.urandom(32)
            # Measuring can take longer than an epoch: the chain records the commit in the
            # epoch it is included in, and the reveal must come in the epoch after that.
            commit_epoch = self.chain.current_epoch()
            try:
                self.chain.commit_weights(SN0, commit_hash(SN0, uids, weights, salt))
            except RuntimeError as e:
                self.log.append(f"commit in epoch {commit_epoch} failed: {e}")
                return
            self.pending = PendingReveal(commit_epoch, uids, weights, salt)
            self.save_pending()
            self.log.append(f"committed in epoch {commit_epoch}: {dict(zip(uids, weights))}")


class SubstrateChain:
    """Chain access through py-substrate-interface (pip install substrate-interface)."""

    def __init__(self, url: str, keypair, epoch_length: int = 360):
        from substrateinterface import SubstrateInterface  # optional dependency

        self.url = url
        self.s = SubstrateInterface(url=url)
        self.kp = keypair
        self.epoch_length = epoch_length

    def _retry(self, f):
        """The node drops idle websocket connections while the validator is busy measuring;
        reconnect once and retry."""
        try:
            return f()
        except (BrokenPipeError, ConnectionError, OSError) as e:
            from substrateinterface import SubstrateInterface

            print(f"chain connection lost ({e!r}); reconnecting", flush=True)
            self.s = SubstrateInterface(url=self.url)
            return f()

    def _submit(self, module: str, fn: str, params: dict):
        def go():
            call = self.s.compose_call(call_module=module, call_function=fn, call_params=params)
            xt = self.s.create_signed_extrinsic(call=call, keypair=self.kp)
            return self.s.submit_extrinsic(xt, wait_for_inclusion=True)

        r = self._retry(go)
        if not r.is_success:
            raise RuntimeError(f"{module}.{fn} failed: {r.error_message}")

    def current_epoch(self) -> int:
        return self._retry(lambda: self.s.get_block_number(None)) // self.epoch_length

    def has_storage(self, module: str, name: str) -> bool:
        return self._retry(lambda: self.s.get_metadata_storage_function(module, name)) is not None

    def has_call(self, module: str, name: str) -> bool:
        return self._retry(lambda: self.s.get_metadata_call_function(module, name)) is not None

    def model_list(self) -> bool:
        """True on runtimes with the open model list (Registry.Models)."""
        if not hasattr(self, "_model_list"):
            self._model_list = self.has_storage("Registry", "Models")
        return self._model_list

    def neuron_model(self, netuid: int, hotkey: str) -> int:
        item = "NeuronModel" if self.model_list() else "NeuronTier"
        return int(self._retry(lambda: self.s.query("Registry", item, [netuid, hotkey]).value) or 0)

    def miners(self, netuid: int):
        return self._retry(lambda: self._miners(netuid))

    def _miners(self, netuid: int):
        out = []
        for key, hotkey in self.s.query_map("Registry", "Neurons", [netuid]):
            uid = key.value
            ep = self.s.query("Registry", "Endpoints", [netuid, hotkey.value])
            endpoint = decode_bytes(ep.value)
            item = "NeuronModel" if self.model_list() else "NeuronTier"
            tier = self.s.query("Registry", item, [netuid, hotkey.value]).value or 0
            out.append((uid, hotkey.value, endpoint, int(tier)))
        return sorted(out)

    def validator_hotkeys(self) -> set[str]:
        """Hotkeys with validator stake: they are not scored as miners."""
        return self._retry(lambda: {k.value for k, v in self.s.query_map("Consensus", "Stake") if v.value})

    def validator_endpoints(self, netuid: int) -> list[str]:
        """Task-server URLs of staked validators, for miners to poll."""
        def go():
            out = []
            for key, hotkey in self.s.query_map("Registry", "Neurons", [netuid]):
                if not self.s.query("Consensus", "Stake", [hotkey.value]).value:
                    continue
                ep = decode_bytes(self.s.query("Registry", "Endpoints", [netuid, hotkey.value]).value)
                if ep:
                    out.append(ep)
            return out
        return self._retry(go)

    def endpoint_of(self, netuid: int, hotkey: str) -> str:
        return self._retry(lambda: decode_bytes(self.s.query("Registry", "Endpoints", [netuid, hotkey]).value))

    def set_endpoint(self, netuid: int, endpoint: str):
        self._submit("Registry", "set_endpoint", {"netuid": netuid, "endpoint": "0x" + endpoint.encode().hex()})

    def tiers(self) -> dict[int, TierInfo]:
        return self._retry(self._tiers)

    def _tiers(self) -> dict[int, TierInfo]:
        if self.model_list():
            return self._models()
        out = {}
        for key, v in self.s.query_map("Registry", "Tiers"):
            t = v.value
            h = t["model_hash"]
            h = h[2:] if isinstance(h, str) and h.startswith("0x") else bytes(h).hex() if not isinstance(h, str) else h
            out[int(key.value)] = TierInfo(t["weight_ppm"] / PPM, t["tolerance_ppm"] / PPM,
                                           decode_bytes(t["model"]), h, bool(t["open"]),
                                           "Open" if t["open"] else "Closed")
        return out

    def _models(self) -> dict[int, TierInfo]:
        out = {}
        for key, v in self.s.query_map("Registry", "Models"):
            m = v.value
            status = m["status"] if isinstance(m["status"], str) else next(iter(m["status"]))
            out[int(key.value)] = TierInfo(m["g_ppm"] / PPM, m["tolerance_ppm"] / PPM, decode_bytes(m["name"]),
                                           hex_of(m["weights_hash"]), status == "Open", status,
                                           int(m["mem_mb"]), decode_bytes(m["engine"]))
        return out

    def declare_tier(self, netuid: int, tier: int):
        """Declare the model (former tier) this neuron runs."""
        if self.model_list():
            self._submit("Registry", "declare_model", {"netuid": netuid, "model": tier})
        else:
            self._submit("Registry", "declare_tier", {"netuid": netuid, "tier": tier})

    declare_model = declare_tier

    def validator_models(self, hotkey: str) -> list[int]:
        if not self.model_list():
            return []
        return u8_list(self._retry(lambda: self.s.query("Registry", "ValidatorModels", [hotkey]).value))

    def declare_validator_models(self, models: list[int]):
        self._submit("Registry", "declare_validator_models", {"models": list(models)})

    def model_holders(self, model: int) -> int:
        """Validators that declare `model` and are registered on SN0 with stake. The chain also
        requires `ModelDeclarerStake` when a model opens; this count is what the validators use
        for tasking."""
        if not self.model_list():
            return MIN_MODEL_HOLDERS

        def go():
            n = 0
            for key, ms in self.s.query_map("Registry", "ValidatorModels"):
                if model in u8_list(ms.value) \
                        and self.s.query("Registry", "NeuronUid", [SN0, key.value]).value is not None \
                        and self.s.query("Consensus", "Stake", [key.value]).value:
                    n += 1
            return n
        return self._retry(go)

    def commit_weights(self, netuid, h):
        self._submit("Weights", "commit_weights", {"netuid": netuid, "hash": "0x" + h.hex()})

    def reveal_weights(self, netuid, uids, weights, salt):
        self._submit("Weights", "reveal_weights",
                     {"netuid": netuid, "uids": uids, "weights": weights, "salt": "0x" + salt.hex()})

    def report_failed_check(self, epoch, uid, evidence):
        self._submit("WorkVerify", "report_failed_check",
                     {"epoch": epoch, "uid": uid, "evidence": "0x" + evidence.hex()})


def read_key(a, ap) -> str:
    """The hotkey secret from --key or --key-file (a secret phrase, or a JSON file with
    `secretPhrase` as written by `incentium-node key generate --output-type json`)."""
    if a.key_file:
        text = open(os.path.expanduser(a.key_file)).read().strip()
        if text.startswith("{"):
            text = json.loads(text)["secretPhrase"]
        return text
    if a.key:
        return a.key
    ap.error("give --key or --key-file")


def u8_list(v) -> list[int]:
    """A Vec<u8> / BoundedVec<u8> as substrate-interface returns it (hex, text or a list)."""
    if not v:
        return []
    if isinstance(v, str):
        # substrate-interface gives a byte vector as 0x-hex, or, when the bytes are valid
        # UTF-8 (model ids below 128 always are), as the decoded text itself.
        h = v[2:] if v.startswith("0x") else None
        if h is not None and len(h) % 2 == 0 and all(c in "0123456789abcdefABCDEF" for c in h):
            return list(bytes.fromhex(h))
        return list(v.encode("utf-8"))
    return [int(x) for x in v]


def hex_of(h) -> str:
    """A [u8; 32] from substrate-interface (0x-str or list of ints) as plain hex."""
    if isinstance(h, str):
        return h[2:] if h.startswith("0x") else h
    return bytes(h).hex()


def decode_bytes(v) -> str:
    """substrate-interface returns Vec<u8> as a str (plain or 0x-hex) or as a list of ints."""
    if not v:
        return ""
    if isinstance(v, str):
        if v.startswith("0x"):
            try:
                return bytes.fromhex(v[2:]).decode()
            except ValueError:
                return v
        return v
    return bytes(v).decode()


def main():
    """python -m incentium_neuron.validator --url ws://127.0.0.1:9944 --key //Alice --uid 0 \
    --reference http://127.0.0.1:8080 --model qwen3.5-2b --public-url http://<ip>:8100"""
    import argparse
    import time

    from substrateinterface import Keypair

    from .backend import OpenAICompatibleBackend

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--key", help="secret URI of the validator hotkey, e.g. //Alice")
    ap.add_argument("--key-file", help="file holding the hotkey's secret phrase (keeps it out of the process list)")
    ap.add_argument("--uid", type=int, required=True)
    ap.add_argument("--reference", required=True, help="OpenAI-compatible base URL of the reference model")
    ap.add_argument("--model", required=True)
    ap.add_argument("--tier", "--model-id", dest="tier", type=int, default=0,
                    help="id in the model list of the reference model (0 = Qwen3.5-2B)")
    ap.add_argument("--epoch-length", type=int, default=360)
    ap.add_argument("--prompts", type=int, default=8, help="prompts per miner per epoch")
    ap.add_argument("--max-tokens", type=int, default=32)
    ap.add_argument("--listen", default="0.0.0.0:8100", help="host:port of the task server miners poll")
    ap.add_argument("--public-url", help="URL miners reach the task server on, published on chain, "
                                         "e.g. http://85.190.108.120:8100")
    ap.add_argument("--collect-seconds", type=float,
                    help="time miners get per epoch (default: 40 %% of an epoch)")
    ap.add_argument("--state-file", default=os.path.expanduser("~/.incentium/validator-state.json"))
    ap.add_argument("--no-probe-endpoints", dest="probe_endpoints", action="store_false",
                    help="do not probe the public RPC and archive endpoints in the on-chain register")
    a = ap.parse_args()
    chain = SubstrateChain(a.url, Keypair.create_from_uri(read_key(a, ap)), a.epoch_length)
    info = chain.tiers().get(a.tier)
    if info:
        print(f"checking model {a.tier}: {info.model} ({info.status}), G {info.g}, tolerance {info.tolerance}",
              flush=True)
    # Declare on chain that this validator holds the model, so it counts towards the three
    # validators a model needs (needs SN0 registration and validator stake; retried next start).
    if chain.model_list() and a.tier not in chain.validator_models(chain.kp.ss58_address):
        try:
            chain.declare_validator_models(sorted(set(chain.validator_models(chain.kp.ss58_address)) | {a.tier}))
            print(f"declared model {a.tier} on chain", flush=True)
        except Exception as e:
            print(f"could not declare model {a.tier} yet: {e!r}", flush=True)
    from .tasks import serve_tasks

    host, port = a.listen.rsplit(":", 1)
    board = TaskBoard()
    serve_tasks(board, host, int(port))
    print(f"task server on {a.listen}", flush=True)
    if a.public_url and chain.endpoint_of(SN0, chain.kp.ss58_address) != a.public_url:
        chain.set_endpoint(SN0, a.public_url)
        print(f"published task server {a.public_url} on chain", flush=True)
    collect = a.collect_seconds if a.collect_seconds is not None else a.epoch_length * 12 * 0.4
    v = Validator(chain=chain, reference=OpenAICompatibleBackend(a.reference, a.model), own_uid=a.uid,
                  board=board, tier=a.tier, prompts_per_miner=a.prompts, max_tokens=a.max_tokens,
                  collect_seconds=collect, state_file=a.state_file)
    if v.pending:
        print(f"loaded pending reveal for epoch {v.pending.epoch} from {a.state_file}", flush=True)
    if a.probe_endpoints:
        from . import probes

        if probes.has_register(chain):
            # Its own connection, so probes never wait on the validator loop's websocket.
            pchain = SubstrateChain(a.url, chain.kp, a.epoch_length)
            probes.start_thread(pchain, os.path.join(os.path.dirname(a.state_file), "endpoint-probes.json"),
                                log=lambda m: print(m, flush=True))
            print("probing the endpoint register every 30 minutes", flush=True)
    last = -1
    while True:
        e = chain.current_epoch()
        if e != last:
            n = len(v.log)
            v.step()
            print("\n".join(v.log[n:]), flush=True)
            last = chain.current_epoch()  # a step can outlast an epoch; wait for the next one
        time.sleep(12)


if __name__ == "__main__":
    main()
