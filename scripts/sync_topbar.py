"""Copy the shared top bar and footer into every static Incentium page.

  python3 scripts/sync_topbar.py          # write the pages
  python3 scripts/sync_topbar.py --check  # exit 1 if a page is out of date (CI)

The parts live in site/partials/. In a page they sit between marker comments:
  <!-- ic:topbar --> ... <!-- /ic:topbar -->   and   <!-- ic:footer --> ... <!-- /ic:footer -->
Pages served by Python (faucet, sign-in) read the same files when they start; see partial().
"""
from __future__ import annotations

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGES = {                                   # page -> shared parts it carries
    "site/index.html": ["topbar", "footer"],
    "site/mine/index.html": ["topbar", "footer"],
    "explorer/web/index.html": ["topbar", "footer"],
    "chat/web/index.html": ["topbar"],         # a full-height app: no footer
    "site/spec/index.html": ["topbar", "footer"],   # made by scripts/build_docs.py
    "site/whitepaper/index.html": ["topbar", "footer"],
    "site/events/index.html": ["topbar", "footer"],
}
PARTS = ["topbar", "footer"]


def partial(name: str) -> str:
    """The shared part `name` ("topbar" or "footer"), or "" if the file is missing."""
    try:
        with open(os.path.join(ROOT, "site", "partials", name + ".html"), encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return ""


def synced(text: str) -> str:
    for name in PARTS:
        pat = re.compile(rf"(<!-- ic:{name} -->)(.*?)(<!-- /ic:{name} -->)", re.S)
        text = pat.sub(lambda m: m.group(1) + "\n" + partial(name) + "\n" + m.group(3), text)
    return text


def main() -> int:
    check, stale = "--check" in sys.argv, []
    for rel, parts in PAGES.items():
        path = os.path.join(ROOT, rel)
        old = open(path, encoding="utf-8").read()
        for name in parts:
            if f"<!-- ic:{name} -->" not in old:
                stale.append(f"{rel}: no <!-- ic:{name} --> marker")
        new = synced(old)
        if new != old:
            if check:
                stale.append(f"{rel}: out of date")
            else:
                open(path, "w", encoding="utf-8").write(new)
                print(f"updated {rel}")
    for s in stale:
        print(s, file=sys.stderr)
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main())
