#!/bin/sh
touch "$1"
echo run >> "$2"
printf '{"version":"1"}' > "$TAY_VALUES"
