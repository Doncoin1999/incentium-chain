"""Every Incentium page carries the shared top bar (logo linking to incentium.org) and is in sync."""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import sync_topbar as S  # noqa: E402


def test_pages_are_in_sync_with_the_shared_parts():
    r = subprocess.run([sys.executable, os.path.join(HERE, "sync_topbar.py"), "--check"], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


def test_top_bar_links_home_and_to_every_site():
    bar = S.partial("topbar")
    assert 'class="ic-brand" href="https://incentium.org/"' in bar
    assert "incentium-mark.svg" in bar
    for url in ("https://incentium.org/mine/", "https://chat.incentium.org/", "https://explorer.incentium.org/",
                "https://faucet.incentium.org/", "https://auth.incentium.org/admin", "https://incentium.org/spec/",
                "https://incentium.org/whitepaper/", "https://incentium.org/events/"):
        assert url in bar
    for rel in S.PAGES:
        assert 'class="ic-brand" href="https://incentium.org/"' in open(os.path.join(S.ROOT, rel)).read(), rel


def test_logo_files_exist():
    for f in ("incentium-mark.svg", "incentium-logo-white-on-black.svg"):
        assert os.path.exists(os.path.join(S.ROOT, "site", "brand", f))


def test_document_pages_are_built_from_their_markdown():
    import pytest

    pytest.importorskip("markdown")
    r = subprocess.run([sys.executable, os.path.join(HERE, "build_docs.py"), "--check"], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    page = open(os.path.join(S.ROOT, "site", "spec", "index.html")).read()
    assert "Incentium Technical Specification v0.1" in page and 'body data-site="spec"' in page
    assert os.path.exists(os.path.join(S.ROOT, "site", "spec", "incentium-technical-specification-v0.1.pdf"))
    wp = open(os.path.join(S.ROOT, "site", "whitepaper", "index.html")).read()
    assert "<title>Incentium Whitepaper" in wp and 'src="architecture.png"' in wp and 'src="roadmap.png"' in wp
    for f in ("incentium-whitepaper-v0.1.pdf", "architecture.png", "roadmap.png"):
        assert os.path.exists(os.path.join(S.ROOT, "site", "whitepaper", f))
