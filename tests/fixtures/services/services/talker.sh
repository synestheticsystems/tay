#!/bin/sh
# Waits for the file talk in the folder $1, then prints a numbered line five
# times a second, for ever, writing the number of the last to talked there.
until [ -e "$1/talk" ]; do sleep 0.2; done
i=0
while :; do
  i=$((i + 1))
  echo "line $i"
  echo "$i" > "$1/talked"
  sleep 0.2
done
