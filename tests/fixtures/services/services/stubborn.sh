#!/bin/sh
# SIGTERM is ignored here and in every process started from here.
trap '' TERM
echo "ignoring SIGTERM"
while :; do sleep 1; done
