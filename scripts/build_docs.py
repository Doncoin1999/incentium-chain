"""Turn the Incentium documents (site/docs/*.md) into pages on incentium.org.

  pip install markdown
  python3 scripts/build_docs.py          # write site/<slug>/index.html
  python3 scripts/build_docs.py --check  # exit 1 if a page is out of date (CI)

Each page has the shared top bar and footer, a table of contents beside the text, and links to
the PDF and the Markdown source. To update a document: replace its .md (and .pdf) and run this.
"""
from __future__ import annotations

import html
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import sync_topbar  # noqa: E402

# slug -> (source markdown, PDF file name in site/<slug>/ or None, top bar id, description)
DOCS = {
    "whitepaper": ("site/docs/whitepaper.md", "incentium-whitepaper-v0.1.pdf", "whitepaper",
                   "Incentium: an open network where machine intelligence is produced, verified and rewarded in ICT."),
    "spec": ("site/docs/technical-specification.md", "incentium-technical-specification-v0.1.pdf", "spec",
             "The rules of the Incentium chain, module by module, with the simulation results behind them."),
}

TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__ · Incentium</title>
<meta name="description" content="__DESC__">
<link rel="icon" type="image/svg+xml" href="/brand/incentium-logo-white-on-black.svg">
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css">
<style>
  :root { --slate: #1c2230; --panel: #242b3a; --line: #39425a; --text: #e9ecf2; --muted: #97a1b4; --lamp: #e8a33d;
    --serif: "Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif;
    --sans: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
    --mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace; }
  * { box-sizing: border-box; }
  html { background: var(--slate); color: var(--text); scroll-behavior: smooth; scroll-padding-top: 24px; }
  body { margin: 0; font: 17px/1.65 var(--sans); }
  a { color: var(--text); text-decoration-color: var(--lamp); text-underline-offset: 3px; }
  a:hover { color: var(--lamp); }
  .page { max-width: 1240px; margin: 0 auto; padding: 0 24px 96px; display: grid; grid-template-columns: 260px minmax(0, 1fr); gap: 56px; }
  nav.toc { position: sticky; top: 0; align-self: start; max-height: 100vh; overflow-y: auto; padding: 32px 0 32px; font-size: 14px; }
  nav.toc b { display: block; font: 600 12px var(--sans); letter-spacing: .1em; text-transform: uppercase; color: var(--muted); margin-bottom: 10px; }
  nav.toc ol { list-style: none; margin: 0; padding: 0; }
  nav.toc li { margin: 0; }
  nav.toc a { display: block; padding: 4px 10px; border-left: 2px solid var(--line); color: var(--muted); text-decoration: none; }
  nav.toc a:hover { color: var(--text); }
  nav.toc a.on { color: var(--text); border-left-color: var(--lamp); }
  nav.toc ol ol a { padding-left: 24px; font-size: 13.5px; }
  article { min-width: 0; max-width: 48em; padding-top: 36px; }
  .doc-head { border-bottom: 1px solid var(--line); padding-bottom: 22px; margin-bottom: 28px; }
  .doc-head .kind { color: var(--lamp); font: 600 13px var(--sans); letter-spacing: .1em; text-transform: uppercase; }
  .doc-head h1 { font: 500 clamp(32px, 4.4vw, 48px)/1.12 var(--serif); margin: 8px 0 10px; }
  .doc-head .by { color: var(--muted); font-size: 15px; }
  .doc-head .dl { display: flex; gap: 10px; flex-wrap: wrap; margin-top: 16px; }
  .doc-head .dl a { border: 1px solid var(--line); border-radius: 8px; padding: 7px 14px; font-size: 14px; text-decoration: none; }
  .doc-head .dl a.main { background: var(--lamp); border-color: var(--lamp); color: #1b1b1b; font-weight: 600; }
  .doc-head .dl a:hover { border-color: var(--lamp); }
  article h2 { font: 500 30px/1.2 var(--serif); margin: 52px 0 14px; padding-top: 8px; }
  article h3 { font: 600 19px/1.3 var(--sans); margin: 34px 0 10px; }
  article h4 { font: 600 16px/1.3 var(--sans); margin: 24px 0 8px; color: var(--muted); }
  article p, article li { color: #d5dae4; }
  article strong { color: var(--text); }
  article code { font: 14.5px var(--mono); background: #151a25; border: 1px solid #2b3345; padding: 1px 5px; border-radius: 4px; }
  article pre { background: #151a25; border: 1px solid var(--line); border-radius: 8px; padding: 14px; overflow-x: auto; }
  article pre code { border: 0; padding: 0; background: none; }
  .tbl { overflow-x: auto; margin: 18px 0 22px; border: 1px solid var(--line); border-radius: 8px; }
  table { border-collapse: collapse; width: 100%; font-size: 15px; }
  th, td { text-align: left; vertical-align: top; padding: 10px 12px; border-bottom: 1px solid #2f3749; }
  th { background: #20273a; font-weight: 600; color: var(--text); white-space: nowrap; }
  tr:last-child td { border-bottom: 0; }
  td { color: #d5dae4; }
  article img { display: block; max-width: 100%; height: auto; margin: 22px auto; border-radius: 10px; background: #fff; }
  .math { overflow-x: auto; margin: 18px 0; padding: 14px; background: #151a25; border-radius: 8px; text-align: center; }
  .math.raw { font: 14px var(--mono); text-align: left; white-space: pre-wrap; }
  h2 .anchor, h3 .anchor { opacity: 0; margin-left: 8px; color: var(--muted); text-decoration: none; font-size: .7em; }
  h2:hover .anchor, h3:hover .anchor { opacity: 1; }
  td:first-child { min-width: 9em; }
  details.mtoc { display: none; margin-top: 18px; border: 1px solid var(--line); border-radius: 8px; padding: 10px 14px; font-size: 15px; }
  details.mtoc summary { cursor: pointer; color: var(--muted); }
  details.mtoc ol { padding-left: 18px; margin: 8px 0 0; } details.mtoc ol ol { display: none; }
  details.mtoc a { display: block; padding: 4px 0; text-decoration: none; color: #d5dae4; }
  @media (max-width: 960px) { .page { grid-template-columns: 1fr; gap: 0; } nav.toc { display: none; } details.mtoc { display: block; } }
</style>
</head>
<body data-site="__SITE__">
<!-- ic:topbar -->
<!-- /ic:topbar -->
<div class="page">
  <nav class="toc" aria-label="Contents"><b>Contents</b>
__TOC__
  </nav>
  <article>
    <header class="doc-head">
      <div class="kind">__KIND__</div>
      <h1>__TITLE__</h1>
      <div class="by">__BYLINE__</div>
      <div class="dl">__DOWNLOADS__</div>
      <details class="mtoc"><summary>Contents</summary>
__TOC__
      </details>
    </header>
__BODY__
  </article>
</div>
<!-- ic:footer -->
<!-- /ic:footer -->
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js"></script>
<script>
// formulas: typeset with KaTeX when it loads, otherwise the TeX stays readable as text
addEventListener("load", () => {
  document.querySelectorAll(".math").forEach((el) => {
    try { katex.render(el.textContent, el, {displayMode: true, throwOnError: false}); }
    catch { el.classList.add("raw"); }
  });
});
// mark the section being read in the contents
const links = [...document.querySelectorAll("nav.toc a")];
const byId = new Map(links.map((a) => [a.hash.slice(1), a]));
const obs = new IntersectionObserver((entries) => {
  for (const e of entries) if (e.isIntersecting) {
    links.forEach((a) => a.classList.remove("on"));
    const a = byId.get(e.target.id); if (a) a.classList.add("on");
  }
}, {rootMargin: "0px 0px -75% 0px"});
document.querySelectorAll("article h2[id], article h3[id]").forEach((h) => obs.observe(h));
</script>
</body>
</html>
"""


def slugify(text: str) -> str:
    s = re.sub(r"<[^>]+>", "", text)
    s = html.unescape(s).lower()
    s = re.sub(r"[^\w\s-]", "", s).strip()
    return re.sub(r"[\s_]+", "-", s)


def render(md_text: str) -> tuple[str, str, str, str]:
    """(title, byline html, body html, toc html)"""
    import markdown

    lines = md_text.splitlines()
    title = lines[0].lstrip("# ").strip()
    rest = lines[1:]
    while rest and not rest[0].strip():
        rest.pop(0)
    byline = ""
    if rest and not rest[0].startswith("#"):
        byline = markdown.markdown(rest.pop(0))[3:-4]          # strip <p></p>
    text = "\n".join(rest)
    # formulas: ```latex blocks become <div class="math">
    maths = []

    def keep(m):
        maths.append(m.group(1).strip())
        return f"\n\nMATHBLOCK{len(maths) - 1}\n\n"

    text = re.sub(r"```latex\n(.*?)\n```", keep, text, flags=re.S)
    body = markdown.markdown(text, extensions=["tables", "fenced_code", "sane_lists"])
    for i, tex in enumerate(maths):
        body = body.replace(f"<p>MATHBLOCK{i}</p>", f'<div class="math">{html.escape(tex)}</div>')
    body = re.sub(r"<table>", '<div class="tbl"><table>', body)
    body = re.sub(r"</table>", "</table></div>", body)
    # headings get ids and anchors; the contents list h2 and h3
    toc, seen = [], set()

    def head(m):
        level, inner = int(m.group(1)), m.group(2)
        sid = slugify(inner) or f"s{len(seen)}"
        while sid in seen:
            sid += "-x"
        seen.add(sid)
        if level in (2, 3):
            toc.append((level, sid, re.sub(r"<[^>]+>", "", inner)))
        return f'<h{level} id="{sid}">{inner}<a class="anchor" href="#{sid}" aria-label="Link to this section">#</a></h{level}>'

    body = re.sub(r"<h([2-4])>(.*?)</h\1>", head, body)
    out, open_sub = ["    <ol>"], False
    for level, sid, label in toc:
        if level == 2:
            if open_sub:
                out.append("      </ol></li>")
                open_sub = False
            elif len(out) > 1:
                out.append("      </li>")
            out.append(f'      <li><a href="#{sid}">{label}</a>')
        else:
            if not open_sub:
                out.append("      <ol>")
                open_sub = True
            out.append(f'        <li><a href="#{sid}">{label}</a></li>')
    out.append("      </ol></li>" if open_sub else "      </li>")
    out.append("    </ol>")
    return title, byline, body, "\n".join(out)


def build(slug: str) -> str:
    src, pdf, site, desc = DOCS[slug]
    title, byline, body, toc = render(open(os.path.join(ROOT, src), encoding="utf-8").read())
    if title == "Incentium":
        title = "Incentium Whitepaper"
    kind = "Technical specification" if slug == "spec" else "Whitepaper"
    dl = []
    if pdf:
        dl.append(f'<a class="main" href="{pdf}" download>Download PDF</a>')
    dl.append(f'<a href="https://github.com/Doncoin1999/incentium-chain/blob/main/{src}">Markdown source</a>')
    page = (TEMPLATE.replace("__TITLE__", html.escape(title)).replace("__DESC__", html.escape(desc))
            .replace("__SITE__", site).replace("__KIND__", kind).replace("__BYLINE__", byline)
            .replace("__DOWNLOADS__", " ".join(dl)).replace("__TOC__", toc).replace("__BODY__", body))
    return sync_topbar.synced(page)


def main() -> int:
    check, stale = "--check" in sys.argv, []
    for slug in DOCS:
        path = os.path.join(ROOT, "site", slug, "index.html")
        new = build(slug)
        old = open(path, encoding="utf-8").read() if os.path.exists(path) else ""
        if new != old:
            if check:
                stale.append(f"site/{slug}/index.html is out of date: run python3 scripts/build_docs.py")
            else:
                os.makedirs(os.path.dirname(path), exist_ok=True)
                open(path, "w", encoding="utf-8").write(new)
                print(f"wrote site/{slug}/index.html")
    for s in stale:
        print(s, file=sys.stderr)
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main())
