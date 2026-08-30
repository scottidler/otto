#![cfg(test)]

use super::*;

#[test]
fn test_when_default_is_success() {
    assert_eq!(When::default(), When::Success);
}

#[test]
fn test_deserialize_bare_string() {
    let yaml = "foo";
    let edge: EdgeSpec = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(edge.task, "foo");
    assert_eq!(edge.when, When::Success);
    assert!(edge.from_sugar);
    assert!(!edge.is_injected_sugar);
}

#[test]
fn test_deserialize_object_with_success() {
    let yaml = "{task: foo, when: success}";
    let edge: EdgeSpec = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(edge.task, "foo");
    assert_eq!(edge.when, When::Success);
    assert!(!edge.from_sugar);
}

#[test]
fn test_deserialize_object_with_failure() {
    let yaml = "{task: foo, when: failure}";
    let edge: EdgeSpec = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(edge.task, "foo");
    assert_eq!(edge.when, When::Failure);
    assert!(!edge.from_sugar);
}

#[test]
fn test_deserialize_object_with_always() {
    let yaml = "{task: foo, when: always}";
    let edge: EdgeSpec = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(edge.task, "foo");
    assert_eq!(edge.when, When::Always);
    assert!(!edge.from_sugar);
}

#[test]
fn test_deserialize_object_missing_when_defaults_to_success() {
    let yaml = "{task: foo}";
    let edge: EdgeSpec = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(edge.task, "foo");
    assert_eq!(edge.when, When::Success);
    assert!(!edge.from_sugar);
}

#[test]
fn test_deserialize_missing_task_field_fails() {
    let yaml = "{when: failure}";
    let result: Result<EdgeSpec, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err());
}

#[test]
fn test_deserialize_unknown_field_fails() {
    let yaml = "{task: foo, when: success, extra: bar}";
    let result: Result<EdgeSpec, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err());
}

#[test]
fn test_serialize_sugar_success_emits_bare_string() {
    let edge = EdgeSpec::sugar("foo");
    let yaml = serde_yaml::to_string(&edge).unwrap();
    assert_eq!(yaml.trim(), "foo");
}

#[test]
fn test_serialize_non_sugar_emits_object() {
    let edge = EdgeSpec {
        task: "foo".to_string(),
        when: When::Success,
        from_sugar: false,
        is_injected_sugar: false,
    };
    let yaml = serde_yaml::to_string(&edge).unwrap();
    assert!(yaml.contains("task: foo"));
    assert!(yaml.contains("when: success"));
}

#[test]
fn test_serialize_sugar_with_failure_emits_object() {
    // sugar + non-success → object form (because when != success)
    let edge = EdgeSpec {
        task: "foo".to_string(),
        when: When::Failure,
        from_sugar: true,
        is_injected_sugar: false,
    };
    let yaml = serde_yaml::to_string(&edge).unwrap();
    assert!(yaml.contains("task: foo"));
    assert!(yaml.contains("when: failure"));
}

#[test]
fn test_serialize_failure_emits_object() {
    let edge = EdgeSpec {
        task: "foo".to_string(),
        when: When::Failure,
        from_sugar: false,
        is_injected_sugar: false,
    };
    let yaml = serde_yaml::to_string(&edge).unwrap();
    assert!(yaml.contains("task: foo"));
    assert!(yaml.contains("when: failure"));
}

#[test]
fn test_serialize_always_emits_object() {
    let edge = EdgeSpec {
        task: "foo".to_string(),
        when: When::Always,
        from_sugar: false,
        is_injected_sugar: false,
    };
    let yaml = serde_yaml::to_string(&edge).unwrap();
    assert!(yaml.contains("task: foo"));
    assert!(yaml.contains("when: always"));
}

#[test]
fn test_round_trip_bare_string() {
    let yaml_in = "foo";
    let edge: EdgeSpec = serde_yaml::from_str(yaml_in).unwrap();
    let yaml_out = serde_yaml::to_string(&edge).unwrap();
    assert_eq!(yaml_out.trim(), "foo");
}

#[test]
fn test_round_trip_object_failure() {
    let yaml_in = "{task: foo, when: failure}";
    let edge: EdgeSpec = serde_yaml::from_str(yaml_in).unwrap();
    let yaml_out = serde_yaml::to_string(&edge).unwrap();
    let edge2: EdgeSpec = serde_yaml::from_str(&yaml_out).unwrap();
    assert_eq!(edge.task, edge2.task);
    assert_eq!(edge.when, edge2.when);
}

#[test]
fn test_round_trip_mixed_list() {
    let yaml_in = r#"
- foo
- {task: bar, when: failure}
- baz
- {task: qux, when: always}
"#;
    let edges: Vec<EdgeSpec> = serde_yaml::from_str(yaml_in).unwrap();
    assert_eq!(edges.len(), 4);

    // foo: bare string sugar
    assert_eq!(edges[0].task, "foo");
    assert_eq!(edges[0].when, When::Success);
    assert!(edges[0].from_sugar);

    // bar: object failure
    assert_eq!(edges[1].task, "bar");
    assert_eq!(edges[1].when, When::Failure);
    assert!(!edges[1].from_sugar);

    // baz: bare string sugar
    assert_eq!(edges[2].task, "baz");
    assert_eq!(edges[2].when, When::Success);
    assert!(edges[2].from_sugar);

    // qux: object always
    assert_eq!(edges[3].task, "qux");
    assert_eq!(edges[3].when, When::Always);
    assert!(!edges[3].from_sugar);

    let yaml_out = serde_yaml::to_string(&edges).unwrap();
    // Bare strings should be bare; objects should be objects
    assert!(yaml_out.contains("- foo"));
    assert!(yaml_out.contains("- baz"));
    assert!(yaml_out.contains("when: failure"));
    assert!(yaml_out.contains("when: always"));
}

#[test]
fn test_sugar_constructor() {
    let edge = EdgeSpec::sugar("hello");
    assert_eq!(edge.task, "hello");
    assert_eq!(edge.when, When::Success);
    assert!(edge.from_sugar);
    assert!(!edge.is_injected_sugar);
}

#[test]
fn test_when_serialize_kebab_case() {
    assert_eq!(serde_yaml::to_string(&When::Success).unwrap().trim(), "success");
    assert_eq!(serde_yaml::to_string(&When::Failure).unwrap().trim(), "failure");
    assert_eq!(serde_yaml::to_string(&When::Always).unwrap().trim(), "always");
}
