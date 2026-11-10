#!/bin/sh
echo "waiting for go"
until [ -e "$1" ]; do sleep 0.1; done
echo "went"
