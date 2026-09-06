use eyre::Result;
use std::path::PathBuf;

use crate::executor::{DagVisualizer, GraphFormat, GraphOptions, NodeStyle};

/// The formats `Graph --format` accepts.
///
/// A CLI-side enum rather than `clap::ValueEnum` on [`GraphFormat`]: the
/// executor's enum also carries `Auto`, which is not a format a user can ask
/// for, so deriving there would advertise a sixth choice that means nothing on
/// the command line. Same arrangement as `StatusFilter` -> `RunStatus` in
/// `history.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GraphFormatArg {
    Ascii,
    Dot,
    Svg,
    Png,
    Pdf,
}

impl From<GraphFormatArg> for GraphFormat {
    fn from(format: GraphFormatArg) -> Self {
        match format {
            GraphFormatArg::Ascii => GraphFormat::Ascii,
            GraphFormatArg::Dot => GraphFormat::Dot,
            GraphFormatArg::Svg => GraphFormat::Svg,
            GraphFormatArg::Png => GraphFormat::Png,
            GraphFormatArg::Pdf => GraphFormat::Pdf,
        }
    }
}

/// Visualize the task dependency graph
///
/// `Graph` is the one builtin with no early route in `main`: it needs the
/// parsed ottofile's task specs, so it is reached by the task route alone. The
/// derive exists anyway, because it is the single declaration the meta task is
/// built from (`cli/parser/meta_tasks.rs`) - the flags in `otto help Graph`
/// are these flags, not a hand-written copy of them.
#[derive(Debug, clap::Parser)]
#[command(name = "Graph")]
pub struct GraphCommand {
    /// Output format
    ///
    /// `ignore_case` per the CLI rule, and so this surface matches the task
    /// route: otto binds a param's `choices` with `ignore_case(true)`
    /// (`cli/parser/command.rs`), and a format spelled `ASCII` must not be
    /// accepted by one route and rejected by the other.
    #[arg(short = 'f', long, value_enum, ignore_case = true, default_value = "ascii")]
    pub format: GraphFormatArg,

    /// Output file path
    #[arg(long)]
    pub output: Option<PathBuf>,
}

impl GraphCommand {
    /// The visualization options this invocation asks for.
    ///
    /// Written out rather than `GraphOptions::default()`, which is `Svg`:
    /// `Graph`'s default is `ascii`, the one format that needs no graphviz and
    /// renders in the terminal that asked for it.
    fn options(&self) -> GraphOptions {
        GraphOptions {
            show_details: true,
            show_file_deps: true,
            format: self.format.into(),
            style: NodeStyle::Detailed,
            output_path: self.output.clone(),
        }
    }

    pub fn execute(&self) -> Result<()> {
        DagVisualizer::render_ottofile_graph(self.options())
    }
}

#[path = "graph_tests.rs"]
mod tests;
