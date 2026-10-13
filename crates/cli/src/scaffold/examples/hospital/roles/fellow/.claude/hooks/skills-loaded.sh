#!/bin/sh
# Refuses a `pass` or a `reject` declared before the skills this stage's
# skill says to load were loaded in this session: the role's own skills name
# them, and this is what holds a worker to it. An `escalate` or a `blocked`
# is never held up.
call=$(cat)
command=$(printf '%s' "$call" | jq -r '.tool_input.command // empty')
# Quoted text is taken out first: a verdict's own text may hold these words.
printf '%s' "$command" | sed -e 's/"[^"]*"//g' -e "s/'[^']*'//g" |
  grep -Eq 'afkconvoy[^ ]* verdict( [^;|&]*)? (pass|reject)( |$)' || exit 0
case "$AFKCONVOY_SKILL" in
  diagnose) needed="codebase-design single-source-of-truth" ;;
  treat) needed="complexity-review single-source-of-truth" ;;
  *) exit 0 ;;
esac
transcript=$(printf '%s' "$call" | jq -r '.transcript_path // empty')
[ -r "$transcript" ] || exit 0
missing=""
for skill in $needed; do
  grep -q "\"skill\":\"$skill\"" "$transcript" || missing="$missing $skill"
done
[ -z "$missing" ] && exit 0
echo "fellow: the skill '$AFKCONVOY_SKILL' says to load these skills and work by them before a verdict, and this session has not loaded:$missing. Load each with the Skill tool, do the step that names it, then declare." >&2
exit 2
