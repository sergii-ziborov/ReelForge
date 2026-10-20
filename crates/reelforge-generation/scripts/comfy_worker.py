"""Model-agnostic resident ComfyUI worker for the generation library protocol.

Use a trusted exported API workflow with SaveImage batch output. Local Qwen image
editing and temporal generators are separate declared capabilities; no name-based
guessing, silent workflow fallback, video retiming, or retry of prompt submission.
"""
import argparse
import hashlib
import json
import os
import time
import urllib.parse
import urllib.request
from pathlib import Path


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def save(path, data):
    tmp = path.with_suffix(".tmp")
    with tmp.open("w", encoding="utf-8") as file:
        json.dump(data, file, ensure_ascii=False)
        file.flush()
        os.fsync(file.fileno())
    tmp.replace(path)


def replace(value, bindings):
    if isinstance(value, dict):
        return {k: replace(v, bindings) for k, v in value.items()}
    if isinstance(value, list):
        return [replace(v, bindings) for v in value]
    if isinstance(value, str) and value.startswith("__RF_"):
        if value not in bindings:
            raise ValueError("unbound workflow input " + value)
        return bindings[value]
    return value


def placeholders(value):
    if isinstance(value, dict):
        return set().union(*(placeholders(v) for v in value.values()))
    if isinstance(value, list):
        return set().union(*(placeholders(v) for v in value))
    return {value} if isinstance(value, str) and value.startswith("__RF_") else set()


class Client:
    def __init__(self, endpoint):
        parsed = urllib.parse.urlparse(endpoint)
        if parsed.scheme != "http" or parsed.hostname not in ("127.0.0.1", "localhost", "::1") or parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ValueError("ComfyUI/Hetero media route must be loopback HTTP")
        self.endpoint = endpoint.rstrip("/")
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def http(self, route, body=None, content_type="application/json", limit=8 * 1024 * 1024):
        request = urllib.request.Request(self.endpoint + route, data=body, headers={"Content-Type": content_type})
        with self.opener.open(request, timeout=30) as response:
            raw = response.read(limit + 1)
        if len(raw) > limit:
            raise ValueError("provider response exceeds byte budget")
        return raw

    def json(self, route, body=None):
        return json.loads(self.http(route, None if body is None else json.dumps(body).encode()))

    def image(self, receipt):
        raw = Path(receipt["path"]).read_bytes()
        if len(raw) > 64 * 1024 * 1024 or digest(raw) != receipt["sha256"]:
            raise ValueError("conditioning image changed")
        name = "reelforge-" + digest(raw) + ".png"
        boundary = "reelforge-" + digest(raw)[:24]
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="image"; filename="{name}"\r\nContent-Type: image/png\r\n\r\n'.encode()
                + raw + f'\r\n--{boundary}--\r\n'.encode())
        output = json.loads(self.http("/upload/image", body, "multipart/form-data; boundary=" + boundary))
        return "/".join(filter(None, [output.get("subfolder", ""), output["name"]]))


def workflow_for(job):
    config, request = job["config"], job["request"]
    conditioned = bool(request["anchors"] or request["shot"]["references"])
    workflow = config.get("conditioned_workflow") if conditioned and request["kind"] == "keyframe" else config["workflow"]
    if workflow is None:
        raise ValueError("keyframe worker needs a conditioned_workflow to preserve canonical references")
    return workflow


def bindings_for(job, client=None):
    r = job["request"]
    indices, anchors = r["indices"], r["anchors"]
    origin = anchors[0] if r["kind"] == "between" else r["shot"]["start"]
    last = anchors[-1] if r["kind"] == "between" else r["shot"]["end"] - 1
    bindings = {"__RF_PROMPT__": r["shot"]["prompt"], "__RF_SEED__": r["seed"],
                "__RF_FRAMES__": r["model_frames"], "__RF_FPS__": r["clock"]["fps"],
                "__RF_WIDTH__": r["clock"]["width"], "__RF_HEIGHT__": r["clock"]["height"],
                "__RF_TARGET_FRAME__": indices[0], "__RF_TARGET_POSITION__": (indices[0] - origin) / max(1, last - origin),
                "__RF_MEMORY_JSON__": json.dumps(r["shot"]["continuity"], ensure_ascii=False),
                "__RF_CONDITIONS_JSON__": json.dumps(r["shot"]["conditions"], ensure_ascii=False)}
    for i, frame in enumerate(anchors):
        bindings[f"__RF_ANCHOR_{i}__"] = client.image(job["anchors"][str(frame)]) if client else "preflight-image.png"
        bindings[f"__RF_ANCHOR_{i}_POSITION__"] = frame - origin
    for i, reference in enumerate(r["shot"]["references"]):
        bindings[f"__RF_REFERENCE_{i}__"] = client.image(reference) if client else "preflight-reference.png"
    return bindings


def preflight(job, client):
    workflow = workflow_for(job)
    inputs = placeholders(workflow)
    required = {"__RF_PROMPT__", "__RF_SEED__", "__RF_WIDTH__", "__RF_HEIGHT__"}
    r = job["request"]
    if r["kind"] == "between":
        required |= {"__RF_ANCHOR_0__", "__RF_ANCHOR_1__"}
        if r["backend"]["kind"] == "temporal_generation":
            required |= {"__RF_FRAMES__", "__RF_ANCHOR_0_POSITION__", "__RF_ANCHOR_1_POSITION__"}
        elif r["backend"]["kind"] == "image_edit":
            required |= {"__RF_TARGET_POSITION__", "__RF_TARGET_FRAME__"}
        else:
            raise ValueError("interpolation/still generation cannot fill temporal content")
    elif r["anchors"]:
        required.add("__RF_ANCHOR_0__")
    if r["kind"] == "keyframe":
        required |= {"__RF_TARGET_POSITION__", "__RF_TARGET_FRAME__"}
    for i in range(len(r["shot"]["references"])):
        required.add(f"__RF_REFERENCE_{i}__")
    if r["shot"]["conditions"]:
        required.add("__RF_CONDITIONS_JSON__")
    if r["shot"]["continuity"]:
        required.add("__RF_MEMORY_JSON__")
    if required - inputs:
        raise ValueError("workflow ignores required conditioning: " + ", ".join(sorted(required - inputs)))
    replace(workflow, bindings_for(job))
    objects = client.json("/object_info")
    for node in workflow.values():
        if node["class_type"] not in objects:
            raise ValueError("unavailable workflow node: " + node["class_type"])
    if workflow[job["config"]["output_node"]]["class_type"] != "SaveImage":
        raise ValueError("generation requires indexed SaveImage PNG output; no video FPS conversion")
    return workflow


def run(job, mode):
    root = Path(job["directory"]).resolve()
    state_path, result_path = root / "comfy.json", root / "result.json"
    client = Client(job["config"]["endpoint"])
    if mode == "preflight":
        preflight(job, client)
        save(result_path, {"status": "ready"})
        return
    state = json.loads(state_path.read_text()) if state_path.exists() else None
    if state is None:
        if mode == "recover":
            save(result_path, {"status": "not_submitted"})
            return
        workflow = preflight(job, client)
        workflow = replace(workflow, bindings_for(job, client))
        state = {"phase": "submitting", "request_id": job["request"]["id"], "workflow_sha256": digest(json.dumps(workflow, sort_keys=True).encode())}
        save(state_path, state)
        # Never retry POST: a lost acknowledgement remains unknown.
        submitted = client.json("/prompt", {"prompt": workflow, "client_id": job["request"]["id"]})
        if not submitted.get("prompt_id") or submitted.get("node_errors"):
            raise ValueError("ComfyUI rejected generation request")
        state.update(phase="queued", prompt_id=submitted["prompt_id"])
        save(state_path, state)
    if state["request_id"] != job["request"]["id"]:
        raise ValueError("worker reservation identity changed")
    if state["phase"] == "submitting":
        save(result_path, {"status": "unknown"})
        return
    started = time.monotonic()
    while time.monotonic() - started < 3300:
        history = client.json("/history/" + urllib.parse.quote(state["prompt_id"], safe=""))
        item = history.get(state["prompt_id"])
        if item:
            status = item.get("status", {})
            if status.get("status_str") == "error":
                raise ValueError("ComfyUI model execution failed; original ID retained")
            if status.get("completed"):
                images = item["outputs"][job["config"]["output_node"]].get("images", [])
                r = job["request"]
                temporal = r["backend"]["kind"] == "temporal_generation"
                expected = r["model_frames"] if temporal else 1
                if len(images) != expected:
                    raise ValueError("provider changed frame count; no holds, interpolation or retiming")
                frames = {}
                for frame in r["indices"]:
                    position = frame - r["anchors"][0] if temporal else 0
                    image = images[position]
                    query = urllib.parse.urlencode({k: image[k] for k in ("filename", "subfolder", "type") if k in image})
                    raw = client.http("/view?" + query, limit=64 * 1024 * 1024)
                    path = root / f"frame-{frame:06d}.png"
                    path.write_bytes(raw)
                    frames[str(frame)] = {"path": str(path), "sha256": digest(raw)}
                save(result_path, {"status": "completed", "frames": frames})
                state["phase"] = "completed"
                save(state_path, state)
                return
        time.sleep(0.5)
    raise TimeoutError("ComfyUI request timed out; recover original ID without resubmission")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--mode", choices=("preflight", "generate", "recover"), required=True)
    args = parser.parse_args()
    run(json.loads(Path(args.input).read_text(encoding="utf-8")), args.mode)
