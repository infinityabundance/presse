import csv, statistics
rows = list(csv.DictReader(open("/tmp/gpu-time/results.csv")))
print(f"{'doc':<34}{'in(MB)':>7}{'cpu(s)':>8}{'cuda(s)':>8}{'ratio':>7}{'cudaKB':>8}{'cpuKB':>8}")
for r in sorted(rows, key=lambda r: float(r['ratio'])):
    print(f"{r['pdf']:<34}{int(r['in_bytes'])/1e6:>7.1f}{r['cpu_s']:>8}{r['cuda_s']:>8}{r['ratio']:>7}{r['cuda_KB']:>8}{r['cpu_KB']:>8}")
cpus = [float(r['cpu_s']) for r in rows]
cudas = [float(r['cuda_s']) for r in rows]
print(f"\ntotal cpu={sum(cpus):.1f}s cuda={sum(cudas):.1f}s  ratio={sum(cpus)/sum(cudas):.2f}x")
print(f"mean cpu={statistics.mean(cpus):.3f} cuda={statistics.mean(cudas):.3f}")
