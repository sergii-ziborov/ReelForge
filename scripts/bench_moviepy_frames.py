#!/usr/bin/env python3
"""Frame-graph microbenchmarks for MoviePy (comparable to reelforge-fx frame_ops)."""

from __future__ import annotations

import statistics
import time

def timed(name: str, fn, loops: int = 40, warmup: int = 5) -> float:
    for _ in range(warmup):
        fn()
    samples = []
    for _ in range(loops):
        t0 = time.perf_counter()
        fn()
        samples.append(time.perf_counter() - t0)
    mean = statistics.mean(samples)
    print(f"{name}: mean={mean*1e3:.3f} ms  ({1.0/mean:.1f} ops/s)  n={loops}")
    return mean


def main() -> None:
    from moviepy import ColorClip, CompositeVideoClip
    from moviepy.video.fx import Crop, Resize, FadeIn

    # 720p chain: color -> crop -> resize -> fade -> get_frame
    base = ColorClip(size=(1280, 720), color=(40, 80, 160), duration=5)
    chain = base.with_effects([Crop(x1=100, y1=50, width=960, height=540), Resize((640, 360)), FadeIn(0.5)])

    def chain_frame():
        frame = chain.get_frame(0.25)
        return frame.shape

    # Composite two layers
    a = ColorClip(size=(640, 360), color=(40, 80, 160), duration=2)
    b = ColorClip(size=(200, 100), color=(200, 40, 40), duration=2).with_position("center")
    comp = CompositeVideoClip([a, b], size=(640, 360))

    def comp_frame():
        frame = comp.get_frame(0.1)
        return frame.shape

    solid = ColorClip(size=(1920, 1080), color=(40, 80, 160), duration=1)

    def solid_frame():
        frame = solid.get_frame(0.0)
        return frame[0, 0, 0]

    print("MoviePy frame-graph benches")
    timed("720p_chain_frame_at", chain_frame)
    timed("composite_two_layers_frame_at", comp_frame)
    timed("1080p_color_frame", solid_frame)

    chain.close()
    comp.close()
    solid.close()
    base.close()
    a.close()
    b.close()


if __name__ == "__main__":
    main()
