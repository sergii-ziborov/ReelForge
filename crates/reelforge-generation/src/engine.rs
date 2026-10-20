use crate::{
    Backend, Error, FrameReceipt, FrameRequest, FrameStore, GenerationPlan, GenerationSpec,
    Resources, Result, compile,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Provider recovery after a reservation/process interruption.
pub enum Recovery {
    /// The original request produced these owned frames; never submit it twice.
    Completed(BTreeMap<u32, FrameReceipt>),
    /// Adapter proves that no model request was submitted; execution can safely start.
    NotSubmitted,
    /// Outcome remains unknown; stop instead of repeating a paid/model request.
    Unknown,
}

/// Adapter implemented by image creation, image editing or a temporal model.
/// Network/model loading and honest capability declarations belong to the adapter.
pub trait Provider {
    /// Current model/configuration capability snapshot used for automatic routing.
    fn backend(&self) -> &Backend;
    /// Check the complete request before any generation in the plan is submitted.
    /// Implementations must not load weights, generate images or submit inference here.
    fn preflight(&self, _request: &FrameRequest, _store: &FrameStore) -> Result<()> {
        Ok(())
    }
    /// Execute exactly the requested frame indices, conditioned on immutable anchors.
    fn generate(
        &mut self,
        request: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
    ) -> Result<BTreeMap<u32, FrameReceipt>>;
    /// Recover without silently submitting the original model request again.
    fn recover(
        &mut self,
        request: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
    ) -> Result<Recovery>;
}

/// Actual delivered frame coverage. Quality beyond pixels/clock remains a separate review.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerationReceipt {
    /// Generation plan fingerprint.
    pub plan_sha256: String,
    /// Exact requested clock, carried into native composition.
    pub clock: crate::Clock,
    /// All displayed frames, including original immutable anchors.
    pub frames: BTreeMap<u32, FrameReceipt>,
    /// Number of generated keyframes in the plan.
    pub keyframes: usize,
    /// Number of model-generated intermediate displayed frames.
    pub intermediate_frames: usize,
    /// Newly submitted requests during this execution; cached/resumed requests excluded.
    pub submitted_requests: usize,
    /// Owned frame data and exact contiguous frame clock passed verification.
    pub frame_clock_verified: bool,
    /// Counts/hashes alone never verify natural anatomy, lighting or motion.
    pub naturalness_verified: bool,
}

/// Generic generation engine with owned cache and durable per-request reservations.
pub struct Engine {
    store: FrameStore,
}
impl Engine {
    /// Open the shared content-addressed generation store.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: FrameStore::open(root)?,
        })
    }
    /// Access owned storage when an adapter imports generated pixels.
    #[must_use]
    pub fn store(&self) -> &FrameStore {
        &self.store
    }
    fn execute(
        &self,
        provider: &mut dyn Provider,
        request: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
    ) -> Result<(BTreeMap<u32, FrameReceipt>, bool)> {
        let mut request = request.clone();
        request.id = crate::store::digest_json(&(&request, anchors))?;
        let dir = self.store.root.join("requests").join(&request.id);
        std::fs::create_dir_all(&dir)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("lock"))?;
        lock.try_lock()
            .map_err(|_| Error::Unavailable("generation request already executing".into()))?;
        let output = dir.join("completed.json");
        if output.exists() {
            let frames =
                serde_json::from_slice(&crate::store::read_bounded(&output, 16 * 1024 * 1024)?)?;
            self.qualify(&request, &frames)?;
            return Ok((frames, false));
        }
        let reservation = dir.join("reserved.json");
        let (frames, submitted) = if reservation.exists() {
            let expected = serde_json::to_value((&request, anchors))?;
            let saved: serde_json::Value = serde_json::from_slice(&crate::store::read_bounded(
                &reservation,
                4 * 1024 * 1024,
            )?)?;
            if saved != expected {
                return Err(Error::Invalid("generation reservation changed".into()));
            }
            match provider.recover(&request, anchors, &self.store)? {
                Recovery::Completed(frames) => (frames, false),
                Recovery::NotSubmitted => {
                    (provider.generate(&request, anchors, &self.store)?, true)
                }
                Recovery::Unknown => return Err(Error::Unknown(request.id)),
            }
        } else {
            FrameStore::save_new(&reservation, &(&request, anchors))?;
            (provider.generate(&request, anchors, &self.store)?, true)
        };
        self.qualify(&request, &frames)?;
        FrameStore::save_new(&output, &frames)?;
        Ok((frames, submitted))
    }
    fn qualify(&self, request: &FrameRequest, frames: &BTreeMap<u32, FrameReceipt>) -> Result<()> {
        if frames.keys().copied().collect::<Vec<_>>() != request.indices {
            return Err(Error::Invalid(
                "missing, extra or retimed generated frames".into(),
            ));
        }
        for r in frames.values() {
            self.store.verify(r)?;
            if (r.width, r.height) != (request.clock.width, request.clock.height) {
                return Err(Error::Invalid(
                    "provider changed the requested canvas".into(),
                ));
            }
        }
        Ok(())
    }
    /// Compile and execute caller intent against current providers/resources.
    /// Planning fails before any model calls if the complete scene cannot be filled.
    /// No public deserialized job plan is blindly trusted for execution.
    pub fn run(
        &self,
        spec: GenerationSpec,
        resources: &Resources,
        providers: &mut BTreeMap<String, Box<dyn Provider>>,
    ) -> Result<GenerationReceipt> {
        if providers.iter().any(|(id, p)| id != &p.backend().id) {
            return Err(Error::Invalid("provider binding identity".into()));
        }
        let backends: Vec<_> = providers.values().map(|p| p.backend().clone()).collect();
        let plan = compile(spec, &backends, resources)?;
        for shot in &plan.spec.shots {
            for reference in &shot.references {
                self.store.verify(reference)?;
            }
        }
        for request in plan.keyframes.iter().chain(&plan.between) {
            providers
                .get(&request.backend.id)
                .ok_or_else(|| Error::Unavailable(request.backend.id.clone()))?
                .preflight(request, &self.store)?;
        }
        self.run_plan(&plan, providers)
    }
    fn run_plan(
        &self,
        plan: &GenerationPlan,
        providers: &mut BTreeMap<String, Box<dyn Provider>>,
    ) -> Result<GenerationReceipt> {
        let mut frames = BTreeMap::new();
        let mut submitted = 0;
        for r in plan.keyframes.iter().chain(&plan.between) {
            let anchors = r
                .anchors
                .iter()
                .map(|i| {
                    frames
                        .get(i)
                        .cloned()
                        .map(|f| (*i, f))
                        .ok_or_else(|| Error::Invalid("missing immutable endpoint".into()))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let provider = providers
                .get_mut(&r.backend.id)
                .ok_or_else(|| Error::Unavailable(r.backend.id.clone()))?;
            let (result, new) = self.execute(provider.as_mut(), r, &anchors)?;
            if result.keys().any(|i| frames.contains_key(i)) {
                return Err(Error::Invalid(
                    "provider attempted to replace an immutable keyframe".into(),
                ));
            }
            frames.extend(result);
            submitted += usize::from(new);
        }
        if frames.keys().copied().ne(0..plan.spec.clock.frames) {
            return Err(Error::Invalid("delivered sequence has a gap".into()));
        }
        for shot in plan.spec.shots.iter().filter(|s| !s.allow_still) {
            let unique = frames
                .range(shot.start..shot.end)
                .map(|(_, r)| &r.pixels_sha256)
                .collect::<BTreeSet<_>>()
                .len();
            let length = (shot.end - shot.start) as usize;
            if unique * 4 < length * 3 {
                return Err(Error::Invalid("generated motion is repeated stills; output FPS does not prove frame generation".into()));
            }
        }
        let keyframes = plan.keyframes.len();
        Ok(GenerationReceipt {
            plan_sha256: crate::store::digest_json(&(
                &plan.spec,
                plan.keyframes
                    .iter()
                    .chain(&plan.between)
                    .map(|r| &r.id)
                    .collect::<Vec<_>>(),
            ))?,
            clock: plan.spec.clock.clone(),
            intermediate_frames: frames.len() - keyframes,
            frames,
            keyframes,
            submitted_requests: submitted,
            frame_clock_verified: true,
            naturalness_verified: false,
        })
    }
}
