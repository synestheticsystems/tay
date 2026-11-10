#!/bin/sh
# Removes what `made` made, unless $ROOT/refuse is there, and writes to
# $ROOT/undone the mode and the token it was given and whether `holder`
# still ran.
if [ -e "$ROOT/refuse" ]; then
  echo refused
  exit 1
fi
if kill -0 "$(cat "$STATE/holder.pid")" 2>/dev/null; then holder=running; else holder=stopped; fi
printf '%s %s %s\n' "$MODE" "$TOKEN" "$holder" > "$ROOT/undone"
rm "$ROOT/made"
