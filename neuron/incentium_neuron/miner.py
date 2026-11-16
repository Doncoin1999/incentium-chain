"""SN0 reference miner: a small HTTP server in front of an inference backend.

POST /sn0/complete {"prompt": str, "max_tokens": int, "seed": int}
  -> {"text", "tokens", "top_logprobs", "hotkey"}

Run: python -m incentium_neuron.miner --backend http://127.0.0.1:8080 --model qwen3.5-9b-q4 --port 8091
Then publish the endpoint on chain with Registry.set_endpoint(0, "http://<public-ip>:8091").
"""
from __future__ import annotations

import argparse
import json
import threading
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_TOKENS = 512


def make_handler(backend, hotkey: str):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):  # quiet
            pass

        def do_POST(self):
            if self.path != "/sn0/complete":
                self.send_error(404)
                return
            try:
                n = int(self.headers.get("Content-Length", "0"))
                req = json.loads(self.rfile.read(n))
                max_tokens = min(int(req.get("max_tokens", 64)), MAX_TOKENS)
                c = backend.complete(str(req["prompt"]), max_tokens, int(req.get("seed", 0)))
                body = json.dumps({**asdict(c), "hotkey": hotkey}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except Exception as e:  # malformed request or backend failure
                self.send_error(400, str(e))

    return Handler


def serve(backend, hotkey: str, host: str = "0.0.0.0", port: int = 8091, background: bool = False):
    srv = ThreadingHTTPServer((host, port), make_handler(backend, hotkey))
    if background:
        t = threading.Thread(target=srv.serve_forever, daemon=True)
        t.start()
        return srv
    srv.serve_forever()


def main():
    from .backend import OpenAICompatibleBackend

    ap = argparse.ArgumentParser()
    ap.add_argument("--backend", required=True, help="OpenAI-compatible base URL")
    ap.add_argument("--model", required=True)
    ap.add_argument("--hotkey", required=True, help="SS58 address of this miner's hotkey")
    ap.add_argument("--port", type=int, default=8091)
    a = ap.parse_args()
    serve(OpenAICompatibleBackend(a.backend, a.model), a.hotkey, port=a.port)


if __name__ == "__main__":
    main()
