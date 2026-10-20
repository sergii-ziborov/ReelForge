"""Protocol/control-mask tests with fake HTTP model output; no neural quality claim."""
import copy
import importlib.util
import io
import json
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from PIL import Image


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


comfy, vace = load("comfy_worker"), load("vace_worker")


def image_receipt(root, value):
    path = root / f"image-{value}.png"
    Image.new("RGB", (16, 16), (value, 0, 0)).save(path)
    return {"path": str(path), "sha256": comfy.digest(path.read_bytes()), "width": 16, "height": 16}


def request(root):
    return {"directory": str(root), "request": {
        "id": "a" * 64, "backend": {"kind": "temporal_generation"}, "kind": "between",
        "anchors": [20, 25], "indices": [21, 22, 23, 24], "model_frames": 9, "seed": 10,
        "clock": {"width": 16, "height": 16, "fps": 24},
        "shot": {"prompt": "An arbitrary changing scene", "start": 0, "end": 40,
                 "conditions": {}, "continuity": {}, "references": []}},
        "anchors": {"20": image_receipt(root, 20), "25": image_receipt(root, 25)}, "config": {}}


def workflow():
    return {"1": {"class_type": "FixtureInputs", "inputs": {
        "prompt": "__RF_PROMPT__", "seed": "__RF_SEED__", "width": "__RF_WIDTH__", "height": "__RF_HEIGHT__",
        "frames": "__RF_FRAMES__", "a": "__RF_ANCHOR_0__", "b": "__RF_ANCHOR_1__",
        "a_position": "__RF_ANCHOR_0_POSITION__", "b_position": "__RF_ANCHOR_1_POSITION__"}},
        "2": {"class_type": "SaveImage", "inputs": {}}}


class FakeComfy:
    def __init__(self):
        self.submissions = 0
        self.fail_view = False
        self.count = 9
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def send(self, data, code=200):
                self.send_response(code)
                self.end_headers()
                self.wfile.write(data if isinstance(data, bytes) else json.dumps(data).encode())

            def do_GET(self):
                if self.path == "/object_info":
                    self.send({"FixtureInputs": {}, "SaveImage": {}})
                elif self.path.startswith("/history/"):
                    self.send({"original": {"status": {"completed": True}, "outputs": {"2": {"images": [
                        {"filename": f"{i}.png", "subfolder": "", "type": "output"} for i in range(owner.count)]}}}})
                elif self.path.startswith("/view?"):
                    if owner.fail_view:
                        owner.fail_view = False
                        self.send(b"fixture transport failure", 503)
                        return
                    buffer = io.BytesIO()
                    Image.new("RGB", (16, 16), (120, 20, 0)).save(buffer, format="PNG")
                    self.send(buffer.getvalue())
                else:
                    self.send(b"unknown", 404)

            def do_POST(self):
                raw = self.rfile.read(int(self.headers["Content-Length"]))
                if self.path == "/upload/image":
                    self.send({"name": "owned.png", "subfolder": ""})
                elif self.path == "/prompt":
                    owner.submissions += 1
                    owner.count = json.loads(raw)["prompt"]["1"]["inputs"]["frames"]
                    self.send({"prompt_id": "original"})
                else:
                    self.send(b"unknown", 404)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class WorkerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()

    def tearDown(self):
        self.temp.cleanup()

    def test_vace_sparse_anchors_are_at_real_indices_not_padded_endpoint(self):
        job = request(self.root)
        frames, masks, _ = vace.prepare_inputs(job)
        self.assertEqual(len(frames), 9)
        self.assertEqual([m.getpixel((0, 0)) for m in masks], [0, 255, 255, 255, 255, 0, 0, 0, 0])
        self.assertEqual(frames[0].getpixel((0, 0))[0], 20)
        self.assertEqual(frames[5].getpixel((0, 0))[0], 25)
        self.assertEqual(frames[8].getpixel((0, 0))[0], 25)

    def test_vace_control_references_are_not_mistaken_for_character_images(self):
        job = request(self.root)
        job["request"]["shot"]["references"] = [image_receipt(self.root, 40), image_receipt(self.root, 50)]
        job["request"]["shot"]["conditions"] = {"22": {"control_reference": 0}}
        frames, _, references = vace.prepare_inputs(job)
        self.assertEqual(frames[2].getpixel((0, 0))[0], 40)
        self.assertEqual(len(references), 1)
        self.assertEqual(references[0].getpixel((0, 0))[0], 50)

    def test_keyframe_time_is_relative_to_shot_and_does_not_always_equal_zero(self):
        job = request(self.root)
        job["request"].update(kind="keyframe", indices=[20], anchors=[], model_frames=1)
        bindings = comfy.bindings_for(job)
        self.assertAlmostEqual(bindings["__RF_TARGET_POSITION__"], 20 / 39)

    def test_local_only_and_unbound_inputs_fail(self):
        for endpoint in ("https://example.com", "http://example.com", "http://user:pass@localhost:8188"):
            with self.assertRaises(ValueError):
                comfy.Client(endpoint)
        with self.assertRaises(ValueError):
            comfy.replace({"reference": "__RF_REFERENCE_99__"}, {})

    def test_preflight_does_not_submit_and_recovery_reuses_original_job(self):
        server = FakeComfy()
        try:
            job = request(self.root)
            job["config"] = {"endpoint": server.endpoint, "workflow": workflow(), "output_node": "2"}
            comfy.run(job, "preflight")
            self.assertEqual(server.submissions, 0)
            (self.root / "result.json").unlink()
            server.fail_view = True
            with self.assertRaises(Exception):
                comfy.run(job, "generate")
            self.assertEqual(server.submissions, 1)
            comfy.run(job, "recover")
            self.assertEqual(server.submissions, 1)
            result = json.loads((self.root / "result.json").read_text())
            self.assertEqual(sorted(map(int, result["frames"])), [21, 22, 23, 24])
        finally:
            server.close()

    def test_unknown_submission_is_never_reposted(self):
        server = FakeComfy()
        try:
            job = request(self.root)
            job["config"] = {"endpoint": server.endpoint, "workflow": workflow(), "output_node": "2"}
            comfy.save(self.root / "comfy.json", {"phase": "submitting", "request_id": job["request"]["id"]})
            comfy.run(job, "recover")
            self.assertEqual(server.submissions, 0)
            self.assertEqual(json.loads((self.root / "result.json").read_text())["status"], "unknown")
        finally:
            server.close()

    def test_workflow_must_consume_temporal_conditions_and_memory(self):
        server = FakeComfy()
        try:
            job = request(self.root)
            job["config"] = {"endpoint": server.endpoint, "workflow": workflow(), "output_node": "2"}
            job["request"]["shot"]["continuity"] = {"jacket": "blue"}
            with self.assertRaisesRegex(ValueError, "conditioning"):
                comfy.preflight(job, comfy.Client(server.endpoint))
            self.assertEqual(server.submissions, 0)
        finally:
            server.close()


if __name__ == "__main__":
    unittest.main()
