#![cfg(test)]

use super::*;

#[test]
fn test_convert_command_creation() {
    let cmd = ConvertCommand {
        strict: false,
        output: None,
    };
    assert!(!cmd.strict);
    assert!(cmd.output.is_none());
}

#[test]
fn test_convert_command_with_output() {
    let cmd = ConvertCommand {
        strict: true,
        output: Some(PathBuf::from("output.yml")),
    };
    assert!(cmd.strict);
    assert!(cmd.output.is_some());
}

#[test]
fn test_convert_makefile_reports_parser_and_converter_warnings_together() {
    let makefile = "%.o: %.c\n\tgcc -c $< -o $@\n\nbuild:\n\techo $(NOWHERE)\n";

    let (yaml, diagnostics) = convert_makefile(makefile.to_string()).unwrap();

    let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("pattern rule")),
        "parser warnings must survive: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("not defined in the Makefile")),
        "converter warnings must survive: {messages:?}"
    );
    assert!(!yaml.contains("%.o"), "{yaml}");
}

#[test]
fn test_convert_makefile_clean_input_warns_about_nothing() {
    let makefile = ".DEFAULT_GOAL := build\n\nNAME := example\n\nbuild:\n\techo $(NAME)\n";

    let (yaml, diagnostics) = convert_makefile(makefile.to_string()).unwrap();

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(yaml.contains("${NAME}"), "{yaml}");
}

#[test]
fn test_convert_makefile_rejects_a_space_indented_recipe() {
    let makefile = "build:\n    docker run -v /a:/b\n";

    let err = convert_makefile(makefile.to_string()).expect_err("must not convert");

    assert!(format!("{err:#}").contains("indented with spaces"), "{err:#}");
}
