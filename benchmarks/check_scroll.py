#!/usr/bin/env python3
"""Compare --scroll-probe screenshots. Run with `uv run --with pillow ...`.
The native harness independently requires a replacement submission/presentation;
this script checks exact pixels, including the frame held during a blocked owner.
"""
import pathlib, sys
from PIL import Image
root = pathlib.Path(sys.argv[1])
assert (root/"presented.txt").is_file(), "native probe did not verify a replacement presentation"
if '--canvases' in sys.argv:
    names = ('before.png', 'horizontal.png', 'vertical.png', 'rebased.png', 'reversed.png', 'reversed-rebased.png')
    def boxes(name):
        alpha = Image.open(root/name).convert('RGBA').getchannel('A')
        occupied = [x for x in range(alpha.width) if alpha.crop((x, 0, x+1, alpha.height)).getbbox()]
        ranges = []
        for x in occupied:
            if not ranges or x != ranges[-1][1]: ranges.append([x, x+1])
            else: ranges[-1][1] += 1
        return [alpha.crop((a, 0, b, alpha.height)).getbbox() for a,b in ranges], ranges
    geometry = []
    for name in names:
        vertical, horizontal = boxes(name)
        assert len(horizontal) == 2, (name, horizontal)
        geometry.append([(a, v[1], b, v[3]) for (a,b),v in zip(horizontal,vertical)])
    before, horizontal, vertical, rebased, reversed_, reversed_rebased = geometry
    shift = lambda b,x,y: (b[0]+x,b[1]+y,b[2]+x,b[3]+y)
    assert horizontal == [shift(before[0],160,0), before[1]], geometry
    assert vertical == [shift(b,0,160) for b in horizontal], geometry
    assert rebased == vertical, geometry
    assert reversed_ == reversed_rebased == before, geometry
    # Owner-held camera affine must already match the subsequently rendered frame.
    camera = [boxes(name) for name in ('camera-held.png','camera-rebased.png','camera-reversed.png','camera-reversed-rebased.png')]
    assert camera[0] == camera[1], camera
    assert camera[2] == camera[3] == boxes('before.png'), camera
    assert camera[0] != boxes('before.png'), 'camera waited for GPU work'
    print('independent horizontal scrolling, shared vertical movement and reversal match:',geometry)
    raise SystemExit(0)
images = [Image.open(root/name).convert('RGBA') for name in ('before.png','translated.png','rebased.png')]
boxes = [im.getbbox() for im in images]
assert all(boxes), f'no visible preview pixels: {boxes}'
a,b,c = boxes
assert a[0] == b[0] and a[2] == b[2] and a[3]-a[1] == b[3]-b[1], boxes
# Fixture uses a backing scale of two: 200 logical points -> 400 captured pixels.
assert b[1]-a[1] == 400 and b[3]-a[3] == 400, boxes
assert b == c, f'GPU rebase moved the preview: {boxes}'
print('cached transaction translation and replacement bounds match:',boxes)
