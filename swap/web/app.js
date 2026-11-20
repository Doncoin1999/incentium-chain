// Incentium Swap trading page (spec Section 8). Plain JavaScript; vendored @polkadot/api and ethers.
// The page reads the order book from the swap API, signs with the user's wallets, and checks the
// chains directly before a maker reveals a secret.
"use strict";

const ICT = 10n ** 9n, USDC = 10n ** 6n, QTY_STEP = 10n ** 7n, MIN_FILL = 5n * USDC;
const H = 3600;
const app = document.getElementById("app"), dlg = document.getElementById("dlg");
const S = window.SwapSecrets, E = window.ethers, P = window.Polka;
const ABI = [
  "function placeBuy(uint256 nonce,uint128 maxQty,uint128 price,bytes32 ictRecipient,uint64 expiry,bytes32[16] fillHashes) returns (bytes32)",
  "function cancelBuy(bytes32 id)",
  "function lock(bytes32 incOrderId,uint8 fill,bytes32 hash,address payee,uint128 qty,uint128 price,uint64 expiry) returns (bytes32)",
  "function locks(bytes32) view returns (uint8 state,uint64 expiry,address taker,address payee,uint128 qty,uint128 amount,bytes32 hash,bytes32 incOrderId,uint8 fill)",
  "function reservations(bytes32,uint8) view returns (uint8 state,uint64 expiry,address payee,uint128 qty,uint128 amount)",
  "function orderId(address maker,uint256 nonce) pure returns (bytes32)",
  "function lockId(address taker,bytes32 incOrderId,uint8 fill) pure returns (bytes32)",
  "function maxOrderQty() view returns (uint256)",
];
const ERC20 = ["function approve(address,uint256) returns (bool)", "function allowance(address,address) view returns (uint256)",
  "function balanceOf(address) view returns (uint256)"];

let cfg = null, incApi = null, baseRead = null;
const state = { inc: null, base: null };   // connected accounts: {address, name} / {address}

// ------------------------------------------------------------------ small helpers
const esc = (v) => String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const big = (v) => { try { return BigInt(v ?? 0); } catch { return 0n; } };
const fmt = (v, dec, digits = 2) => { const x = big(v), s = 10n ** BigInt(dec), w = x / s, f = (x % s).toString().padStart(dec, "0").slice(0, digits); return w.toLocaleString("en-US") + (digits ? "." + f : ""); };
const ict = (v, d = 2) => fmt(v, 9, d), usd = (v, d = 2) => fmt(v, 6, d);
const price = (p) => fmt(p, 6, 4);
const short = (a) => a ? String(a).slice(0, 6) + "…" + String(a).slice(-4) : "";
const nowS = () => Math.floor(Date.now() / 1000);
const left = (t) => { const s = t - nowS(); if (s <= 0) return "ended"; const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60); return h ? `${h} h ${m} min` : `${m} min`; };
async function get(p) { const r = await fetch(p); const j = await r.json(); if (!r.ok) throw new Error(j.error || r.statusText); return j; }
async function post(p, body) { const r = await fetch(p, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }); const j = await r.json(); if (!r.ok) throw new Error(j.error || r.statusText); return j; }
const store = { get(k) { try { return JSON.parse(localStorage.getItem(k)); } catch { return null; } }, set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch { /* off */ } } };
const usdcFor = (qty, p) => big(qty) * big(p) / ICT;
const u64le = (n) => { const b = new Uint8Array(8); let x = BigInt(n); for (let i = 0; i < 8; i++) { b[i] = Number(x & 0xffn); x >>= 8n; } return b; };
const randomNonce = () => BigInt(Date.now()) * 1000n + BigInt(Math.floor(Math.random() * 1000));
function download(name, obj) { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([JSON.stringify(obj, null, 1)], { type: "application/json" })); a.download = name; a.click(); }

// ------------------------------------------------------------------ chains
async function inc() {
  if (!incApi) incApi = await P.ApiPromise.create({ provider: new P.WsProvider(cfg.incentium_rpc_public) });
  return incApi;
}
const incNow = async () => Number((await (await inc()).query.timestamp.now()).toBigInt() / 1000n);
function baseR() {
  if (!baseRead) baseRead = new E.JsonRpcProvider(cfg.base_rpc_public, Number(cfg.base_chain_id));
  return baseRead;
}
const baseNow = async () => (await baseR().getBlock("latest")).timestamp;
const swapRead = () => new E.Contract(cfg.contract, ABI, baseR());

// Several wallets may inject window.ethereum (Talisman, TAO wallet, MetaMask). Use EIP-6963 to find MetaMask itself.
const evmWallets = [];
window.addEventListener("eip6963:announceProvider", (e) => { if (!evmWallets.some((w) => w.info.uuid === e.detail.info.uuid)) evmWallets.push(e.detail); });
window.dispatchEvent(new Event("eip6963:requestProvider"));
function evmProvider() {
  const mm = evmWallets.find((w) => w.info.rdns === "io.metamask") || evmWallets.find((w) => /metamask/i.test(w.info.name));
  if (mm) return mm.provider;
  if (window.ethereum) return window.ethereum;
  throw new Error("No Ethereum wallet found. Install MetaMask.");
}

async function baseSigner() {
  const eth = evmProvider();
  await eth.request({ method: "eth_requestAccounts" });          // connect first, then the network can be switched
  const want = "0x" + Number(cfg.base_chain_id).toString(16);
  if ((await eth.request({ method: "eth_chainId" })).toLowerCase() !== want) {
    try { await eth.request({ method: "wallet_switchEthereumChain", params: [{ chainId: want }] }); }
    catch (e) { throw new Error(`Switch MetaMask to ${Number(cfg.base_chain_id) === 84532 ? "Base Sepolia" : "Base"} and try again. (${e.message || e})`); }
  }
  const signer = await new E.BrowserProvider(eth).getSigner();
  state.base = { address: (await signer.getAddress()).toLowerCase() };
  renderWallets();
  return signer;
}

async function connectInc() {
  const ext = await P.web3Enable("Incentium Swap");
  if (!ext.length) throw new Error("No Polkadot wallet found. Install Talisman, SubWallet or Polkadot.js.");
  // Several wallets may inject (Talisman, a TAO wallet, ...): prefer Talisman, then let the user pick the account.
  state.exts = ext;
  const names = ext.map((e) => e.name);
  const pick = names.includes("talisman") ? ["talisman"] : names;
  const accs = (await P.web3Accounts({ extensions: pick })).filter((a) => a.type !== "ethereum");
  if (!accs.length) throw new Error("Your wallet shared no Polkadot (5…) accounts with this site.");
  state.incAll = accs.map((a) => ({ address: P.encodeAddress(P.decodeAddress(a.address), 42), name: a.meta.name || "", source: a.meta.source }));
  state.inc = state.incAll.length === 1 ? state.incAll[0] : await chooseAccount(state.incAll);
  renderWallets();
  return state.inc;
}

function chooseAccount(list) {
  return new Promise((resolve, reject) => {
    dlg.innerHTML = `<h2 style="margin-top:0">Choose your Incentium account</h2>
      <div class="fields">${list.map((a, i) => `<button class="btn" data-pick="${i}" style="text-align:left">${esc(a.name || "Account")} · <span class="mono">${esc(short(a.address))}</span> <span class="muted">(${esc(a.source)})</span></button>`).join("")}
      <button class="btn" data-pick="-1">Cancel</button></div>`;
    dlg.onclick = (e) => {
      const b = e.target.closest("[data-pick]"); if (!b) return;
      dlg.onclick = null; dlg.close();
      const i = Number(b.dataset.pick);
      i < 0 ? reject(new Error("No account chosen.")) : resolve(list[i]);
    };
    dlg.showModal();
  });
}

function sendInc(tx, from) {
  return new Promise(async (resolve, reject) => {
    try {
      // Sign with the extension the account came from; web3FromAddress would ask every wallet (and can hang on one that was dismissed).
      const own = (state.exts || []).find((e) => e.name === state.inc?.source);
      const injector = own || await P.web3FromAddress(from);
      const unsub = await tx.signAndSend(from, { signer: injector.signer }, ({ status, dispatchError }) => {
        if (dispatchError) {
          let msg = dispatchError.toString();
          if (dispatchError.isModule) { const d = incApi.registry.findMetaError(dispatchError.asModule); msg = `${d.section}.${d.name}`; }
          unsub(); reject(new Error(msg));
        } else if (status.isInBlock) { unsub(); resolve(status.asInBlock.toHex()); }
      });
    } catch (e) { reject(e); }
  });
}

function renderWallets() {
  document.getElementById("wallets").innerHTML =
    (state.inc ? `<span class="wallet"><span class="dot"></span>Incentium · <span class="mono">${esc(short(state.inc.address))}</span></span>` : `<button class="btn" data-act="conn-inc">Connect Talisman</button>`) +
    (state.base ? `<span class="wallet"><span class="dot"></span>Base · <span class="mono">${esc(short(state.base.address))}</span></span>` : `<button class="btn" data-act="conn-base">Connect MetaMask</button>`);
}

// ------------------------------------------------------------------ ids (must match the contract and pallet)
function incOrderId(maker, nonce) { return P.u8aToHex(P.blake2AsU8a(P.u8aConcat(P.decodeAddress(maker), u64le(nonce)), 256)); }
function incLockId(taker, baseOrder, fill) { return P.u8aToHex(P.blake2AsU8a(P.u8aConcat(P.decodeAddress(taker), P.hexToU8a(baseOrder), new Uint8Array([fill])), 256)); }
const baseOrderId = (maker, nonce) => E.keccak256(E.AbiCoder.defaultAbiCoder().encode(["address", "uint256"], [maker, nonce]));
const baseLockId = (taker, incOrder, fill) => E.keccak256(E.AbiCoder.defaultAbiCoder().encode(["address", "bytes32", "uint8"], [taker, incOrder, fill]));
const capNow = async () => {
  const day = Math.floor((nowS() - Number(cfg.genesis_time)) / 86400);
  return day < 90 ? 100n * ICT : day < 120 ? 1000n * ICT : 5000n * ICT;
};

// ------------------------------------------------------------------ pages
async function bookPage() {
  const b = await get("/api/book");
  const rows = (list, side) => list.length ? list.map((o) => `<tr>
      <td class="num"><b>${price(o.price)}</b></td><td class="num">${ict(o.remaining)}</td>
      <td class="num">${usd(usdcFor(o.remaining, o.price))}</td><td class="muted">${left(o.expiry)}</td>
      <td class="muted">${o.open_reservations}/3 open · ${16 - o.used_fills} fills left</td>
      <td><button class="btn" data-act="fill" data-chain="${o.chain}" data-id="${o.id}">${side === "sell" ? "Buy" : "Sell"}</button></td></tr>`).join("")
    : `<tr><td colspan="6" class="muted">No orders.</td></tr>`;
  app.innerHTML = `
  <div class="headrow"><h1>ICT ⇄ USDC</h1><p class="muted" style="margin:0;font-size:14px">USDC on Base · prices in USDC per ICT · fee 0.1%</p></div>
  <div class="grid2">
    <section class="panel"><div class="bar"><h2>Sell orders (ICT on Incentium)</h2><a class="btn" href="#/new">Place order</a></div>
      <div class="scroll"><table><thead><tr><th class="num">Price</th><th class="num">ICT</th><th class="num">USDC</th><th>Ends in</th><th>Fills</th><th></th></tr></thead><tbody>${rows(b.sells, "sell")}</tbody></table></div></section>
    <section class="panel"><div class="bar"><h2>Buy orders (USDC on Base)</h2></div>
      <div class="scroll"><table><thead><tr><th class="num">Price</th><th class="num">ICT</th><th class="num">USDC</th><th>Ends in</th><th>Fills</th><th></th></tr></thead><tbody>${rows(b.buys, "buy")}</tbody></table></div></section>
  </div>
  <p class="muted" style="font-size:14px;margin:0">Buying fills a sell order: you pay USDC on Base and receive ICT on Incentium once the seller confirms (within 12 h, otherwise your USDC comes back). Selling fills a buy order the same way in reverse.</p>`;
}

function newPage() {
  app.innerHTML = `<h1>Place an order</h1>
  <div class="grid2">
    <section class="panel pad"><h2>Sell ICT</h2>
      <p class="muted" style="margin:0;font-size:14px">Your ICT is held in your own Incentium account until a buyer pays. Signed with Talisman. You will download an encrypted backup of the order's secrets; keep it until the order is done.</p>
      <form class="fields" data-form="sell">
        <label>Quantity (ICT)<input name="qty" inputmode="decimal" required></label>
        <label>Minimum price (USDC per ICT)<input name="price" inputmode="decimal" required></label>
        <label>Base address to receive USDC<input name="recipient" class="mono" placeholder="0x…" required></label>
        <label>Order lasts (hours, 1–24)<input name="hours" type="number" min="1" max="24" value="24" required></label>
        <label>Password for the secret backup<input name="pw" type="password" minlength="10" required></label>
        <label>Repeat password<input name="pw2" type="password" minlength="10" required></label>
        <button class="btn primary" type="submit">Place sell order</button><p data-msg style="margin:0"></p>
      </form></section>
    <section class="panel pad"><h2>Buy ICT</h2>
      <p class="muted" style="margin:0;font-size:14px">Your USDC is locked in the swap contract on Base until a seller delivers. Signed with MetaMask (or Talisman's Ethereum account). Your wallet signs a message that creates the order's secrets, so no backup is needed.</p>
      <form class="fields" data-form="buy">
        <label>Quantity (ICT)<input name="qty" inputmode="decimal" required></label>
        <label>Maximum price (USDC per ICT)<input name="price" inputmode="decimal" required></label>
        <label>Incentium address to receive ICT<input name="recipient" class="mono" placeholder="5…" required></label>
        <label>Order lasts (hours, 1–24)<input name="hours" type="number" min="1" max="24" value="24" required></label>
        <button class="btn primary" type="submit">Place buy order</button><p data-msg style="margin:0"></p>
      </form></section>
  </div>`;
  if (state.base) app.querySelector('[data-form="sell"] [name=recipient]').value = state.base.address;
  if (state.inc) app.querySelector('[data-form="buy"] [name=recipient]').value = state.inc.address;
}

function parseOrder(f) {
  const qty = E.parseUnits(f.qty.value.trim(), 9), p = E.parseUnits(f.price.value.trim(), 6);
  if (qty <= 0n || qty % QTY_STEP) throw new Error("Quantity must be a multiple of 0.01 ICT.");
  if (p <= 0n) throw new Error("Price must be above 0.");
  if (usdcFor(qty, p) < MIN_FILL) throw new Error("The order must be worth at least 5 USDC.");
  const hours = Math.max(1, Math.min(24, parseInt(f.hours.value, 10) || 24));
  return { qty, p, hours };
}

async function placeSell(f, say) {
  const { qty, p, hours } = parseOrder(f);
  if (qty > await capNow()) throw new Error(`Orders are limited to ${ict(await capNow(), 0)} ICT for now.`);
  if (!E.isAddress(f.recipient.value.trim())) throw new Error("Enter the Base address (0x…) that receives the USDC.");
  if (f.pw.value !== f.pw2.value) throw new Error("The passwords differ.");
  const me = state.inc || await connectInc();
  const api = await inc();
  const nonce = randomNonce(), order = incOrderId(me.address, nonce);
  say("Creating secrets and the encrypted backup…");
  const seed = S.randomSeed(), hashes = await S.hashesFor(seed);
  const backup = await S.encryptSeed(seed, f.pw.value, "inc", order);
  store.set("swapseed:" + order, backup);
  download(`incentium-swap-${order.slice(2, 10)}.json`, backup);
  say("Backup downloaded. Approve the order in Talisman…");
  const expiry = (await incNow()) + hours * H;
  await sendInc(api.tx.swap.placeSell(nonce, qty, p, f.recipient.value.trim().toLowerCase(), expiry, hashes), me.address);
  say(`Order placed. Keep the backup file until the order is done. Order ${short(order)}.`, true);
}

async function placeBuy(f, say) {
  const { qty, p, hours } = parseOrder(f);
  let rec;
  try { rec = P.u8aToHex(P.decodeAddress(f.recipient.value.trim())); } catch { throw new Error("Enter the Incentium address (5…) that receives the ICT."); }
  const signer = await baseSigner(), me = state.base.address;
  const sw = new E.Contract(cfg.contract, ABI, signer);
  if (qty > await sw.maxOrderQty()) throw new Error(`Orders are limited to ${ict(await sw.maxOrderQty(), 0)} ICT for now.`);
  const nonce = randomNonce(), order = baseOrderId(me, nonce);
  say("Sign the message in your wallet (twice) to create the order's secrets…");
  const msg = S.seedMessage("base", nonce.toString());
  const s1 = await signer.signMessage(msg), s2 = await signer.signMessage(msg);
  if (s1 !== s2) throw new Error("Your wallet's signatures are not deterministic, so the secrets could not be recreated later. Use a different Ethereum account.");
  const hashes = await S.hashesFor(await S.seedFromSignature(s1));
  store.set("swapnonce:" + order, nonce.toString());
  const total = usdcFor(qty, p), usdc = new E.Contract(cfg.usdc, ERC20, signer);
  if (await usdc.allowance(me, cfg.contract) < total) { say("Approve USDC in your wallet…"); await (await usdc.approve(cfg.contract, total)).wait(); }
  say("Confirm the order in your wallet…");
  const expiry = (await baseNow()) + hours * H;
  await (await sw.placeBuy(nonce, qty, p, rec, expiry, hashes)).wait();
  say(`Order placed: ${usd(total)} USDC locked. Order ${short(order)}.`, true);
}

// ------------------------------------------------------------------ filling (taker)
async function fillDialog(chain, id) {
  const o = await get(`/api/order/${chain}/${id}`);
  const selling = chain === "base";                 // filling a buy order = selling ICT
  dlg.innerHTML = `<h2 style="margin-top:0">${selling ? "Sell ICT to this buy order" : "Buy ICT from this sell order"}</h2>
    <p class="muted" style="font-size:14px">Price ${price(o.price)} USDC per ICT · up to ${ict(o.remaining)} ICT.
    ${selling ? "You lock ICT on Incentium; the buyer's USDC is reserved for you on Base. You receive it when the buyer confirms (within 12 h), otherwise your ICT comes back."
              : "You lock USDC on Base; the seller's ICT is reserved for you on Incentium. You receive it when the seller confirms (within 12 h), otherwise your USDC comes back."}</p>
    <form class="fields" data-form="fill" data-chain="${chain}" data-id="${id}">
      <label>Quantity (ICT)<input name="qty" inputmode="decimal" required></label>
      <label>${selling ? "Base address to receive USDC" : "Incentium address to receive ICT"}<input name="recipient" class="mono" required value="${esc(selling ? state.base?.address || "" : state.inc?.address || "")}"></label>
      <ol class="steps" data-steps><li>Reserve the fill (the keeper sends it)</li><li>Lock your ${selling ? "ICT (Talisman)" : "USDC (MetaMask)"}</li><li>Wait for the ${selling ? "buyer" : "seller"}'s confirmation</li></ol>
      <div class="pills"><button class="btn primary" type="submit">Continue</button><button class="btn" type="button" data-act="close">Close</button></div>
      <p data-msg style="margin:0"></p></form>`;
  dlg.showModal();
}

async function doFill(f, say, step) {
  const chain = f.dataset.chain, id = f.dataset.id;
  const o = await get(`/api/order/${chain}/${id}`);
  const qty = E.parseUnits(f.qty.value.trim(), 9);
  if (qty % QTY_STEP || qty <= 0n || qty > big(o.remaining)) throw new Error("Quantity must be a multiple of 0.01 ICT and no more than what is left.");
  if (usdcFor(qty, o.price) < MIN_FILL) throw new Error("A fill must be worth at least 5 USDC.");
  const recipient = f.recipient.value.trim();
  // prepare the wallet that pays before reserving, so a reservation is not left unpaid
  if (chain === "inc") await baseSigner(); else { if (!state.inc) await connectInc(); }
  step(0); say("Reserving…");
  const { fill } = await post("/api/reserve", { chain, order: id, qty: qty.toString(), recipient });
  const h = o.hashes[fill];
  step(1);
  if (chain === "inc") {
    // check the reservation on Incentium ourselves, then lock USDC on Base
    const api = await inc();
    const r = (await api.query.swap.reservations(id, fill)).unwrap();
    const tRes = r.expiry.toNumber(), tInc = await incNow();
    if (!r.state.isActive || r.recipient.toString() !== P.encodeAddress(P.decodeAddress(recipient), 42) || r.qty.toBigInt() !== qty)
      throw new Error("The reservation on Incentium does not match. Nothing was paid.");
    if (tRes - tInc < 18 * H) throw new Error("The reservation is more than an hour old. Nothing was paid.");
    const signer = await baseSigner(), me = state.base.address;
    const amount = usdcFor(qty, o.price), usdc = new E.Contract(cfg.usdc, ERC20, signer);
    if (await usdc.allowance(me, cfg.contract) < amount) { say("Approve USDC…"); await (await usdc.approve(cfg.contract, amount)).wait(); }
    const tBase = await baseNow();
    const expiry = Math.min(tBase + 12 * H, tBase + (tRes - tInc) - 6 * H);
    say("Lock USDC in your wallet…");
    await (await new E.Contract(cfg.contract, ABI, signer).lock(id, fill, h, E.getAddress(o.recipient), qty, o.price, expiry)).wait();
    store.set("swaptake:" + baseLockId(me, id, fill), { chain, order: id, fill, at: nowS() });
  } else {
    const sw = swapRead(), r = await sw.reservations(id, fill);
    const tRes = Number(r.expiry), tBase = await baseNow();
    if (Number(r.state) !== 1 || r.payee.toLowerCase() !== recipient.toLowerCase() || r.qty !== qty)
      throw new Error("The reservation on Base does not match. Nothing was locked.");
    if (tRes - tBase < 18 * H) throw new Error("The reservation is more than an hour old. Nothing was locked.");
    const api = await inc(), tInc = await incNow();
    const expiry = Math.min(tInc + 12 * H, tInc + (tRes - tBase) - 6 * H);
    say("Lock ICT in Talisman…");
    await sendInc(api.tx.swap.lock(id, fill, h, o.recipient, qty, expiry), state.inc.address);
    store.set("swaptake:" + incLockId(state.inc.address, id, fill), { chain, order: id, fill, at: nowS() });
  }
  step(2); say(`Done. The ${chain === "inc" ? "seller" : "buyer"} has 12 hours to confirm; you can follow it under My orders.`, true);
}

// ------------------------------------------------------------------ my orders (maker confirmations)
async function mePage() {
  const addrs = [state.inc?.address, state.base?.address].filter(Boolean);
  if (!addrs.length) {
    app.innerHTML = `<h1>My orders</h1><div class="panel pad"><p style="margin:0">Connect Talisman (Incentium) and/or MetaMask (Base) to see your orders and confirm fills.</p>
      <div class="pills"><button class="btn primary" data-act="conn-inc">Connect Talisman</button><button class="btn primary" data-act="conn-base">Connect MetaMask</button></div></div>`;
    return;
  }
  const mine = await get("/api/account?" + addrs.map((a) => "addr=" + encodeURIComponent(a)).join("&"));
  const details = await Promise.all(mine.orders.map((o) => get(`/api/order/${o.chain}/${o.id}`)));
  const fillRow = (o, f) => {
    const lock = (f.locks || []).find((l) => l.state === "active");
    const waiting = f.state === "active" && lock;
    return `<tr><td>${f.fill}</td><td class="num">${ict(f.qty)} ICT</td><td class="num">${usd(f.usdc)} USDC</td>
      <td>${f.state === "claimed" ? '<span class="ok">done</span>' : f.state === "expired" ? "expired" : lock ? '<span style="color:var(--lamp)">paid, waiting for you</span>' : "reserved, not paid yet"}</td>
      <td class="muted">${f.state === "active" ? left(lock ? lock.expiry : f.expiry) : ""}</td>
      <td>${waiting ? `<button class="btn primary" data-act="confirm" data-chain="${o.chain}" data-id="${o.id}" data-fill="${f.fill}" data-lock="${lock.id}">Confirm</button>` : ""}</td></tr>`;
  };
  app.innerHTML = `<h1>My orders</h1>
    ${details.length ? details.map((o) => `<section class="panel"><div class="bar"><h2>${o.side === "sell" ? "Sell" : "Buy"} ${ict(o.max_qty)} ICT at ${price(o.price)} · <span class="mono muted">${short(o.id)}</span></h2>
      <span class="pills"><span class="muted">${o.open ? `${ict(o.remaining)} ICT left · ends in ${left(o.expiry)}` : "closed"}</span>
      ${o.open ? `<button class="btn" data-act="cancel" data-chain="${o.chain}" data-id="${o.id}">Cancel</button>` : ""}</span></div>
      ${o.fills.length ? `<div class="scroll"><table><thead><tr><th>Fill</th><th class="num">ICT</th><th class="num">USDC</th><th>Status</th><th>Time left</th><th></th></tr></thead><tbody>${o.fills.map((f) => fillRow(o, f)).join("")}</tbody></table></div>` : `<p class="empty">No fills yet.</p>`}</section>`).join("")
      : `<div class="panel empty">No orders from the connected accounts.</div>`}
    <section class="panel pad"><h2>Fills I am taking</h2>
      ${mine.locks.length ? `<table><thead><tr><th>Order</th><th>Fill</th><th class="num">Locked</th><th>Status</th><th>Time left</th></tr></thead><tbody>${mine.locks.map((l) => `<tr><td class="mono">${short(l.order_id)}</td><td>${l.fill}</td>
        <td class="num">${l.chain === "base" ? usd(l.amount) + " USDC" : ict(l.qty) + " ICT"}</td><td>${l.state === "claimed" ? '<span class="ok">done</span>' : l.state === "refunded" ? "refunded" : "waiting for confirmation"}</td>
        <td class="muted">${l.state === "active" ? left(l.expiry) : ""}</td></tr>`).join("")}</tbody></table>` : `<p class="muted" style="margin:0">None.</p>`}
    </section>
    <section class="panel pad"><h2>Secret backups</h2><p class="muted" style="margin:0;font-size:14px">Sell orders keep their secrets in this browser and in the backup file you downloaded. On another computer, load the file here before confirming.</p>
      <label class="btn" style="align-self:flex-start">Load backup file<input type="file" accept="application/json" data-act="load-backup" hidden></label><p data-msg style="margin:0"></p></section>`;
}

async function seedFor(o) {
  if (o.chain === "base") {
    const nonce = store.get("swapnonce:" + o.id);
    if (!nonce) throw new Error("This browser does not know the order's nonce. Confirm from the browser you placed the order in.");
    const signer = await baseSigner();
    return S.seedFromSignature(await signer.signMessage(S.seedMessage("base", nonce)));
  }
  const backup = store.get("swapseed:" + o.id);
  if (!backup) throw new Error("No secret backup for this order in this browser. Load your backup file below.");
  const pw = prompt("Password for this order's secret backup:");
  if (!pw) throw new Error("Cancelled.");
  try { return await S.decryptSeed(backup, pw); } catch { throw new Error("Wrong password."); }
}

async function confirmFill(chain, id, fill, lockId) {
  // Read both chains directly: revealing the secret without a valid lock would give the ICT/USDC away.
  const o = await get(`/api/order/${chain}/${id}`);
  const f = o.fills.find((x) => x.fill === fill), h = o.hashes[fill];
  if (chain === "inc") {
    const api = await inc(), r = await api.query.swap.reservations(id, fill);
    if (r.isNone || !r.unwrap().state.isActive) throw new Error("The reservation is no longer active.");
    const l = await swapRead().locks(lockId), tBase = await baseNow();
    if (Number(l.state) !== 1) throw new Error("No active USDC lock for this fill on Base.");
    if (l.hash.toLowerCase() !== h.toLowerCase()) throw new Error("The lock uses a different hash.");
    if (l.payee.toLowerCase() !== o.recipient.toLowerCase()) throw new Error("The lock does not pay your Base address.");
    if (l.amount < big(f.usdc)) throw new Error("The lock holds less USDC than this fill needs.");
    if (Number(l.expiry) < tBase + 1800) throw new Error("Too close to the lock's deadline to confirm safely.");
  } else {
    const r = await swapRead().reservations(id, fill);
    if (Number(r.state) !== 1) throw new Error("The reservation is no longer active.");
    const api = await inc(), lo = await api.query.swap.locks(lockId), tInc = await incNow();
    if (lo.isNone) throw new Error("No ICT lock for this fill on Incentium.");
    const l = lo.unwrap();
    if (!l.state.isActive) throw new Error("The ICT lock is no longer active.");
    if (l.hash.toHex() !== h.toLowerCase()) throw new Error("The lock uses a different hash.");
    if (l.payee.toString() !== o.recipient) throw new Error("The lock does not pay your Incentium address.");
    if (l.qty.toBigInt() < big(f.qty)) throw new Error("The lock holds less ICT than this fill needs.");
    if (l.expiry.toNumber() < tInc + 1800) throw new Error("Too close to the lock's deadline to confirm safely.");
  }
  const seed = await seedFor(o), secret = await S.secretAt(seed, fill);
  if ((await S.sha256(secret)).length !== 32 || S.toHex(await S.sha256(secret)) !== h.toLowerCase()) throw new Error("These secrets do not belong to this order.");
  return post("/api/confirm", { chain: chain === "inc" ? "base" : "inc", lock: lockId, secret: S.toHex(secret) });
}

async function cancelOrder(chain, id) {
  if (chain === "inc") { const api = await inc(); const me = state.inc || await connectInc(); return sendInc(api.tx.swap.cancelSell(id), me.address); }
  const signer = await baseSigner(); return (await new E.Contract(cfg.contract, ABI, signer).cancelBuy(id)).wait();
}

function howPage() {
  app.innerHTML = `<h1>How it works</h1><section class="panel pad" style="max-width:820px">
  <ol class="steps" style="font-size:15px;gap:10px">
    <li><b>A maker places an order.</b> A sell order holds ICT in the seller's own Incentium account; a buy order locks USDC in the swap contract on Base. Each order commits 16 secret hashes, one per possible fill.</li>
    <li><b>A taker reserves a fill.</b> Part of the order is set aside for the taker for 19 hours.</li>
    <li><b>The taker locks payment</b> on the other chain under the fill's hash. If nothing happens, it comes back after 12 hours.</li>
    <li><b>The maker confirms</b> with one click. That reveals the fill's secret and pays the maker.</li>
    <li><b>The same secret releases the taker's side.</b> The keeper does it at once; anyone can.</li>
  </ol>
  <p class="muted" style="margin:0">No bridge and no custodian. Funds only move to addresses fixed when the order, reservation or lock was made. Orders are limited to 100 ICT until day 90, 1,000 ICT until day 120 and 5,000 ICT after. Fee 0.1% of the USDC side.</p></section>`;
}

// ------------------------------------------------------------------ router and events
async function route() {
  const page = (location.hash.slice(2) || "").split("?")[0];
  document.querySelectorAll("[data-nav]").forEach((a) => a.dataset.nav === (page || "book") ? a.setAttribute("aria-current", "page") : a.removeAttribute("aria-current"));
  try {
    if (!page) await bookPage(); else if (page === "new") newPage(); else if (page === "me") await mePage(); else if (page === "how") howPage();
    else app.innerHTML = `<div class="panel empty">Not found.</div>`;
  } catch (e) { app.innerHTML = `<div class="panel empty">Could not load: ${esc(e.message)}</div>`; }
}

function sayer(form) {
  const m = form.querySelector("[data-msg]");
  return (t, ok) => { m.textContent = t; m.className = ok ? "ok" : ""; };
}

document.addEventListener("submit", async (ev) => {
  const f = ev.target.closest("form[data-form]"); if (!f) return;
  ev.preventDefault();
  const say = sayer(f), btn = f.querySelector("[type=submit]");
  btn.disabled = true;
  try {
    if (f.dataset.form === "sell") await placeSell(f, say);
    else if (f.dataset.form === "buy") await placeBuy(f, say);
    else if (f.dataset.form === "fill") {
      const steps = f.querySelectorAll("[data-steps] li");
      await doFill(f, say, (i) => steps.forEach((li, j) => { li.className = j < i ? "done" : j === i ? "now" : ""; }));
    }
  } catch (e) { say(e.shortMessage || e.message || String(e)); m_err(f); }
  finally { btn.disabled = false; }
});
function m_err(f) { const m = f.querySelector("[data-msg]"); if (m) m.className = "err"; }

document.addEventListener("click", async (ev) => {
  const t = ev.target.closest("[data-act]"); if (!t) return;
  const act = t.dataset.act;
  try {
    if (act === "conn-inc") { await connectInc(); route(); }
    else if (act === "conn-base") { await baseSigner(); route(); }
    else if (act === "fill") await fillDialog(t.dataset.chain, t.dataset.id);
    else if (act === "close") { dlg.close(); route(); }
    else if (act === "cancel") { if (confirm("Cancel this order? Open reservations keep their deadlines.")) { t.disabled = true; await cancelOrder(t.dataset.chain, t.dataset.id); route(); } }
    else if (act === "confirm") {
      t.disabled = true; t.textContent = "Checking…";
      const r = await confirmFill(t.dataset.chain, t.dataset.id, Number(t.dataset.fill), t.dataset.lock);
      t.textContent = "Confirmed"; alert("Confirmed. You have been paid; the taker's side follows shortly. Transaction " + r.tx);
      route();
    }
  } catch (e) { alert(e.shortMessage || e.message || String(e)); if (act === "confirm") { t.disabled = false; t.textContent = "Confirm"; } }
});

document.addEventListener("change", async (ev) => {
  if (ev.target.dataset?.act !== "load-backup") return;
  const msg = ev.target.closest("section").querySelector("[data-msg]");
  try {
    const b = JSON.parse(await ev.target.files[0].text());
    if (b.kind !== "incentium-swap-seed" || !b.order) throw new Error("Not an Incentium Swap backup file.");
    store.set("swapseed:" + b.order, b); msg.textContent = `Loaded the backup for order ${short(b.order)}.`; msg.className = "ok";
  } catch (e) { msg.textContent = e.message; msg.className = "err"; }
});

window.addEventListener("hashchange", route);
(async () => {
  try { cfg = await get("/api/config"); } catch (e) { app.innerHTML = `<div class="panel empty">The swap service is not reachable: ${esc(e.message)}</div>`; return; }
  renderWallets();
  route();
})();
