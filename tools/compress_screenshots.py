"""Second pass: better palette, and a sharper look at how much changes.

The first pass quantised RGBA with the fast octree (low quality). This one checks
whether the alpha is actually used, flattens if not, and compares median-cut with
and without dithering, reporting the error distribution rather than just its mean.
"""
import os
import pathlib

from PIL import Image, ImageChops, ImageStat

BACKUP = pathlib.Path(os.environ['LOCALAPPDATA']) / 'Temp' / 'floaty-screenshots-original'
SCRATCH = pathlib.Path(os.environ['LOCALAPPDATA']) / 'Temp' / 'floaty-shots-work2'
SCRATCH.mkdir(parents=True, exist_ok=True)


def kb(p):
    return p.stat().st_size / 1024


def stats(orig, cand):
    """worst channel delta, mean, 99th percentile, share of pixels off by >8."""
    diff = ImageChops.difference(orig.convert('RGB'), cand.convert('RGB'))
    worst = max(ch[1] for ch in diff.getextrema())
    mean = sum(ImageStat.Stat(diff).mean) / 3
    hist = diff.convert('L').histogram()
    total = sum(hist)
    running = 0
    p99 = 0
    for level, count in enumerate(hist):
        running += count
        if running >= total * 0.99:
            p99 = level
            break
    big = sum(hist[9:]) / total * 100
    return worst, mean, p99, big


for name in ['palette.png', 'settings-general.png']:
    src = BACKUP / name
    im = Image.open(src)
    im.load()
    alpha = im.convert('RGBA').getchannel('A').getextrema()
    opaque = alpha == (255, 255)
    print(f'\n{name}  {im.size[0]}x{im.size[1]} {im.mode}  {kb(src):.0f} KB  '
          f'alpha range {alpha} {"(opaque)" if opaque else "(has transparency)"}')

    base = im.convert('RGB') if opaque else im.convert('RGBA')
    cands = {}

    flat = SCRATCH / f'{name}-rgb-lossless.png'
    base.save(flat, format='PNG', optimize=True, compress_level=9)
    cands['rgb lossless'] = flat

    if opaque:
        q = base.quantize(colors=256, method=Image.MEDIANCUT)
        p1 = SCRATCH / f'{name}-q256-median.png'
        q.save(p1, format='PNG', optimize=True)
        cands['q256 median-cut'] = p1

        p2 = SCRATCH / f'{name}-q256-median-dither.png'
        base.quantize(colors=256, method=Image.MEDIANCUT,
                      dither=Image.Dither.FLOYDSTEINBERG).save(p2, format='PNG', optimize=True)
        cands['q256 median + dither'] = p2

    best = None
    for label, path in cands.items():
        cand = Image.open(path)
        cand.load()
        worst, mean, p99, big = stats(base, cand)
        print(f'  {label:<24} {kb(path):>7.1f} KB   worst {worst:>3}  mean {mean:>5.2f}  '
              f'p99 {p99:>3}  pixels off by >8: {big:>4.1f}%')
        if label.startswith('q256') and mean < 2.5 and (best is None or kb(path) < kb(best[1])):
            best = (label, path)

    if best:
        target = pathlib.Path('screenshots') / name
        before = kb(target)
        import shutil
        shutil.copy2(best[1], target)
        print(f'  -> shipped "{best[0]}": {before:.0f} KB -> {kb(target):.0f} KB '
              f'({100 - kb(target) / before * 100:.0f}% smaller than the lossless pass)')
    else:
        print('  -> kept the lossless version (no quantised candidate was clean enough)')
