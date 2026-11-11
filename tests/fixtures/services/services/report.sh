#!/bin/sh
# Says the folder it runs in and every variable it was given. Then, given
# `check`, says so and exits with 1, as a command's check that finds it not
# done; given `once`, ends; given nothing, waits for ever.
echo "in $(pwd)"
env | sort | sed 's/^/variable /'
case "$1" in
  once) ;;
  check) echo checked; exit 1 ;;
  *) while :; do sleep 1; done ;;
esac
