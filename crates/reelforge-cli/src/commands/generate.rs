//! Generic generation command; no avatar or walking-specific behavior.
use reelforge::generation::{
    Engine, GeneratedClip, GenerationSpec, ProcessConfig, ProcessProvider, Provider, Resources,
    compile,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

#[derive(Deserialize)]
struct Input {
    spec: GenerationSpec,
    resources: Resources,
    providers: Vec<ProcessConfig>,
}

/// Explain or execute a generation through caller-configured local workers.
pub fn run(
    path: &str,
    execute: bool,
    store: Option<&str>,
    receipt: Option<&str>,
    output: Option<&str>,
) -> Result<(), String> {
    execute_inner(path, execute, store, receipt, output).map_err(|e| e.to_string())
}
fn execute_inner(
    path: &str,
    execute: bool,
    store: Option<&str>,
    receipt: Option<&str>,
    output: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    if data.len() > 16 * 1024 * 1024 {
        return Err("generation input exceeds byte budget".into());
    }
    let input: Input = serde_json::from_slice(&data)?;
    let mut providers: BTreeMap<String, Box<dyn Provider>> = BTreeMap::new();
    for config in input.providers {
        let provider = ProcessProvider::new(config)?;
        if providers
            .insert(provider.backend().id.clone(), Box::new(provider))
            .is_some()
        {
            return Err("duplicate generation provider".into());
        }
    }
    if !execute {
        let backends: Vec<_> = providers.values().map(|p| p.backend().clone()).collect();
        let plan = compile(input.spec, &backends, &input.resources)?;
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    let store = store.ok_or("--store is required with --run")?;
    let receipt = receipt.ok_or("--receipt is required with --run")?;
    if Path::new(receipt).exists() || output.is_some_and(|p| Path::new(p).exists()) {
        return Err("output exists; choose a new receipt/video path".into());
    }
    let engine = Engine::open(store)?;
    let result = engine.run(input.spec, &input.resources, &mut providers)?;
    if let Some(output) = output {
        let clip = GeneratedClip::open(engine.store().clone(), &result)?;
        reelforge::write_video(
            &clip,
            &reelforge::WriteVideoOptions::new(output, f64::from(result.clock.fps)),
        )?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(receipt)?;
    file.write_all(&serde_json::to_vec_pretty(&result)?)?;
    file.sync_all()?;
    println!(
        "{} keyframes, {} generated intermediate frames; clock verified, visual quality unverified",
        result.keyframes, result.intermediate_frames
    );
    Ok(())
}
