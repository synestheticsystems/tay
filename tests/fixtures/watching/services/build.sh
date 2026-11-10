#!/bin/sh
# Builds a program from each of the sources `one` and `two` in $1, into $2,
# and counts its runs there. While $2/hold is there it waits for $2/go, and
# while $1/fail is there it fails, saying what that file says.
src=$1
out=$2
echo run >> "$out/runs"
if [ -e "$out/hold" ]; then
  echo holding
  while [ ! -e "$out/go" ]; do sleep 0.1; done
  rm -f "$out/hold" "$out/go"
fi
if [ -e "$src/fail" ]; then
  echo "cannot build: $(cat "$src/fail")"
  exit 1
fi
for name in one two; do
  printf '#!/bin/sh\necho %s says %s\nexec sleep 3600\n' "$name" "$(cat "$src/$name.txt")" > "$out/$name.new"
  chmod +x "$out/$name.new"
  mv "$out/$name.new" "$out/$name"
done
echo built
