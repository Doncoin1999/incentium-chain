"""Put sites in a Caddyfile behind Incentium sign-in (or take them out again).

  sudo python3 protect-caddy.py /etc/caddy/Caddyfile faucet.incentium.org register.incentium.org \
       --remote https://auth.incentium.org
  sudo python3 protect-caddy.py /etc/caddy/sites/swap.caddy trade-test.incentium.org --no-snippet
  sudo python3 protect-caddy.py /etc/caddy/Caddyfile faucet.incentium.org --remove

Each protected site's body is wrapped in `route { import incentium_auth ... }`, so the sign-in
check runs first, before redir, file_server or reverse_proxy. The `incentium_auth` snippet is added
at the top of the file unless --no-snippet (on server B it lives in sites/00-auth.caddy). A copy of
the file is kept as <file>.bak-auth, and Caddy validates the result before it is written.
"""
from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile

MARK = "import incentium_auth"


def snippet(remote: str | None) -> str:
    if remote:
        host = re.sub(r"^https?://", "", remote).rstrip("/")
        # the auth server's own Caddy replaces X-Forwarded-Host, so send the original host separately
        return (f"(incentium_auth) {{\n\tforward_auth {remote} {{\n\t\turi /verify\n"
                f"\t\theader_up Host {host}\n\t\theader_up X-Incentium-Host {{host}}\n\t}}\n}}\n\n")
    return "(incentium_auth) {\n\tforward_auth 127.0.0.1:8300 {\n\t\turi /verify\n\t\theader_up X-Incentium-Host {host}\n\t}\n}\n\n"


def blocks(text: str):
    """Top-level blocks as (start, end, header) with end just after the closing brace."""
    out, depth, start = [], 0, None
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == "#" and (i == 0 or text[i - 1] in " \t\n"):   # comment to end of line
            i = text.find("\n", i)
            if i < 0:
                break
            continue
        if ch == "{":
            if depth == 0:
                start = text.rfind("\n", 0, i) + 1
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0 and start is not None:
                out.append((start, i + 1, text[start:text.find("{", start)].strip()))
                start = None
        i += 1
    return out


def hosts_of(header: str):
    return [h.strip().removeprefix("https://").removeprefix("http://") for h in header.split(",") if h.strip()]


def transform(text: str, targets: set[str], remove: bool, add_snippet: bool, remote: str | None) -> tuple[str, list[str]]:
    done, pieces, last = [], [], 0
    for s, e, header in blocks(text):
        if header.startswith("(") or not targets.intersection(hosts_of(header)):
            continue
        body = text[text.find("{", s) + 1:e - 1]
        protected = MARK in body
        if remove and protected:
            inner = re.search(r"\n\troute \{\n\t\t" + re.escape(MARK) + r"\n(.*)\n\t\}\n", body, re.S)
            if not inner:
                continue
            new_body = "\n" + "\n".join(l[1:] if l.startswith("\t") else l for l in inner.group(1).split("\n")) + "\n"
        elif not remove and not protected:
            lines = [l for l in body.strip("\n").split("\n")]
            new_body = "\n\troute {\n\t\t" + MARK + "\n" + "\n".join("\t" + l if l.strip() else l for l in lines) + "\n\t}\n"
        else:
            continue
        pieces += [text[last:s], header + " {" + new_body + "}"]
        last = e
        done.append(header)
    pieces.append(text[last:])
    new = "".join(pieces)
    if add_snippet and not remove and "(incentium_auth)" not in new:
        new = snippet(remote) + new
    return new, done


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("hosts", nargs="*", help="site addresses to change (or --all)")
    ap.add_argument("--all", action="store_true", help="every site in the file")
    ap.add_argument("--remote", help="auth service URL when it runs on another server")
    ap.add_argument("--no-snippet", action="store_true")
    ap.add_argument("--remove", action="store_true")
    a = ap.parse_args()
    text = open(a.file).read()
    targets = set(a.hosts)
    if a.all:
        targets = {h for _, _, hd in blocks(text) if not hd.startswith("(") for h in hosts_of(hd)}
    if not targets:
        sys.exit("name the sites to change, or use --all")
    new, done = transform(text, targets, a.remove, not a.no_snippet, a.remote)
    if new == text:
        print("nothing to change")
        return
    shutil.copy(a.file, a.file + ".bak-auth")
    open(a.file, "w").write(new)
    r = subprocess.run(["caddy", "validate", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        shutil.copy(a.file + ".bak-auth", a.file)
        sys.exit("Caddy rejected the change; the old file is back.\n" + r.stderr[-2000:])
    print(("unprotected: " if a.remove else "protected: ") + ", ".join(done))
    print("now run: sudo systemctl reload caddy")


if __name__ == "__main__":
    main()
