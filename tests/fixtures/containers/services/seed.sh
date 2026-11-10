#!/bin/sh
# Keeps what it is fed as the store's seed, and gives it.
cat > /tmp/seed
echo "seeded with $(cat /tmp/seed)"
printf '{"seed": "%s"}' "$(cat /tmp/seed)" > "$TAY_VALUES"
