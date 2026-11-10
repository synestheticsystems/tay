#!/bin/sh
echo "minting for $1"
echo "valid for $2 hours"
printf '{"session":"s-%s-%s","token":"t0ken-for-%s"}' "$1" "$2" "$1" > "$TAY_VALUES"
