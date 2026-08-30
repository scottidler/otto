use otto::makefile::{MakefileParser, OttoConverter};
use std::fs;

#[test]
fn test_convert_go_build_project_makefile() {
    let makefile_path = "makefiles/go-build-project/Makefile";
    let content = fs::read_to_string(makefile_path).unwrap_or_else(|_| panic!("Failed to read {}", makefile_path));

    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Verify basic structure
    assert!(!config.tasks.is_empty(), "Should have tasks");
    assert!(!config.otto.envs.is_empty(), "Should have environment variables");

    // Verify it can be serialized to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");
    assert!(!yaml.is_empty(), "YAML output should not be empty");
}

#[test]
fn test_convert_python_poetry_service_makefile() {
    let makefile_path = "makefiles/python-poetry-service/Makefile";
    let content = fs::read_to_string(makefile_path).unwrap_or_else(|_| panic!("Failed to read {}", makefile_path));

    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Verify basic structure
    assert!(!config.tasks.is_empty(), "Should have tasks");

    // Verify it can be serialized to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");
    assert!(!yaml.is_empty(), "YAML output should not be empty");

    // Verify it contains expected tasks
    assert!(config.tasks.contains_key("dev"), "Should have 'dev' task");
    assert!(
        config.tasks.contains_key("test") || config.tasks.contains_key("unit-test"),
        "Should have a test task"
    );
}

#[test]
fn test_convert_makefile_example() {
    let makefile_path = "makefiles/makefile-example/Makefile";
    let content = fs::read_to_string(makefile_path).unwrap_or_else(|_| panic!("Failed to read {}", makefile_path));

    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Verify it can be serialized to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");
    assert!(!yaml.is_empty(), "YAML output should not be empty");
}

#[test]
fn test_convert_python_pre_commit_makefile() {
    let makefile_path = "makefiles/python-pre-commit/Makefile";
    let content = fs::read_to_string(makefile_path).unwrap_or_else(|_| panic!("Failed to read {}", makefile_path));

    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Verify it can be serialized to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");
    assert!(!yaml.is_empty(), "YAML output should not be empty");
}

#[test]
fn test_convert_docker_compose_service_makefile() {
    let makefile_path = "makefiles/docker-compose-service/Makefile";
    let content = fs::read_to_string(makefile_path).unwrap_or_else(|_| panic!("Failed to read {}", makefile_path));

    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Verify it can be serialized to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");
    assert!(!yaml.is_empty(), "YAML output should not be empty");
}

#[test]
fn test_roundtrip_conversion() {
    // Test that we can convert a Makefile to Otto and then serialize/deserialize
    let simple_makefile = r#"
VAR := value

.DEFAULT_GOAL := build

.PHONY: build clean

# Build the project
build:
	echo "Building..."
	mkdir -p dist

clean:
	rm -rf dist
"#;

    let mut parser = MakefileParser::new(simple_makefile.to_string());
    let ast = parser.parse().expect("Failed to parse Makefile");

    let converter = OttoConverter::new(ast);
    let config = converter.convert().expect("Failed to convert to Otto");

    // Serialize to YAML
    let yaml = serde_yaml::to_string(&config).expect("Failed to serialize to YAML");

    // Deserialize back
    let config2: otto::ConfigSpec = serde_yaml::from_str(&yaml).expect("Failed to deserialize from YAML");

    // Verify key properties are preserved
    assert_eq!(config.otto.tasks, config2.otto.tasks);
    assert_eq!(config.tasks.len(), config2.tasks.len());
}
