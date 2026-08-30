#![cfg(test)]

use super::*;

#[test]
fn test_display_with_line() {
    let d = Diagnostic::at(12, "pattern rules are not supported");
    assert_eq!(d.to_string(), "Makefile:12: warning: pattern rules are not supported");
}

#[test]
fn test_display_without_line() {
    let d = Diagnostic::detached("duplicate target");
    assert_eq!(d.to_string(), "Makefile: warning: duplicate target");
}
