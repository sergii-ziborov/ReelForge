"""Real CLI/process/cache/encoder check against labelled synthetic fixtures.

Writes only to the explicitly supplied evaluation directory. This is not a
generated-neural-video demonstration or a visual-quality score.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path


def main(binary, root):
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    fixture = Path(__file__).with_name("process_fixture.py").resolve()
    providers = []
    for theme in ["keys", "water", "anime", "realism"]:
        providers.append({"backend": {"id": "fixture-" + theme, "fingerprint": "f" * 64,
                          "kind": "keyframe" if theme == "keys" else "temporal_generation",
                          "themes": [] if theme == "keys" else [theme], "available": True,
                          "ram_bytes": 1, "vram_bytes": 0, "rank": 0,
                          "constraints": {"max_frames": 81, "frame_multiple": 4, "frame_remainder": 1}},
                          "program": sys.executable, "args": [str(fixture)], "config": {"test_fixture": True},
                          "input_files": [str(fixture)], "timeout_secs": 15})
    shots = [{"id": theme, "start": i * 80, "end": (i + 1) * 80, "theme": theme,
              "prompt": "SYNTHETIC CONTRACT FIXTURE: " + theme, "continuity": {"location": theme},
              "references": [], "conditions": {}, "allow_still": False}
             for i, theme in enumerate(["water", "anime", "realism"])]
    job = {"spec": {"clock": {"fps": 24, "frames": 240, "width": 16, "height": 16}, "keyframe_count": 10,
                    "seed": 42, "shots": shots, "allow_image_edit_between": False},
           "resources": {"ram_bytes": 100, "vram_bytes": 0}, "providers": providers}
    path = root / "job.json"
    path.write_text(json.dumps(job), encoding="utf-8")

    def execute(name, video=False):
        args = [str(binary), "generate", str(path), "--run", "--store", str(root / "store"),
                "--receipt", str(root / (name + ".json"))]
        if video:
            args += ["--output", str(root / "fixture-clock.mp4")]
        result = subprocess.run(args, check=True, capture_output=True, text=True, timeout=120)
        return json.loads((root / (name + ".json")).read_text()), result.stdout.strip()

    first, _ = execute("first", True)
    cached, _ = execute("cached")
    assert (first["keyframes"], first["intermediate_frames"], len(first["frames"])) == (10, 230, 240)
    assert cached["submitted_requests"] == 0
    assert first["frames"] == cached["frames"]
    assert not first["naturalness_verified"]
    shots[1]["continuity"] = {"location": "anime", "remembered_event": "changed"}
    path.write_text(json.dumps(job), encoding="utf-8")
    edited, _ = execute("edited")
    assert 0 < edited["submitted_requests"] < first["submitted_requests"]
    probe = json.loads(subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0",
                                     "-show_entries", "stream=nb_read_frames,r_frame_rate:format=duration", "-of", "json",
                                     str(root / "fixture-clock.mp4")], check=True, capture_output=True).stdout)
    assert probe["streams"][0]["nb_read_frames"] == "240"
    assert probe["streams"][0]["r_frame_rate"] == "24/1"
    assert float(probe["format"]["duration"]) == 10
    proof = {"fixture_only": True, "neural_inference_verified": False, "themes": [s["theme"] for s in shots],
             "keyframes": first["keyframes"], "intermediates": first["intermediate_frames"],
             "initial_submissions": first["submitted_requests"], "cache_submissions": cached["submitted_requests"],
             "edited_shot_submissions": edited["submitted_requests"], "encoded_frames": 240, "fps": 24, "duration": 10}
    (root / "proof.json").write_text(json.dumps(proof, indent=2))
    print(json.dumps(proof))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    main(args.binary, args.output)
