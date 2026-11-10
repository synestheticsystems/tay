#!/bin/sh
# Gives its values the first time it runs; later runs give none.
if [ ! -f "$1" ]; then
  printf '{"id": 7}' > "$TAY_VALUES"
  touch "$1"
fi
