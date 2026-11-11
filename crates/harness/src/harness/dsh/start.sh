#!/bin/sh
# Keep the host's launch command opaque and append DSH's arguments as words.
# The native loader dependency, not this wrapper, enforces guard activation.
set -eu
launch=$1
kept=$2
role=$3
worktree=$4
profile=$5
for file in afkconvoy.mjs afkconvoy.patch.yml; do
    if [ ! -r "$kept/$file" ]; then
        echo "afkconvoy: DSH guard file $file is missing; refusing worker" >&2
        exit 1
    fi
done
set --
if [ "$profile" = fleet ]; then
    set -- --patch "$kept/fleet.patch.yml"
fi
if [ -f "$role/.dsh/cordis.patch.yml" ]; then
    set -- "$@" --patch "$role/.dsh/cordis.patch.yml"
fi
set -- "$@" --patch "$kept/afkconvoy.patch.yml" "$worktree"
exec sh -c "$launch \"\$@\"" afkconvoy-dsh "$@"
