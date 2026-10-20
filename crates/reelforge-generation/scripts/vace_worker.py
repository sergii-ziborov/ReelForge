"""Local sparse-frame inference adapter for Diffusers WanVACEPipeline.

No download or chat-model fallback. Core library/worker code is MIT; model and
Diffusers licenses are independent. Requires an isolated user-configured runtime.
The driver fills only missing indices; native library preserves original anchors.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path


def digest(data):
    return hashlib.sha256(data).hexdigest()


def save(path, data):
    tmp = path.with_suffix(".tmp")
    with tmp.open("w", encoding="utf-8") as stream:
        json.dump(data, stream, ensure_ascii=False)
        stream.flush()
        os.fsync(stream.fileno())
    tmp.replace(path)


def preflight(job):
    request, config = job["request"], job["config"]
    if request["backend"]["kind"] != "temporal_generation" or request["kind"] != "between":
        raise ValueError("VACE worker requires temporal intermediate generation")
    model = Path(config["model_path"])
    if not model.is_absolute() or not model.is_dir() or not (model / "model_index.json").is_file():
        raise ValueError("configure an existing local Diffusers VACE checkpoint; no automatic download")
    if json.loads((model / "model_index.json").read_text())["_class_name"] != "WanVACEPipeline":
        raise ValueError("checkpoint is not WanVACEPipeline")
    if request["model_frames"] % 4 != 1 or request["model_frames"] > 81:
        raise ValueError("VACE adapter supports 4n+1 windows up to 81 frames")
    clock = request["clock"]
    if clock["width"] % 16 or clock["height"] % 16:
        raise ValueError("VACE canvas must be divisible by 16; no silent resizing")
    if config.get("device", "cuda") not in ("cuda", "cpu"):
        raise ValueError("VACE adapter supports explicitly configured CUDA or CPU")
    if config.get("device", "cuda") == "cpu" and not config.get("allow_slow_cpu", False):
        raise ValueError("CPU inference must be explicitly admitted with allow_slow_cpu")
    if not 1 <= config.get("steps", 30) <= 100:
        raise ValueError("steps must be 1..100")
    if config.get("dtype", "bfloat16") not in ("float16", "bfloat16", "float32"):
        raise ValueError("unsupported precision")
    memory = request["shot"]["continuity"]
    memory_text = memory.get("visual_description", "") if isinstance(memory, dict) else ""
    if not isinstance(memory_text, str):
        raise ValueError("continuity.visual_description must be text")
    if len(request["shot"]["prompt"]) + len(memory_text) > 4096:
        raise ValueError("VACE prompt/memory description exceeds adapter context budget")
    # Dense pose/depth/sketch conditions arrive as owned indexed reference images.
    # Other conditions are not silently converted into guessed geometry.
    for condition in request["shot"]["conditions"].values():
        if not isinstance(condition, dict) or set(condition) - {"control_reference", "mask_reference"}:
            raise ValueError("VACE conditions must use control_reference/mask_reference; render graph geometry to owned images first")
        for index in condition.values():
            if type(index) is not int or not 0 <= index < len(request["shot"]["references"]):
                raise ValueError("control reference outside owned reference list")
    return model


def checked_image(receipt):
    from PIL import Image
    raw = Path(receipt["path"]).read_bytes()
    if len(raw) > 64 * 1024 * 1024 or digest(raw) != receipt["sha256"]:
        raise ValueError("conditioning image changed")
    with Image.open(receipt["path"]) as image:
        if image.format != "PNG" or image.size != (receipt["width"], receipt["height"]):
            raise ValueError("invalid conditioning image")
        return image.convert("RGB")


def prepare_inputs(job):
    """Anchor/mask placement uses source indices, including padded unused tail."""
    from PIL import Image
    request = job["request"]
    size = (request["clock"]["width"], request["clock"]["height"])
    indices = request["anchors"]
    if len(indices) != 2 or indices[1] <= indices[0]:
        raise ValueError("two ordered immutable anchors are required")
    if sorted(map(int, job["anchors"])) != indices:
        raise ValueError("anchor images do not match compiled request")
    start, end = indices
    n = request["model_frames"]
    if n < end - start + 1:
        raise ValueError("model window cannot cover the requested interval")
    frames = [Image.new("RGB", size, (128, 128, 128)) for _ in range(n)]
    masks = [Image.new("L", size, 255) for _ in range(n)]
    references = [checked_image(r) for r in request["shot"]["references"]]
    for frame, condition in request["shot"]["conditions"].items():
        local = int(frame) - start
        if 0 <= local < n:
            if "control_reference" in condition:
                image = references[condition["control_reference"]]
                if image.size != size:
                    raise ValueError("control canvas differs; no automatic distortion")
                frames[local] = image.copy()
            if "mask_reference" in condition:
                image = references[condition["mask_reference"]]
                if image.size != size:
                    raise ValueError("mask canvas differs")
                masks[local] = image.convert("L")
    for frame in indices:
        image = checked_image(job["anchors"][str(frame)])
        if image.size != size:
            raise ValueError("anchor canvas differs")
        frames[frame - start] = image
        masks[frame - start] = Image.new("L", size, 0)
    # Padded tail is conditioning context, never stretched into displayed frames.
    for i in range(end - start + 1, n):
        frames[i] = frames[end - start].copy()
        masks[i] = Image.new("L", size, 0)
    controls = {i for c in request["shot"]["conditions"].values() for i in c.values()}
    return frames, masks, [image for i, image in enumerate(references) if i not in controls]


def run(job, mode):
    root = Path(job["directory"]).resolve()
    state_path, result_path = root / "inference.json", root / "result.json"
    if mode == "preflight":
        preflight(job)
        save(result_path, {"status": "ready"})
        return
    if mode == "recover":
        if result_path.is_file():
            return
        save(result_path, {"status": "unknown" if state_path.exists() else "not_submitted"})
        return
    if state_path.exists():
        raise ValueError("reserved local inference already started; recover without duplicate inference")
    model = preflight(job)
    frames, masks, references = prepare_inputs(job)
    # No implicit hub access, even if local configs refer to optional components.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    import torch
    from diffusers import AutoencoderKLWan, WanVACEPipeline
    from diffusers.schedulers import UniPCMultistepScheduler
    config, request = job["config"], job["request"]
    device = config.get("device", "cuda")
    if device == "cuda":
        if not torch.cuda.is_available():
            raise ValueError("configured CUDA accelerator is unavailable; no CPU fallback")
        available, _ = torch.cuda.mem_get_info()
        if available < request["backend"]["vram_bytes"]:
            raise ValueError("accelerator memory admission changed before inference")
    dtype = getattr(torch, config.get("dtype", "bfloat16"))
    save(state_path, {"phase": "loading", "request_id": request["id"], "model_path": str(model)})
    vae = AutoencoderKLWan.from_pretrained(str(model), subfolder="vae", torch_dtype=torch.float32, local_files_only=True)
    pipe = WanVACEPipeline.from_pretrained(str(model), vae=vae, torch_dtype=dtype, local_files_only=True)
    pipe.scheduler = UniPCMultistepScheduler.from_config(pipe.scheduler.config, flow_shift=config.get("flow_shift", 3.0))
    pipe.to(device)
    pipe.vae.enable_tiling()
    save(state_path, {"phase": "inference", "request_id": request["id"]})
    output = pipe(video=frames, mask=masks, reference_images=references or None,
                  prompt=request["shot"]["prompt"] + "\n" + (request["shot"]["continuity"].get("visual_description", "") if isinstance(request["shot"]["continuity"], dict) else ""),
                  negative_prompt=config.get("negative_prompt", ""),
                  height=request["clock"]["height"], width=request["clock"]["width"],
                  num_frames=request["model_frames"], num_inference_steps=config.get("steps", 30),
                  guidance_scale=config.get("guidance_scale", 5.0),
                  generator=torch.Generator(device=device).manual_seed(request["seed"]), output_type="pil").frames[0]
    if len(output) != request["model_frames"]:
        raise ValueError("model changed frame window; refusing retiming or duplicate padding")
    indexed = {}
    for frame in request["indices"]:
        image = output[frame - request["anchors"][0]]
        if image.size != (request["clock"]["width"], request["clock"]["height"]):
            raise ValueError("model changed output canvas")
        path = root / f"frame-{frame:06d}.png"
        image.save(path, format="PNG")
        indexed[str(frame)] = {"path": str(path), "sha256": digest(path.read_bytes())}
    save(result_path, {"status": "completed", "frames": indexed})
    save(state_path, {"phase": "completed", "request_id": request["id"]})


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--mode", choices=("preflight", "generate", "recover"), required=True)
    args = parser.parse_args()
    job = json.loads(Path(args.input).read_text(encoding="utf-8"))
    run(job, args.mode)
