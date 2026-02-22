use std::cmp::Reverse;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::error::{Result, TurlError};
use crate::model::{ProviderKind, ResolutionMeta, ResolvedThread};
use crate::provider::Provider;

#[derive(Debug, Deserialize)]
struct SessionsIndex {
    #[serde(default)]
    entries: Vec<SessionIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fullPath")]
    full_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    root: PathBuf,
}

impl ClaudeProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn projects_root(&self) -> PathBuf {
        self.root.join("projects")
    }

    fn choose_latest(paths: Vec<PathBuf>) -> Option<(PathBuf, usize)> {
        if paths.is_empty() {
            return None;
        }

        let mut scored = paths
            .into_iter()
            .map(|path| {
                let modified = fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (path, modified)
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(_, modified)| Reverse(*modified));
        let count = scored.len();
        scored.into_iter().next().map(|(path, _)| (path, count))
    }

    fn find_from_sessions_index(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.file_name() == "sessions-index.json")
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|content| serde_json::from_str::<SessionsIndex>(&content).ok())
            .flat_map(|index| {
                index.entries.into_iter().filter_map(|entry| {
                    if entry.session_id == session_id {
                        entry.full_path
                    } else {
                        None
                    }
                })
            })
            .filter(|path| path.exists())
            .collect()
    }

    fn find_by_filename(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        let needle = format!("{session_id}.jsonl");
        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == needle)
            })
            .collect()
    }

    fn file_contains_session_id(path: &Path, session_id: &str) -> bool {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return false,
        };
        let reader = BufReader::new(file);

        for line in reader.lines().take(30).flatten() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id == session_id)
            {
                return true;
            }
        }

        false
    }

    fn find_by_header_scan(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "jsonl")
            })
            .filter(|path| Self::file_contains_session_id(path, session_id))
            .collect()
    }

    fn make_resolved(
        session_id: &str,
        selected: PathBuf,
        count: usize,
        source: &str,
    ) -> ResolvedThread {
        let mut metadata = ResolutionMeta {
            source: source.to_string(),
            candidate_count: count,
            warnings: Vec::new(),
        };

        if count > 1 {
            metadata.warnings.push(format!(
                "multiple matches found ({count}) for session_id={session_id}; selected latest: {}",
                selected.display()
            ));
        }

        ResolvedThread {
            provider: ProviderKind::Claude,
            session_id: session_id.to_string(),
            path: selected,
            metadata,
        }
    }
}

impl Provider for ClaudeProvider {
    fn resolve(&self, session_id: &str) -> Result<ResolvedThread> {
        let projects = self.projects_root();

        let index_hits = Self::find_from_sessions_index(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(index_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:sessions-index",
            ));
        }

        let filename_hits = Self::find_by_filename(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(filename_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:filename",
            ));
        }

        let scanned_hits = Self::find_by_header_scan(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(scanned_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:header-scan",
            ));
        }

        Err(TurlError::ThreadNotFound {
            provider: ProviderKind::Claude.to_string(),
            session_id: session_id.to_string(),
            searched_roots: vec![projects],
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::provider::Provider;
    use crate::provider::claude::ClaudeProvider;

    #[test]
    fn resolves_from_sessions_index() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-a");
        fs::create_dir_all(&projects).expect("mkdir");
        let thread_file = projects.join("2823d1df-720a-4c31-ac55-ae8ba726721f.jsonl");
        fs::write(&thread_file, "{}\n").expect("write thread");

        let index = projects.join("sessions-index.json");
        fs::write(
            &index,
            format!(
                "{{\"entries\":[{{\"sessionId\":\"2823d1df-720a-4c31-ac55-ae8ba726721f\",\"fullPath\":\"{}\"}}]}}",
                thread_file.display()
            ),
        )
        .expect("write index");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("2823d1df-720a-4c31-ac55-ae8ba726721f")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:sessions-index");
    }

    #[test]
    fn resolves_from_filename_when_index_misses() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-b");
        fs::create_dir_all(&projects).expect("mkdir");

        let thread_file = projects.join("8c06e0f0-2978-48ac-bb42-90d13e3b0470.jsonl");
        fs::write(&thread_file, "{}\n").expect("write thread");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("8c06e0f0-2978-48ac-bb42-90d13e3b0470")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:filename");
    }

    #[test]
    fn resolves_from_header_scan() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-c");
        fs::create_dir_all(&projects).expect("mkdir");

        let thread_file = projects.join("renamed.jsonl");
        fs::write(
            &thread_file,
            "{\"type\":\"user\",\"sessionId\":\"1bd3c108-41b8-4291-93e8-8a472ab09de8\"}\n",
        )
        .expect("write thread");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("1bd3c108-41b8-4291-93e8-8a472ab09de8")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:header-scan");
    }
}
