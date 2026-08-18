import numpy as np
from PIL import Image

def ssim(a, b, win=11, c1=0.01**2, c2=0.03**2):
    a = a.astype(np.float64)
    b = b.astype(np.float64)
    # gaussian window
    x = np.arange(win) - win // 2
    g = np.exp(-(x**2) / (2 * 1.5**2))
    g2d = np.outer(g, g)
    g2d /= g2d.sum()
    pad = win // 2
    # pad via reflect
    pa = np.pad(a, ((pad, pad), (pad, pad)), mode="reflect")
    pb = np.pad(b, ((pad, pad), (pad, pad)), mode="reflect")
    def conv(x):
        out = np.zeros_like(x)
        for i in range(x.shape[0] - win + 1):
            for j in range(x.shape[1] - win + 1):
                out[i, j] = (x[i:i+win, j:j+win] * g2d).sum()
        return out
    mu_a = conv(pa)[pad:pad+a.shape[0], pad:pad+a.shape[1]]
    mu_b = conv(pb)[pad:pad+b.shape[0], pad:pad+b.shape[1]]
    mu_a2, mu_b2, mu_ab = mu_a**2, mu_b**2, mu_a*mu_b
    va = conv(pa**2)[pad:pad+a.shape[0], pad:pad+a.shape[1]] - mu_a2
    vb = conv(pb**2)[pad:pad+b.shape[0], pad:pad+b.shape[1]] - mu_b2
    vab = conv(pa*pb)[pad:pad+a.shape[0], pad:pad+b.shape[1]] - mu_ab
    s = ((2*mu_ab + c1)*(2*vab + c2)) / ((mu_a2 + mu_b2 + c1)*(va + vb + c2))
    return s.mean()

def load(p, size=None):
    im = Image.open(p).convert("RGB")
    if size:
        im = im.resize(size, Image.LANCZOS)
    return np.asarray(im)

orig = load("/tmp/big-photo.jpg", (400, 300))
gpu = load("/tmp/gpu-encoded.jpg", (400, 300))
# CPU re-encode at q50 for comparison baseline
cpu = load("/tmp/cpu-q50.jpg", (400, 300)) if __import__("os").path.exists("/tmp/cpu-q50.jpg") else None

for name, arr in [("gpu", gpu)] + ([("cpu", cpu)] if cpu is not None else []):
    chans = []
    for c in range(3):
        chans.append(ssim(orig[:, :, c], arr[:, :, c]))
    print(f"ssim vs orig ({name}): r={chans[0]:.4f} g={chans[1]:.4f} b={chans[2]:.4f} mean={np.mean(chans):.4f}")
    mae = np.abs(orig.astype(np.int32) - arr.astype(np.int32)).mean()
    print(f"  mae={mae:.2f}")
