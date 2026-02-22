use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::error::{Result, TurlError};
use crate::model::{ProviderKind, ResolutionMeta, ResolvedThread};
use crate::provider::Provider;

#[derive(Debug, Clone)]
pub struct CodexProvider {
    root: PathBuf,
}

impl CodexProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn sessions_root(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn archived_root(&self) -> PathBuf {
        self.root.join("archived_sessions")
    }

    fn find_candidates(root: &Path, session_id: &str) -> Vec<PathBuf> {
        let needle = format!("{session_id}.jsonl");
        if !root.exists() {
            return Vec::new();
        }

        WalkDir::new(root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&needle))
            })
            .collect()
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
}

impl Provider for CodexProvider {
    fn resolve(&self, session_id: &str) -> Result<ResolvedThread> {
        let sessions = self.sessions_root();
        let archived = self.archived_root();

        let active_candidates = Self::find_candidates(&sessions, session_id);
        if let Some((selected, count)) = Self::choose_latest(active_candidates) {
            let mut meta = ResolutionMeta {
                source: "codex:sessions".to_string(),
                candidate_count: count,
                warnings: Vec::new(),
            };
            if count > 1 {
                meta.warnings.push(format!(
                    "multiple matches found ({count}) for session_id={session_id}; selected latest: {}",
                    selected.display()
                ));
            }

            return Ok(ResolvedThread {
                provider: ProviderKind::Codex,
                session_id: session_id.to_string(),
                path: selected,
                metadata: meta,
            });
        }

        let archived_candidates = Self::find_candidates(&archived, session_id);
        if let Some((selected, count)) = Self::choose_latest(archived_candidates) {
            let mut meta = ResolutionMeta {
                source: "codex:archived_sessions".to_string(),
                candidate_count: count,
                warnings: Vec::new(),
            };
            if count > 1 {
                meta.warnings.push(format!(
                    "multiple archived matches found ({count}) for session_id={session_id}; selected latest: {}",
                    selected.display()
                ));
            }

            return Ok(ResolvedThread {
                provider: ProviderKind::Codex,
                session_id: session_id.to_string(),
                path: selected,
                metadata: meta,
            });
        }

        Err(TurlError::ThreadNotFound {
            provider: ProviderKind::Codex.to_string(),
            session_id: session_id.to_string(),
            searched_roots: vec![sessions, archived],
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::provider::Provider;
    use crate::provider::codex::CodexProvider;

    #[test]
    fn resolves_from_sessions() {
        let temp = tempdir().expect("tempdir");
        let path = temp
            .path()
            .join("sessions/2026/02/23/rollout-2026-02-23T04-48-50-019c871c-b1f9-7f60-9c4f-87ed09f13592.jsonl");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{}\n").expect("write");

        let provider = CodexProvider::new(temp.path());
        let resolved = provider
            .resolve("019c871c-b1f9-7f60-9c4f-87ed09f13592")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, path);
    }

    #[test]
    fn resolves_from_archived_when_not_in_sessions() {
        let temp = tempdir().expect("tempdir");
        let path = temp
            .path()
            .join("archived_sessions/rollout-2026-02-22T01-05-36-019c8129-f668-7951-8d56-cc5513541c26.jsonl");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{}\n").expect("write");

        let provider = CodexProvider::new(temp.path());
        let resolved = provider
            .resolve("019c8129-f668-7951-8d56-cc5513541c26")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, path);
        assert_eq!(resolved.metadata.source, "codex:archived_sessions");
    }

    #[test]
    fn returns_not_found_when_missing() {
        let temp = tempdir().expect("tempdir");
        let provider = CodexProvider::new(temp.path());
        let err = provider
            .resolve("019c8129-f668-7951-8d56-cc5513541c26")
            .expect_err("should fail");
        assert!(format!("{err}").contains("thread not found"));
    }
}
