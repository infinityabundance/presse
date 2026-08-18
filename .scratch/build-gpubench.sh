#!/bin/sh
# Build large image-heavy PDFs for GPU-vs-CPU benchmarking.
# Real photos via Lorem Picsum (deterministic seeds), embedded as DCTDecode.
set -u
OUT=/tmp/gpubench
mkdir -p "$OUT/img"
cd "$OUT/img"

echo "== fetching photos =="
i=1
while [ "$i" -le 20 ]; do
  [ -f "p6m_$i.jpg" ] || timeout 60 curl -sL -o "p6m_$i.jpg" "https://picsum.photos/seed/gpubench$i/3000/2000"
  i=$((i+1))
done
i=1
while [ "$i" -le 4 ]; do
  [ -f "p17m_$i.jpg" ] || timeout 90 curl -sL -o "p17m_$i.jpg" "https://picsum.photos/seed/gpubenchbig$i/5000/3500"
  i=$((i+1))
done
ls -la *.jpg | head -5
du -sh .

echo "== building PDFs =="
# 20 unique 6MP photos (20 pages)
magick p6m_*.jpg ../photos20.pdf
# 60 images: 20 unique x3 copies (dedup-friendly, large object count)
magick p6m_1.jpg p6m_1.jpg p6m_1.jpg p6m_2.jpg p6m_2.jpg p6m_2.jpg p6m_3.jpg p6m_3.jpg p6m_3.jpg \
  p6m_4.jpg p6m_4.jpg p6m_4.jpg p6m_5.jpg p6m_5.jpg p6m_5.jpg p6m_6.jpg p6m_6.jpg p6m_6.jpg \
  p6m_7.jpg p6m_7.jpg p6m_7.jpg p6m_8.jpg p6m_8.jpg p6m_8.jpg p6m_9.jpg p6m_9.jpg p6m_9.jpg \
  p6m_10.jpg p6m_10.jpg p6m_10.jpg p6m_11.jpg p6m_11.jpg p6m_11.jpg p6m_12.jpg p6m_12.jpg p6m_12.jpg \
  p6m_13.jpg p6m_13.jpg p6m_13.jpg p6m_14.jpg p6m_14.jpg p6m_14.jpg p6m_15.jpg p6m_15.jpg p6m_15.jpg \
  p6m_16.jpg p6m_16.jpg p6m_16.jpg p6m_17.jpg p6m_17.jpg p6m_17.jpg p6m_18.jpg p6m_18.jpg p6m_18.jpg \
  p6m_19.jpg p6m_19.jpg p6m_19.jpg p6m_20.jpg p6m_20.jpg p6m_20.jpg \
  ../photos60.pdf
# 10 unique 17.5MP photos (10 pages)
magick p17m_*.jpg p17m_*.jpg p17m_1.jpg p17m_2.jpg ../photos10big.pdf
# 1 photo repeated 50x (pure dedup + repack stress)
magick $(for i in $(seq 1 50); do printf "p6m_1.jpg "; done) ../dup50.pdf

echo "== results =="
for f in ../photos20.pdf ../photos60.pdf ../photos10big.pdf ../dup50.pdf; do
  echo "$f: $(du -h "$f" | cut -f1)  pages=$(pdfinfo "$f" 2>/dev/null | grep Pages | awk '{print $2}')"
done
