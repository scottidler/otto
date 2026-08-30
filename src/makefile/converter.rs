use eyre::Result;
use std::collections::{HashMap, HashSet};

use crate::cfg::config::ConfigSpec;
use crate::cfg::otto::OttoSpec;
use crate::cfg::task::{TaskSpec, TaskSpecs};

use super::ast::{AssignmentType, MakefileAst, Target};
use super::diagnostic::Diagnostic;

/// Make variables that name something about make itself. Emitting them as
/// shell variables is the only thing otto can do, and it is always wrong, so
/// each one is called out by name.
const SPECIAL_MAKE_VARIABLES: &[&str] = &[
    "MAKE",
    "MAKEFILE_LIST",
    "MAKECMDGOALS",
    "MAKEFLAGS",
    "MAKELEVEL",
    "CURDIR",
    "SUFFIXES",
    "VARIABLES",
];

/// Make's automatic variables. They are defined per-rule from the target and
/// its prerequisites; otto has no equivalent, and leaving them in the script
/// means bash reads `$@` as its own positional arguments.
const AUTOMATIC_VARIABLES: &[char] = &['@', '<', '^', '?', '*', '+', '%', '|'];

pub struct OttoConverter {
    ast: MakefileAst,
    diagnostics: Vec<Diagnostic>,
}

impl OttoConverter {
    pub fn new(ast: MakefileAst) -> Self {
        Self {
            ast,
            diagnostics: Vec::new(),
        }
    }

    /// Everything the conversion could see but not translate faithfully.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn warn(&mut self, line: usize, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::at(line, message));
    }

    pub fn convert(&mut self) -> Result<ConfigSpec> {
        let otto_spec = self.convert_otto_spec();
        let tasks = self.convert_targets();

        // A default goal naming a rule that was skipped (a pattern rule, say)
        // produces an ottofile that cannot run its own default.
        for goal in &otto_spec.tasks {
            if goal != "*" && !tasks.contains_key(goal) {
                self.diagnostics.push(Diagnostic::detached(format!(
                    "default goal `{goal}` has no converted task; otto will not be able to run it"
                )));
            }
        }

        Ok(ConfigSpec { otto: otto_spec, tasks })
    }

    fn convert_otto_spec(&mut self) -> OttoSpec {
        let envs = self.convert_variables();
        let tasks = self.determine_default_tasks();

        // Everything but the four fields a conversion actually determines is
        // left at its default, so the emitted ottofile carries no value it did
        // not learn from the Makefile. `jobs` in particular used to be baked to
        // the converting machine's CPU count.
        OttoSpec {
            about: "Converted from Makefile".to_string(),
            tasks,
            envs,
            ..OttoSpec::default()
        }
    }

    /// The rule names the Makefile itself defines, which is exactly the set of
    /// tasks the conversion will produce.
    fn known_targets(&self) -> HashSet<String> {
        self.ast.targets.iter().map(|t| t.name.clone()).collect()
    }

    /// The variable names the Makefile itself defines. A `$(NAME)` that is one
    /// of these converts cleanly; one that is not can only come from the
    /// environment, and the operator is told so.
    fn known_variables(&self) -> HashSet<String> {
        self.ast.variables.iter().map(|v| v.name.clone()).collect()
    }

    fn convert_variables(&mut self) -> HashMap<String, String> {
        let known = self.known_variables();
        let mut envs = HashMap::new();

        for var in self.ast.variables.clone() {
            if var.assignment_type == AssignmentType::Append {
                self.warn(
                    var.line,
                    format!(
                        "`{} +=` cannot append in otto; only the appended text is kept",
                        var.name
                    ),
                );
            }

            // `VERSION := $(shell git describe)` used to be emitted verbatim,
            // and otto then tried to run a command called `shell`, failing the
            // whole config load - which took every other env down with it.
            let value = self.rewrite_expansions(&var.value, var.line, &known);
            envs.insert(var.name.clone(), value);
        }

        envs
    }

    fn convert_targets(&mut self) -> TaskSpecs {
        let mut tasks = TaskSpecs::new();
        let mut seen: HashMap<String, usize> = HashMap::new();

        for target in self.ast.targets.clone() {
            // Last-wins matches make's own "overriding recipe" behavior; what
            // it never did was say so.
            if let Some(previous) = seen.insert(target.name.clone(), target.line) {
                self.warn(
                    target.line,
                    format!(
                        "duplicate target `{}` overrides the rule at line {}, discarding its dependencies and recipe",
                        target.name, previous
                    ),
                );
            }
            let task = self.convert_target_to_task(&target);
            tasks.insert(target.name.clone(), task);
        }

        tasks
    }

    fn convert_target_to_task(&mut self, target: &Target) -> TaskSpec {
        let known = self.known_variables();
        let action = self.build_bash_action(&target.commands, target.line, &known);

        // Dependencies in Make become "before" in Otto
        // (task X depends on Y means Y must run before X)
        // Emit as sugar (bare-string) edges so converted ottofiles read naturally.
        let known_targets = self.known_targets();
        for dependency in &target.dependencies {
            if dependency.contains('$') {
                self.warn(
                    target.line,
                    format!(
                        "dependency `{dependency}` of `{}` is a make expansion; otto task names cannot be computed",
                        target.name
                    ),
                );
            } else if !known_targets.contains(dependency) {
                // In make this is a file, or a rule that lives in an included
                // Makefile. In otto it is a `before:` edge to a task that does
                // not exist, and the whole ottofile fails to load.
                self.warn(
                    target.line,
                    format!(
                        "dependency `{dependency}` of `{}` has no rule in this Makefile; otto will reject the edge",
                        target.name
                    ),
                );
            }
        }

        let before: Vec<crate::cfg::edge::EdgeSpec> = target
            .dependencies
            .iter()
            .map(crate::cfg::edge::EdgeSpec::sugar)
            .collect();

        TaskSpec {
            name: target.name.clone(),
            help: target.comment.clone(),
            after: Vec::new(),
            before,
            input: Vec::new(),
            output: Vec::new(),
            envs: HashMap::new(),
            params: crate::cfg::param::ParamSpecs::new(),
            action,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: Vec::new(),
        }
    }

    fn build_bash_action(&mut self, commands: &[String], line: usize, known: &HashSet<String>) -> String {
        if commands.is_empty() {
            return "#!/bin/bash\n".to_string();
        }

        let mut script = String::from("#!/bin/bash\n");

        for cmd in commands {
            let trimmed = cmd.trim();

            // Remove Make-specific prefixes
            let cleaned_cmd = if let Some(cmd_without_at) = trimmed.strip_prefix('@') {
                // @ suppresses echo in Make, not needed in Otto
                cmd_without_at.trim_start()
            } else if let Some(cmd_without_dash) = trimmed.strip_prefix('-') {
                // - ignores errors in Make
                // In Otto/bash, we can use `|| true` for this
                let cmd_without_prefix = cmd_without_dash.trim_start().to_string();
                let rewritten = self.rewrite_expansions(&cmd_without_prefix, line, known);
                script.push_str(&rewritten);
                script.push_str(" || true\n");
                continue;
            } else {
                trimmed
            };

            // `$(VAR)` used to go into bash verbatim, where it is command
            // substitution: it printed "VAR: command not found", expanded to
            // nothing, and the task still succeeded.
            let rewritten = self.rewrite_expansions(cleaned_cmd, line, known);
            script.push_str(&rewritten);
            script.push('\n');
        }

        // No trailing newline: a YAML block scalar loses it on the way back in,
        // so keeping it would make convert -> serialize -> load a config that
        // no longer equals the one that was written.
        script.truncate(script.trim_end_matches('\n').len());
        script
    }

    /// Rewrite make's expansion syntax into bash, warning about every construct
    /// that cannot survive the trip.
    fn rewrite_expansions(&mut self, text: &str, line: usize, known: &HashSet<String>) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;

        while i < chars.len() {
            if chars[i] != '$' {
                out.push(chars[i]);
                i += 1;
                continue;
            }

            match chars.get(i + 1) {
                // `$$` is make's escape for a literal `$`; the shell must see
                // one dollar, not two (`awk '{print $$1}'` -> `$1`, not the pid).
                Some('$') => {
                    out.push('$');
                    i += 2;
                }
                Some(&open @ ('(' | '{')) => match matching_close(&chars, i + 1) {
                    Some(end) => {
                        let inner: String = chars[i + 2..end].iter().collect();
                        let rewritten = self.rewrite_expansion(open, &inner, line, known);
                        out.push_str(&rewritten);
                        i = end + 1;
                    }
                    None => {
                        self.warn(line, format!("unbalanced `${open}` in `{text}`; left as written"));
                        out.push('$');
                        i += 1;
                    }
                },
                Some(&c) if AUTOMATIC_VARIABLES.contains(&c) => {
                    self.warn(
                        line,
                        format!("automatic variable `${c}` has no otto equivalent; left as written"),
                    );
                    out.push('$');
                    out.push(c);
                    i += 2;
                }
                Some(&c) => {
                    out.push('$');
                    out.push(c);
                    i += 2;
                }
                None => {
                    out.push('$');
                    i += 1;
                }
            }
        }

        out
    }

    /// Rewrite the inside of one `$(...)` / `${...}`.
    fn rewrite_expansion(&mut self, open: char, inner: &str, line: usize, known: &HashSet<String>) -> String {
        let verbatim = || match open {
            '(' => format!("$({inner})"),
            _ => format!("${{{inner}}}"),
        };

        // `$(shell CMD)` is exactly bash's `$(CMD)`, which otto's env evaluator
        // and every task script already understand.
        if let Some(command) = inner.strip_prefix("shell ") {
            let rewritten = self.rewrite_expansions(command.trim(), line, known);
            return format!("$({rewritten})");
        }

        if let Some(first) = inner.chars().next()
            && AUTOMATIC_VARIABLES.contains(&first)
        {
            self.warn(
                line,
                format!(
                    "automatic variable `{}` has no otto equivalent; left as written",
                    verbatim()
                ),
            );
            return verbatim();
        }

        if !is_shell_identifier(inner) {
            // A make function call, or a name bash cannot spell. Translating it
            // would be a guess; saying so is not.
            self.warn(
                line,
                format!(
                    "`{}` is a make expression otto cannot translate; left as written",
                    verbatim()
                ),
            );
            return verbatim();
        }

        if SPECIAL_MAKE_VARIABLES.contains(&inner) {
            self.warn(
                line,
                format!("`{inner}` is a make-internal variable; it will be empty in otto"),
            );
        } else if !known.contains(inner) {
            self.warn(
                line,
                format!("`{inner}` is not defined in the Makefile; it will come from the environment"),
            );
        }

        format!("${{{inner}}}")
    }

    fn determine_default_tasks(&self) -> Vec<String> {
        if let Some(ref default_goal) = self.ast.default_goal {
            vec![default_goal.clone()]
        } else if !self.ast.targets.is_empty() {
            // If no default goal, use the first target
            vec![self.ast.targets[0].name.clone()]
        } else {
            vec!["*".to_string()]
        }
    }
}

/// Index of the `)`/`}` matching the opener at `open`, counting nested pairs of
/// the same kind, or `None` when the expansion is never closed.
fn matching_close(chars: &[char], open: usize) -> Option<usize> {
    let (opener, closer) = match chars.get(open)? {
        '(' => ('(', ')'),
        '{' => ('{', '}'),
        _ => return None,
    };

    let mut depth = 0usize;
    for (offset, c) in chars[open..].iter().enumerate() {
        if *c == opener {
            depth += 1;
        } else if *c == closer {
            depth -= 1;
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }

    None
}

/// True when the name can be spelled as a bash variable. `$(BINARY-NAME)` can
/// not: bash reads `${BINARY-NAME}` as "BINARY, or NAME if unset".
fn is_shell_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[path = "converter_tests.rs"]
mod tests;
