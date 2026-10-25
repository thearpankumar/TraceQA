//! Writes the run folder (`run.json`, `events.jsonl`, one folder per target)
//! as laid out in `spec/README.md`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};
use traceqa_protocol::types::{ComponentsFile, Event, Rect, RunManifest};

mod clock;
mod crops;
mod read;

pub use clock::{new_run_id, now_rfc3339};
pub use crops::{save_crops, PageImage};
pub use read::{open_run, read_components, ReadError, RunFolder, TargetRef, MAX_FILE_BYTES};

/// Owns one run folder.
pub struct RunWriter {
    dir: PathBuf,
    run_id: String,
    events: File,
    next_seq: u64,
}

impl RunWriter {
    /// Creates `<out_dir>/<run_id>/` and an empty `events.jsonl`.
    /// Fails with `AlreadyExists` when that run folder is taken, so two runs never share one.
    pub fn create(out_dir: &Path, run_id: &str) -> io::Result<Self> {
        let dir = out_dir.join(run_id);
        fs::create_dir_all(out_dir)?;
        fs::create_dir(&dir)?;
        // Not `canonicalize`: on Windows that returns a `\\?\` path, which other programs handle badly.
        let dir = std::path::absolute(dir)?;
        let events = File::create(dir.join("events.jsonl"))?;
        Ok(Self {
            dir,
            run_id: run_id.to_string(),
            events,
            next_seq: 0,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Appends one event to `events.jsonl` and returns it so the caller can forward it live.
    pub fn event(&mut self, kind: &str, target: Option<&str>, message: String, data: Value) -> io::Result<Event> {
        let event = Event {
            seq: self.next_seq,
            ts: now_rfc3339(),
            run_id: self.run_id.clone(),
            type_: kind.to_string(),
            target: target.map(str::to_string),
            message,
            data: match data {
                Value::Object(map) => map,
                _ => Map::new(),
            },
        };
        self.next_seq += 1;
        let mut line = serde_json::to_string(&event)?;
        line.push('\n');
        self.events.write_all(line.as_bytes())?;
        Ok(event)
    }

    pub fn write_manifest(&self, manifest: &RunManifest) -> io::Result<()> {
        write_json(&self.dir.join("run.json"), manifest)
    }

    /// Creates the folder for one target and returns its path.
    pub fn target_dir(&self, name: &str) -> io::Result<PathBuf> {
        let dir = self.dir.join(name);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn write_components(&self, target: &str, file: &ComponentsFile) -> io::Result<()> {
        write_json(&self.dir.join(target).join("components.json"), file)
    }
}

/// Writes `run.json` into `dir` the way [`RunWriter::write_manifest`] does, without the writer.
/// For the one caller that must save the manifest while the writer is out of reach.
pub fn write_manifest_in(dir: &Path, manifest: &RunManifest) -> io::Result<()> {
    write_json(&dir.join("run.json"), manifest)
}

/// Makes every temporary name of this process different, so two writers of one file never share one.
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes pretty JSON through a temporary file, so a reader never sees half a file. The temporary
/// file is named after the process and a counter, so two writers of the same file (the end of the
/// run and an abort) can never write to, or move, each other's file.
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = path.with_file_name(name);
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

fn unix_time() -> (u64, u32) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (now.as_secs(), now.subsec_nanos())
}

/// A crop to cut from the page screenshot: the file name to write and the area in page coordinates.
pub struct CropRequest {
    pub file_name: String,
    pub area: Rect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_numbered_and_appended() {
        let out = std::env::temp_dir().join(format!("traceqa-recorder-{}", new_run_id()));
        let mut writer = RunWriter::create(&out, "r_test").unwrap();
        let first = writer
            .event("run.started", None, "Started".into(), Value::Null)
            .unwrap();
        let second = writer
            .event(
                "page.opened",
                Some("firefox-1280x800"),
                "Opened".into(),
                serde_json::json!({"status": 200}),
            )
            .unwrap();
        assert_eq!((first.seq, second.seq), (0, 1));

        let lines: Vec<Event> = fs::read_to_string(writer.dir().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines, vec![first, second]);
        assert_eq!(lines[1].data["status"], 200);
        assert_eq!(
            RunWriter::create(&out, "r_test").err().map(|e| e.kind()),
            Some(io::ErrorKind::AlreadyExists)
        );
        fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn writers_of_one_file_never_share_a_temporary_name() {
        let out = std::env::temp_dir().join(format!("traceqa-recorder-tmp-{}", new_run_id()));
        fs::create_dir_all(&out).unwrap();
        let path = out.join("run.json");
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for round in 0..25 {
                        write_json(path, &serde_json::json!({ "writer": writer, "round": round })).unwrap();
                    }
                });
            }
        });
        let names: Vec<String> = fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["run.json"], "no temporary file is left behind");
        assert!(serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap()).is_ok());
        fs::remove_dir_all(out).unwrap();
    }
}
