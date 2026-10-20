# ReelForge Generation

MIT library for an arbitrary multi-shot generation, rather than a collection of
pre-rendered character animations. Model weights, runtimes and their licenses are
separate dependencies; no neural model is bundled.

The caller describes shots, exact output clock, sparse keyframe budget, theme,
scene/identity memory, owned image references and per-frame control conditions.
The engine routes those requests to connected providers, generates keyframes,
fills every interior frame, retains original anchors and exposes a `VideoClip`
for ReelForge effects, composition and audio. Providers can be hosted tools,
local inference processes or resident servers.

```rust,no_run
use reelforge::generation::{Engine, GeneratedClip, Provider};
use std::collections::BTreeMap;
# fn example(spec: reelforge::generation::GenerationSpec,
# resources: reelforge::generation::Resources,
# mut providers: BTreeMap<String, Box<dyn Provider>>) -> Result<(), Box<dyn std::error::Error>> {
let engine = Engine::open("owned-generation-store")?;
let receipt = engine.run(spec, &resources, &mut providers)?;
let clip = GeneratedClip::open(engine.store().clone(), &receipt)?;
// clip implements ReelForge's existing VideoClip contract.
# Ok(()) }
```

For a 240-frame sequence, ten keyframes leave 230 frames for generation. All
requests stay inside their shot; scene cuts are explicit. Later keyframes use the
shot's first generated image plus canonical references. Editing a shot's memory,
conditions or model/configuration fingerprint changes its cache identity.
Changing another shot does not invalidate untouched work.

Temporal generation is preferred. `ImageEdit` is an explicit experimental option
that receives both endpoints and an exact target frame. `Interpolation` (including
optical flow/RIFE) cannot satisfy a request for new visual content. Increasing FPS
does not qualify repeated images as generated motion. A static shot requires
`allow_still`. Clock/pixel checks do **not** certify natural movement or anatomy.

## Execute from the CLI

`reelforge generate job.json` explains the complete plan without inference.

`reelforge generate job.json --run --store owned-store --receipt result.json --output result.mp4`

`job.json` contains `spec: GenerationSpec`, available `resources: Resources`, and
`providers: ProcessConfig[]`. Use absolute runtime/script/weight paths. The caller
supplies available memory after other jobs' reservations; estimates in a backend
descriptor are admission requirements, not measured performance claims.

Process providers receive `--input input.json --mode preflight|generate|recover`.
The typed input contains the request, immutable anchor receipts, configuration
snapshot and owned working directory. The worker writes `result.json`:

```json
{"status":"completed","frames":{"12":{"path":"/owned/job/frame-000012.png","sha256":"<exact-PNG-SHA256>"}}}
```

Preflight returns `{"status":"ready"}` without loading weights or submitting a
job. Recovery returns completed frames, `not_submitted` only when that is proven,
or `unknown`. The engine reserves requests before generation and refuses duplicate
submission after an unknown outcome. Cache/source tampering, missing indices,
changed canvas and replacement of anchors are errors, not reasons to pad output.
Worker timeout/log budgets and per-request locks bound execution. Imported PNGs
are immutable content-addressed artifacts, independent of model temporary files.

## Included optional local workers

- `scripts/comfy_worker.py` connects to a configured loopback ComfyUI or Hetero
  media route using an exported **API** workflow. It supports resident generators
  with indexed `SaveImage` batch output, and local Qwen image editing as a separate
  capability. It does not install models, invent node names or retry a lost POST.
- `scripts/vace_worker.py` runs an installed local Diffusers `WanVACEPipeline`
  checkpoint offline. Sparse endpoint masks use actual frame positions, including
  unused model padding. CUDA is explicit; slow CPU use requires an explicit flag.
  Pose/depth/sketch controls use `control_reference`/`mask_reference` indices into
  owned reference images. Control maps are excluded from subject references.
  A compact `continuity.visual_description` supplements the prompt; canonical
  appearance images remain pixel conditioning. Other graph geometry must be
  rendered into control images before this adapter can consume it.

For ComfyUI set configuration `endpoint`, `workflow`, `output_node`, and a
`conditioned_workflow` for keyframes with earlier/canonical reference images.
Workflow placeholders use whole JSON values: `__RF_PROMPT__`, `__RF_SEED__`,
`__RF_WIDTH__`, `__RF_HEIGHT__`, `__RF_FRAMES__`, `__RF_FPS__`,
`__RF_TARGET_FRAME__`, `__RF_TARGET_POSITION__`, `__RF_ANCHOR_0__`,
`__RF_ANCHOR_1__`, `__RF_ANCHOR_0_POSITION__`, `__RF_ANCHOR_1_POSITION__`,
`__RF_REFERENCE_0__` etc., `__RF_MEMORY_JSON__` and `__RF_CONDITIONS_JSON__`.
Required conditioning must be consumed; missing inputs fail preflight. Temporal
output must contain the entire native model window; only requested interior
indices are imported. Image editing outputs exactly one target image per request.

For VACE set `model_path`, `device`, `dtype`, `steps`, `guidance_scale`, optionally
`flow_shift`, `negative_prompt`, `allow_slow_cpu`. Register native constraints
`frame_multiple: 4`, `frame_remainder: 1`, `max_frames: 81`; canvas multiples of 16.
The worker loads existing weights with `local_files_only=True`. It cannot make a
text-only Ollama/Qwen model generate frames.

The Rust fixtures verify scheduling, storage, cache recovery, multiple themes and
native frame clocks. Python HTTP/mask fixtures verify worker protocols without
loading a neural model. Neither set is evidence of rendered visual quality.
