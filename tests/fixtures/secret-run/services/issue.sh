#!/bin/sh
token=$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')
printf '{"token": "tk%s"}' "$token" > "$TAY_VALUES"
echo "issued a token"
