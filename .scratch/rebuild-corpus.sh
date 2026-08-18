#!/bin/sh
# Rebuild the 100-PDF corpus at /tmp/pdfcorpus/final (arXiv + IRS + specs + pdf.js).
set -u
FETCH=/run/media/one/1tb_kingston1/presse/benches/docker/fetch_batch.sh
CORPUS=/tmp/pdfcorpus
mkdir -p "$CORPUS/arxiv" "$CORPUS/irs" "$CORPUS/specs" "$CORPUS/pdfjs" "$CORPUS/final"

echo "== arXiv =="
bash "$FETCH" "$CORPUS/arxiv" \
"attention1706|https://arxiv.org/pdf/1706.03762" \
"sam2203|https://arxiv.org/pdf/2203.15556" \
"bert1810|https://arxiv.org/pdf/1810.04805" \
"gpt3_2005|https://arxiv.org/pdf/2005.14165" \
"clip2103|https://arxiv.org/pdf/2103.00020" \
"llama2302|https://arxiv.org/pdf/2302.13971" \
"resnet1512|https://arxiv.org/pdf/1512.03385" \
"adam1412|https://arxiv.org/pdf/1412.6980" \
"yolo1506|https://arxiv.org/pdf/1506.02640" \
"inception1509|https://arxiv.org/pdf/1509.04226" \
"diffusion2304|https://arxiv.org/pdf/2304.10557" \
"roberta1904|https://arxiv.org/pdf/1904.12848" \
"vit2010|https://arxiv.org/pdf/2010.11929" \
"deeplab1706|https://arxiv.org/pdf/1706.05587" \
"unet1505|https://arxiv.org/pdf/1505.04597" \
"gan1406|https://arxiv.org/pdf/1406.2661" \
"stylegan1812|https://arxiv.org/pdf/1812.04948" \
"nerf2003|https://arxiv.org/pdf/2003.08934" \
"alphago1601|https://arxiv.org/pdf/1601.00670" \
"transformer_xl1901|https://arxiv.org/pdf/1901.02860" \
"electra2003|https://arxiv.org/pdf/2003.10555" \
"wavenet1609|https://arxiv.org/pdf/1609.03499" \
"pix2pix1611|https://arxiv.org/pdf/1611.07004" \
"cyclegan1703|https://arxiv.org/pdf/1703.10593" \
"imagenet_classification|https://arxiv.org/pdf/1409.1556" \
"mobilenet1704|https://arxiv.org/pdf/1704.04861" \
"efficientnet1905|https://arxiv.org/pdf/1905.11946" \
"retinanet1708|https://arxiv.org/pdf/1708.02002" \
"fasterrcnn1506|https://arxiv.org/pdf/1506.01497" \
"ssd1512|https://arxiv.org/pdf/1512.02325" \
"maskrcnn1703|https://arxiv.org/pdf/1703.06870" \
"densenet1608|https://arxiv.org/pdf/1608.06993"

echo "== IRS =="
bash "$FETCH" "$CORPUS/irs" \
"fw4|https://www.irs.gov/pub/irs-pdf/fw4.pdf" \
"f1040|https://www.irs.gov/pub/irs-pdf/f1040.pdf" \
"i1040|https://www.irs.gov/pub/irs-pdf/i1040.pdf" \
"f1040s1|https://www.irs.gov/pub/irs-pdf/f1040s1.pdf" \
"f1040s2|https://www.irs.gov/pub/irs-pdf/f1040s2.pdf" \
"f1040s3|https://www.irs.gov/pub/irs-pdf/f1040s3.pdf" \
"f1040sr|https://www.irs.gov/pub/irs-pdf/f1040sr.pdf" \
"i1040sr|https://www.irs.gov/pub/irs-pdf/i1040sr.pdf" \
"f1040x|https://www.irs.gov/pub/irs-pdf/f1040x.pdf" \
"i1040x|https://www.irs.gov/pub/irs-pdf/i1040x.pdf" \
"f1040es|https://www.irs.gov/pub/irs-pdf/f1040es.pdf" \
"f2441|https://www.irs.gov/pub/irs-pdf/f2441.pdf" \
"i2441|https://www.irs.gov/pub/irs-pdf/i2441.pdf" \
"f4562|https://www.irs.gov/pub/irs-pdf/f4562.pdf" \
"f4797|https://www.irs.gov/pub/irs-pdf/f4797.pdf" \
"f6251|https://www.irs.gov/pub/irs-pdf/f6251.pdf" \
"f8582|https://www.irs.gov/pub/irs-pdf/f8582.pdf" \
"f8606|https://www.irs.gov/pub/irs-pdf/f8606.pdf" \
"i8606|https://www.irs.gov/pub/irs-pdf/i8606.pdf" \
"f8812|https://www.irs.gov/pub/irs-pdf/f8812.pdf" \
"f8863|https://www.irs.gov/pub/irs-pdf/f8863.pdf" \
"f8962|https://www.irs.gov/pub/irs-pdf/f8962.pdf" \
"f8995|https://www.irs.gov/pub/irs-pdf/f8995.pdf" \
"f8995a|https://www.irs.gov/pub/irs-pdf/f8995a.pdf" \
"fw9|https://www.irs.gov/pub/irs-pdf/fw9.pdf" \
"iw9|https://www.irs.gov/pub/irs-pdf/iw9.pdf" \
"fw2|https://www.irs.gov/pub/irs-pdf/fw2.pdf" \
"iw2|https://www.irs.gov/pub/irs-pdf/iw2.pdf" \
"f5498|https://www.irs.gov/pub/irs-pdf/f5498.pdf" \
"f1099r|https://www.irs.gov/pub/irs-pdf/f1099r.pdf"

echo "== Specs =="
bash "$FETCH" "$CORPUS/specs" \
"pdf32000|https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf" \
"pdfref17old|https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/pdfreference1.7old.pdf" \
"unicode150|https://www.unicode.org/versions/Unicode15.0.0/UnicodeStandard-15.0.pdf" \
"pymupdf_guide|https://pymupdf.readthedocs.io/_/downloads/en/latest/pdf/" \
"census_acs|https://www2.census.gov/programs-surveys/acs/tech_docs/subject_definitions/2021_ACSSubjectDefinitions.pdf" \
"fda_guidance|https://www.fda.gov/media/72168/download" \
"nih_pub|https://www.nhlbi.nih.gov/sites/default/files/publications/sleep-deprivation-health-effects.pdf" \
"epa_report|https://www.epa.gov/system/files/documents/2023-03/epa-456-f-23-001.pdf" \
"state_report|https://www.state.gov/wp-content/uploads/2023/06/2023-Investment-Climate-Statements-Report.pdf"

echo "== pdf.js sparse clone =="
cd "$CORPUS" && git clone --depth 1 --filter=blob:limit=200k --sparse https://github.com/mozilla/pdf.js pdfjs 2>&1 | tail -1
cd "$CORPUS/pdfjs" && git sparse-checkout set test/pdfs 2>&1 | tail -1
echo "== pdf.js pdfs: $(find "$CORPUS/pdfjs/test/pdfs" -name '*.pdf' | wc -l) =="

echo "== assembling final =="
n=0
for f in "$CORPUS"/arxiv/*.pdf; do cp "$f" "$CORPUS/final/arxiv_$(basename "$f")"; n=$((n+1)); done
for f in "$CORPUS"/irs/*.pdf; do cp "$f" "$CORPUS/final/irs_$(basename "$f")"; n=$((n+1)); done
for f in "$CORPUS"/specs/*.pdf; do cp "$f" "$CORPUS/final/specs_$(basename "$f")"; n=$((n+1)); done
# pick 33 diverse pdf.js files
i=0
for f in "$CORPUS/pdfjs/test/pdfs"/*.pdf; do
  case "$(basename "$f")" in
    issue3188.pdf|22060_A1_01_Plans.pdf|issue19517.pdf|xfa_filled_imm1344e.pdf|freeculture.pdf|images.pdf|tracemonkey.pdf|comments.pdf|highlights.pdf|form_two_pages.pdf|issue19971.pdf|issue20513.pdf|issue7229.pdf|issue13193.pdf|issue1905.pdf|issue9949.pdf|issue5549.pdf|issue6127.pdf|issue6894.pdf|issue6961.pdf|issue5994.pdf|issue19360.pdf|issue5567.pdf|issue5481.pdf|issue6286.pdf|issue12337.pdf|issue11922_reduced.pdf|issue17808.pdf|issue18911.pdf|bug1992868.pdf|bug1978317.pdf|Brotli-Prototype-FileA.pdf|magazine.pdf)
      cp "$f" "$CORPUS/final/pdfjs_$(basename "$f")"; i=$((i+1));;
  esac
done
echo "== selected pdf.js: $i =="
echo "== final count: $(ls "$CORPUS/final" | wc -l) =="
echo "DONE"
