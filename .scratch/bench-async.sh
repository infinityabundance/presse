#!/bin/sh
# Compare the async pipeline (streams+YUV+pinned) vs the pool backend vs the
# original mutex backend vs CPU-parallel/serial/gs on the photo PDFs.
set -u
cd /run/media/one/1tb_kingston1/presse
DIR=/tmp/gpubench
OUT=$DIR/bench2
mkdir -p "$OUT"

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

echo "pdf,in_MB,cpu_par_s,cpu_ser_s,cuda_async_s,cuda_pool_s,cuda_mutex_s,gs_s,async_KB,pool_KB" > "$OUT/results.csv"
for f in "$DIR"/*.pdf; do
  b=$(basename "$f")
  in_mb=$(awk "BEGIN{printf \"%.1f\", $(stat -c %s "$f")/1e6}")
  cpu_par=$(best_of_3 env RAYON_NUM_THREADS=16 /tmp/presse-async press -a cpu -q 50 "$f" -o "$OUT/cpu-$b")
  cpu_ser=$(best_of_3 env RAYON_NUM_THREADS=1 /tmp/presse-async press -a cpu -q 50 "$f" -o "$OUT/cpus-$b")
  async=$(best_of_3 /tmp/presse-async press -a cuda -q 50 "$f" -o "$OUT/async-$b")
  pool=$(best_of_3 /tmp/presse-pool press -a cuda -q 50 "$f" -o "$OUT/pool-$b")
  mutex=$(best_of_3 /tmp/presse-mutex press -a cuda -q 50 "$f" -o "$OUT/mutex-$b")
  gs_s=$(best_of_3 gs -q -sDEVICE=pdfwrite -dPDFSETTINGS=/ebook -dNOPAUSE -dBATCH -sOutputFile="$OUT/gs-$b" "$f")
  a_kb=$(( $(stat -c %s "$OUT/async-$b") / 1024 ))
  p_kb=$(( $(stat -c %s "$OUT/pool-$b") / 1024 ))
  echo "$b,$in_mb,$cpu_par,$cpu_ser,$async,$pool,$mutex,$gs_s,$a_kb,$p_kb" | tee -a "$OUT/results.csv"
done
echo DONE
