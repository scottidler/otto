#![cfg(test)]

use super::*;
use clap::Parser as _;

#[test]
fn test_graph_command_defaults_to_ascii() {
    let cmd = GraphCommand::try_parse_from(["Graph"]).expect("no args is a valid Graph invocation");
    assert_eq!(cmd.format, GraphFormatArg::Ascii);
    assert!(cmd.output.is_none());
}

#[test]
fn test_graph_command_reads_format_and_output() {
    let cmd = GraphCommand::try_parse_from(["Graph", "--format", "dot", "--output", "dag.dot"])
        .expect("dot is a declared format");
    assert_eq!(cmd.format, GraphFormatArg::Dot);
    assert_eq!(cmd.output, Some(PathBuf::from("dag.dot")));
}

#[test]
fn test_graph_command_format_ignores_case() {
    let cmd = GraphCommand::try_parse_from(["Graph", "-f", "SVG"]).expect("case does not matter");
    assert_eq!(cmd.format, GraphFormatArg::Svg);
}

#[test]
fn test_graph_command_rejects_an_unknown_format() {
    let err =
        GraphCommand::try_parse_from(["Graph", "--format", "mermaid"]).expect_err("mermaid is not a declared format");
    assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
}

#[test]
fn test_graph_options_carry_the_requested_format_not_the_default_svg() {
    let cmd = GraphCommand::try_parse_from(["Graph"]).expect("no args is a valid Graph invocation");
    let options = cmd.options();
    assert!(matches!(options.format, GraphFormat::Ascii));
    assert!(options.output_path.is_none());
    assert!(options.show_details);
    assert!(options.show_file_deps);
}

#[test]
fn test_graph_options_carry_the_output_path() {
    let cmd = GraphCommand::try_parse_from(["Graph", "--format", "png", "--output", "/tmp/dag.png"])
        .expect("png is a declared format");
    let options = cmd.options();
    assert!(matches!(options.format, GraphFormat::Png));
    assert_eq!(options.output_path, Some(PathBuf::from("/tmp/dag.png")));
}
