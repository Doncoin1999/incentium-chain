// Incentium Explorer front end. Plain JavaScript, no build step.
// Data comes from the explorer API (/api/...); amounts are strings in base units (1 ICT = 10^9).
"use strict";

const UNIT = 1000000000n;
const MAX_SUPPLY = 21000000n * UNIT;
const app = document.getElementById("app");
const WALLETS = [["talisman", "Talisman"], ["subwallet-js", "SubWallet"], ["polkadot-js", "Polkadot.js"]];
let refreshTimer = null;

// ------------------------------------------------------------------ helpers
const esc = (v) => String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const big = (v) => { try { return BigInt(v ?? 0); } catch { return 0n; } };

function ict(v, digits = 2) {
  const x = big(v), neg = x < 0n, a = neg ? -x : x;
  const scale = 10n ** BigInt(9 - digits);
  const r = (a + scale / 2n) / scale;               // rounded to `digits` decimals
  const whole = r / 10n ** BigInt(digits), frac = r % 10n ** BigInt(digits);
  const w = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return (neg ? "−" : "") + w + (digits ? "." + frac.toString().padStart(digits, "0") : "");
}
const signed = (v, d = 2) => (big(v) > 0n ? "+" : "") + ict(v, d);
const pct = (num, den, d = 2) => { const D = big(den); return D ? (Number(big(num) * 1000000n / D) / 10000).toFixed(d) + "%" : "–"; };
const short = (a) => a ? a.slice(0, 6) + "…" + a.slice(-6) : "";
const n = (x) => Number(x ?? 0).toLocaleString("en-US");
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
function date(ts, time = true) {
  if (!ts) return "–";
  const d = new Date(ts * 1000), p = (x) => String(x).padStart(2, "0");
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()}` + (time ? ` ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}` : "");
}
function ago(ts) {
  if (!ts) return "–";
  const s = Math.max(0, Math.round(Date.now() / 1000 - ts));
  if (s < 90) return s + " s ago";
  if (s < 5400) return Math.round(s / 60) + " min ago";
  if (s < 172800) return Math.round(s / 3600) + " h ago";
  return Math.round(s / 86400) + " days ago";
}
async function get(path) {
  const r = await fetch(path, { headers: { Accept: "application/json" } });
  if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
  return r.json();
}
const store = {
  get(k) { try { return JSON.parse(localStorage.getItem(k)); } catch { return null; } },
  set(k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, JSON.stringify(v)); } catch { /* storage off */ } },
};
const acctLink = (addr, label) => addr
  ? `<a href="#/account/${esc(addr)}">${label ? `<b>${esc(label)}</b> ` : ""}<span class="mono muted">${esc(short(addr))}</span></a>`
  : "–";
const amountCell = (v, note) => v == null ? `<td class="num muted">–</td>`
  : `<td class="num ${big(v) > 0n ? "pos" : ""}">${signed(v)}${note === "locked" ? ' <span class="muted">locked</span>' : ""}</td>`;
const fail = (e) => `<div class="panel empty">Could not load: ${esc(e.message)}. The indexer may still be catching up; try again in a minute.</div>`;

// ------------------------------------------------------------------ wallet (read-only)
function walletState() { return store.get("wallet"); }

function renderWalletSlot() {
  const w = walletState(), slot = document.getElementById("wallet-slot");
  if (w && w.accounts && w.accounts.length) {
    const cur = w.accounts[w.current || 0];
    slot.innerHTML = `<a class="wallet" href="#/me"><span class="dot" aria-hidden="true"></span>
      <span>${esc(w.id === "watch" ? "Following" : w.name)} · <span class="mono">${esc(cur.address.slice(0, 4) + "…" + cur.address.slice(-4))}</span></span>
      <span class="muted" style="padding:0 8px">My account</span></a>`;
  } else {
    slot.innerHTML = `<a class="btn primary" href="#/me">Connect wallet</a>`;
  }
}

async function findWallets() {
  for (let i = 0; i < 10 && !window.injectedWeb3; i++) await new Promise((r) => setTimeout(r, 100));
  const inj = window.injectedWeb3 || {};
  return WALLETS.filter(([id]) => inj[id]);
}

async function connect(id, name, say) {
  const inj = window.injectedWeb3 && window.injectedWeb3[id];
  if (!inj) throw new Error(`${name} is not available in this browser. Reload the page and try again.`);
  say(`Waiting for ${name}… Approve the request in the ${name} window. If no window opened, click the ${name} icon in the browser toolbar.`);
  const ext = await Promise.race([
    inj.enable("Incentium Explorer"),
    new Promise((_, no) => setTimeout(() => no(new Error(`${name} did not answer within 2 minutes. Click the ${name} icon in the toolbar to see the request, then try again.`)), 120000)),
  ]);
  const list = await ext.accounts.get();
  const accounts = list
    .filter((a) => a.type !== "ethereum" && !String(a.address).startsWith("0x"))
    .map((a) => ({ address: a.address, name: a.name || "" }));
  if (!accounts.length) {
    throw new Error(list.length
      ? `${name} shared ${list.length} account(s), but only Ethereum (0x…) ones. Incentium uses Polkadot-format accounts (5…): add one in ${name}, allow it for this site under Connected sites, and try again.`
      : `${name} shared no accounts with this site. In ${name}, open Connected sites, allow this site and select your Polkadot (5…) accounts, then try again.`);
  }
  store.set("wallet", { id, name, accounts, current: 0 });
  renderWalletSlot();
}

// ------------------------------------------------------------------ pages
async function overview() {
  const [s, blocks, act, top] = await Promise.all([get("/api/stats"), get("/api/blocks?limit=8"), get("/api/activity?limit=8"), get("/api/accounts?limit=5")]);
  const L = s.launch || {};
  app.innerHTML = `
  <div class="headrow"><h1>Network overview</h1>
    <p class="muted" style="margin:0;font-size:14px">Mainnet · finalized #${n(s.finalized)} · refreshes every 12 s</p></div>
  <section class="tiles" aria-label="Key figures">
    ${tile("Block height", "#" + n(s.best), "finalized #" + n(s.finalized))}
    ${tile("ICT issued", ict(s.issued, 0), "of 21,000,000 max (" + pct(s.issued, MAX_SUPPLY) + ")")}
    ${tile("Locked in launch vault", ict(s.vault, 0) + " ICT", "released from day 150")}
    ${tile("Development fund", ict(s.fund, 0) + " ICT", "spent only by on-chain vote")}
    ${tile("SN0 miners / validators", n(s.miners) + " / " + n(s.validators), "registered in subnet 0")}
    ${tile("Holders", n(s.holders), "accounts with ICT")}
  </section>
  ${L.in_launch ? launchPanel(s) : ""}
  <div class="row2">
    <section class="panel" aria-labelledby="lb"><div class="bar"><h2 id="lb">Latest blocks</h2></div>
      <div class="scroll"><table><thead><tr><th>Block</th><th>Age</th><th>Author</th><th class="num">Events</th><th class="num">Extrinsics</th></tr></thead><tbody>
      ${blocks.map((b) => `<tr><td><a href="#/block/${b.n}">#${n(b.n)}</a></td><td class="muted">${ago(b.ts)}</td>
        <td class="mono">${esc(short(b.author) || "–")}</td><td class="num">${b.n_events}</td><td class="num">${b.n_extrinsics}</td></tr>`).join("")}
      </tbody></table></div></section>
    <section class="panel" aria-labelledby="la"><div class="bar"><h2 id="la">Latest activity</h2></div>
      <div class="scroll"><table><thead><tr><th>What</th><th>Account</th><th class="num">ICT</th><th>Age</th></tr></thead><tbody>
      ${act.length ? act.map((e) => `<tr><td>${esc(e.title)}</td><td>${acctLink(e.account, e.account_label)}</td>${amountCell(e.amount, e.note)}<td class="muted">${ago(e.ts)}</td></tr>`).join("")
        : `<tr><td colspan="4" class="muted">No activity indexed yet.</td></tr>`}
      </tbody></table></div></section>
  </div>
  <section class="panel" aria-labelledby="ta"><div class="bar"><h2 id="ta">Largest accounts</h2><a href="#/accounts">See top 100</a></div>
    ${accountsTable(top.rows, s.issued, false)}</section>`;
  clearInterval(refreshTimer);
  refreshTimer = setInterval(() => { if (location.hash === "" || location.hash === "#/") overview().catch(() => {}); }, 12000);
}

const tile = (l, v, sub) => `<div class="panel tile"><span class="l">${esc(l)}</span><span class="v">${esc(v)}</span><span class="s">${esc(sub)}</span></div>`;

function launchPanel(s) {
  const L = s.launch, done = Math.min(100, (s.finalized / L.until_block) * 100);
  const ms = [
    ["Day 0", "Mainnet genesis", "launch period starts", true],
    [`Day ${L.days} · ${date(L.ends_at, false)}`, "Registration opens", "sudo removed, bounties created", !L.in_launch],
    [`Day ${L.days + 60} · ${date(L.first_release_at, false)}`, "First vault release", "10% of locked rewards", false],
    ["Every 2 months", "+10% released", "claim_launch_rewards", false],
    [`${date(L.full_release_at, false)}`, "Vault fully released", "100% unlocked", false],
  ];
  return `<section class="panel pad" aria-labelledby="lp" id="launch">
    <div class="headrow"><h2 class="serif" id="lp">Launch period: day ${L.day} of ${L.days}</h2>
      <span class="muted" style="font-size:14px">Ends at block ${n(L.until_block)} (about ${date(L.ends_at, false)}) · whitepaper Section 5.1</span></div>
    <div class="progress" role="img" aria-label="${L.day} of ${L.days} days passed"><div style="width:${done.toFixed(1)}%"></div></div>
    <div class="milestones">${ms.map(([w, t, d, ok]) => `<div class="${ok ? "done" : ""}"><span>${esc(w)}</span><b>${esc(t)}</b><span>${esc(d)}</span></div>`).join("")}</div>
  </section>`;
}

function accountsTable(rows, issued, full) {
  if (!rows.length) return `<p class="empty">No accounts yet.</p>`;
  return `<div class="scroll"><table style="min-width:${full ? 980 : 640}px"><thead><tr><th>#</th><th>Account</th>
    <th class="num">Total ICT</th><th class="num">Free</th><th class="num">Locked</th>${full ? "<th>Locked in</th>" : ""}
    <th class="num">Share of issued</th>${full ? '<th class="num">24 h</th>' : ""}</tr></thead><tbody>
    ${rows.map((r) => {
      const locked = big(r.total) - big(r.free) - big(r.reserved);
      const where = [[r.stake, "stake"], [r.unstaking, "unstaking"], [r.bond, "bond"], [r.unbonding, "unbonding"],
        [big(r.vault_locked) - big(r.vault_claimed), "vault"]].filter(([v]) => big(v) > 0n).map(([v, k]) => `${k} ${ict(v, 0)}`).join(" · ");
      const share = Number(r.share_ppm) / 10000;
      return `<tr><td class="muted">${r.rank}</td>
        <td><a href="#/account/${esc(r.addr)}">${r.label ? `<b>${esc(r.label)}</b> ` : ""}${r.protocol ? '<span class="tag proto">protocol</span> ' : ""}<span class="mono muted">${esc(short(r.addr))}</span></a></td>
        <td class="num"><b>${ict(r.total)}</b></td><td class="num">${ict(r.free)}</td><td class="num">${r.protocol ? "–" : ict(locked)}</td>
        ${full ? `<td class="muted">${r.protocol ? "held for others" : esc(where || "–")}</td>` : ""}
        <td class="num"><span class="share"><i aria-hidden="true"><b style="width:${Math.min(100, Math.max(2, share)).toFixed(0)}%"></b></i>${share < 1 ? share.toFixed(2) : share.toFixed(1)}%</span></td>
        ${full ? (r.change_24h == null ? '<td class="num muted">–</td>' : `<td class="num ${big(r.change_24h) > 0n ? "pos" : ""}">${signed(r.change_24h)}</td>`) : ""}</tr>`;
    }).join("")}</tbody></table></div>`;
}

async function accountsPage(params) {
  const which = params.get("filter") || "holders", offset = +params.get("offset") || 0, limit = +params.get("limit") || 100;
  const [s, res] = await Promise.all([get("/api/stats"), get(`/api/accounts?filter=${which}&offset=${offset}&limit=${limit}`)]);
  const link = (o = {}) => "#/accounts?" + new URLSearchParams({ filter: which, offset, limit, ...o });
  const pills = [["holders", "Holders"], ["protocol", "Protocol accounts"], ["all", "Both"]];
  app.innerHTML = `
  <div style="display:flex;flex-direction:column;gap:8px"><h1>Top accounts</h1>
    <p class="muted" style="margin:0;max-width:52em">Ranked by total ICT: free balance plus everything locked — validator stake, work bonds and the account's share of the launch vault. ICT held by protocol accounts on someone's behalf is counted with that someone, so those accounts are listed separately. Snapshot at block #${n(res.snapshot_block)}, refreshed about every 10 minutes.</p></div>
  <div class="headrow" style="align-items:center">
    <div class="pills" role="group" aria-label="Which accounts">${pills.map(([k, l]) => `<button class="pill" aria-pressed="${k === which}" data-go="${link({ filter: k, offset: 0 })}">${l}</button>`).join("")}</div>
    <div class="muted" style="display:flex;flex-wrap:wrap;gap:8px 20px;font-size:14px">
      <span>Issued <b style="color:var(--text)">${ict(s.issued, 0)} ICT</b></span><span>Accounts <b style="color:var(--text)">${n(res.count)}</b></span>
      <a href="/api/accounts?filter=${which}&offset=${offset}&limit=${limit}&format=csv">Download CSV</a></div>
  </div>
  <section class="panel" aria-label="Accounts">${accountsTable(res.rows, s.issued, true)}</section>
  <nav class="pager" aria-label="Pages"><span>Showing ${n(res.count ? offset + 1 : 0)}–${n(Math.min(offset + limit, res.count))} of ${n(res.count)}</span>
    <span class="pills">
      ${offset > 0 ? `<a class="btn" href="${link({ offset: Math.max(0, offset - limit) })}">Previous</a>` : ""}
      ${offset + limit < res.count ? `<a class="btn" href="${link({ offset: offset + limit })}">Next ${limit}</a>` : ""}
      <label class="sr" for="per">Per page</label>
      <select id="per" class="btn" data-per>${[100, 250, 500].map((x) => `<option ${x === limit ? "selected" : ""}>${x}</option>`).join("")}</select>
    </span></nav>`;
  app.querySelector("[data-per]").addEventListener("change", (e) => { location.hash = link({ limit: e.target.value, offset: 0 }); });
}

async function accountView(addr, opts = {}) {
  const a = await get(`/api/account/${encodeURIComponent(addr)}`);
  if (!a.known) {
    return `${opts.head || ""}<div style="display:flex;flex-direction:column;gap:8px"><h1>${esc(opts.title || "Account")}</h1>
      <code class="mono muted" style="word-break:break-all">${esc(a.addr)}</code></div>
      <div class="panel empty">This address has no ICT and no activity on Incentium yet.</div>`;
  }
  const free = big(a.free) + big(a.reserved), staked = big(a.stake) + big(a.unstaking), bond = big(a.bond) + big(a.unbonding);
  const vault = big(a.vault_locked) - big(a.vault_claimed), total = big(a.total) || 1n;
  const w = (x) => (Number(x * 10000n / total) / 100).toFixed(2) + "%";
  const parts = [["Free", free, "can be sent now", "var(--text)"], ["Staked", staked, big(a.unstaking) ? `${ict(a.unstaking)} unstaking` : "validator stake in SN0", "var(--lamp)"],
    ["Work bond", bond, "miner or pool bond", "#b7c0d1"], ["Launch vault", vault, `claimable now: ${ict(a.vault_schedule?.claimable)}`, "var(--vault)"]]
    .filter(([l, v]) => l === "Free" || l === "Launch vault" || v > 0n);
  const title = opts.title || a.label || "Account";
  const tags = [a.protocol ? '<span class="tag proto">protocol account</span>' : "", a.role ? `<span class="tag">${esc(a.role === "validator" ? "Validator" : "Miner")} · SN0${a.role === "miner" ? " · tier " + (a.tier ?? 0) : ""}</span>` : ""].join(" ");
  const V = a.vault_schedule || {};
  const vaultBox = big(a.vault_locked) > 0n ? `
    <section class="panel pad" aria-labelledby="vault" style="flex:1 1 520px">
      <h2 id="vault">Launch vault releases</h2>
      <p class="muted" style="margin:0;font-size:14px">${ict(V.locked)} ICT locked so far${V.growing ? "; the amount grows until the launch period ends" : ""}. Claimed: ${ict(V.claimed)} ICT.</p>
      <div class="inset"><span>Claimable now <b>${ict(V.claimable)} ICT</b></span><span class="muted">Claim in your wallet with claim_launch_rewards</span></div>
      <div class="scroll"><table><thead><tr><th>Date (approx.)</th><th>Block</th><th class="num">Unlocked</th><th class="num">At current amount</th></tr></thead><tbody>
      ${(V.releases || []).map((r) => `<tr><td>${date(r.at, false)}</td><td class="muted">#${n(r.block)}</td><td class="num">${r.pct}%</td><td class="num">${ict(r.unlocked)}</td></tr>`).join("")}
      </tbody></table></div></section>` : "";
  const roleBox = `<section class="panel pad" aria-labelledby="role" style="flex:1 1 340px"><h2 id="role">Details</h2><dl class="kv">
      <dt>Rank</dt><dd>${a.protocol ? "protocol account" : "#" + n(a.rank) + " of " + n(a.holders)}</dd>
      <dt>Share of issued ICT</dt><dd>${(Number(a.share_ppm) / 10000).toFixed(2)}%</dd>
      <dt>Change last 24 h</dt><dd>${a.change_24h == null ? "–" : signed(a.change_24h) + " ICT"}</dd>
      ${a.role ? `<dt>SN0 uid</dt><dd>${a.uid}</dd>` : ""}
      ${a.first_block != null ? `<dt>First seen</dt><dd><a href="#/block/${a.first_block}">block #${n(a.first_block)}</a></dd>` : ""}
      <dt>Recorded events</dt><dd>${n(a.tx_count)}</dd></dl></section>`;
  return `${opts.head || ""}
  <div style="display:flex;flex-direction:column;gap:10px">
    ${opts.crumb ?? `<p class="muted" style="margin:0;font-size:14px"><a href="#/accounts" class="muted">Accounts</a> › ${a.protocol ? "protocol" : "rank #" + n(a.rank)}</p>`}
    <div style="display:flex;flex-wrap:wrap;align-items:center;gap:8px 14px"><h1 style="font-size:36px">${esc(title)}</h1>${tags}</div>
    <div style="display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px"><code class="mono muted" style="font-size:14px;word-break:break-all">${esc(a.addr)}</code>
      <button class="btn" data-copy="${esc(a.addr)}">Copy address</button></div>
  </div>
  <section class="panel pad" aria-labelledby="bal">
    <div class="headrow"><h2 id="bal">Balance</h2><span class="muted" style="font-size:14px">snapshot, refreshed about every 10 minutes</span></div>
    <div style="display:flex;flex-wrap:wrap;align-items:baseline;gap:6px 14px"><span class="big">${ict(a.total)} ICT</span></div>
    ${a.protocol ? "" : `<div class="stack" role="img" aria-label="${parts.map(([l, v]) => `${l} ${w(v)}`).join(", ")}">${parts.map(([, v, , c]) => `<div style="width:${w(v)};background:${c}"></div>`).join("")}</div>`}
    <div class="parts">${parts.map(([l, v, sub, c]) => `<div><span class="sw" style="background:${c}" aria-hidden="true"></span><div style="display:flex;flex-direction:column">
      <span class="muted" style="font-size:13px">${l}</span><span class="v">${ict(v)} ICT</span><span class="muted" style="font-size:13px">${esc(sub)}</span></div></div>`).join("")}</div>
  </section>
  <div class="row2">${vaultBox}${roleBox}</div>
  <section class="panel" aria-labelledby="tx" data-events="${esc(a.addr)}">
    <div class="bar"><h2 id="tx">${opts.mine ? "My transactions and rewards" : "Transactions and rewards"}</h2>
      <div class="pills" role="group" aria-label="Filter by type">
        ${[["", "All"], ["transfer", "Transfers"], ["reward", "Rewards"], ["stake", "Staking"], ["vault", "Vault"], ["other", "Other"]]
          .map(([k, l]) => `<button class="pill" data-kind="${k}" aria-pressed="${k === ""}">${l}</button>`).join("")}
        <a class="btn" data-csv href="/api/account/${esc(a.addr)}/events?format=csv">Download CSV</a></div></div>
    <div data-rows></div>
  </section>`;
}

async function loadEvents(section, kind, offset, append) {
  const addr = section.dataset.events, box = section.querySelector("[data-rows]");
  const res = await get(`/api/account/${encodeURIComponent(addr)}/events?offset=${offset}&limit=25${kind ? "&kind=" + kind : ""}`);
  const rows = res.rows.map((e) => `<tr><td class="muted" style="white-space:nowrap">${date(e.ts)}</td><td><a href="#/block/${e.block}">#${n(e.block)}</a></td>
    <td>${esc(e.title)}</td><td>${e.counterparty ? acctLink(e.counterparty, e.counterparty_label) : '<span class="muted">–</span>'}</td>${amountCell(e.amount, e.note)}
    <td class="muted">${esc(e.note && e.note !== "locked" ? e.note : "")}</td></tr>`).join("");
  const more = offset + res.rows.length < res.count;
  const footer = `<p class="pager" style="margin:0;padding:14px 20px;border-top:1px solid var(--line)" data-foot>Showing ${n(offset + res.rows.length)} of ${n(res.count)}
    ${more ? `<button class="btn" data-more="${offset + res.rows.length}">Load more</button>` : ""}</p>`;
  if (append) {
    box.querySelector("tbody").insertAdjacentHTML("beforeend", rows);
    box.querySelector("[data-foot]").outerHTML = footer;
  } else {
    box.innerHTML = res.count ? `<div class="scroll"><table style="min-width:860px"><thead><tr><th>Time (UTC)</th><th>Block</th><th>What</th><th>Counterparty</th>
      <th class="num">ICT</th><th>Note</th></tr></thead><tbody>${rows}</tbody></table></div>${footer}` : `<p class="empty">Nothing of this type yet.</p>`;
  }
  section.dataset.kind = kind;
  section.querySelector("[data-csv]").href = `/api/account/${encodeURIComponent(addr)}/events?format=csv${kind ? "&kind=" + kind : ""}`;
}

function wireAccount() {
  const sec = app.querySelector("[data-events]");
  if (sec) loadEvents(sec, "", 0, false).catch((e) => { sec.querySelector("[data-rows]").innerHTML = `<p class="empty">${esc(e.message)}</p>`; });
}

async function accountPage(addr) {
  app.innerHTML = await accountView(addr);
  wireAccount();
}

async function myAccount() {
  const w = walletState();
  if (!w || !w.accounts || !w.accounts.length) {
    const found = await findWallets();
    app.innerHTML = `<h1>Connect your wallet</h1>
    <section class="panel pad" style="max-width:760px">
      <p style="margin:0">See your balance, staked ICT, launch-vault releases and history. The explorer only reads your public addresses; it never asks you to sign or send anything.</p>
      ${found.length ? `<div class="pills">${found.map(([id, name]) => `<button class="btn primary" data-connect="${id}" data-name="${name}">Connect ${name}</button>`).join("")}</div>`
        : `<p class="muted" style="margin:0">No Polkadot wallet extension found in this browser. Install <a href="https://talisman.xyz">Talisman</a>, <a href="https://www.subwallet.app">SubWallet</a> or <a href="https://polkadot.js.org/extension/">Polkadot.js</a>, then reload this page.</p>`}
      <p style="margin:0;font-size:14px" data-msg role="status" aria-live="polite"></p>
      <form data-watch style="display:flex;flex-wrap:wrap;gap:8px;align-items:center;border-top:1px solid var(--line);padding-top:16px">
        <label for="watch" style="flex:1 0 100%;font-size:14px" class="muted">Or follow an address without a wallet extension:</label>
        <input id="watch" name="addr" autocomplete="off" spellcheck="false" placeholder="5…" style="flex:1 1 320px;min-width:0;height:44px;padding:0 14px;background:var(--slate);border:1px solid var(--line);border-radius:6px;color:var(--text);font:14px var(--mono)">
        <button class="btn" type="submit">Follow address</button>
      </form>
    </section>`;
    app.querySelector("[data-watch]").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const msg = app.querySelector("[data-msg]"), q = ev.target.querySelector("[name=addr]").value.trim();
      try {
        const r = await get("/api/search?q=" + encodeURIComponent(q));
        if (r.type !== "account") { msg.textContent = "That is not a valid address."; return; }
        store.set("wallet", { id: "watch", name: "Followed address", accounts: [{ address: r.id, name: "" }], current: 0 });
        renderWalletSlot(); route();
      } catch (e) { msg.textContent = e.message; }
    });
    return;
  }
  const cur = Math.min(w.current || 0, w.accounts.length - 1);
  const head = `<div class="notice"><span class="muted">${w.id === "watch" ? "Following an address (no wallet connected)." : `Connected read-only with ${esc(w.name)}: the explorer never asks you to sign.`}</span>
    <span class="pills" role="group" aria-label="Accounts in your wallet">${w.accounts.map((a, i) =>
      `<button class="pill" data-acc="${i}" aria-pressed="${i === cur}">${esc(a.name || short(a.address))}</button>`).join("")}
      <button class="btn" data-disconnect>Disconnect</button></span></div>`;
  app.innerHTML = await accountView(w.accounts[cur].address, { head, title: w.accounts[cur].name || "My account", mine: true, crumb: "" });
  wireAccount();
}

async function blockPage(id) {
  const b = await get(`/api/block/${encodeURIComponent(id)}`);
  app.innerHTML = `<div style="display:flex;flex-direction:column;gap:8px"><p class="muted" style="margin:0;font-size:14px">Blocks</p><h1>Block #${n(b.n)}</h1></div>
  <section class="panel pad"><dl class="kv">
    <dt>Time (UTC)</dt><dd>${date(b.ts)} · ${ago(b.ts)}</dd><dt>Hash</dt><dd class="mono" style="word-break:break-all">${esc(b.hash)}</dd>
    <dt>Author (Aura key)</dt><dd class="mono">${esc(b.author || "–")}</dd><dt>Extrinsics</dt><dd>${b.n_extrinsics}</dd></dl>
    <div class="pills">${b.n > 0 ? `<a class="btn" href="#/block/${b.n - 1}">Previous block</a>` : ""}<a class="btn" href="#/block/${b.n + 1}">Next block</a></div></section>
  <section class="panel"><div class="bar"><h2>Events (${b.events.length})</h2></div>
    <div class="scroll"><table><thead><tr><th>#</th><th>Event</th><th>Data</th></tr></thead><tbody>
    ${b.events.map((e) => `<tr><td class="muted">${e.idx}</td><td style="white-space:nowrap">${esc(e.module)}.${esc(e.name)}</td><td><pre class="ev">${esc(JSON.stringify(e.data))}</pre></td></tr>`).join("") || '<tr><td colspan="3" class="muted">No events.</td></tr>'}
    </tbody></table></div></section>`;
}

async function launchPage() {
  const s = await get("/api/stats");
  app.innerHTML = s.launch && s.launch.until_block ? `<h1>Launch period</h1>${launchPanel(Object.assign({}, s, { launch: Object.assign({}, s.launch, { in_launch: s.launch.in_launch }) }))}
    <section class="panel pad"><p style="margin:0">During the first ${s.launch.days} days only the launch operator registers miners and validators. A quarter of every reward is paid at once; three quarters go into the launch vault and are released 10% at a time from day ${s.launch.days + 60}. At the end of the period the runtime removes the sudo key and creates the marketing and exchange-listing bounties. Details: <a href="https://incentium.org">whitepaper Section 5.1</a>.</p>
    <div class="tiles">${tile("In the launch vault", ict(s.vault, 0) + " ICT", "locked rewards of all accounts")}${tile("Development fund", ict(s.fund, 0) + " ICT", "on-chain treasury")}</div></section>`
    : `<h1>Launch period</h1><div class="panel empty">The launch period has ended.</div>`;
}

// ------------------------------------------------------------------ router and events
async function route() {
  clearInterval(refreshTimer);
  const [path, query] = (location.hash.slice(1) || "/").split("?");
  const parts = path.split("/").filter(Boolean), params = new URLSearchParams(query || "");
  const nav = { "": "overview", accounts: "accounts", launch: "launch" }[parts[0] || ""];
  document.querySelectorAll("[data-nav]").forEach((a) => a.dataset.nav === nav ? a.setAttribute("aria-current", "page") : a.removeAttribute("aria-current"));
  try {
    if (!parts.length) await overview();
    else if (parts[0] === "accounts") await accountsPage(params);
    else if (parts[0] === "account" && parts[1]) await accountPage(decodeURIComponent(parts[1]));
    else if (parts[0] === "me") await myAccount();
    else if (parts[0] === "block" && parts[1]) await blockPage(parts[1]);
    else if (parts[0] === "launch") await launchPage();
    else app.innerHTML = `<div class="panel empty">Page not found. <a href="#/">Back to overview</a></div>`;
  } catch (e) {
    app.innerHTML = fail(e);
  }
  document.title = (app.querySelector("h1")?.textContent || "Overview") + " · Incentium Explorer";
}

document.getElementById("search").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const q = ev.target.q.value.trim();
  if (!q) return;
  try {
    const r = await get("/api/search?q=" + encodeURIComponent(q));
    if (r.type === "account") location.hash = "#/account/" + r.id;
    else if (r.type === "block") location.hash = "#/block/" + r.id;
    else app.innerHTML = `<div class="panel empty">Nothing found for “${esc(q)}”. Search takes an address (5…), a block number or a block hash.</div>`;
  } catch (e) { app.innerHTML = fail(e); }
});

app.addEventListener("click", async (ev) => {
  const t = ev.target.closest("button, [data-go]");
  if (!t) return;
  if (t.dataset.go) { location.hash = t.dataset.go; return; }
  if (t.dataset.copy) { try { await navigator.clipboard.writeText(t.dataset.copy); t.textContent = "Copied"; } catch { t.textContent = "Select and copy"; } return; }
  if (t.dataset.kind !== undefined) {
    const sec = t.closest("[data-events]");
    sec.querySelectorAll("[data-kind]").forEach((b) => b.setAttribute("aria-pressed", String(b === t)));
    loadEvents(sec, t.dataset.kind, 0, false).catch(() => {});
    return;
  }
  if (t.dataset.more) { const sec = t.closest("[data-events]"); loadEvents(sec, sec.dataset.kind || "", +t.dataset.more, true).catch(() => {}); return; }
  if (t.dataset.connect) {
    const msg = app.querySelector("[data-msg]"), say = (x) => { if (msg) msg.textContent = x; };
    app.querySelectorAll("[data-connect]").forEach((b) => { b.disabled = true; });
    try { await connect(t.dataset.connect, t.dataset.name, say); route(); }
    catch (e) {
      say(String((e && e.message) || e || "").match(/reject|cancel|denied/i)
        ? `${t.dataset.name} declined the request. Click Connect again and choose Allow (or Connect) in ${t.dataset.name}.`
        : ((e && e.message) || `${t.dataset.name} did not share any accounts.`));
      app.querySelectorAll("[data-connect]").forEach((b) => { b.disabled = false; });
    }
    return;
  }
  if (t.dataset.acc !== undefined) { const w = walletState(); w.current = +t.dataset.acc; store.set("wallet", w); renderWalletSlot(); route(); return; }
  if (t.dataset.disconnect !== undefined) { store.set("wallet", null); renderWalletSlot(); location.hash = "#/"; }
});

window.addEventListener("hashchange", () => { route(); app.focus({ preventScroll: true }); window.scrollTo(0, 0); });
renderWalletSlot();
route();
