#!/bin/sh
# Stamps what it is fed, giving the stamp and, as a secret, how long a
# password it was given.
echo "stamping $(cat)"
printf '{"stamp": "stamped", "proof": "%s"}' "${#PASSWORD}" > "$TAY_VALUES"
