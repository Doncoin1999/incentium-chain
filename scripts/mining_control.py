"""Mining control board: the operator's view of the SN0 miners, with an off switch per miner
for the launch period (90 days from each network's first block).

  python scripts/mining_control.py --net testnet=ws://127.0.0.1:9954=~/.incentium/validator-testnet-state.json \
      --net mainnet=ws://127.0.0.1:9944=~/.incentium/validator-mainnet-state.json --listen 127.0.0.1:8500

Each --net is NAME=RPC=STATE_FILE[=START], where STATE_FILE is the --state-file of the SN0
validator on this server and START (YYYY-MM-DD, optional) overrides the first block's date.
Switching a miner off writes <validator>-blocked.json next to that file: the validator then
gives the miner no tasks and no weight from its next epoch (see neuron/incentium_neuron/control.py).
After the window closes the board shows the miners read-only and the validator ignores the file.

It listens on localhost only. Caddy puts it on control.incentium.org behind the admin sign-in
(forward_auth to auth.incentium.org/verify-admin), see scripts/install-mining-control.sh.
Needs substrate-interface (the neuron venv has it).
"""
from __future__ import annotations

import argparse
import calendar
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "neuron"))
from incentium_neuron import control  # noqa: E402

UNIT = 10**9
DAY = 86400
SN0 = 0


def site_partial(name: str) -> str:
    try:
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "site", "partials", name + ".html"),
                  encoding="utf-8") as f:
            return f.read()
    except OSError:
        return ""


class SubstrateReader:
    """What the board needs from a chain, through py-substrate-interface."""

    def __init__(self, url: str):
        self.url, self.s = url, None
        self.lock = threading.Lock()

    def _call(self, f):
        from substrateinterface import SubstrateInterface

        with self.lock:
            for attempt in range(2):
                try:
                    if self.s is None:
                        self.s = SubstrateInterface(url=self.url)
                    return f(self.s)
                except Exception:
                    self.s = None
                    if attempt:
                        raise

    def first_block_time(self) -> float:
        def go(s):
            h = s.get_block_hash(1)
            return s.query("Timestamp", "Now", block_hash=h).value / 1000
        return self._call(go)

    def miners(self) -> list[dict]:
        def go(s):
            staked = {k.value for k, v in s.query_map("Consensus", "Stake") if v.value}
            out = []
            for key, hk in s.query_map("Registry", "Neurons", [SN0]):
                hotkey = hk.value
                ep = s.query("Registry", "Endpoints", [SN0, hotkey]).value
                tier = s.query("Registry", "NeuronTier", [SN0, hotkey]).value or 0
                acct = s.query("System", "Account", [hotkey]).value
                out.append({"uid": int(key.value), "hotkey": hotkey, "tier": int(tier),
                            "endpoint": decode(ep), "balance": acct["data"]["free"] / UNIT,
                            "validator": hotkey in staked})
            return sorted(out, key=lambda m: m["uid"])
        return self._call(go)


def decode(v) -> str:
    if v is None:
        return ""
    if isinstance(v, str):
        if v.startswith("0x"):
            try:
                return bytes.fromhex(v[2:]).decode(errors="replace")
            except ValueError:
                return v
        return v
    try:
        return bytes(v).decode(errors="replace")
    except (TypeError, ValueError):
        return str(v)


class Network:
    def __init__(self, name: str, chain, state_file: str, start: float | None = None, days: int = control.WINDOW_DAYS):
        self.name, self.chain, self.days = name, chain, days
        self.blocklist = control.blocklist_path(state_file)
        self.stats = control.stats_path(state_file)
        self.start_override = start
        self.lock = threading.Lock()
        self.cache, self.cached_at = None, 0.0

    def window(self) -> dict:
        """start/until from the blocklist file; else from START or the first block, written down once.
        The window is never moved later than it was first written."""
        with self.lock:
            d = control.load(self.blocklist)
            if isinstance(d.get("until"), (int, float)):
                return d
            start = self.start_override if self.start_override is not None else self.chain.first_block_time()
            d["start"], d["until"] = int(start), int(start + self.days * DAY)
            control.write_json(self.blocklist, d)
            return d

    def state(self, now: float | None = None) -> dict:
        now = time.time() if now is None else now
        try:
            w = self.window()
        except Exception as e:  # chain not reachable yet
            return {"name": self.name, "error": f"chain not reachable: {e}"}
        if self.cache is None or now - self.cached_at > 15:
            try:
                self.cache, self.cached_at, err = self.chain.miners(), now, None
            except Exception as e:
                err = f"chain not reachable: {e}"
        else:
            err = None
        stats = {}
        try:
            with open(self.stats) as f:
                stats = json.load(f)
        except (OSError, ValueError, TypeError):
            pass
        blocked = w["blocked"]
        miners = []
        for m in self.cache or []:
            b = blocked.get(m["hotkey"])
            miners.append(dict(m, off=bool(b), off_since=(b or {}).get("since"),
                               last=(stats.get("miners") or {}).get(str(m["uid"]))))
        return {"name": self.name, "start": w["start"], "until": w["until"], "open": control.window_open(w, now),
                "days_left": max(0, int((w["until"] - now + DAY - 1) // DAY)), "error": err,
                "stats_epoch": stats.get("epoch"), "stats_time": stats.get("time"), "miners": miners,
                "unknown_off": sorted(set(blocked) - {m["hotkey"] for m in self.cache or []})}

    def switch(self, hotkey: str, off: bool, note: str = "", now: float | None = None) -> str:
        now = time.time() if now is None else now
        if not isinstance(hotkey, str) or not 40 <= len(hotkey) <= 60 or not hotkey.isalnum():
            raise ValueError("not a hotkey")
        self.window()
        with self.lock:
            d = control.load(self.blocklist)
            if not control.window_open(d, now):
                raise PermissionError("the launch window has closed: miners can no longer be switched off")
            if off:
                d["blocked"].setdefault(hotkey, {"since": int(now), "note": str(note)[:200]})
            else:
                d["blocked"].pop(hotkey, None)
            control.write_json(self.blocklist, d)
        self.cache = None
        return "off" if off else "on"


def make_handler(nets: dict[str, Network], host_name: str = "control.incentium.org"):
    page = (PAGE.replace("__TOPBAR__", site_partial("topbar")).replace("__FOOTER__", site_partial("footer"))).encode()

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code, body: bytes, ctype="application/json"):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Frame-Options", "DENY")
            self.send_header("Referrer-Policy", "no-referrer")
            self.end_headers()
            self.wfile.write(body)

        def json(self, code, obj):
            self.send(code, json.dumps(obj).encode())

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path == "/":
                return self.send(200, page, "text/html; charset=utf-8")
            if path == "/api/state":
                return self.json(200, {"user": self.headers.get("X-Incentium-User", ""),
                                       "networks": [n.state() for n in nets.values()]})
            if path == "/healthz":
                return self.send(200, b"ok", "text/plain")
            self.json(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/api/switch":
                return self.json(404, {"error": "not found"})
            # The page posts JSON from its own origin; refuse anything else (CSRF).
            origin = self.headers.get("Origin", "")
            if origin not in (f"https://{host_name}", "") or self.headers.get("Content-Type", "") != "application/json":
                return self.json(403, {"error": "forbidden"})
            try:
                n = int(self.headers.get("Content-Length", "0"))
                req = json.loads(self.rfile.read(min(n, 4096)) or b"{}")
                net = nets[req["net"]]
                state = net.switch(req["hotkey"], bool(req["off"]), req.get("note", ""))
            except PermissionError as e:
                return self.json(409, {"error": str(e)})
            except (KeyError, ValueError, TypeError):
                return self.json(400, {"error": "bad request"})
            who = self.headers.get("X-Incentium-User", "?")
            print(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {who} switched {req['hotkey']} {state} on {req['net']}",
                  flush=True)
            self.json(200, {"ok": True, "state": state})

    return H


def parse_net(spec: str) -> tuple[str, str, str, float | None]:
    parts = spec.split("=")
    if len(parts) not in (3, 4):
        raise argparse.ArgumentTypeError("use NAME=RPC=STATE_FILE[=YYYY-MM-DD]")
    start = calendar.timegm(time.strptime(parts[3], "%Y-%m-%d")) if len(parts) == 4 and parts[3] else None
    return parts[0], parts[1], os.path.expanduser(parts[2]), start


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--net", type=parse_net, action="append", required=True)
    ap.add_argument("--listen", default="127.0.0.1:8500")
    ap.add_argument("--days", type=int, default=control.WINDOW_DAYS)
    a = ap.parse_args()
    nets = {name: Network(name, SubstrateReader(url), state, start, a.days) for name, url, state, start in a.net}
    host, port = a.listen.rsplit(":", 1)
    srv = ThreadingHTTPServer((host, int(port)), make_handler(nets))
    print(f"mining control on {a.listen}: {', '.join(nets)}", flush=True)
    srv.serve_forever()


PAGE = r"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Mining control · Incentium</title><meta name="robots" content="noindex">
<link rel="icon" href="https://incentium.org/brand/incentium-mark.svg">
<style>
:root { --bg:#151a24; --panel:#1d2330; --line:#2c3446; --text:#e8e6e1; --muted:#9aa0ad; --lamp:#e8a33d; --ok:#5fb98a; --bad:#e0685c;
  --sans: Inter, system-ui, -apple-system, "Segoe UI", sans-serif; --serif: "EB Garamond", Georgia, serif; }
* { box-sizing: border-box; } body { margin:0; background:var(--bg); color:var(--text); font:15px/1.5 var(--sans); }
main { max-width: 1180px; margin: 0 auto; padding: 40px 24px 80px; }
.eyebrow { color: var(--lamp); font-size: 12px; letter-spacing: .14em; text-transform: uppercase; font-weight: 600; }
h1 { font: 400 42px/1.1 var(--serif); margin: 8px 0 6px; } h2 { font: 400 28px var(--serif); margin: 0; }
.lead { color: var(--muted); margin: 0 0 28px; max-width: 760px; }
.net { background: var(--panel); border: 1px solid var(--line); border-radius: 12px; padding: 22px; margin-bottom: 28px; }
.nethead { display:flex; flex-wrap:wrap; align-items:baseline; gap: 10px 18px; margin-bottom: 14px; }
.pill { font-size: 13px; border-radius: 999px; padding: 3px 11px; border: 1px solid var(--line); color: var(--muted); }
.pill.open { color: #1b1b1b; background: var(--lamp); border-color: var(--lamp); font-weight: 600; }
.pill.closed { color: var(--muted); }
.summary { color: var(--muted); font-size: 14px; }
table { width:100%; border-collapse: collapse; font-size: 14px; }
th { text-align:left; color: var(--muted); font-weight: 500; font-size: 12px; text-transform: uppercase; letter-spacing: .06em; padding: 8px; border-bottom: 1px solid var(--line); }
td { padding: 10px 8px; border-bottom: 1px solid #252c3b; vertical-align: middle; }
tr.off td { opacity: .55; } tr.off td.act { opacity: 1; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 13px; }
.dot { display:inline-block; width:8px; height:8px; border-radius:50%; margin-right:6px; background: var(--muted); }
.dot.ok { background: var(--ok); } .dot.bad { background: var(--bad); }
button { font: 600 13px var(--sans); border-radius: 8px; padding: 6px 12px; cursor: pointer; border: 1px solid var(--line); background: transparent; color: var(--text); }
button.off { border-color: var(--bad); color: var(--bad); } button.on { background: var(--ok); border-color: var(--ok); color: #10241a; }
button:disabled { opacity: .4; cursor: default; }
.err { color: var(--bad); } .muted { color: var(--muted); } .copy { cursor: pointer; border-bottom: 1px dotted var(--muted); }
@media (max-width: 760px) { .hide-s { display: none; } h1 { font-size: 32px; } }
</style></head>
<body data-site="admin">__TOPBAR__
<main>
  <div class="eyebrow">Operator only</div>
  <h1>Mining control</h1>
  <p class="lead">Every miner registered on SN0, with its last epoch as measured by your validator. A miner you switch off
  gets no tasks and no weight from your validator from the next epoch, so it earns nothing through it.
  The switch works for the first 90 days of each network; after that this page is read-only.</p>
  <div id="nets"><p class="muted">Loading…</p></div>
</main>
__FOOTER__
<script>
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
const day = (t) => t ? new Date(t * 1000).toISOString().slice(0, 10) : "–";
const short = (h) => h.slice(0, 6) + "…" + h.slice(-6);
const ago = (t) => { if (!t) return ""; const m = Math.round((Date.now() / 1000 - t) / 60); return m < 60 ? m + " min ago" : Math.round(m / 60) + " h ago"; };
const tiers = ["2B", "4B", "9B", "27B"];
function row(n, m) {
  const l = m.last;
  const work = m.validator ? '<span class="muted">validator</span>'
    : m.off ? '<span class="muted">switched off ' + day(m.off_since) + '</span>'
    : !l ? '<span class="muted">not measured yet</span>'
    : `<span class="dot ${l.answered ? "ok" : "bad"}"></span>${l.answered}/${l.requests} answered · ${l.tokens} tokens · checks ${l.checks_passed}/${l.checks}`;
  const act = m.validator ? "" : `<button class="${m.off ? "on" : "off"}" data-net="${esc(n.name)}" data-hotkey="${esc(m.hotkey)}" data-off="${m.off ? 0 : 1}" ${n.open ? "" : "disabled"}>${m.off ? "Switch on" : "Switch off"}</button>`;
  return `<tr class="${m.off ? "off" : ""}"><td>${m.uid}</td>
    <td class="mono"><span class="copy" title="${esc(m.hotkey)} (click to copy)" data-copy="${esc(m.hotkey)}">${esc(short(m.hotkey))}</span></td>
    <td>${esc(tiers[m.tier] || m.tier)}</td><td>${work}</td><td class="hide-s">${m.last && !m.off ? m.last.weight : "–"}</td>
    <td class="hide-s">${m.balance.toLocaleString("en", {maximumFractionDigits: 2})} ICT</td><td class="act">${act}</td></tr>`;
}
function render(d) {
  document.getElementById("nets").innerHTML = d.networks.map((n) => {
    if (n.error && !n.miners) return `<section class="net"><h2>${esc(n.name)}</h2><p class="err">${esc(n.error)}</p></section>`;
    const miners = n.miners.filter((m) => !m.validator), off = miners.filter((m) => m.off).length;
    return `<section class="net"><div class="nethead"><h2>${esc(n.name)}</h2>
      <span class="pill ${n.open ? "open" : "closed"}">${n.open ? `Switch open · ${n.days_left} days left (until ${day(n.until)})` : `Read-only since ${day(n.until)}`}</span>
      <span class="summary">${miners.length} miners · ${off} switched off · last epoch ${n.stats_epoch ?? "–"} ${n.stats_time ? "(" + ago(n.stats_time) + ")" : ""}</span></div>
      ${n.error ? `<p class="err">${esc(n.error)} (showing the last list)</p>` : ""}
      <table><tr><th>UID</th><th>Hotkey</th><th>Tier</th><th>Last epoch</th><th class="hide-s">Weight</th><th class="hide-s">Balance</th><th></th></tr>
      ${n.miners.map((m) => row(n, m)).join("") || '<tr><td colspan="7" class="muted">No miners registered.</td></tr>'}</table>
      ${n.unknown_off.length ? `<p class="muted">Also switched off, no longer registered: ${n.unknown_off.map(short).join(", ")}</p>` : ""}</section>`;
  }).join("");
}
async function load() {
  try { const r = await fetch("/api/state", {cache: "no-store"}); render(await r.json()); }
  catch (e) { document.getElementById("nets").innerHTML = '<p class="err">Could not load the miners.</p>'; }
}
document.addEventListener("click", async (e) => {
  const c = e.target.closest("[data-copy]");
  if (c) { navigator.clipboard?.writeText(c.dataset.copy); return; }
  const b = e.target.closest("button[data-hotkey]");
  if (!b) return;
  const off = b.dataset.off === "1";
  if (off && !confirm(`Switch off miner ${b.dataset.hotkey}?\nIt gets no tasks and no weight from your validator from the next epoch.`)) return;
  b.disabled = true;
  const r = await fetch("/api/switch", {method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({net: b.dataset.net, hotkey: b.dataset.hotkey, off})});
  if (!r.ok) alert((await r.json().catch(() => ({}))).error || "Failed");
  load();
});
load(); setInterval(load, 30000);
</script></body></html>"""

if __name__ == "__main__":
    main()
