#!/bin/sh
echo "in $(pwd)"
env | sort | sed 's/^/variable /'
if [ "$1" != "once" ]; then
  while :; do sleep 1; done
fi
