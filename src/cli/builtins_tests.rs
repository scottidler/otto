#![cfg(test)]

use super::*;

#[test]
fn test_builtin_commands_are_capitalized() {
    for cmd in BUILTIN_COMMANDS {
        assert!(cmd.chars().next().unwrap().is_uppercase());
    }
}

#[test]
fn test_is_builtin() {
    assert!(is_builtin("Stats"));
    assert!(is_builtin("Clean"));
    assert!(is_builtin("Graph"));
    assert!(is_builtin("History"));
    assert!(is_builtin("Convert"));
    assert!(is_builtin("Upgrade"));

    // Lowercase should NOT match
    assert!(!is_builtin("stats"));
    assert!(!is_builtin("clean"));

    // Random names should NOT match
    assert!(!is_builtin("test"));
    assert!(!is_builtin("build"));
}

#[test]
fn test_builtin_params_are_capitalized() {
    for param in BUILTIN_PARAMS {
        assert!(param.chars().next().unwrap().is_uppercase());
    }
}

#[test]
fn test_is_builtin_param() {
    assert!(is_builtin_param("Serial"));

    // Lowercase should NOT match
    assert!(!is_builtin_param("serial"));

    // Random names should NOT match
    assert!(!is_builtin_param("verbose"));
    assert!(!is_builtin_param("format"));
}
