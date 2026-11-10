#!/bin/sh
# Done once the store holds its seed; says the seed as it was given.
[ -f /tmp/seed ] || exit 1
printf '{"seed": "%s"}' "$(cat /tmp/seed)" > "$TAY_VALUES"
