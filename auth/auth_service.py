"""Incentium access control: sign-in with manual approval for the incentium.org web pages.

People request access with email and password; an admin approves each one. Caddy asks this
service before serving a protected site (`forward_auth` to /verify). One session cookie on
.incentium.org covers every subdomain, on every server. Standard library only.

  python auth_service.py serve --db ~/incentium-auth/auth.db --port 8300
  python auth_service.py add-admin you@example.org      # asks for a password
  python auth_service.py list | approve EMAIL | revoke EMAIL
"""
from __future__ import annotations

import argparse
import getpass
import hashlib
import hmac
import html
import os
import re
import secrets
import sqlite3
import sys
import threading
import time
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, quote, urlsplit

COOKIE = "inc_session"
COOKIE_DOMAIN = ".incentium.org"
SESSION_DAYS = 30
BASE = "https://auth.incentium.org"
EMAIL_RE = re.compile(r"^[^@\s]{1,64}@[^@\s]{1,190}\.[^@\s]{2,}$")

SCHEMA = """
CREATE TABLE IF NOT EXISTS users (
  email TEXT PRIMARY KEY, pw TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'pending',          -- pending | approved | rejected
  admin INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, decided INTEGER);
CREATE TABLE IF NOT EXISTS sessions (
  token TEXT PRIMARY KEY, email TEXT NOT NULL, csrf TEXT NOT NULL, expires INTEGER NOT NULL);
"""


# ------------------------------------------------------------------ passwords and storage
def hash_pw(pw: str) -> str:
    salt = secrets.token_bytes(16)
    h = hashlib.scrypt(pw.encode(), salt=salt, n=2**14, r=8, p=1, dklen=32)
    return "scrypt$" + salt.hex() + "$" + h.hex()


def check_pw(pw: str, stored: str) -> bool:
    try:
        _, salt, h = stored.split("$")
        got = hashlib.scrypt(pw.encode(), salt=bytes.fromhex(salt), n=2**14, r=8, p=1, dklen=32)
        return hmac.compare_digest(got.hex(), h)
    except ValueError:
        return False


class Store:
    def __init__(self, path: str):
        self.con = sqlite3.connect(path, check_same_thread=False)
        self.con.row_factory = sqlite3.Row
        self.con.executescript(SCHEMA)
        self.lock = threading.Lock()

    def q(self, sql, args=()):
        with self.lock:
            cur = self.con.execute(sql, args)
            self.con.commit()
            return cur.fetchall()

    def user(self, email):
        r = self.q("SELECT * FROM users WHERE email = ?", (email,))
        return r[0] if r else None

    def register(self, email, pw, note, status="pending", admin=0):
        self.q("INSERT INTO users (email, pw, note, status, admin, created) VALUES (?,?,?,?,?,?)",
               (email, hash_pw(pw), note[:500], status, admin, int(time.time())))

    def decide(self, email, status):
        self.q("UPDATE users SET status = ?, decided = ? WHERE email = ?", (status, int(time.time()), email))
        if status != "approved":
            self.q("DELETE FROM sessions WHERE email = ?", (email,))

    def delete(self, email):
        self.q("DELETE FROM sessions WHERE email = ?", (email,))
        self.q("DELETE FROM users WHERE email = ?", (email,))

    def new_session(self, email):
        tok, csrf = secrets.token_urlsafe(32), secrets.token_urlsafe(16)
        self.q("INSERT INTO sessions VALUES (?,?,?,?)", (tok, email, csrf, int(time.time()) + SESSION_DAYS * 86400))
        return tok

    def session(self, tok):
        """(user row, csrf) for a live session of an approved user, else (None, None)."""
        if not tok:
            return None, None
        r = self.q("SELECT s.csrf, u.* FROM sessions s JOIN users u ON u.email = s.email "
                   "WHERE s.token = ? AND s.expires > ? AND u.status = 'approved'", (tok, int(time.time())))
        return (r[0], r[0]["csrf"]) if r else (None, None)

    def end_session(self, tok):
        self.q("DELETE FROM sessions WHERE token = ?", (tok,))


# ------------------------------------------------------------------ helpers
def safe_next(url: str | None) -> str:
    """Only send people back to https pages on incentium.org."""
    if not url:
        return "https://incentium.org/"
    p = urlsplit(url)
    host = (p.hostname or "").lower()
    if p.scheme == "https" and (host == "incentium.org" or host.endswith(".incentium.org")):
        return url
    return "https://incentium.org/"


class Limiter:
    """At most `n` attempts per key per `window` seconds."""

    def __init__(self, n=10, window=900):
        self.n, self.window, self.hits, self.lock = n, window, {}, threading.Lock()

    def ok(self, key) -> bool:
        now = time.time()
        with self.lock:
            h = [t for t in self.hits.get(key, []) if now - t < self.window]
            h.append(now)
            self.hits[key] = h
            return len(h) <= self.n


STYLE = """body{font:16px/1.55 -apple-system,Segoe UI,Helvetica,Arial,sans-serif;background:#1c2230;color:#e9ecf2;
margin:0;display:flex;flex-direction:column;min-height:100vh}main{width:min(420px,92vw);padding:28px 0;margin:auto}
h1{font-size:26px;margin:0 0 6px}p{color:#a9adb8;margin:0 0 16px}label{display:block;margin:12px 0 4px;font-size:14px}
input,textarea{width:100%;box-sizing:border-box;padding:10px;border-radius:8px;border:1px solid #3a3f4b;background:#1d2028;color:#e8e6e1;font:inherit}
button{margin-top:16px;padding:10px 16px;border:0;border-radius:8px;background:#e0a84e;color:#1a1a1a;font-weight:600;font:inherit;cursor:pointer}
a{color:#e0a84e}.msg{margin-top:14px;color:#e8a29a}.ok{color:#9fd6a4}.small{font-size:13px;color:#8b909b;margin-top:22px}
table{width:100%;border-collapse:collapse;font-size:14px}td,th{padding:8px 6px;border-bottom:1px solid #2b2f39;text-align:left;vertical-align:top}
.wide{width:min(980px,94vw)}form.inline{display:inline}form.inline button{margin:0 4px 0 0;padding:6px 10px;font-size:13px}
button.ghost{background:#2b2f39;color:#e8e6e1}"""


def site_partial(name: str) -> str:
    """The shared Incentium top bar or footer (site/partials), or "" if it is missing."""
    try:
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "site", "partials", name + ".html"),
                  encoding="utf-8") as f:
            return f.read()
    except OSError:
        return ""


TOPBAR, FOOTER = site_partial("topbar"), site_partial("footer")


def page(title, body, wide=False):
    return (f"<!doctype html><html lang='en'><head><meta charset='utf-8'><meta name='viewport' "
            f"content='width=device-width,initial-scale=1'><title>{html.escape(title)} · Incentium</title>"
            f"<link rel='icon' type='image/svg+xml' href='https://incentium.org/brand/incentium-logo-white-on-black.svg'>"
            f"<style>{STYLE}</style></head><body>{TOPBAR}<main class='{'wide' if wide else ''}'>{body}</main>{FOOTER}</body></html>")


def e(s) -> str:
    return html.escape(str(s or ""), quote=True)


# ------------------------------------------------------------------ HTTP
def make_handler(store: Store, base: str = BASE, cookie_domain: str = COOKIE_DOMAIN):
    limiter = Limiter()

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        # -- plumbing
        def ip(self):
            xff = self.headers.get("X-Forwarded-For")
            return xff.split(",")[0].strip() if xff else self.client_address[0]

        def token(self):
            c = SimpleCookie(self.headers.get("Cookie", ""))
            return c[COOKIE].value if COOKIE in c else None

        def send(self, code, body="", ctype="text/html; charset=utf-8", headers=()):
            b = body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(b)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Frame-Options", "DENY")
            self.send_header("Referrer-Policy", "no-referrer")
            for k, v in headers:
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(b)

        def redirect(self, url, headers=()):
            self.send(303, "", headers=[("Location", url), *headers])

        def form(self):
            n = int(self.headers.get("Content-Length", "0") or 0)
            data = parse_qs(self.rfile.read(min(n, 20000)).decode(errors="replace"))
            return {k: v[0] for k, v in data.items()}

        def query(self):
            return {k: v[0] for k, v in parse_qs(urlsplit(self.path).query).items()}

        # -- routes
        def do_GET(self):
            path = urlsplit(self.path).path
            if path == "/verify":
                return self.verify()
            if path in ("/", "/login"):
                return self.login_page(self.query().get("next"))
            if path == "/register":
                return self.register_page()
            if path == "/admin":
                return self.admin_page()
            if path == "/logout":
                tok = self.token()
                if tok:
                    store.end_session(tok)
                return self.redirect(base + "/login", headers=[("Set-Cookie", self.cookie("", 0))])
            self.send(404, page("Not found", "<h1>Not found</h1>"))

        def do_POST(self):
            path = urlsplit(self.path).path
            f = self.form()
            if path == "/login":
                return self.login(f)
            if path == "/register":
                return self.register(f)
            if path == "/admin":
                return self.admin_action(f)
            self.send(404, page("Not found", "<h1>Not found</h1>"))

        def cookie(self, value, max_age):
            dom = f"; Domain={cookie_domain}" if cookie_domain else ""
            return f"{COOKIE}={value}{dom}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Lax"

        def verify(self):
            """Caddy forward_auth: 200 lets the request through; anything else is sent to the browser."""
            user, _ = store.session(self.token())
            if user:
                return self.send(200, "", ctype="text/plain", headers=[("X-Incentium-User", user["email"])])
            host = self.headers.get("X-Incentium-Host") or self.headers.get("X-Forwarded-Host", "incentium.org")
            uri = self.headers.get("X-Forwarded-Uri", "/")
            nxt = safe_next(f"https://{host}{uri}")
            self.redirect(f"{base}/login?next={quote(nxt, safe='')}")

        def login_page(self, nxt, msg="", ok=False):
            user, _ = store.session(self.token())
            if user and not msg:
                return self.redirect(safe_next(nxt))
            self.send(200, page("Sign in", f"""<h1>Incentium</h1>
<p>The Incentium web pages are open to invited people for now. Sign in, or request access.</p>
<form method="post" action="/login"><input type="hidden" name="next" value="{e(safe_next(nxt))}">
<label>Email<input name="email" type="email" autocomplete="username" required></label>
<label>Password<input name="pw" type="password" autocomplete="current-password" required></label>
<button>Sign in</button></form>
{f'<p class="msg {"ok" if ok else ""}">{e(msg)}</p>' if msg else ''}
<p class="small">No account yet? <a href="/register">Request access</a>. · <a href="https://incentium.org/">incentium.org</a></p>"""))

        def login(self, f):
            if not limiter.ok("login:" + self.ip()):
                return self.login_page(f.get("next"), "Too many attempts. Try again in 15 minutes.")
            email = f.get("email", "").strip().lower()
            u = store.user(email)
            if not u or not check_pw(f.get("pw", ""), u["pw"]):
                return self.login_page(f.get("next"), "Wrong email or password.")
            if u["status"] == "pending":
                return self.login_page(f.get("next"), "Your request is waiting for approval. You can sign in once it is approved.")
            if u["status"] != "approved":
                return self.login_page(f.get("next"), "This account does not have access.")
            tok = store.new_session(email)
            self.redirect(safe_next(f.get("next")), headers=[("Set-Cookie", self.cookie(tok, SESSION_DAYS * 86400))])

        def register_page(self, msg="", ok=False, keep=None):
            k = keep or {}
            form = "" if ok else f"""<form method="post" action="/register">
<label>Email<input name="email" type="email" autocomplete="username" value="{e(k.get('email'))}" required></label>
<label>Password (at least 10 characters)<input name="pw" type="password" autocomplete="new-password" minlength="10" required></label>
<label>Who are you, and why are you interested? (optional)<textarea name="note" rows="3" maxlength="500">{e(k.get('note'))}</textarea></label>
<button>Request access</button></form>"""
            self.send(200, page("Request access", f"""<h1>Request access</h1>
<p>Each request is approved by hand. You can sign in once it is approved.</p>{form}
{f'<p class="msg {"ok" if ok else ""}">{e(msg)}</p>' if msg else ''}
<p class="small">Your email is used only to sign you in. Ask us at any time to delete your account.
<br><a href="/login">Sign in</a> · <a href="https://incentium.org/">incentium.org</a></p>"""))

        def register(self, f):
            if not limiter.ok("register:" + self.ip()):
                return self.register_page("Too many requests from your connection. Try again later.", keep=f)
            email, pw = f.get("email", "").strip().lower(), f.get("pw", "")
            if not EMAIL_RE.match(email):
                return self.register_page("Enter a valid email address.", keep=f)
            if len(pw) < 10:
                return self.register_page("The password must be at least 10 characters.", keep=f)
            if store.user(email):
                # same answer as for a new request, so the form does not reveal who has an account
                return self.register_page("Thank you. Your request is registered and waits for approval.", ok=True)
            store.register(email, pw, f.get("note", "").strip())
            print(f"access request: {email}", flush=True)
            self.register_page("Thank you. Your request is registered and waits for approval.", ok=True)

        def admin_user(self):
            user, csrf = store.session(self.token())
            return (user, csrf) if user and user["admin"] else (None, None)

        def admin_page(self, msg=""):
            user, csrf = self.admin_user()
            if not user:
                return self.redirect(f"{base}/login?next={quote(base + '/admin', safe='')}")
            rows = store.q("SELECT * FROM users ORDER BY CASE status WHEN 'pending' THEN 0 WHEN 'approved' THEN 1 ELSE 2 END, created DESC")

            def btn(email, action, label, ghost=False):
                return (f"<form class='inline' method='post' action='/admin'><input type='hidden' name='csrf' value='{e(csrf)}'>"
                        f"<input type='hidden' name='email' value='{e(email)}'><input type='hidden' name='action' value='{action}'>"
                        f"<button class='{'ghost' if ghost else ''}'>{label}</button></form>")

            def actions(r):
                if r["admin"]:
                    return "<span class='small'>admin</span>"
                if r["status"] == "pending":
                    return btn(r["email"], "approve", "Approve") + btn(r["email"], "reject", "Reject", True)
                if r["status"] == "approved":
                    return btn(r["email"], "revoke", "Revoke", True)
                return btn(r["email"], "approve", "Approve") + btn(r["email"], "delete", "Delete", True)

            body = "".join(
                f"<tr><td>{e(r['email'])}</td><td>{e(r['status'])}</td><td>{e(r['note'])}</td>"
                f"<td>{time.strftime('%Y-%m-%d', time.gmtime(r['created']))}</td><td>{actions(r)}</td></tr>" for r in rows)
            pending = sum(1 for r in rows if r["status"] == "pending")
            self.send(200, page("Access requests", f"""<h1>Access requests</h1>
<p>{pending} waiting · signed in as {e(user['email'])} · <a href="/logout">Sign out</a></p>
{f'<p class="msg ok">{e(msg)}</p>' if msg else ''}
<table><tr><th>Email</th><th>Status</th><th>Note</th><th>Requested</th><th></th></tr>{body or '<tr><td colspan=5>No requests.</td></tr>'}</table>""", wide=True))

        def admin_action(self, f):
            user, csrf = self.admin_user()
            if not user or not hmac.compare_digest(f.get("csrf", ""), csrf or ""):
                return self.send(403, page("Forbidden", "<h1>Forbidden</h1>"))
            email, action = f.get("email", ""), f.get("action")
            target = store.user(email)
            if not target or target["admin"]:
                return self.admin_page("Nothing to change.")
            if action == "approve":
                store.decide(email, "approved")
            elif action == "reject":
                store.decide(email, "rejected")
            elif action == "revoke":
                store.decide(email, "rejected")
            elif action == "delete":
                store.delete(email)
            self.admin_page(f"{email}: {action} done.")

    return H


# ------------------------------------------------------------------ CLI
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["serve", "add-admin", "list", "approve", "revoke", "delete"])
    ap.add_argument("email", nargs="?")
    ap.add_argument("--db", default=os.path.expanduser("~/incentium-auth/auth.db"))
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8300)
    a = ap.parse_args()
    os.makedirs(os.path.dirname(a.db), exist_ok=True)
    store = Store(a.db)
    if a.cmd == "serve":
        print(f"incentium auth on {a.host}:{a.port}", flush=True)
        ThreadingHTTPServer((a.host, a.port), make_handler(store)).serve_forever()
    elif a.cmd == "add-admin":
        email = (a.email or "").strip().lower()
        if not EMAIL_RE.match(email):
            sys.exit("usage: add-admin EMAIL")
        pw = getpass.getpass("Password (at least 10 characters): ")
        if len(pw) < 10 or pw != getpass.getpass("Repeat: "):
            sys.exit("Passwords too short or different.")
        store.delete(email)
        store.register(email, pw, "admin", status="approved", admin=1)
        print(f"{email} is an admin.")
    elif a.cmd == "list":
        for r in store.q("SELECT email, status, admin, note FROM users ORDER BY created"):
            print(f"{r['status']:9} {'admin ' if r['admin'] else '      '}{r['email']}  {r['note']}")
    else:
        if not a.email or not store.user(a.email.lower()):
            sys.exit("unknown email")
        if a.cmd == "delete":
            store.delete(a.email.lower())
        else:
            store.decide(a.email.lower(), "approved" if a.cmd == "approve" else "rejected")
        print("done")


if __name__ == "__main__":
    main()
