#!/bin/sh
# Benchmark large image-heavy PDFs: cpu-parallel, cpu-serial, cuda (pool),
# cuda (old mutex), and ghostscript /ebook. 3 runs each, best-of-3.
set -u
cd /run/media/one/1tb_kingston1/presse
BIN_POOL=target/release/presse
BIN_MUTEX=/tmp/presse-mutex
DIR=/tmp/gpubench
OUT=$DIR/bench
mkdir -p "$OUT"

best_of_3() { # cmd... -> best seconds
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

echo "pdf,in_MB,cpu_par_s,cpu_ser_s,cuda_pool_s,cuda_mutex_s,gs_s,cpu_KB,cuda_KB,gs_KB" > "$OUT/results.csv"
for f in "$DIR"/*.pdf; do
  b=$(basename "$f")
  in_mb=$(awk "BEGIN{printf \"%.1f\", $(stat -c %s "$f")/1e6}")
  cpu_par=$(best_of_3 env RAYON_NUM_THREADS=16 "$BIN_POOL" press -a cpu -q 50 "$f" -o "$OUT/cpu-$b")
  cpu_ser=$(best_of_3 env RAYON_NUM_THREADS=1 "$BIN_POOL" press -a cpu -q 50 "$f" -o "$OUT/cpus-$b")
  cuda_pool=$(best_of_3 "$BIN_POOL" press -a cuda -q 50 "$f" -o "$OUT/cuda-$b")
  cuda_mutex=$(best_of_3 "$BIN_MUTEX" press -a cuda -q 50 "$f" -o "$OUT/cudam-$b")
  gs_s=$(best_of_3 gs -q -sDEVICE=pdfwrite -dPDFSETTINGS=/ebook -dNOPAUSE -dBATCH -sOutputFile="$OUT/gs-$b" "$f")
  cpu_kb=$(( $(stat -c %s "$OUT/cpu-$b") / 1024 ))
  cuda_kb=$(( $(stat -c %s "$OUT/cuda-$b") / 1024 ))
  gs_kb=$(( $(stat -c %s "$OUT/gs-$b") / 1024 ))
  echo "$b,$in_mb,$cpu_par,$cpu_ser,$cuda_pool,$cuda_mutex,$gs_s,$cpu_kb,$cuda_kb,$gs_kb" | tee -a "$OUT/results.csv"
done
echo "DONE"
