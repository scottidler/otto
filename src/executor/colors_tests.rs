#![cfg(test)]

use super::*;

#[test]
fn test_consistent_color_assignment() {
    // Same task name should always get same color
    let color1 = get_task_color("build");
    let color2 = get_task_color("build");
    assert_eq!(color1, color2);

    // Test that we're cycling through our expected range
    for i in 0..16 {
        let task_name = format!("task_{i}");
        let color = get_task_color(&task_name);
        // Just ensure we can get a color without panicking
        let _ = format!("{color:?}");
    }
}

#[test]
fn test_colorize_functions() {
    let task_name = "test_task";

    // These should not panic and should return strings
    let colored_name = colorize_task_name(task_name);
    let colored_prefix = colorize_task_prefix(task_name);

    assert!(colored_name.contains("test_task"));
    // The colored prefix contains ANSI escape codes, so we need to check for the task name
    // and the brackets separately, or check that it contains the task name
    assert!(colored_prefix.contains("test_task"));
    assert!(colored_prefix.contains("["));
    assert!(colored_prefix.contains("]"));
}

#[test]
fn test_get_task_color_combination() {
    // Test direct color combination retrieval
    let (bracket, text) = get_task_color_combination("some_task");
    // Bracket and text colors should be different
    assert_ne!(format!("{bracket:?}"), format!("{text:?}"));
}

#[test]
fn test_set_global_task_order() {
    // Test that setting global task order doesn't panic
    set_global_task_order(vec!["z".to_string(), "a".to_string(), "m".to_string()]);

    // Get color for a task after setting order
    let (bracket, text) = get_task_color_combination("a");
    let _ = format!("{bracket:?} {text:?}");
}

#[test]
fn test_color_combinations_count() {
    // Verify we have 15 unique color combinations
    assert_eq!(COLOR_COMBINATIONS.len(), 15);

    // Verify all bracket and text colors are different in each combination
    for (bracket, text) in COLOR_COMBINATIONS.iter() {
        assert_ne!(format!("{bracket:?}"), format!("{text:?}"));
    }
}
