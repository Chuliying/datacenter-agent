use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn reported_regression_exits_nonzero() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_nanos();
    let artifact = std::env::temp_dir().join(format!("eval-cli-failing-{nanos}.json"));
    fs::write(
        &artifact,
        r#"{
          "status": "loaded",
          "provenance": "process exit regression test",
          "cases": [{
            "id": "must-fail",
            "prompt": "營收",
            "actual_response": "wrong",
            "must_include": ["required"]
          }]
        }"#,
    )
    .expect("failing replay artifact should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_eval"))
        .args(["--response", "--replay"])
        .arg(&artifact)
        .output()
        .expect("eval binary should run");
    let _ = fs::remove_file(artifact);

    assert_eq!(
        output.status.code(),
        Some(1),
        "reported regressions must exit 1; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("failed=1"),
        "exit 1 must come from the reported regression"
    );
}
