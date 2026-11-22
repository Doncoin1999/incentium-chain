"""Tests for the access-control service: python -m pytest auth/test_auth.py"""
import http.client
import os
import re
import sys
import tempfile
import threading
from http.server import ThreadingHTTPServer
from urllib.parse import urlencode

sys.path.insert(0, os.path.dirname(__file__))
import auth_service as A  # noqa: E402


def start():
    store = A.Store(os.path.join(tempfile.mkdtemp(), "auth.db"))
    srv = ThreadingHTTPServer(("127.0.0.1", 0), A.make_handler(store, base="https://auth.incentium.org"))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return store, srv.server_address[1]


def req(port, method, path, body=None, cookie=None, headers=None):
    c = http.client.HTTPConnection("127.0.0.1", port)
    h = dict(headers or {})
    if body is not None:
        body = urlencode(body)
        h["Content-Type"] = "application/x-www-form-urlencoded"
    if cookie:
        h["Cookie"] = f"{A.COOKIE}={cookie}"
    c.request(method, path, body=body, headers=h)
    r = c.getresponse()
    return r.status, dict(r.getheaders()), r.read().decode()


def session_from(headers):
    m = re.search(A.COOKIE + r"=([^;]+)", headers.get("Set-Cookie", ""))
    return m.group(1) if m else None


def test_password_hash():
    h = A.hash_pw("correct horse battery")
    assert A.check_pw("correct horse battery", h) and not A.check_pw("wrong", h)


def test_safe_next():
    assert A.safe_next("https://trade-test.incentium.org/#/me") == "https://trade-test.incentium.org/#/me"
    assert A.safe_next("https://evil.example/x") == "https://incentium.org/"
    assert A.safe_next("https://incentium.org.evil.example/") == "https://incentium.org/"
    assert A.safe_next("http://faucet.incentium.org/") == "https://incentium.org/"


def test_flow_register_approve_verify_revoke():
    store, port = start()
    # not signed in: verify redirects to the login page with the original URL
    s, h, _ = req(port, "GET", "/verify", headers={"X-Forwarded-Host": "faucet.incentium.org", "X-Forwarded-Uri": "/x"})
    assert s == 303 and h["Location"].startswith("https://auth.incentium.org/login?next=https%3A%2F%2Ffaucet.incentium.org%2Fx")
    # request access: pending, cannot sign in yet
    s, _, b = req(port, "POST", "/register", {"email": "Ann@Example.org", "pw": "a-long-password", "note": "hi"})
    assert s == 200 and "waits for approval" in b
    s, _, b = req(port, "POST", "/login", {"email": "ann@example.org", "pw": "a-long-password"})
    assert "waiting for approval" in b
    # admin approves through the page (with CSRF)
    store.register("boss@example.org", "admin-password-1", "admin", status="approved", admin=1)
    s, h, _ = req(port, "POST", "/login", {"email": "boss@example.org", "pw": "admin-password-1", "next": "https://auth.incentium.org/admin"})
    admin = session_from(h)
    assert s == 303 and admin
    s, _, b = req(port, "GET", "/admin", cookie=admin)
    csrf = re.search(r"name='csrf' value='([^']+)'", b).group(1)
    s, _, _ = req(port, "POST", "/admin", {"csrf": "bad", "email": "ann@example.org", "action": "approve"}, cookie=admin)
    assert s == 403
    s, _, b = req(port, "POST", "/admin", {"csrf": csrf, "email": "ann@example.org", "action": "approve"}, cookie=admin)
    assert "approve done" in b
    # Ann signs in and passes verify; the cookie is for every incentium.org site
    s, h, _ = req(port, "POST", "/login", {"email": "ann@example.org", "pw": "a-long-password", "next": "https://faucet.incentium.org/"})
    assert s == 303 and h["Location"] == "https://faucet.incentium.org/"
    assert "Domain=.incentium.org" in h["Set-Cookie"] and "Secure" in h["Set-Cookie"] and "HttpOnly" in h["Set-Cookie"]
    ann = session_from(h)
    assert req(port, "GET", "/verify", cookie=ann)[0] == 200
    # Ann is not an admin: no admin page, and verify-admin (mining control) refuses her
    assert req(port, "GET", "/admin", cookie=ann)[0] == 303
    assert req(port, "GET", "/verify-admin", cookie=ann)[0] == 403
    s, h, _ = req(port, "GET", "/verify-admin", cookie=admin)
    assert s == 200 and h["X-Incentium-User"] == "boss@example.org"
    s, h, _ = req(port, "GET", "/verify-admin", headers={"X-Incentium-Host": "control.incentium.org", "X-Forwarded-Uri": "/"})
    assert s == 303 and "next=https%3A%2F%2Fcontrol.incentium.org%2F" in h["Location"]
    # revoking ends her sessions at once
    req(port, "POST", "/admin", {"csrf": csrf, "email": "ann@example.org", "action": "revoke"}, cookie=admin)
    assert req(port, "GET", "/verify", cookie=ann)[0] == 303


def test_wrong_password_and_duplicate_register():
    store, port = start()
    req(port, "POST", "/register", {"email": "a@b.org", "pw": "password-123"})
    _, _, b = req(port, "POST", "/login", {"email": "a@b.org", "pw": "nope-nope-nope"})
    assert "Wrong email or password" in b
    # a second request for the same email gives the same answer and keeps the first password
    _, _, b = req(port, "POST", "/register", {"email": "a@b.org", "pw": "other-password"})
    assert "waits for approval" in b
    assert A.check_pw("password-123", store.user("a@b.org")["pw"])


def test_login_rate_limit():
    _, port = start()
    for _ in range(10):
        req(port, "POST", "/login", {"email": "x@y.org", "pw": "whatever-123"})
    _, _, b = req(port, "POST", "/login", {"email": "x@y.org", "pw": "whatever-123"})
    assert "Too many attempts" in b
