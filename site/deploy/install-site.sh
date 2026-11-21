#!/usr/bin/env bash
# Publish incentium.org (the site/ folder) on the server that serves it, behind Caddy.
#
#   cd ~/incentium-chain && git pull && bash site/deploy/install-site.sh
#   WEBROOT=/some/dir bash site/deploy/install-site.sh    # copy into a folder you choose
#
# Where the files go:
#   1. WEBROOT, if you set it;
#   2. else the folder Caddy already serves incentium.org from ("root" in its site block);
#   3. else /var/www/incentium-site, and a new Caddy site for incentium.org and www.incentium.org.
# Files already in the web folder that are not in site/ are left alone. Run it again after git pull.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"          # .../incentium-chain/site
[ -f "$HERE/index.html" ] || { echo "site/index.html not found next to this script"; exit 1; }

caddy_files() { sudo sh -c 'ls /etc/caddy/Caddyfile /etc/caddy/sites/*.caddy 2>/dev/null' || true; }

# the root of an existing incentium.org site block, if any
existing_root() {
  for f in $(caddy_files); do
    sudo awk '
      /^[^#[:space:]].*\{[[:space:]]*$/ { inblock = ($0 ~ /(^|[ ,])(https?:\/\/)?(www\.)?incentium\.org(:[0-9]+)?([ ,{]|$)/) }
      inblock && $1 == "root" { for (i = 2; i <= NF; i++) if ($i ~ /^\//) { print $i; exit } }
    ' "$f"
  done | head -n 1
}

NEW_SITE=0
if [ -n "${WEBROOT:-}" ]; then
  DEST="$WEBROOT"
elif DEST="$(existing_root)"; [ -n "$DEST" ]; then
  echo "Caddy already serves incentium.org from $DEST"
else
  DEST=/var/www/incentium-site
  if [ -n "$(for f in $(caddy_files); do sudo grep -lE '(^|[ ,])(www\.)?incentium\.org([ ,{:]|$)' "$f"; done 2>/dev/null)" ]; then
    echo "Caddy has an incentium.org site, but no 'root' folder this script can read."
    echo "Run it again with WEBROOT=<the folder that site serves>, or check: grep -rn incentium.org /etc/caddy"
    exit 1
  fi
  NEW_SITE=1
fi

echo "== copying site/ to $DEST"
sudo mkdir -p "$DEST"
if command -v rsync >/dev/null; then
  sudo rsync -a --exclude deploy/ --exclude partials/ --exclude docs/ "$HERE/" "$DEST/"
else
  ( cd "$HERE" && tar cf - --exclude ./deploy --exclude ./partials --exclude ./docs . ) | sudo tar xf - -C "$DEST"
fi
sudo find "$DEST" -type d -exec chmod 755 {} + ; sudo find "$DEST" -type f -exec chmod 644 {} +

if [ "$NEW_SITE" = 1 ]; then
  echo "== caddy: new site for incentium.org"
  sudo mkdir -p /etc/caddy/sites
  sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
  sudo tee /etc/caddy/sites/incentium-site.caddy >/dev/null <<CADDY
incentium.org, www.incentium.org {
	root * $DEST
	encode gzip
	file_server
}
CADDY
fi
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

echo "OK: https://incentium.org (from $DEST)"
[ "$NEW_SITE" = 1 ] && echo "DNS: incentium.org and www.incentium.org must point to this server."
exit 0
