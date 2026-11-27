#!/usr/bin/env bash
# Keep Forgejo (git.incentium.org, the source) up to date with branches pushed to GitHub, so the
# Forgejo -> GitHub push mirror never wipes them.
#
# Why: Forgejo's push mirror runs `git push --mirror`: every branch on GitHub becomes exactly what
# Forgejo has. A branch pushed only to GitHub is deleted, and a GitHub branch that moved ahead is
# reset. Work pushed to GitHub (Claude's sessions can only reach GitHub) was lost that way twice.
#
# What it does, every minute from cron:
#   * fetches every GitHub branch into a local bare copy and keeps the newest commit it has seen
#     for each, so a branch the mirror already reset or deleted on GitHub is not lost;
#   * pushes to Forgejo only fast-forwards and new branches. It never force-pushes and never deletes;
#   * a branch that was deleted on Forgejo on purpose is not brought back (the list of every branch
#     Forgejo has had since setup tells it apart from a branch that was never there);
#   * branches that diverged (different commits on both) are only logged, for a person to resolve.
#
#   bash forgejo-sync.sh setup     # once: bare copy, stores a Forgejo access token, adds the cron line
#   bash forgejo-sync.sh           # one run (what cron calls)
#   bash forgejo-sync.sh status    # what differs now, without pushing
#
# The token is a Forgejo access token (Settings > Applications, scope: repository read and write),
# not your password. It is stored in ~/.config/incentium/forgejo-credentials (mode 600).
set -euo pipefail

GITHUB=${GITHUB:-https://github.com/Doncoin1999/incentium-chain.git}
FORGEJO=${FORGEJO:-https://git.incentium.org/incentium/incentium-chain.git}
HOME_DIR=${SYNC_DIR:-$HOME/git-sync}
REPO=$HOME_DIR/incentium-chain.git
CRED=$HOME/.config/incentium/forgejo-credentials
SEEN=$HOME_DIR/forgejo-branches.seen    # every branch Forgejo has had since setup
LOG=$HOME_DIR/sync.log
MODE=${1:-run}

log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$LOG"; }

setup() {
  mkdir -p "$HOME_DIR" "$(dirname "$CRED")"
  [ -d "$REPO" ] || git init -q --bare "$REPO"
  git -C "$REPO" remote get-url github >/dev/null 2>&1 || git -C "$REPO" remote add github "$GITHUB"
  git -C "$REPO" remote get-url forgejo >/dev/null 2>&1 || git -C "$REPO" remote add forgejo "$FORGEJO"
  git -C "$REPO" config credential.helper "store --file $CRED"
  if [ ! -s "$CRED" ]; then
    read -rp "Forgejo user name (e.g. mail@incentium.org): " user
    read -rsp "Forgejo access token (not your password): " token; echo
    host=$(echo "$FORGEJO" | sed -E 's#^https://([^/]+)/.*#\1#')
    enc=$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$user")
    ( umask 077; echo "https://$enc:$token@$host" > "$CRED" )
  fi
  chmod 600 "$CRED"
  git -C "$REPO" ls-remote --heads forgejo >/dev/null && echo "Forgejo: token works"
  git -C "$REPO" ls-remote --heads forgejo | awk '{sub("refs/heads/","",$2); print $2}' | sort > "$SEEN"
  line="* * * * * flock -n $HOME_DIR/.lock bash $(readlink -f "$0") >/dev/null 2>&1"
  if ! command -v crontab >/dev/null; then
    echo "no crontab here: add a timer that runs '$line' yourself"
  elif ! crontab -l 2>/dev/null | grep -qF "forgejo-sync.sh"; then
    ( crontab -l 2>/dev/null; echo "$line" ) | crontab -
    echo "cron: runs every minute"
  fi
  echo "done; log: $LOG"
}

# newest of two commits if one contains the other, else empty
newest() {
  local a=$1 b=$2
  if [ -z "$a" ]; then echo "$b"; return; fi
  if [ -z "$b" ] || [ "$a" = "$b" ]; then echo "$a"; return; fi
  if git -C "$REPO" merge-base --is-ancestor "$a" "$b"; then echo "$b"
  elif git -C "$REPO" merge-base --is-ancestor "$b" "$a"; then echo "$a"
  else echo ""; fi
}

run() {
  [ -d "$REPO" ] || { echo "run setup first"; exit 1; }
  touch "$SEEN"
  # Objects from both; GitHub's branches land in refs/github/* (forced: GitHub may have been reset),
  # the newest commit ever seen per branch is kept in refs/keep/*.
  git -C "$REPO" fetch -q github '+refs/heads/*:refs/github/*'
  git -C "$REPO" fetch -q forgejo '+refs/heads/*:refs/forgejo/*'
  declare -A F
  while read -r sha ref; do F[${ref#refs/heads/}]=$sha; done < <(git -C "$REPO" ls-remote --heads forgejo)

  git -C "$REPO" for-each-ref --format='%(refname:strip=2) %(objectname)' refs/github/ |
  while read -r b g; do
    keep=$(git -C "$REPO" rev-parse -q --verify "refs/keep/$b" || true)
    best=$(newest "$keep" "$g")
    if [ -z "$best" ]; then
      log "WARN $b: GitHub ($g) and the last kept commit ($keep) diverged; keeping $keep"
      best=$keep
    fi
    git -C "$REPO" update-ref "refs/keep/$b" "$best"
    f=${F[$b]:-}
    if [ -z "$f" ]; then
      if grep -qxF "$b" "$SEEN"; then
        continue   # deleted on Forgejo on purpose: leave it deleted
      fi
      [ "$MODE" = status ] && { echo "new on GitHub: $b ${best:0:7}"; continue; }
      git -C "$REPO" push -q forgejo "$best:refs/heads/$b" && log "new branch $b -> ${best:0:7}"
    elif [ "$f" = "$best" ]; then
      :
    elif git -C "$REPO" merge-base --is-ancestor "$f" "$best"; then
      [ "$MODE" = status ] && { echo "ahead on GitHub: $b ${f:0:7}..${best:0:7}"; continue; }
      git -C "$REPO" push -q forgejo "$best:refs/heads/$b" && log "fast-forward $b ${f:0:7}..${best:0:7}"
    elif git -C "$REPO" merge-base --is-ancestor "$best" "$f"; then
      :   # Forgejo is ahead; its mirror updates GitHub
    else
      log "DIVERGED $b: Forgejo ${f:0:7}, GitHub ${best:0:7}; not touched (resolve by hand)"
    fi
  done
  [ "$MODE" = status ] && return
  # Every branch Forgejo has ever had, so one deleted there later is known as deleted on purpose.
  { cat "$SEEN"; git -C "$REPO" ls-remote --heads forgejo | awk '{sub("refs/heads/","",$2); print $2}'; } \
    | sort -u > "$SEEN.new" && mv "$SEEN.new" "$SEEN"
}

case "$MODE" in
  setup) setup ;;
  status) run ;;
  run) mkdir -p "$HOME_DIR"; MODE=run; run ;;
  *) echo "usage: $0 [setup|status]"; exit 2 ;;
esac
