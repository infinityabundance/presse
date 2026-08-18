#!/bin/sh
# Run the 100-PDF corpus through `presse -a cuda` (degraded GPU -> fallback) and `-a cpu`.
# Detached runner to avoid the tool's pty hang.
set -u
BIN=/run/media/one/1tb_kingston1/presse/target/release/presse
IN=/tmp/flat100/in
OUT=/tmp/flat100/cuda-out
CPUOUT=/tmp/flat100/cpu-out
mkdir -p "$OUT" "$CPUOUT"
: > /tmp/flat100/cuda-errors.log
: > /tmp/flat100/cpu-errors.log
n=0
for f in "$IN"/*.pdf; do
  b=$(basename "$f")
  "$BIN" press -a cuda -q 50 "$f" -o "$OUT/$b" </dev/null >>/dev/null 2>>/tmp/flat100/cuda-errors.log
  s1=$?
  "$BIN" press -a cpu -q 50 "$f" -o "$CPUOUT/$b" </dev/null >>/dev/null 2>>/tmp/flat100/cpu-errors.log
  s2=$?
  if [ "$s1" != "0" ] || [ "$s2" != "0" ]; then
    echo "FAIL($s1/$s2): $b" >> /tmp/flat100/loop.log
  fi
  n=$((n+1))
done
echo "DONE $n files" >> /tmp/flat100/loop.log
