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
    s = ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2))
    return s

pre = np.asarray(Image.open("/tmp/plans-pre-1.png").convert("RGB"))
fixed = np.asarray(Image.open("/tmp/plans-fixed-1.png").convert("RGB"))
cpu = np.asarray(Image.open("/tmp/plans-cpu2-1.png").convert("RGB"))
print(f"pre vs cuda-fixed: SSIM={fast_ssim(pre, fixed):.4f} MAE={np.abs(pre.astype(np.int32)-fixed.astype(np.int32)).mean():.3f}")
print(f"pre vs cpu      : SSIM={fast_ssim(pre, cpu):.4f} MAE={np.abs(pre.astype(np.int32)-cpu.astype(np.int32)).mean():.3f}")
print(f"cuda-fixed vs cpu: SSIM={fast_ssim(fixed, cpu):.4f} MAE={np.abs(fixed.astype(np.int32)-cpu.astype(np.int32)).mean():.3f}")
