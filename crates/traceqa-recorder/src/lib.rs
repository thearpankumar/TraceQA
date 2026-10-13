//! Writes the run folder (`run.json`, `events.jsonl`, one folder per target)
//! as laid out in `spec/README.md`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};
use traceqa_protocol::types::{ComponentsFile, Event, Rect, RunManifest};

mod clock;
mod crops;

pub use clock::{new_run_id, now_rfc3339};
pub use crops::{save_crops, PageImage};

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

/// Writes pretty JSON through a temporary file, so a reader never sees half a file.
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(temporary, path)
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
}
