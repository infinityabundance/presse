import numpy as np
from PIL import Image

def fast_ssim(pre, post):
    a = np.asarray(Image.fromarray(pre).convert("L").resize((64, 64), Image.BILINEAR), dtype=np.float64)
    b = np.asarray(Image.fromarray(post).convert("L").resize((64, 64), Image.BILINEAR), dtype=np.float64)
    n = a.size
    ma, mb = a.mean(), b.mean()
    va = ((a - ma) ** 2).sum() / n
    vb = ((b - mb) ** 2).sum() / n
    cov = ((a - ma) * (b - mb)).sum() / n
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    return ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2))

worst_g, worst_c = 1.0, 1.0
for i in range(1, 21):
    p = f"{i:02d}"
    pre = np.asarray(Image.open(f"/tmp/pg-pre-{p}.png").convert("RGB"))
    cuda = np.asarray(Image.open(f"/tmp/pg-cuda-{p}.png").convert("RGB"))
    cpu = np.asarray(Image.open(f"/tmp/pg-cpu-{p}.png").convert("RGB"))
    sg = fast_ssim(pre, cuda)
    sc = fast_ssim(pre, cpu)
    worst_g = min(worst_g, sg)
    worst_c = min(worst_c, sc)
    mae_g = np.abs(pre.astype(np.int32) - cuda.astype(np.int32)).mean()
    mae_c = np.abs(pre.astype(np.int32) - cpu.astype(np.int32)).mean()
    print(f"p{p}: cuda SSIM={sg:.4f} MAE={mae_g:.2f} | cpu SSIM={sc:.4f} MAE={mae_c:.2f}")
print(f"worst: cuda={worst_g:.4f} cpu={worst_c:.4f}")
