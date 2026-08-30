#![cfg(test)]

use super::*;

#[test]
fn test_minimal_metadata() {
    let meta = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);

    assert_eq!(meta.ottofile, Some(PathBuf::from("/test/otto.yml")));
    assert_eq!(meta.hash, "abc123");
    assert_eq!(meta.timestamp, 1234567890);
    assert_eq!(meta.cwd, None);
    assert_eq!(meta.user, None);
    assert_eq!(meta.hostname, None);
    assert_eq!(meta.args, None);
    assert_eq!(meta.run_dir, None);
}

#[test]
fn test_with_run_dir_records_the_directory() {
    let meta = RunMetadata::minimal(None, "abc12345".to_string(), 1234567890)
        .with_run_dir(PathBuf::from("/home/u/.otto/widget-abc12345/1234567890"));

    assert_eq!(
        meta.run_dir,
        Some(PathBuf::from("/home/u/.otto/widget-abc12345/1234567890"))
    );
}

#[test]
fn test_full_metadata() {
    let meta = RunMetadata::full(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        1234567890,
        Some(PathBuf::from("/home/user/project")),
        Some("testuser".to_string()),
        Some("testhost".to_string()),
        Some(vec!["build".to_string(), "test".to_string()]),
    );

    assert_eq!(meta.ottofile, Some(PathBuf::from("/test/otto.yml")));
    assert_eq!(meta.hash, "abc123");
    assert_eq!(meta.timestamp, 1234567890);
    assert_eq!(meta.cwd, Some(PathBuf::from("/home/user/project")));
    assert_eq!(meta.user, Some("testuser".to_string()));
    assert_eq!(meta.hostname, Some("testhost".to_string()));
    assert_eq!(meta.args, Some(vec!["build".to_string(), "test".to_string()]));
}

#[test]
fn test_serde_roundtrip() {
    let meta = RunMetadata::full(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        1234567890,
        Some(PathBuf::from("/home/user/project")),
        Some("testuser".to_string()),
        Some("testhost".to_string()),
        Some(vec!["build".to_string()]),
    );

    let yaml = serde_yaml::to_string(&meta).unwrap();
    let parsed: RunMetadata = serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(meta, parsed);
}

#[test]
fn test_backward_compatible_minimal_yaml() {
    // Test that we can parse old run.yaml files that only have minimal fields
    let yaml = r#"
ottofile: /test/otto.yml
hash: abc123
timestamp: 1234567890
"#;

    let parsed: RunMetadata = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(parsed.ottofile, Some(PathBuf::from("/test/otto.yml")));
    assert_eq!(parsed.hash, "abc123");
    assert_eq!(parsed.timestamp, 1234567890);
    assert_eq!(parsed.cwd, None);
}

#[test]
fn test_current_system_info() {
    let (user, hostname) = RunMetadata::current_system_info();

    // At least one should be available on most systems
    assert!(user.is_some() || hostname.is_some());
}
