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
    /// Targets whose name is one of otto's built-in command names, mapped to
    /// the task name they are emitted under. See [`builtin_renames`].
    renames: HashMap<String, String>,
}

impl OttoConverter {
    pub fn new(ast: MakefileAst) -> Self {
        let renames = builtin_renames(&ast);
        Self {
            ast,
            diagnostics: Vec::new(),
            renames,
        }
    }

    /// The task a target is emitted as: its own name, unless that name is a
    /// built-in command's. Every place a target name reaches the output goes
    /// through here (the task key, `before:` edges, `otto.tasks`), so a rename
    /// is applied everywhere or nowhere.
    fn task_name(&self, target: &str) -> String {
        self.renames.get(target).cloned().unwrap_or_else(|| target.to_string())
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

        // A conversion that produced no tasks is a conversion that did
        // nothing, and it used to say so only by printing `tasks: {}` at exit
        // 0 - including under `--strict`, whose whole job is to refuse a lossy
        // conversion. Empty input, a Makefile of nothing but variables, or one
        // whose every rule was skipped all land here, and none of them is what
        // the operator asked for.
        if tasks.is_empty() {
            self.diagnostics.push(Diagnostic::detached(
                "no targets were converted; the output has no tasks".to_string(),
            ));
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

            // `?=` assigns only if the variable has no value yet, so make lets
            // the ambient environment win. otto's `envs:` is "the system
            // environment minus every declared key" (`cfg/env.rs`), so a
            // declared key unconditionally shadows the ambient one and the
            // conditional half is lost. Measured against real make:
            //   VERSION=9.9 make -s show  -> 9.9
            //   VERSION=9.9 otto show     -> 1.0
            // `+=` already warns for exactly this class of loss; `?=` warned
            // for nothing at all, which is the silent corruption this phase's
            // "every silent corruption above becomes a warning at minimum"
            // criterion exists to forbid.
            if var.assignment_type == AssignmentType::Conditional {
                self.warn(
                    var.line,
                    format!(
                        "`{} ?=` is conditional in make; otto always uses this value, ignoring any `{}` already in the environment",
                        var.name, var.name
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
            // `is_phony` is set from `.PHONY` and was read by nobody:
            // `TaskSpec` has no phony field, so the converter dropped it. It
            // carries real information, and losing it is the same silent class
            // as `?=`. make skips a non-phony target's recipe when the target
            // is newer than its prerequisites; otto has no such check and runs
            // the converted task every time.
            //
            // The signal is prerequisites, not the target's NAME. A first
            // attempt guessed from the name - `/` or a dot-extension - and was
            // wrong in both directions: `myapp: main.o`, the canonical Unix
            // file rule, got no warning, while a bare `deploy.prod` got a false
            // one. Prerequisites are what make's up-to-date comparison actually
            // needs, so that is what is tested. A recipe is required too: a
            // target with prerequisites and no recipe is a dependency
            // declaration, and there is nothing for otto to run differently.
            //
            // Deliberate miss: a file target with NO prerequisites (`app.tar.gz:`
            // alone). make still checks existence there, but nothing in the rule
            // distinguishes it from a plain task name, and guessing from the
            // name is what this replaced.
            // `!target.is_phony` only means "not phony" when `.PHONY` was fully
            // resolvable. `.PHONY: $(TARGETS)` is a variable this parser does not
            // expand, so every target it covers looks non-phony and warns - and
            // under `--strict` that is exit 1 on a correct Makefile. Measured
            // before this guard: a three-line Makefile with `.PHONY: $(TARGETS)`
            // failed `otto Convert --strict` with two warnings, both wrong.
            if !self.ast.phony_unresolved
                && !target.is_phony
                && !target.dependencies.is_empty()
                && !target.commands.is_empty()
            {
                self.warn(
                    target.line,
                    format!(
                        "`{}` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
                        target.name
                    ),
                );
            }

            // The loader reserves the built-in command names (`Clean`,
            // `History`, ...) for the builtins themselves, so an ottofile with
            // a task called `Clean` fails to load. Emitting the target under
            // that name produced output this converter's own consumer rejects,
            // at exit 0, with `--strict` silent: v2.2.1 loaded it because the
            // reservation did not exist yet. The rename is a lossy conversion
            // like any other here, so it warns, and `--strict` refuses it.
            let name = self.task_name(&target.name);
            if name != target.name {
                self.warn(
                    target.line,
                    format!(
                        "target `{}` is otto's built-in command name and cannot be a task; converted as `{name}`",
                        target.name
                    ),
                );
            }

            let task = self.convert_target_to_task(&target);
            tasks.insert(name, task);
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

        // A dependency that is a make expansion is dropped, not emitted. Its
        // name cannot be computed here, so the edge can only ever name a task
        // that does not exist, and otto refuses the whole run with `Task 'foo'
        // has unknown dependency '$(OBJS)'`. Emitting it turned every Makefile
        // carrying one into an ottofile that converts at exit 0 and then cannot
        // run at all, with only `--strict` (which refuses any warning) catching
        // it. Same treatment the target side already gives an expansion it
        // cannot compute: warn above, and leave it out.
        //
        // Deliberately narrower than it could be: a dependency that merely
        // names no rule in THIS Makefile (`package: build`) keeps its edge,
        // because that name may be a real task in an included Makefile or a
        // file the author means to add. An expansion can never be a task name.
        let before: Vec<crate::cfg::edge::EdgeSpec> = target
            .dependencies
            .iter()
            .filter(|dependency| !dependency.contains('$'))
            .map(|dependency| crate::cfg::edge::EdgeSpec::sugar(self.task_name(dependency)))
            .collect();

        TaskSpec {
            name: self.task_name(&target.name),
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
            let mut rest = cmd.trim();
            // Make allows `@`, `-`, and `+` in any order, repeated
            // (`@-cmd`, `-@cmd`, `+cmd`, ...): `@` suppresses echo, `-`
            // ignores the command's exit status, `+` always runs the
            // recipe line even under `-n`. Otto's script always echoes and
            // `+` has no otto equivalent (there is no "dry run" to override),
            // so both are just stripped; `-` becomes `|| true`. The old code
            // peeled at most one prefix character, so `@-rm -rf dist` kept
            // the literal `-` in the command instead of enabling ignore-errors,
            // and `-@echo hi` left a literal `@` for bash to choke on.
            let mut ignore_errors = false;
            loop {
                if let Some(r) = rest.strip_prefix('@') {
                    rest = r.trim_start();
                } else if let Some(r) = rest.strip_prefix('-') {
                    ignore_errors = true;
                    rest = r.trim_start();
                } else if let Some(r) = rest.strip_prefix('+') {
                    rest = r.trim_start();
                } else {
                    break;
                }
            }

            // `$(VAR)` used to go into bash verbatim, where it is command
            // substitution: it printed "VAR: command not found", expanded to
            // nothing, and the task still succeeded.
            let rewritten = self.rewrite_expansions(rest, line, known);
            script.push_str(&rewritten);
            if ignore_errors {
                script.push_str(" || true");
            }
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
            vec![self.task_name(default_goal)]
        } else if !self.ast.targets.is_empty() {
            // If no default goal, use the first target
            vec![self.task_name(&self.ast.targets[0].name)]
        } else {
            vec!["*".to_string()]
        }
    }
}

/// For every target named like a built-in command (`Clean`, `Convert`, `Graph`,
/// `History`, `Stats`, `Upgrade`), the task name it is emitted under: the same
/// name with its first letter lowercased, which is how the Makefile would have
/// spelled it in the first place, and `-task` appended as many times as it
/// takes to avoid a target that already has that name. The check is the
/// loader's own (`is_builtin`), so the two cannot disagree about which names
/// are reserved.
fn builtin_renames(ast: &MakefileAst) -> HashMap<String, String> {
    let taken: HashSet<&str> = ast.targets.iter().map(|t| t.name.as_str()).collect();
    let mut renames = HashMap::new();
    for target in &ast.targets {
        if !crate::cli::builtins::is_builtin(&target.name) || renames.contains_key(&target.name) {
            continue;
        }
        let mut chars = target.name.chars();
        let mut candidate = match chars.next() {
            Some(first) => first.to_lowercase().chain(chars).collect::<String>(),
            None => continue,
        };
        while taken.contains(candidate.as_str()) || renames.values().any(|v| v == &candidate) {
            candidate.push_str("-task");
        }
        renames.insert(target.name.clone(), candidate);
    }
    renames
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
