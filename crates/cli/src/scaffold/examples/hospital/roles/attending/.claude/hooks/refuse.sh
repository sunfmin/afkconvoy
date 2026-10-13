#!/bin/sh
# Refuses what an attending never does, whatever it was asked: see CLAUDE.md.
call=$(cat)
case "$call" in
  *"git commit"*|*"git push"*) echo "attending: an attending writes no code: say what is wrong in the text of your verdict" >&2; exit 2 ;;
esac
exit 0
