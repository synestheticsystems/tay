#!/bin/sh
# Ready after the seconds it is given, as its log says.
echo "starting, ready in $1 seconds"
sleep "$1"
echo "listening"
while :; do sleep 1; done
