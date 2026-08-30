use clap::Parser;
use eyre::{Context, Result, bail};
use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::makefile::{Diagnostic, MakefileParser, OttoConverter};

/// Convert Makefile to Otto YAML format
#[derive(Parser, Debug)]
#[command(name = "convert")]
#[command(about = "Convert Makefile to Otto YAML format")]
pub struct ConvertCommand {
    /// Treat warnings as errors
    #[arg(long)]
    pub strict: bool,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

impl ConvertCommand {
    pub fn execute(&self) -> Result<()> {
        // Read from stdin
        let mut content = String::new();
        io::stdin()
            .read_to_string(&mut content)
            .wrap_err("Failed to read from stdin")?;

        let (yaml, diagnostics) = convert_makefile(content)?;

        // Warnings go to stderr because stdout carries the YAML, which is
        // routinely piped straight into an ottofile.
        let mut stderr = io::stderr();
        for diagnostic in &diagnostics {
            writeln!(stderr, "{diagnostic}").ok();
        }

        // `--strict` was a flag nothing read. It now does the only thing it
        // could honestly mean: refuse to emit a conversion that lost something.
        if self.strict && !diagnostics.is_empty() {
            bail!(
                "conversion produced {} warning(s) and --strict was given; no output written",
                diagnostics.len()
            );
        }

        // Write to stdout or file
        if let Some(output_path) = &self.output {
            std::fs::write(output_path, yaml)
                .wrap_err_with(|| format!("Failed to write to file: {}", output_path.display()))?;
        } else {
            io::stdout()
                .write_all(yaml.as_bytes())
                .wrap_err("Failed to write to stdout")?;
        }

        Ok(())
    }
}

/// Convert Makefile text into ottofile YAML plus everything the conversion
/// could not translate. Returns data rather than writing, so the warning policy
/// (`--strict`) and the destination live in one place above.
pub fn convert_makefile(content: String) -> Result<(String, Vec<Diagnostic>)> {
    // Neither call is wrapped: both already report the Makefile line and the
    // construct they choked on, and `main` prints only the outermost message,
    // so a wrapper here is exactly what turned a precise error into
    // "Failed to parse Makefile".
    let mut parser = MakefileParser::new(content);
    let ast = parser.parse()?;

    let mut converter = OttoConverter::new(ast);
    let config = converter.convert()?;

    // Parse-time and convert-time warnings interleave by line rather than by
    // phase, so the operator reads them in the order they appear in the file.
    let mut diagnostics: Vec<Diagnostic> = parser
        .diagnostics()
        .iter()
        .chain(converter.diagnostics())
        .cloned()
        .collect();
    diagnostics.sort_by_key(|d| d.line);

    let yaml = serde_yaml::to_string(&config).wrap_err("Failed to serialize to YAML")?;

    Ok((yaml, diagnostics))
}

#[path = "convert_tests.rs"]
mod tests;
