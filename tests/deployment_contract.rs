#[test]
fn dockerfile_runtime_stage_copies_default_config_tree() {
    let dockerfile = std::fs::read_to_string("Dockerfile").expect("Dockerfile should be readable");
    let runtime_stage = dockerfile
        .rfind("FROM ")
        .map(|index| &dockerfile[index..])
        .expect("Dockerfile should contain a runtime stage");

    assert!(
        runtime_stage.lines().any(|line| {
            let line = line.trim();
            line.starts_with("COPY config ") || line.starts_with("COPY ./config ")
        }),
        "the final runtime stage must copy config/"
    );
    assert!(
        runtime_stage.contains("config/config.toml"),
        "the final runtime stage must launch with the copied default config"
    );
    assert!(std::path::Path::new("config/config.toml").is_file());
    assert!(std::path::Path::new("config/prompt_guide/agent_system.md").is_file());
    assert!(std::path::Path::new("config/runtime/intents.toml").is_file());
}
