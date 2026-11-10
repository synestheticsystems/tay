#!/bin/sh
# Runs for the seconds it is given, then exits with the code it is given.
echo "up for $1 seconds"
sleep "$1"
echo "exiting with $2"
exit "$2"
