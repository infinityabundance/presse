#!/bin/sh
# CPU-parallel with limited cores vs CUDA pool — find the crossover.
set -u
cd /run/media/one/1tb_kingston1/presse
DIR=/tmp/gpubench
OUT=$DIR/threads
mkdir -p "$OUT"
BIN=target/release/presse

best_of_3() {
  best=""
  i=0
  while [ "$i" -lt 3 ]; do
    t0=$(date +%s.%N)
    "$@" >/dev/null 2>&1
    t1=$(date +%s.%N)
    t=$(awk "BEGIN{printf \"%.3f\", $t1-$t0}")
    if [ -z "$best" ] || [ "$(awk "BEGIN{print ($t<$best)?1:0}")" = "1" ]; then best=$t; fi
    i=$((i+1))
  done
  printf "%s" "$best"
}

echo "pdf,cpu2,cpu4,cpu8,cuda_pool" > "$OUT/results.csv"
for f in "$DIR"/*.pdf; do
  b=$(basename "$f")
  c2=$(best_of_3 env RAYON_NUM_THREADS=2 "$BIN" press -a cpu -q 50 "$f" -o "$OUT/c2-$b")
  c4=$(best_of_3 env RAYON_NUM_THREADS=4 "$BIN" press -a cpu -q 50 "$f" -o "$OUT/c4-$b")
  c8=$(best_of_3 env RAYON_NUM_THREADS=8 "$BIN" press -a cpu -q 50 "$f" -o "$OUT/c8-$b")
  g=$(best_of_3 "$BIN" press -a cuda -q 50 "$f" -o "$OUT/g-$b")
  echo "$b,$c2,$c4,$c8,$g" | tee -a "$OUT/results.csv"
done
echo DONE
