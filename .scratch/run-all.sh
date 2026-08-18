#!/bin/sh
# Master verification runner for the rebuilt 100-PDF corpus:
#  1. build release binary with cuda feature + target-cpu=native
#  2. -a cuda sweep  (GPU path; small streams stay CPU per the 128KiB guard)
#  3. -a cpu sweep    (deterministic CPU baseline)
#  4. qpdf gate on GPU outputs
#  5. visual sweep (pdftoppm + SSIM) on GPU and CPU outputs
#  6. 3-way timing benchmark (parallel / serial / ghostscript) via bench3.py
set -u
cd /run/media/one/1tb_kingston1/presse
BIN=target/release/presse
IN=/tmp/pdfcorpus/final
OUT_CUDA=/tmp/pdfcorpus/out-cuda
OUT_CPU=/tmp/pdfcorpus/out-cpu
mkdir -p "$OUT_CUDA" "$OUT_CPU"

echo "== [1/6] building release binary (cuda feature, target-cpu=native) =="
RUSTFLAGS="-C target-cpu=native" cargo build --release --features cuda 2>&1 | tail -1

echo "== [2/6] GPU sweep (-a cuda, q50) =="
n=0
for f in "$IN"/*.pdf; do
  b=$(basename "$f")
  "$BIN" press -a cuda -q 50 "$f" -o "$OUT_CUDA/$b" </dev/null >>/tmp/pdfcorpus/run.log 2>>/tmp/pdfcorpus/cuda-errors.log
  s=$?
  [ "$s" != "0" ] && echo "FAIL cuda($s): $b" >> /tmp/pdfcorpus/run.log
  n=$((n+1))
done
echo "cuda sweep done: $n (failures: $(grep -c '^FAIL' /tmp/pdfcorpus/run.log 2>/dev/null || echo 0))"
echo "cuda GPU-warnings: $(grep -c 'GPU transcoding failed' /tmp/pdfcorpus/cuda-errors.log 2>/dev/null || echo 0)"

echo "== [3/6] CPU sweep (-a cpu, q50) =="
n=0
for f in "$IN"/*.pdf; do
  b=$(basename "$f")
  "$BIN" press -a cpu -q 50 "$f" -o "$OUT_CPU/$b" </dev/null >>/tmp/pdfcorpus/run.log 2>>/tmp/pdfcorpus/cpu-errors.log
  s=$?
  [ "$s" != "0" ] && echo "FAIL cpu($s): $b" >> /tmp/pdfcorpus/run.log
  n=$((n+1))
done
echo "cpu sweep done: $n (failures: $(grep -c '^FAIL' /tmp/pdfcorpus/run.log 2>/dev/null || echo 0))"

echo "== [4/6] qpdf gate on GPU outputs =="
: > /tmp/pdfcorpus/qpdf-cuda.log
for f in "$OUT_CUDA"/*.pdf; do
  qpdf --check "$f" >> /tmp/pdfcorpus/qpdf-cuda.log 2>&1
  [ "$?" != "0" ] && echo "QPDF-FAIL: $(basename "$f")" >> /tmp/pdfcorpus/qpdf-fails.log
done
grep -c "No syntax or stream encoding errors" /tmp/pdfcorpus/qpdf-cuda.log

echo "== [5/6] visual sweep =="
python3 benches/docker/visual_sweep.py /tmp/pdfcorpus/final ../out-cuda 40 > /tmp/pdfcorpus/sweep-cuda.txt 2>&1 || true
python3 benches/docker/visual_sweep.py /tmp/pdfcorpus/final ../out-cpu 40 > /tmp/pdfcorpus/sweep-cpu.txt 2>&1 || true

echo "== [6/6] 3-way benchmark =="
python3 benches/docker/bench3.py "$IN" /tmp/pdfcorpus/bench3.csv "$BIN" 2>&1 | tail -3

echo "ALL DONE"
