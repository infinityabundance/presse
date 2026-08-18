import csv, statistics, pathlib
rows = list(csv.DictReader(open("/tmp/pdfcorpus/bench3.csv")))
def col(r, k):
    try: return float(r[k])
    except (ValueError, KeyError): return None
par = [col(r, 'p_parallel_s') for r in rows]
ser = [col(r, 'p_serial_s') for r in rows]
gs  = [col(r, 'gs_s') for r in rows]
# guard against a missing/timed-out cell
for i in range(len(rows)):
    if par[i] is None: par[i] = 0.0
    if ser[i] is None: ser[i] = 0.0
    if gs[i] is None: gs[i] = 0.0
# crude image count: /Subtype /Image dicts in raw bytes (works through object streams)
def imgcount(pdf):
    b = pathlib.Path(pdf).read_bytes()
    return b.count(b"/Subtype /Image")
imgs = [imgcount(f"/tmp/pdfcorpus/final/{r['pdf']}") for r in rows]

tot_p, tot_s, tot_g = sum(par), sum(ser), sum(gs)
print(f"total wall: parallel {tot_p:.1f}s  serial {tot_s:.1f}s  gs {tot_g:.1f}s")
print(f"parallel vs serial (total): {tot_s/tot_p:.2f}x  (mean of times: {statistics.mean(par):.2f} vs {statistics.mean(ser):.2f})")
print(f"parallel vs gs     (total): {tot_g/tot_p:.2f}x")
for lo, hi, label in [(0, 0, "0 imgs"), (1, 2, "1-2 imgs"), (3, 9, "3-9 imgs"), (10, 10**9, "10+ imgs")]:
    idx = [i for i, n in enumerate(imgs) if lo <= n <= hi]
    if not idx: continue
    tp = sum(par[i] for i in idx); ts = sum(ser[i] for i in idx)
    print(f"  {label:8s}: n={len(idx):3d}  parallel {tp:6.1f}s vs serial {ts:6.1f}s = {ts/max(tp,1e-9):.2f}x")
# biggest wins by image count
sp = [(ser[i]/par[i], rows[i]['pdf'], imgs[i]) for i in range(len(rows)) if par[i] > 0.01]
sp.sort(reverse=True)
print("biggest parallel wins (docs >10ms):")
for ratio, pdf, n in sp[:5]:
    print(f"  {ratio:5.2f}x  {pdf} ({n} imgs)")
ins = [int(r['in_bytes']) for r in rows]
outs = [int(r['out_parallel']) for r in rows]
red = [(1 - o/i) * 100 for i, o in zip(ins, outs)]
print(f"size reduction presse: mean={statistics.mean(red):.1f}% median={statistics.median(red):.1f}%")
