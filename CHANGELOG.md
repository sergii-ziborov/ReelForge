# Changelog

## 0.2.0 — 2026-08-15

Breaking release. **Do not treat crates.io `0.1.5` and this tree as the same API.**

After `0.1.5` was published, required fields were added to public structs
(notably `OperationDescriptor.executor_kind`) while the workspace version
stayed `0.1.5`. Intelligence built against crates.io `0.1.5` could not switch
to current `main`. That is the reason for `0.2.0`.

### Breaking

- Workspace version is `0.2.0` for every crate, including
  `reelforge-project`, `reelforge-render-graph`, and
  `reelforge-sightloom-adapter`.
- `OperationDescriptor` is `#[non_exhaustive]` and includes
  `executor_kind`. Construct with `OperationDescriptor::new` (or
  `unary` / `nary`). Struct literals from other crates no longer compile.
- `MediaContract`, `CapabilitySet`, and `OperationLimits` are
  `#[non_exhaustive]`. Use their constructors / `Default`.
- JSON that omits `executor_kind` still deserializes (`Unary` by default).
  Only Rust struct literals were incompatible.

### Added (stable construction)

- `OperationDescriptor::new` / `unary` / `nary` plus
  `with_executor_kind`, `with_capabilities`, `with_parameter_schema`,
  `with_limits`, `with_deterministic`.
- `MediaContract::with_notes` / `with_masks`.
- `CapabilitySet::from_tags`.
- `OperationLimits::new`.

### Added (mask package security)

- Path confinement + canonicalize: blob paths must stay inside the package.
- Schema version check (reject newer than `MASK_PACKAGE_VERSION`).
- Max dimensions / decoded bytes / blob count.
- RLE completeness and run-count limits; polygon vertex / bbox / finite checks.
- Optional `content_hash`, `package_hash`, `source_width` / `source_height` /
  `source_hash` (SHA-256). Verified when present; `MaskPackageLimits` can
  require them.
- `MaskPackage::open_with` / `SightloomPackageHost::open_with`.
- `apply_region_redaction` rejects malformed inline mask assets instead of
  allocating or ignoring them.

### Publish

Publish order now includes the crates that `reelforge` and `reelforge-io`
actually depend on:

`core` → `compose` → `fx` → `text` → `render-graph` →
`sightloom-adapter` → `project` → `io` → `reelforge` → `cli`

### Migration (Intelligence / Capture)

```rust
// 0.1.5 crates.io (and old main literals)
OperationDescriptor {
    id: OperationId::new("rf.custom.op"),
    version: SemVer::V1,
    input,
    output,
    backend: BackendClass::Rust,
    deterministic: true,
    capabilities,
    parameter_schema,
    limits,
    // executor_kind missing on crates.io 0.1.5 — required on later 0.1.5 main
}

// 0.2.0
OperationDescriptor::new("rf.custom.op", SemVer::V1, input, output, BackendClass::Rust)
    .with_capabilities(["privacy"])
    .with_parameter_schema(schema)
    .with_executor_kind(ExecutorKind::Unary)
```

Depend on `0.2`:

```toml
reelforge-core = "0.2.0"
reelforge-render-graph = "0.2.0"
# or
reelforge = "0.2"
```

### Added (timeline compile)

- CaptureProject compile cursor stays in `MediaTime` ticks (no `as_secs`
  accumulation). Compose / mix `start` is `{ticks, timescale}`; the
  executor still accepts a float for older graphs.
- Subtitle tracks compile to `rf.subtitle.burn` (file URI + record start).
- Wipe compiles to opposing slides (`slide_out` left / `slide_in` right)
  over the overlap, with an explicit warning.
- Semantic `refs: [{kind, id}]` are passed through. Empty redaction is
  attached only when a subject or policy is present.
- `rf.timeline.concat` executes as n-ary `concatenate_video` (duration =
  sum of inputs). `TypedParams::executor_kind` reports `Nary`.

### Added (stage resume)

- `JobCheckpoint.stage_artifacts` + `StageArtifactRecord` (file URI +
  content hash). Job schema is now `RENDER_JOB_VERSION = 2` (old files
  still load; new fields default empty).
- Each completed execution stage is persisted under `{job}/stages/`.
- Resume recomputes live stage fingerprints, validates files, and
  re-enters at the first invalid stage instead of rematerializing
  the whole graph.

### Added (preview planner)

- `plan_preview` / `slice_preview_graph`: requested frame → output cone,
  skip inactive compose layers, bypass encode/audio-only nodes.
- `preview_graph` uses the planner, proxies seeds to the Draft/Proxy box,
  and drops dense mask assets in Draft.

### Added (e2e bench)

- `run_e2e_case` + `smoke_cases` / `standard_cases` / `full_cases`:
  720p/1080p/4K, 10/50/100 subjects, dense masks, H.264/H.265/AV1,
  NVENC/QSV/AMF (skipped when missing), peak RSS, p50/p95, A/V drift,
  FFmpeg `-vf` baseline for the edit workload.
- Example: `cargo run -p reelforge-io --example e2e_bench --release -- --quick`

### Not in this slice

GPU compute kernels are not part of this release.
