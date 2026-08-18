#!/bin/sh
# Time -a cuda vs -a cpu on the 25 image-heavy docs (10+ /Subtype /Image).
set -u
BIN=/run/media/one/1tb_kingston1/presse/target/release/presse
IN=/tmp/pdfcorpus/final
mkdir -p /tmp/gpu-time
: > /tmp/gpu-time/results.csv
echo "pdf,in_bytes,cpu_s,cuda_s,cpu_KB,cuda_KB,ratio" > /tmp/gpu-time/results.csv
n=0
for f in "$IN"/*.pdf; do
  c=$(grep -c "/Subtype /Image" "$f" 2>/dev/null || true)
  [ "$c" -lt 10 ] && continue
  b=$(basename "$f")
  n=$((n+1))
  t0=$(date +%s.%N)
  "$BIN" press -a cpu -q 50 "$f" -o /tmp/gpu-time/cpu-$b </dev/null >/dev/null 2>&1
  t1=$(date +%s.%N)
  "$BIN" press -a cuda -q 50 "$f" -o /tmp/gpu-time/cuda-$b </dev/null >/dev/null 2>&1
  t2=$(date +%s.%N)
  cpu_s=$(awk "BEGIN{printf \"%.3f\", $t1-$t0}")
  cuda_s=$(awk "BEGIN{printf \"%.3f\", $t2-$t1}")
  in_b=$(stat -c %s "$f")
  cpu_b=$(stat -c %s /tmp/gpu-time/cpu-$b)
  cuda_b=$(stat -c %s /tmp/gpu-time/cuda-$b)
  ratio=$(awk "BEGIN{printf \"%.2f\", $cpu_s/$cuda_s}")
  echo "$b,$in_b,$cpu_s,$cuda_s,$((cpu_b/1024)),$((cuda_b/1024)),$ratio" >> /tmp/gpu-time/results.csv
  echo "$n: $b cpu=${cpu_s}s cuda=${cuda_s}s ratio=${ratio}x cudaKB=$((cuda_b/1024))"
done
echo "DONE $n docs"
