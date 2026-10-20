use crate::{Backend, Error, FrameReceipt, FrameRequest, FrameStore, Provider, Recovery, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Configuration of a local inference worker. Arguments are passed directly, without a shell.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessConfig {
    /// Honest model capability, weight/configuration fingerprint and resource admission.
    pub backend: Backend,
    /// Existing local executable, normally the isolated model runtime's Python.
    pub program: PathBuf,
    /// Fixed arguments, normally a provider script path.
    pub args: Vec<String>,
    /// Adapter configuration (local weights, endpoint or trusted exported workflow).
    pub config: serde_json::Value,
    /// Scripts/configuration files whose bytes must be included in request identity.
    pub input_files: Vec<PathBuf>,
    /// Bounded worker deadline, seconds (1..=3600).
    pub timeout_secs: u64,
}

/// JSON input supplied to a worker via --input; mode is generate or recover.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessRequest {
    /// Complete typed job, including scene memory and target-frame conditions.
    pub request: FrameRequest,
    /// Immutable, verified endpoint/keyframe image receipts.
    pub anchors: BTreeMap<u32, FrameReceipt>,
    /// Config snapshot included in the provider fingerprint.
    pub config: serde_json::Value,
    /// Absolute owned working directory; write result.json here.
    pub directory: PathBuf,
}

/// A worker output file, imported into the owned content-addressed store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputFrame {
    /// Absolute PNG path inside the request working directory.
    pub path: PathBuf,
    /// Exact PNG byte fingerprint, independently checked before import.
    pub sha256: String,
}

/// Durable result.json emitted by an inference worker. Recovery must never submit a job.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProcessResponse {
    /// Preflight accepted the request without model execution or submission.
    Ready,
    /// Original execution completed with these indexed PNGs.
    Completed {
        /// Exactly the requested displayed frame indices.
        #[serde(deserialize_with = "deserialize_frames")]
        frames: BTreeMap<u32, OutputFrame>,
    },
    /// Worker proves no submission has occurred; generate is safe.
    NotSubmitted,
    /// Lost submission acknowledgement or otherwise unknown model outcome.
    Unknown,
}

// Tagged serde enums buffer map keys as strings. Decode numeric frame indices
// explicitly instead of depending on the JSON-only map-key deserializer.
fn deserialize_frames<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<u32, OutputFrame>, D::Error> {
    let wire = BTreeMap::<String, OutputFrame>::deserialize(deserializer)?;
    wire.into_iter()
        .map(|(key, value)| {
            let index: u32 = key.parse().map_err(serde::de::Error::custom)?;
            if index.to_string() != key {
                return Err(serde::de::Error::custom("non-canonical frame index"));
            }
            Ok((index, value))
        })
        .collect()
}

/// Concrete inference-process adapter with durable inputs, recovery and output ownership.
pub struct ProcessProvider {
    config: ProcessConfig,
    backend: Backend,
    inputs: BTreeMap<PathBuf, String>,
}
impl ProcessProvider {
    /// Fingerprint executable/scripts/configuration before admitting a connected worker.
    pub fn new(mut config: ProcessConfig) -> Result<Self> {
        if !(1..=3600).contains(&config.timeout_secs)
            || config.args.len() > 128
            || config.input_files.len() > 64
            || serde_json::to_vec(&config.config)?.len() > 1_048_576
        {
            return Err(Error::Invalid("worker configuration bounds".into()));
        }
        config.program = config.program.canonicalize()?;
        let mut inputs = BTreeMap::new();
        for p in std::iter::once(&config.program).chain(&config.input_files) {
            let p = p.canonicalize()?;
            inputs.insert(
                p.clone(),
                crate::store::digest(&crate::store::read_bounded(&p, 64 * 1024 * 1024)?),
            );
        }
        let mut backend = config.backend.clone();
        backend.fingerprint = crate::store::digest(&serde_json::to_vec(&(&config, &inputs))?);
        Ok(Self {
            config,
            backend,
            inputs,
        })
    }
    fn verify_inputs(&self) -> Result<()> {
        for (p, hash) in &self.inputs {
            if crate::store::digest(&crate::store::read_bounded(p, 64 * 1024 * 1024)?) != *hash {
                return Err(Error::Invalid(
                    "worker source changed; reconnect the provider".into(),
                ));
            }
        }
        Ok(())
    }
    fn invoke(
        &self,
        r: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
        mode: &str,
    ) -> Result<Recovery> {
        if r.backend != self.backend || !crate::store::is_digest(&r.id) {
            return Err(Error::Invalid("worker request binding".into()));
        }
        self.verify_inputs()?;
        let dir = store.root.join("requests").join(&r.id).join("worker");
        std::fs::create_dir_all(&dir)?;
        let input = dir.join("input.json");
        let request = ProcessRequest {
            request: r.clone(),
            anchors: anchors.clone(),
            config: self.config.config.clone(),
            directory: dir.clone(),
        };
        if input.exists() {
            let saved: serde_json::Value =
                serde_json::from_slice(&crate::store::read_bounded(&input, 4 * 1024 * 1024)?)?;
            if saved != serde_json::to_value(&request)? {
                return Err(Error::Invalid("worker input changed".into()));
            }
        } else {
            FrameStore::save_new(&input, &request)?;
        }
        let response = dir.join("result.json");
        if !response.exists() {
            self.launch(&input, &dir, mode)?;
        }
        // NotSubmitted/Unknown are transient statuses; do not accept them as completed output.
        let result: ProcessResponse =
            serde_json::from_slice(&crate::store::read_bounded(&response, 16 * 1024 * 1024)?)?;
        match result {
            ProcessResponse::Ready => Err(Error::Invalid(
                "preflight response used as generated output".into(),
            )),
            ProcessResponse::Completed { frames } => {
                if frames.keys().copied().collect::<Vec<_>>() != r.indices {
                    return Err(Error::Invalid(
                        "worker returned incomplete/extra frames".into(),
                    ));
                }
                let mut owned = BTreeMap::new();
                for (index, f) in frames {
                    if !crate::store::is_digest(&f.sha256) {
                        return Err(Error::Invalid("worker output digest".into()));
                    }
                    let path = f.path.canonicalize()?;
                    if !path.starts_with(&dir) {
                        return Err(Error::Invalid(
                            "worker output outside its owned directory".into(),
                        ));
                    }
                    let bytes = crate::store::read_bounded(&path, 64 * 1024 * 1024)?;
                    if crate::store::digest(&bytes) != f.sha256 {
                        return Err(Error::Invalid("worker output changed".into()));
                    }
                    owned.insert(index, store.ingest_png(&bytes)?);
                }
                Ok(Recovery::Completed(owned))
            }
            ProcessResponse::NotSubmitted => {
                std::fs::remove_file(response)?;
                Ok(Recovery::NotSubmitted)
            }
            ProcessResponse::Unknown => {
                std::fs::remove_file(response)?;
                Ok(Recovery::Unknown)
            }
        }
    }
    fn launch(&self, input: &Path, dir: &Path, mode: &str) -> Result<()> {
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .args(["--input"])
            .arg(input)
            .args(["--mode", mode])
            .current_dir(dir)
            .stdin(Stdio::null());
        let logs = [dir.join("stdout.log"), dir.join("stderr.log")];
        command
            .stdout(Stdio::from(std::fs::File::create(&logs[0])?))
            .stderr(Stdio::from(std::fs::File::create(&logs[1])?));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: workers do not create visible consoles.
        }
        let mut child = command.spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(Error::Provider(format!(
                        "worker exited {status}; logs: {}",
                        dir.display()
                    )));
                }
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(self.config.timeout_secs)
                || logs
                    .iter()
                    .any(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 4 * 1024 * 1024))
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Unknown(input.display().to_string()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
impl Provider for ProcessProvider {
    fn backend(&self) -> &Backend {
        &self.backend
    }
    fn preflight(&self, request: &FrameRequest, store: &FrameStore) -> Result<()> {
        self.verify_inputs()?;
        let dir = tempfile::tempdir_in(store.root.join("requests"))?;
        let input = dir.path().join("input.json");
        let job = ProcessRequest {
            request: request.clone(),
            anchors: BTreeMap::new(),
            config: self.config.config.clone(),
            directory: dir.path().to_path_buf(),
        };
        FrameStore::save_new(&input, &job)?;
        if let Err(error) = self.launch(&input, dir.path(), "preflight") {
            let diagnostics = dir.keep();
            return Err(Error::Unavailable(format!(
                "{} preflight failed: {error}; diagnostics: {}",
                self.backend.id,
                diagnostics.display()
            )));
        }
        let result: ProcessResponse = serde_json::from_slice(&crate::store::read_bounded(
            &dir.path().join("result.json"),
            16 * 1024 * 1024,
        )?)?;
        if !matches!(result, ProcessResponse::Ready) {
            return Err(Error::Unavailable(
                "worker preflight did not accept the generation request".into(),
            ));
        }
        Ok(())
    }
    fn generate(
        &mut self,
        r: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
    ) -> Result<BTreeMap<u32, FrameReceipt>> {
        match self.invoke(r, anchors, store, "generate")? {
            Recovery::Completed(frames) => Ok(frames),
            Recovery::NotSubmitted => {
                Err(Error::Provider("worker did not start generation".into()))
            }
            Recovery::Unknown => Err(Error::Unknown(r.id.clone())),
        }
    }
    fn recover(
        &mut self,
        r: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
    ) -> Result<Recovery> {
        self.invoke(r, anchors, store, "recover")
    }
}
