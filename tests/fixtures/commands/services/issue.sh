#!/bin/sh
# Reads its request on standard input and writes what it issues to the file
# TAY_VALUES names.
read request
echo "issuing for: $request ($GREETING)"
printf '{"token": "tok-%s-secret", "id": 42}' "$request" > "$TAY_VALUES"
