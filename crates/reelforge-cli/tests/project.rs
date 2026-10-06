//! `reelforge project` compiles a project file. No ffmpeg.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_reelforge")
}

fn project_json(audio_extra: &str) -> String {
    format!(
        r#"{{
  "version": 1,
  "id": "p",
  "name": "av",
  "media": [{{ "id": "a", "uri": "a.mp4" }}],
  "sequences": [{{
    "id": "s",
    "name": "main",
    "tracks": [
      {{
        "id": "v0",
        "kind": "video",
        "items": [{{
          "kind": "clip",
          "id": "pic",
          "media": "a",
          "source": {{
            "start": {{ "ticks": 0, "timescale": 1000 }},
            "duration": {{ "ticks": 2000, "timescale": 1000 }}
          }}
        }}]
      }},
      {{
        "id": "a0",
        "kind": "audio",
        "items": [{{
          "kind": "clip",
          "id": "snd",
          "media": "a",
          "source": {{
            "start": {{ "ticks": 0, "timescale": 1000 }},
            "duration": {{ "ticks": 2000, "timescale": 1000 }}
          }}{audio_extra}
        }}]
      }}
    ]
  }}]
}}"#,
    )
}

fn write_project(body: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("temp project");
    std::io::Write::write_all(&mut file, body.as_bytes()).expect("write project");
    file
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("spawn reelforge")
}

#[test]
fn help_lists_the_project_command() {
    let output = run(&["project", "--help"]);
    let text = String::from_utf8(output.stdout).expect("utf8");
    assert!(output.status.success(), "{text}");
    assert!(text.contains("CaptureProject"), "{text}");
    assert!(text.contains("--graph"), "{text}");
}

#[test]
fn project_command_explains_audio_speed() {
    let file = write_project(&project_json(
        r#", "retiming": { "mode": "speed", "factor": 2.0 }"#,
    ));
    let output = run(&["project", file.path().to_str().expect("path")]);
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(output.status.success(), "{stderr}\n{stdout}");
    assert!(stdout.contains("rf.transform.speed"), "{stdout}");
    assert!(stdout.contains("rf.audio.mix"), "{stdout}");
    assert!(!stdout.contains("rf.transform.freeze"), "{stdout}");
}

#[test]
fn project_command_prints_the_compiled_graph() {
    let file = write_project(&project_json(
        r#", "retiming": { "mode": "speed", "factor": 2.0 }"#,
    ));
    let output = run(&["project", "--graph", file.path().to_str().expect("path")]);
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(output.status.success(), "{stderr}\n{stdout}");
    assert!(stdout.contains("\"rf.transform.speed\""), "{stdout}");
    assert!(stdout.contains("\"rf.audio.mix\""), "{stdout}");
}

#[test]
fn project_command_refuses_picture_ops_on_audio() {
    let freeze = write_project(&project_json(
        r#", "retiming": { "mode": "freeze", "at": { "ticks": 500, "timescale": 1000 }, "hold": { "ticks": 1000, "timescale": 1000 } }"#,
    ));
    let freeze_out = run(&["project", freeze.path().to_str().expect("path")]);
    let freeze_err = String::from_utf8(freeze_out.stderr).expect("utf8");
    assert!(!freeze_out.status.success(), "{freeze_err}");
    assert!(
        freeze_err.contains("clip snd: freeze is a picture retime"),
        "{freeze_err}"
    );
    assert!(freeze_err.contains("audio track"), "{freeze_err}");

    let wipe = write_project(&project_json(
        r#", "transition_in": { "kind": "wipe", "duration": { "ticks": 500, "timescale": 1000 } }"#,
    ));
    let wipe_out = run(&["project", wipe.path().to_str().expect("path")]);
    let wipe_err = String::from_utf8(wipe_out.stderr).expect("utf8");
    let wipe_out_text = String::from_utf8(wipe_out.stdout).expect("utf8");
    assert!(!wipe_out.status.success(), "{wipe_err}");
    assert!(
        wipe_err.contains("clip snd: wipe is a picture transition"),
        "{wipe_err}"
    );
    assert!(
        !wipe_out_text.contains("slide"),
        "an audio wipe must not compile into a slide: {wipe_out_text}"
    );
}
