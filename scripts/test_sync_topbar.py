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
                "https://faucet.incentium.org/", "https://auth.incentium.org/admin"):
        assert url in bar
    for rel in S.PAGES:
        assert 'class="ic-brand" href="https://incentium.org/"' in open(os.path.join(S.ROOT, rel)).read(), rel


def test_logo_files_exist():
    for f in ("incentium-mark.svg", "incentium-logo-white-on-black.svg"):
        assert os.path.exists(os.path.join(S.ROOT, "site", "brand", f))
