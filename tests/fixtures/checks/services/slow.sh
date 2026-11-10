#!/bin/sh
# Writes the file READY_FILE names after the seconds it is given.
sleep "$1"
touch "$READY_FILE"
echo "written"
while :; do sleep 1; done
