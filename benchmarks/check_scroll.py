#!/usr/bin/env python3
"""Compare --scroll-probe screenshots. Run with `uv run --with pillow ...`.
The native harness independently requires a replacement submission/presentation;
this script checks exact pixels, including the frame held during a blocked owner.
"""
import pathlib, sys
from PIL import Image
root = pathlib.Path(sys.argv[1])
assert (root/"presented.txt").is_file(), "native probe did not verify a replacement presentation"
images = [Image.open(root/name).convert('RGBA') for name in ('before.png','translated.png','rebased.png')]
boxes = [im.getbbox() for im in images]
assert all(boxes), f'no visible preview pixels: {boxes}'
a,b,c = boxes
assert a[0] == b[0] and a[2] == b[2] and a[3]-a[1] == b[3]-b[1], boxes
# Fixture uses a backing scale of two: 200 logical points -> 400 captured pixels.
assert b[1]-a[1] == 400 and b[3]-a[3] == 400, boxes
assert b == c, f'GPU rebase moved the preview: {boxes}'
print('cached transaction translation and replacement bounds match:',boxes)
