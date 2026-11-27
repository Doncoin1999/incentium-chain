// Incentium RPC picker for web pages (spec Section 19.2).
//
// Pages ship with a few seed endpoints. This script reads the on-chain endpoint register
// (pallet `endpoints`) through whichever seed answers, measures how fast each active public
// endpoint answers, and keeps the list ordered fastest first. The list is cached in the browser
// for an hour, so the next visit starts with the best endpoints at once.
//
//   const RPCS = IctRpc.list(testnet);   // seeds, or last visit's best list, right away
//   IctRpc.refresh(testnet, RPCS);       // reads the register and reorders RPCS in place
//
// If the chain has no register yet, or nothing in it answers, the seeds are used as before.
(function () {
  "use strict";
  var SEEDS = {
    mainnet: ["wss://rpc.incentium.org", "wss://rpc2.incentium.org"],
    testnet: ["wss://testnet-rpc.incentium.org"],
  };
  // twox128("Endpoints") ++ twox128("Entries")
  var PREFIX = "0x256502b006697fb142d2eb87ba5e1ca1f2c528f439cb4e3ed0c3631044c7e5de";
  var TTL = 3600 * 1000, TIMEOUT = 3000, MAX = 8;
  var KINDS = ["Rpc", "Archive", "Bootnode"];

  function key(testnet) { return "ict-rpc-" + (testnet ? "testnet" : "mainnet"); }
  function cached(testnet) {
    try {
      var c = JSON.parse(localStorage.getItem(key(testnet)) || "null");
      if (c && Date.now() - c.at < TTL && Array.isArray(c.list) && c.list.length) return c.list;
    } catch (e) { /* storage can be off */ }
    return null;
  }
  function save(testnet, list) {
    try { localStorage.setItem(key(testnet), JSON.stringify({ at: Date.now(), list: list })); } catch (e) { /* ignore */ }
  }

  // SCALE decoding of an Entry: owner, kind, url, region, deposit, registered_at, active, failing_days, last_score.
  function bytes(hex) {
    var h = hex.slice(0, 2) === "0x" ? hex.slice(2) : hex, b = new Uint8Array(h.length / 2);
    for (var i = 0; i < b.length; i++) b[i] = parseInt(h.substr(2 * i, 2), 16);
    return b;
  }
  function decodeEntry(hex) {
    var b = bytes(hex), i = 32;
    var compact = function () {
      var m = b[i] & 3;
      if (m === 0) return b[i++] >> 2;
      if (m === 1) { var v = (b[i] | (b[i + 1] << 8)) >> 2; i += 2; return v; }
      var w = (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 2; i += 4; return w;
    };
    var text = function () { var n = compact(), s = ""; for (var k = 0; k < n; k++) s += String.fromCharCode(b[i + k]); i += n; return s; };
    var kind = KINDS[b[i++]] || "?";
    var url = text(), region = text();
    i += 16 + 4; // deposit, registered_at
    var active = b[i++] === 1;
    i++; // failing_days
    var score = b[i];
    return { kind: kind, url: url, region: region, active: active, score: score };
  }

  function rpc(url, method, params) {
    return new Promise(function (ok, fail) {
      var ws, done = false;
      var t = setTimeout(function () { finish(new Error("timeout")); }, TIMEOUT);
      function finish(err, res) {
        if (done) return; done = true; clearTimeout(t);
        try { ws.close(); } catch (e) { /* ignore */ }
        err ? fail(err) : ok(res);
      }
      try { ws = new WebSocket(url); } catch (e) { return finish(e); }
      ws.onopen = function () { ws.send(JSON.stringify({ id: 1, jsonrpc: "2.0", method: method, params: params })); };
      ws.onmessage = function (m) {
        var d = JSON.parse(m.data);
        if (d.id === 1) d.error ? finish(new Error(d.error.message)) : finish(null, d.result);
      };
      ws.onerror = function () { finish(new Error("unreachable")); };
    });
  }

  // Registered, active RPC and archive endpoints, read through the first seed that answers.
  function register(seeds) {
    var at = 0;
    function next() {
      if (at >= seeds.length) return Promise.resolve([]);
      var s = seeds[at++];
      return rpc(s, "state_getKeysPaged", [PREFIX, 512, PREFIX])
        .then(function (keys) {
          if (!keys || !keys.length) return [];
          return rpc(s, "state_queryStorageAt", [keys]).then(function (res) {
            var changes = (res && res[0] && res[0].changes) || [];
            return changes.filter(function (c) { return c[1]; }).map(function (c) { return decodeEntry(c[1]); })
              .filter(function (e) { return e.active && (e.kind === "Rpc" || e.kind === "Archive") && /^wss:\/\//.test(e.url); })
              .map(function (e) { return e.url; });
          });
        })
        .catch(next);
    }
    return next();
  }

  function latency(url) {
    var t0 = Date.now();
    return rpc(url, "system_health", []).then(function () { return Date.now() - t0; }, function () { return Infinity; });
  }

  var IctRpc = {
    seeds: function (testnet) { return SEEDS[testnet ? "testnet" : "mainnet"].slice(); },
    // Right away: last visit's best list if fresh, else the seeds.
    list: function (testnet) { return cached(testnet) || IctRpc.seeds(testnet); },
    // Read the register, measure, and reorder `into` in place (fastest first, seeds kept last
    // as a fallback). Resolves to the new list.
    refresh: function (testnet, into) {
      var seeds = IctRpc.seeds(testnet);
      return register(seeds).then(function (found) {
        var all = [];
        found.concat(seeds).forEach(function (u) { if (all.indexOf(u) < 0) all.push(u); });
        all = all.slice(0, MAX + seeds.length);
        return Promise.all(all.map(latency)).then(function (ms) {
          var ok = all.map(function (u, k) { return { u: u, ms: ms[k] }; }).filter(function (x) { return x.ms !== Infinity; });
          ok.sort(function (a, b) { return a.ms - b.ms; });
          var list = ok.map(function (x) { return x.u; });
          seeds.forEach(function (u) { if (list.indexOf(u) < 0) list.push(u); });
          save(testnet, list);
          if (into) { into.length = 0; Array.prototype.push.apply(into, list); }
          return list;
        });
      }).catch(function () { return into || seeds; });
    },
    _decodeEntry: decodeEntry,
    _prefix: PREFIX,
  };
  if (typeof window !== "undefined") window.IctRpc = IctRpc;
  if (typeof module !== "undefined") module.exports = IctRpc;
})();
