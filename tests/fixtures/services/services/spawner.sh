#!/bin/sh
sleep 300 &
sleep 300 &
echo "started two"
while :; do sleep 1; done
