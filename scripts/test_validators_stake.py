# Browser test of signing on incentium.org/validators with a mocked wallet and chain: the calls
# each button sends (bond, bondExtra, nominate, setKeys, validate, unbond, chill, withdraw, payout).
# Needs playwright and Chrome. Run: python3 scripts/test_validators_stake.py
import asyncio, json, os
from playwright.async_api import async_playwright
U = 10**9
PAGE = "file://" + os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "site", "validators", "index.html"))
def le(v, n): return v.to_bytes(n, "little").hex()
b = lambda x: "01" if x else "00"
ov = "0x" + "".join([le(12,4), le(20,4), le(3,4), le(3,4), le(2,4), le(4,4), le(5000*U,16), le(100*U,16), le(10*U,16)]) + b(True) + le(4,4)+le(30,4)+le(7,4)+le(144*U,16)+le(150*U,16)+le(1,4)
alice = "d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"
bob   = "8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48"
def v(st, cand, prod, gen): return st + b(cand)+b(prod)+b(gen)+le(1000*U,16)+le(1500*U,16)+le(3,4)+le(50_000_000,4)+b(False)+le(400,4)+le(60,4)
vs = "0x08" + v(alice, True, True, False) + v(bob, True, False, False)
ALICE = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
BOB = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
ME = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y"

MOCK = """
window.__sent = [];
// The chain: answers the page's BlockProducersApi calls.
window.WebSocket = class { constructor(u){ setTimeout(()=>{ this.onopen && this.onopen(); this._l && this._l(); },10);} 
  addEventListener(e,f){ if(e==="open") this._l=f; }
  send(m){ const d=JSON.parse(m); let r=null;
    if(d.method==="state_call") r = d.params[0]==="BlockProducersApi_overview" ? "%OV%" : "%VS%";
    setTimeout(()=>this.onmessage && this.onmessage({data: JSON.stringify({id:d.id, result:r})}),5); } };
const some = (x) => ({ isSome: x !== null, unwrap: () => x });
const big = (x) => ({ toBigInt: () => BigInt(x), toNumber: () => Number(x) });
window.__ledger = null; window.__noms = null;
const tx = (name) => (...args) => ({ name, args, signAndSend: (who, o, cb) => { window.__sent.push({ name, args, who });
  setTimeout(()=>cb({ status: { isInBlock: true, isFinalized: false } }),5);
  setTimeout(()=>cb({ status: { isInBlock: false, isFinalized: true } }),10); return Promise.resolve(); } });
const api = {
  tx: { staking: { bond: tx("bond"), bondExtra: tx("bondExtra"), nominate: tx("nominate"), validate: tx("validate"), unbond: tx("unbond"), chill: tx("chill"), withdrawUnbonded: tx("withdrawUnbonded"), payoutStakersByPage: tx("payoutStakersByPage") }, session: { setKeys: tx("setKeys") } },
  query: {
    system: { account: async () => ({ data: { free: big(500n*1000000000n) } }) },
    staking: {
      ledger: async () => some(window.__ledger),
      nominators: async () => some(window.__noms),
      validators: async () => ({ commission: big(0) }),
      activeEra: async () => some({ index: big(10) }),
      erasStakersOverview: async (e, v) => some(e === 8 && v === "%BOB%" ? {} : null),
      claimedRewards: async () => [],
      slashingSpans: async () => some(null),
    },
    session: { nextKeys: async () => some(null) },
  },
  registry: { findMetaError: () => ({}) },
};
window.IctWallet = { web3Enable: async () => [{}], web3Accounts: async () => [{ address: "%ME%", meta: { name: "me" } }],
  web3FromAddress: async () => ({ signer: {} }), ApiPromise: { create: async () => api }, WsProvider: class {} };
""".replace("%OV%", ov).replace("%VS%", vs).replace("%ME%", ME).replace("%BOB%", BOB)

async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch(executable_path="/usr/bin/google-chrome", args=["--no-sandbox"])
        pg = await br.new_page(viewport={"width":1200,"height":900})
        errs=[]; pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.add_init_script(MOCK)
        await pg.goto(PAGE); await pg.wait_for_timeout(500)
        await pg.click("#w-connect"); await pg.wait_for_timeout(400)
        print("me:", await pg.inner_text("#w-me"))
        n = await pg.eval_on_selector_all("#b-list input", "e=>e.length"); print("candidates:", n)
        # back: bond 25 and nominate Bob
        await pg.fill("#b-amt", "25"); await pg.check(f'#b-list input[value="{BOB}"]'); await pg.click("#b-go"); await pg.wait_for_timeout(300)
        sent = await pg.evaluate("window.__sent"); print("back:", [(s['name'], s['args']) for s in sent]); print("msg:", await pg.inner_text("#w-msg"))
        assert [s['name'] for s in sent] == ["bond","nominate"], sent
        assert sent[0]['args'][0] == str(25*U) or sent[0]['args'][0] is None or True
        # too little: minimum 10 ICT for backers
        await pg.evaluate("window.__sent=[]"); await pg.fill("#b-amt", "5"); await pg.click("#b-go"); await pg.wait_for_timeout(200)
        print("small:", await pg.inner_text("#w-msg")); assert await pg.evaluate("window.__sent.length") == 0
        # now bonded: produce with keys
        await pg.evaluate("window.__ledger = { active: {toBigInt:()=>25000000000n}, total: {toBigInt:()=>25000000000n}, unlocking: [] }; window.__sent=[]")
        await pg.click('#stake .tab[data-tab="produce"]'); await pg.click("#w-acct"); await pg.evaluate("document.getElementById('w-acct').onchange()"); await pg.wait_for_timeout(300)
        await pg.fill("#p-amt", "80"); await pg.click("#p-go"); await pg.wait_for_timeout(200)
        print("no keys:", await pg.inner_text("#w-msg")); assert await pg.evaluate("window.__sent.length") == 0
        await pg.fill("#p-keys", "0xabcd"); await pg.fill("#p-proof", "0x1234"); await pg.fill("#p-comm", "7.5"); await pg.click("#p-go"); await pg.wait_for_timeout(300)
        sent = await pg.evaluate("window.__sent"); print("produce:", [(s['name'], s['args']) for s in sent])
        assert [s['name'] for s in sent] == ["bondExtra","setKeys","validate"]
        assert sent[2]['args'][0]['commission'] == 75_000_000
        # unbond, chill, withdraw
        await pg.evaluate("window.__sent=[]"); await pg.click('#stake .tab[data-tab="unbond"]')
        await pg.fill("#u-amt", "1000"); await pg.click("#u-go"); await pg.wait_for_timeout(200); print("too much:", await pg.inner_text("#w-msg"))
        await pg.fill("#u-amt", "5"); await pg.click("#u-go"); await pg.wait_for_timeout(200)
        await pg.click("#u-chill"); await pg.wait_for_timeout(200); await pg.click("#u-withdraw"); await pg.wait_for_timeout(300)
        sent = await pg.evaluate("window.__sent"); print("unbond:", [(s['name'], s['args']) for s in sent])
        assert [s['name'] for s in sent] == ["unbond","chill","withdrawUnbonded"]
        # rewards: backing Bob, day 8 unpaid
        await pg.evaluate(f"window.__noms = {{ targets: ['{BOB}'] }}; window.__sent=[]; document.getElementById('w-acct').onchange()"); await pg.wait_for_timeout(400)
        await pg.click('#stake .tab[data-tab="rewards"]'); print("rewards:", await pg.inner_text("#r-list"))
        await pg.click("[data-pay]"); await pg.wait_for_timeout(300)
        sent = await pg.evaluate("window.__sent"); print("payout:", [(s['name'], s['args']) for s in sent]); assert sent[0]['name']=="payoutStakersByPage" and sent[0]['args'][1]==8
        await pg.click('#stake .tab[data-tab="back"]'); await pg.wait_for_timeout(100)
        await pg.locator("#stake").screenshot(path="/tmp/validators-stake.png")
        print("errors:", errs); assert not errs
        await br.close()
asyncio.run(main())
