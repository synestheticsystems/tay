#!/bin/sh
# Adds a line to the file it is given; when it is given a second file, waits
# for that one to exist before it ends.
echo "run" >> "$1"
echo "counted in $1"
if [ -n "$2" ]; then
  while [ ! -f "$2" ]; do sleep 0.1; done
fi
