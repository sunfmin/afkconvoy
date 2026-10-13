#!/bin/sh
# Refuses what a fellow never does, whatever it was asked: see CLAUDE.md.
call=$(cat)
case "$call" in
  *--no-verify*) echo "fellow: a commit is never made with --no-verify" >&2; exit 2 ;;
  *"push --force"*|*"push -f "*|*--force-with-lease*) echo "fellow: a branch is never force-pushed" >&2; exit 2 ;;
  *"git rebase"*) echo "fellow: a branch is never rebased; merge instead" >&2; exit 2 ;;
esac
exit 0
