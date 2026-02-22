use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn setup_codex_tree() -> (tempfile::TempDir, String) {
    let temp = tempdir().expect("tempdir");
    let thread_path = temp
        .path()
        .join("sessions/2026/02/23/rollout-2026-02-23T04-48-50-019c871c-b1f9-7f60-9c4f-87ed09f13592.jsonl");
    fs::create_dir_all(thread_path.parent().expect("parent")).expect("mkdir");
    fs::write(
        &thread_path,
        "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]}}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"world\"}]}}\n",
    )
    .expect("write");

    (
        temp,
        "codex://019c871c-b1f9-7f60-9c4f-87ed09f13592".to_string(),
    )
}

#[test]
fn default_outputs_markdown() {
    let (temp, uri) = setup_codex_tree();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("turl"));
    cmd.env("CODEX_HOME", temp.path())
        .env("CLAUDE_CONFIG_DIR", temp.path().join("missing-claude"))
        .arg(uri)
        .assert()
        .success()
        .stdout(predicate::str::contains("# Thread"))
        .stdout(predicate::str::contains("## 1. User"))
        .stdout(predicate::str::contains("hello"));
}

#[test]
fn raw_outputs_json() {
    let (temp, uri) = setup_codex_tree();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("turl"));
    cmd.env("CODEX_HOME", temp.path())
        .env("CLAUDE_CONFIG_DIR", temp.path().join("missing-claude"))
        .arg(uri)
        .arg("--raw")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"response_item\""));
}

#[test]
fn missing_thread_returns_non_zero() {
    let temp = tempdir().expect("tempdir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("turl"));
    cmd.env("CODEX_HOME", temp.path())
        .env("CLAUDE_CONFIG_DIR", temp.path())
        .arg("codex://019c871c-b1f9-7f60-9c4f-87ed09f13592")
        .assert()
        .failure()
        .stderr(predicate::str::contains("thread not found"));
}
