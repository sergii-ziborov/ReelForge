"""Explicit TEST FIXTURE for local-process end-to-end checks; never a neural model."""
import argparse
import hashlib
import json
from pathlib import Path
from PIL import Image


def run(job, mode):
    if job["config"] != {"test_fixture": True}:
        raise ValueError("fixture cannot be used as an actual model")
    root = Path(job["directory"])
    path = root / "result.json"
    if mode == "preflight":
        path.write_text(json.dumps({"status": "ready"}))
        return
    if mode == "recover":
        path.write_text(json.dumps({"status": "unknown" if (root / "started.json").exists() else "not_submitted"}))
        return
    for receipt in job["anchors"].values():
        if hashlib.sha256(Path(receipt["path"]).read_bytes()).hexdigest() != receipt["sha256"]:
            raise ValueError("fixture received changed anchors")
    (root / "started.json").write_text(json.dumps({"request_id": job["request"]["id"]}))
    frames = {}
    for frame in job["request"]["indices"]:
        output = root / f"frame-{frame}.png"
        Image.new("RGB", (job["request"]["clock"]["width"], job["request"]["clock"]["height"]),
                  (frame % 256, frame // 256, 127)).save(output)
        frames[str(frame)] = {"path": str(output), "sha256": hashlib.sha256(output.read_bytes()).hexdigest()}
    path.write_text(json.dumps({"status": "completed", "frames": frames}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--mode", required=True)
    args = parser.parse_args()
    run(json.loads(Path(args.input).read_text(encoding="utf-8")), args.mode)
