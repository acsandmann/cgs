#!/usr/bin/env python3
"""Compare --scroll-probe screenshots. Run with `uv run --with pillow ...`.
The native harness independently requires a replacement submission/presentation;
this script checks exact pixels, including the frame held during a blocked owner.
"""
import pathlib, sys
from PIL import Image
root = pathlib.Path(sys.argv[1])
assert (root/"presented.txt").is_file(), "native probe did not verify a replacement presentation"
if '--surfaces' in sys.argv:
    def bodies(name):
        image = Image.open(root/name).convert('RGBA')
        alpha = image.getchannel('A').point(lambda a: 255 if a > 128 else 0)
        occupied = [x for x in range(alpha.width) if alpha.crop((x,0,x+1,alpha.height)).getbbox()]
        ranges = []
        for x in occupied:
            if not ranges or x != ranges[-1][1]: ranges.append([x,x+1])
            else: ranges[-1][1] += 1
        return [(a, alpha.crop((a,0,b,alpha.height)).getbbox()[1], b,
                 alpha.crop((a,0,b,alpha.height)).getbbox()[3]) for a,b in ranges]
    source = Image.open(root/'source.png').convert('RGB')
    preview = Image.open(root/'before.png').convert('RGB')
    native_color = source.getpixel((int(source.width*.8),source.height//2))
    preview_color = preview.getpixel((504,480))
    assert max(abs(a-b) for a,b in zip(native_color,preview_color)) <= 2, (native_color,preview_color)
    before = bodies('before.png')
    assert len(before) == 5, before  # Includes fully parked and hidden native sources.
    for name, dx, dy, scale in [('before.png',0,0,1), ('translated.png',160,0,1),
                                ('scaled.png',160,0,.5), ('reversed.png',0,0,1)]:
        actual = bodies(name)
        expected = [(int((120+i*520)*scale+dx), int(200*scale+dy+120),
                     int((600+i*520)*scale+dx), int(520*scale+dy+120)) for i in range(5)]
        assert len(actual) == len(expected), (name, actual)
        assert all(abs(a-b) <= 1 for box,want in zip(actual,expected) for a,b in zip(box,want)), (name,actual,expected)
        image = Image.open(root/name).convert('RGBA')
        for x,y,right,bottom in actual:
            r,g,b,a = image.getpixel((int(x+(right-x)*.8), (y+bottom)//2))
            assert a > 240 and g > r*1.5 and g > b*1.5, (name,r,g,b,a)
    assert bodies('clipped.png') == [(220,220,460,380),(480,220,720,380),(740,220,880,380)], bodies('clipped.png')
    assert bodies('cleared.png') == [], 'cleared capture layers still display source pixels'
    print('hidden IOSurface previews, stopped-capture transforms, reversal, clipping and clear passed')
    raise SystemExit(0)
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
