#!/bin/sh
# Writes the file it is given, as a prepare step of the unit.
echo "marking $1"
touch "$1"
