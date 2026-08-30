use eyre::{Result, bail};

use super::ast::{AssignmentType, MakefileAst, Target, Variable};
use super::diagnostic::Diagnostic;

/// One logical Makefile line: every `\`-continued physical line already joined,
/// tagged with the 1-based physical line the logical line starts on.
///
/// Continuation joining is a preprocessing pass rather than something each
/// construct handles for itself, because make does it that way and the old
/// per-construct handling only covered recipes: `SOURCES := a.c \` silently
/// truncated, and a space-indented continuation inside a recipe was parsed as
/// a brand-new target.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LogicalLine {
    number: usize,
    text: String,
}

impl LogicalLine {
    /// A recipe line is tab-indented. Make's one piece of syntax that is
    /// whitespace-sensitive, and the reason the space-indented cases below are
    /// errors instead of targets.
    fn is_recipe(&self) -> bool {
        self.text.starts_with('\t')
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

pub struct MakefileParser {
    content: String,
    diagnostics: Vec<Diagnostic>,
}

impl MakefileParser {
    pub fn new(content: String) -> Self {
        Self {
            content,
            diagnostics: Vec::new(),
        }
    }

    /// Everything the parse could see but not translate. Empty is the only
    /// honest claim that a Makefile converted cleanly.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn warn(&mut self, line: usize, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::at(line, message));
    }

    pub fn parse(&mut self) -> Result<MakefileAst> {
        let mut ast = MakefileAst::new();
        let lines = logical_lines(&self.content);
        let mut i = 0;
        let mut last_comment: Option<String> = None;
        let mut conditional_depth = 0; // Track nesting depth of conditionals

        while i < lines.len() {
            let number = lines[i].number;
            let raw = lines[i].text.clone();
            let is_recipe = lines[i].is_recipe();
            let trimmed = raw.trim().to_string();

            // Skip empty lines
            if trimmed.is_empty() {
                last_comment = None;
                i += 1;
                continue;
            }

            // Handle comments
            if let Some(comment_text) = trimmed.strip_prefix('#') {
                last_comment = Some(comment_text.trim().to_string());
                i += 1;
                continue;
            }

            // Track conditional block depth
            if trimmed.starts_with("ifeq")
                || trimmed.starts_with("ifneq")
                || trimmed.starts_with("ifdef")
                || trimmed.starts_with("ifndef")
            {
                if conditional_depth == 0 {
                    self.warn(number, "conditional block skipped; its body is not converted");
                }
                conditional_depth += 1;
                last_comment = None;
                i += 1;
                continue;
            }

            if trimmed.starts_with("endif") {
                if conditional_depth > 0 {
                    conditional_depth -= 1;
                }
                last_comment = None;
                i += 1;
                continue;
            }

            // Skip content inside conditional blocks
            if conditional_depth > 0 || trimmed.starts_with("else") {
                last_comment = None;
                i += 1;
                continue;
            }

            // A `define ... endef` body is arbitrary text, not makefile syntax:
            // parsing it line by line invents targets out of any line holding a
            // colon. Skip the whole block, once, out loud.
            if trimmed.starts_with("define ") || trimmed == "define" {
                self.warn(number, "multi-line variable (define) is not supported; skipped");
                i += 1;
                while i < lines.len() && !lines[i].text.trim().starts_with("endef") {
                    i += 1;
                }
                i += 1; // consume the endef
                last_comment = None;
                continue;
            }

            // Directives otto has no equivalent for. Each one used to vanish.
            if let Some(directive) = unsupported_directive(&trimmed) {
                self.warn(number, format!("`{directive}` is not supported; the line is ignored"));
                last_comment = None;
                i += 1;
                continue;
            }

            // Check for .PHONY declaration
            if let Some(phony_targets) = is_phony_declaration(&trimmed) {
                for target in phony_targets {
                    ast.phony_targets.insert(target);
                }
                last_comment = None;
                i += 1;
                continue;
            }

            // Check for .DEFAULT_GOAL
            if let Some(goal) = extract_default_goal(&trimmed) {
                ast.default_goal = Some(goal);
                last_comment = None;
                i += 1;
                continue;
            }

            // `export VAR := value` is an assignment wearing a directive: otto
            // exports every declared env to its tasks, so the keyword is
            // recorded and dropped rather than becoming a variable literally
            // named "export VAR".
            let (assignment_text, exported) = match strip_export(&trimmed) {
                Some(rest) if !is_recipe => (rest.to_string(), true),
                _ => (raw.clone(), false),
            };

            // Check for variable assignment
            if !is_recipe && let Some(mut var) = parse_variable(&assignment_text)? {
                var.line = number;
                if exported {
                    self.warn(
                        number,
                        format!("`export` on `{}` dropped; otto exports every declared env", var.name),
                    );
                }
                // Make functions have no otto equivalent, so the variable is
                // dropped - but never again in silence.
                if contains_make_functions(&var.value) {
                    self.warn(
                        number,
                        format!(
                            "variable `{}` uses a make function otto cannot evaluate; the variable is dropped",
                            var.name
                        ),
                    );
                } else {
                    ast.variables.push(var);
                }
                last_comment = None;
                i += 1;
                continue;
            }

            if exported {
                self.warn(
                    number,
                    format!("`{trimmed}` has no value to convert; the line is ignored"),
                );
                last_comment = None;
                i += 1;
                continue;
            }

            // Check for target definition. The old guard tested `trimmed` for a
            // leading tab, which a trimmed string can never have, so recipe
            // lines holding a colon became targets. Test the raw line.
            if trimmed.contains(':') && !is_recipe {
                // A space-indented line that looks like a recipe IS a recipe
                // the author mis-indented; make itself errors here. Converting
                // it into a target is how `-v /a:/b` became a task named
                // `-v /a`.
                if raw.starts_with(' ') {
                    bail!(
                        "Makefile:{number}: recipe line is indented with spaces, not a tab: `{trimmed}`. \
                         Make requires a tab; otto will not guess whether this is a recipe or a target."
                    );
                }
                if let Some(targets) = self.parse_rule(&lines, &mut i, last_comment.clone())? {
                    ast.targets.extend(targets);
                }
                last_comment = None;
                continue;
            }

            if is_recipe {
                self.warn(number, "recipe line does not belong to any rule; it is ignored");
            } else {
                self.warn(number, format!("unrecognized line ignored: `{trimmed}`"));
            }
            last_comment = None;
            i += 1;
        }

        // `.PHONY` was collected and then thrown away: every target shipped
        // with `is_phony: false` regardless.
        for target in &mut ast.targets {
            target.is_phony = ast.phony_targets.contains(&target.name);
        }

        Ok(ast)
    }

    /// Parse one rule. Returns every target it declares: `test check: build` is
    /// two rules in make, and used to become a single task named "test check".
    fn parse_rule(
        &mut self,
        lines: &[LogicalLine],
        index: &mut usize,
        comment: Option<String>,
    ) -> Result<Option<Vec<Target>>> {
        let number = lines[*index].number;
        let raw = lines[*index].text.clone();
        let (body, inline_comment) = split_inline_comment(&raw);
        let trimmed = body.trim().to_string();

        let colon_pos = match trimmed.find(':') {
            Some(pos) => pos,
            None => {
                self.warn(number, format!("unrecognized line ignored: `{trimmed}`"));
                *index += 1;
                return Ok(None);
            }
        };

        let target_part = trimmed[..colon_pos].trim().to_string();

        // `install:: build` - double-colon rules accumulate recipes in make and
        // have no otto equivalent. The old code kept the second colon and
        // emitted `before: [':', 'build']`.
        let double_colon = trimmed[colon_pos + 1..].starts_with(':');
        let dep_start = if double_colon { colon_pos + 2 } else { colon_pos + 1 };
        let dep_part = trimmed[dep_start..].trim().to_string();

        // Special targets (.SUFFIXES, .SILENT, ...) are not rules to convert.
        if target_part.is_empty() || target_part.starts_with('.') {
            self.warn(number, format!("special target `{target_part}` is not converted"));
            *index += 1;
            self.skip_recipe(lines, index);
            return Ok(None);
        }

        if double_colon {
            self.warn(
                number,
                format!("`{target_part}` is a double-colon rule; converted as an ordinary rule"),
            );
        }

        // `build: CFLAGS=-g` sets a variable for one rule's recipes. It is not
        // a rule and has no recipe of its own; it used to become
        // `before: ['CFLAGS=-g']`.
        if is_target_specific_variable(&dep_part) {
            self.warn(
                number,
                format!("target-specific variable on `{target_part}` is not supported; the line is ignored"),
            );
            *index += 1;
            return Ok(None);
        }

        // `objs: %.o: %.c` - a static pattern rule, whose middle field is not a
        // dependency list at all.
        if dep_part.contains(':') {
            self.warn(
                number,
                format!("static pattern rule `{target_part}` is not supported; the rule is skipped"),
            );
            *index += 1;
            self.skip_recipe(lines, index);
            return Ok(None);
        }

        let names: Vec<String> = target_part.split_whitespace().map(|s| s.to_string()).collect();

        // `%.o: %.c` - a pattern rule is a template, not a task. It used to
        // become a task literally named `%.o`, which then also became the
        // default task and failed the load with "unknown dependency '%.c'".
        if names.iter().any(|n| n.contains('%')) {
            self.warn(
                number,
                format!("pattern rule `{target_part}` is not supported; the rule is skipped"),
            );
            *index += 1;
            self.skip_recipe(lines, index);
            return Ok(None);
        }

        if names.len() > 1 {
            self.warn(
                number,
                format!(
                    "`{target_part}` declares {} targets in one rule; each becomes its own task with the same recipe",
                    names.len()
                ),
            );
        }

        let dependencies: Vec<String> = dep_part.split_whitespace().map(|s| s.to_string()).collect();

        *index += 1;
        let commands = self.parse_commands(lines, index)?;

        // `help: ## Help me.` is the self-documenting-makefile convention, and
        // `#` starts a comment on any non-recipe line anyway: the old parser
        // read `## Help me.` as three dependencies.
        let help = comment.or_else(|| {
            inline_comment
                .map(|c| c.trim_start_matches('#').trim().to_string())
                .filter(|c| !c.is_empty())
        });

        Ok(Some(
            names
                .into_iter()
                .map(|name| Target {
                    name,
                    dependencies: dependencies.clone(),
                    commands: commands.clone(),
                    comment: help.clone(),
                    is_phony: false, // set from .PHONY once the whole file is parsed
                    line: number,
                })
                .collect(),
        ))
    }

    /// Consume the recipe of a rule that is not being converted, so its command
    /// lines do not resurface at top level as invented targets.
    fn skip_recipe(&mut self, lines: &[LogicalLine], index: &mut usize) {
        while *index < lines.len() && (lines[*index].is_recipe() || lines[*index].is_blank()) {
            *index += 1;
        }
    }

    fn parse_commands(&mut self, lines: &[LogicalLine], index: &mut usize) -> Result<Vec<String>> {
        let mut commands = Vec::new();

        loop {
            // A blank line does not end a recipe in make. Treating it as the end
            // dropped everything after the first blank line inside a recipe, and
            // any of those orphaned lines holding a colon became a target.
            let mut probe = *index;
            while probe < lines.len() && lines[probe].is_blank() {
                probe += 1;
            }
            let Some(line) = lines.get(probe) else { break };

            if let Some(command) = line.text.strip_prefix('\t') {
                *index = probe + 1;
                commands.push(command.to_string());
                continue;
            }

            // Space-indented where a recipe belongs: make errors, and the old
            // parser invented a target. An indented assignment is legal make,
            // so it ends the recipe instead of failing the parse.
            if line.text.starts_with(' ')
                && !line.text.trim_start().starts_with('#')
                && parse_variable(&line.text)?.is_none()
            {
                bail!(
                    "Makefile:{}: recipe line is indented with spaces, not a tab: `{}`. \
                     Make requires a tab; otto will not guess whether this is a recipe or a target.",
                    line.number,
                    line.text.trim()
                );
            }

            break;
        }

        Ok(commands)
    }
}

/// Join `\`-continued physical lines, as make does, for every kind of line.
fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let physical: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < physical.len() {
        let number = i + 1;
        let mut text = String::new();

        loop {
            let line = physical[i];
            let continued = continuation_head(line);
            let piece = continued.unwrap_or(line);

            if text.is_empty() {
                // Leading whitespace of the FIRST physical line is syntax
                // (tab = recipe), so it survives; a continuation's indentation
                // is not, and collapses to the single joining space.
                text.push_str(piece.trim_end());
            } else {
                text.push(' ');
                text.push_str(piece.trim());
            }

            i += 1;
            if continued.is_none() || i >= physical.len() {
                break;
            }
        }

        out.push(LogicalLine { number, text });
    }

    out
}

/// The line without its trailing continuation backslash, or `None` if the line
/// does not continue. A doubled backslash escapes itself and does not continue.
fn continuation_head(line: &str) -> Option<&str> {
    let trimmed = line.trim_end();
    let backslashes = trimmed.chars().rev().take_while(|c| *c == '\\').count();
    if backslashes % 2 == 1 { Some(&trimmed[..trimmed.len() - 1]) } else { None }
}

/// Split a non-recipe line at its first unescaped `#`. Make comments run to end
/// of line anywhere outside a recipe, quotes included, which is why
/// `VERSION := 1.0 # bump me` used to convert with the comment inside the value.
fn split_inline_comment(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'#' => return (&text[..i], Some(&text[i..])),
            _ => i += 1,
        }
    }
    (text, None)
}

fn strip_export(trimmed: &str) -> Option<&str> {
    for keyword in ["export", "unexport"] {
        if let Some(rest) = trimmed.strip_prefix(keyword)
            && rest.starts_with(char::is_whitespace)
        {
            return Some(rest.trim());
        }
    }
    None
}

fn unsupported_directive(trimmed: &str) -> Option<&'static str> {
    const DIRECTIVES: &[&str] = &["include", "-include", "sinclude", "vpath", "$(eval", "$(call"];
    DIRECTIVES.iter().copied().find(|d| {
        trimmed == *d
            || (trimmed.starts_with(d) && trimmed[d.len()..].starts_with(|c: char| c.is_whitespace() || c == '('))
    })
}

fn is_phony_declaration(line: &str) -> Option<Vec<String>> {
    line.strip_prefix(".PHONY:").map(|targets_part| {
        split_inline_comment(targets_part)
            .0
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    })
}

fn extract_default_goal(line: &str) -> Option<String> {
    if line.starts_with(".DEFAULT_GOAL") {
        let body = split_inline_comment(line).0;
        if let Some(pos) = body.find(":=") {
            return Some(body[pos + 2..].trim().to_string());
        } else if let Some(pos) = body.find('=') {
            return Some(body[pos + 1..].trim().to_string());
        }
    }
    None
}

/// True when a rule's dependency field is really `VAR = value`, i.e. the rule
/// is a target-specific variable assignment.
fn is_target_specific_variable(dep_part: &str) -> bool {
    let Some(pos) = dep_part.find('=') else { return false };
    let name = dep_part[..pos].trim_end_matches([':', '?', '+']).trim();
    !name.is_empty() && is_variable_name(name)
}

fn is_variable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn parse_variable(line: &str) -> Result<Option<Variable>> {
    // A recipe is shell text; `CGO_ENABLED=0 go build` is not an assignment.
    if line.starts_with('\t') {
        return Ok(None);
    }

    let (body, _) = split_inline_comment(line);

    // Longest operator first: `?=` and `+=` both contain no `=` prefix, but
    // searching for `=` first would split `VAR ?= x` into name `VAR ?`.
    let assignment_ops = [":=", "::=", "?=", "+=", "="];

    let mut best: Option<(usize, &str)> = None;
    for op in &assignment_ops {
        if let Some(pos) = body.find(op)
            && best.is_none_or(|(best_pos, best_op)| pos < best_pos || (pos == best_pos && op.len() > best_op.len()))
        {
            best = Some((pos, op));
        }
    }

    let Some((pos, op)) = best else { return Ok(None) };

    let name = body[..pos].trim().to_string();
    let value = body[pos + op.len()..].trim().to_string();

    if !is_variable_name(&name) {
        return Ok(None);
    }

    let assignment_type = match op {
        ":=" | "::=" => {
            if value.contains("$(shell ") {
                AssignmentType::ShellExecution
            } else {
                AssignmentType::Simple
            }
        }
        "?=" => AssignmentType::Conditional,
        "+=" => AssignmentType::Append,
        _ => AssignmentType::Recursive,
    };

    Ok(Some(Variable {
        name,
        value,
        assignment_type,
        line: 0, // filled in by the caller, which knows the physical line
    }))
}

/// Make functions otto's env evaluator cannot run. `$(shell ...)` is absent on
/// purpose: the converter rewrites it into bash command substitution.
fn contains_make_functions(value: &str) -> bool {
    const MAKE_FUNCTIONS: &[&str] = &[
        "$(abspath",
        "$(realpath",
        "$(dir",
        "$(notdir",
        "$(suffix",
        "$(basename",
        "$(addsuffix",
        "$(addprefix",
        "$(join",
        "$(wildcard",
        "$(firstword",
        "$(lastword",
        "$(wordlist",
        "$(words",
        "$(word",
        "$(subst",
        "$(patsubst",
        "$(strip",
        "$(findstring",
        "$(filter",
        "$(filter-out",
        "$(sort",
        "$(foreach",
        "$(if",
        "$(or",
        "$(and",
        "$(call",
        "$(eval",
        "$(value",
        "$(MAKEFILE_LIST)",
        "$(MAKECMDGOALS)",
    ];

    MAKE_FUNCTIONS.iter().any(|func| value.contains(func))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> (MakefileAst, Vec<Diagnostic>) {
        let mut parser = MakefileParser::new(content.to_string());
        let ast = parser.parse().expect("parse failed");
        let diagnostics = parser.diagnostics().to_vec();
        (ast, diagnostics)
    }

    fn messages(diagnostics: &[Diagnostic]) -> String {
        diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn test_parse_simple_variable() {
        let (ast, diagnostics) = parse("VAR := value");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].name, "VAR");
        assert_eq!(ast.variables[0].value, "value");
        assert_eq!(ast.variables[0].assignment_type, AssignmentType::Simple);
        assert_eq!(ast.variables[0].line, 1);
        assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
    }

    #[test]
    fn test_parse_recursive_variable() {
        let (ast, _) = parse("VAR = value");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].assignment_type, AssignmentType::Recursive);
    }

    #[test]
    fn test_parse_conditional_variable() {
        let (ast, _) = parse("VAR ?= default");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].name, "VAR");
        assert_eq!(ast.variables[0].value, "default");
        assert_eq!(ast.variables[0].assignment_type, AssignmentType::Conditional);
    }

    #[test]
    fn test_parse_shell_variable() {
        let (ast, _) = parse("VERSION := $(shell git describe --tags)");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].assignment_type, AssignmentType::ShellExecution);
        assert!(ast.variables[0].value.contains("$(shell"));
    }

    #[test]
    fn test_parse_simple_target() {
        let (ast, _) = parse("build:\n\techo Building");

        assert_eq!(ast.targets.len(), 1);
        assert_eq!(ast.targets[0].name, "build");
        assert_eq!(ast.targets[0].commands, vec!["echo Building".to_string()]);
        assert_eq!(ast.targets[0].line, 1);
    }

    #[test]
    fn test_parse_target_with_dependencies() {
        let (ast, _) = parse("build: test clean\n\techo Building");

        assert_eq!(
            ast.targets[0].dependencies,
            vec!["test".to_string(), "clean".to_string()]
        );
    }

    #[test]
    fn test_parse_target_with_comment() {
        let (ast, _) = parse("# Build the project\nbuild:\n\techo Building");

        assert_eq!(ast.targets[0].comment, Some("Build the project".to_string()));
    }

    #[test]
    fn test_parse_phony_declaration() {
        let (ast, _) = parse(".PHONY: build clean test");

        assert_eq!(ast.phony_targets.len(), 3);
        assert!(ast.phony_targets.contains("build"));
        assert!(ast.phony_targets.contains("clean"));
        assert!(ast.phony_targets.contains("test"));
    }

    #[test]
    fn test_phony_targets_mark_their_rules() {
        let (ast, _) = parse(".PHONY: build\n\nbuild:\n\techo hi\n\napp.bin:\n\techo bin");

        let build = ast.targets.iter().find(|t| t.name == "build").unwrap();
        let app = ast.targets.iter().find(|t| t.name == "app.bin").unwrap();
        assert!(build.is_phony, "a .PHONY target must be marked phony");
        assert!(!app.is_phony);
    }

    #[test]
    fn test_parse_default_goal() {
        let (ast, _) = parse(".DEFAULT_GOAL := build");

        assert_eq!(ast.default_goal, Some("build".to_string()));
    }

    #[test]
    fn test_parse_multiline_command() {
        let (ast, _) = parse("build:\n\tmkdir -p dist && \\\n\techo Done");

        assert_eq!(ast.targets[0].commands.len(), 1);
        assert!(ast.targets[0].commands[0].contains("mkdir -p dist"));
        assert!(ast.targets[0].commands[0].contains("echo Done"));
    }

    #[test]
    fn test_parse_multiple_targets() {
        let (ast, _) = parse("build:\n\techo Building\n\ntest:\n\techo Testing");

        assert_eq!(ast.targets.len(), 2);
        assert_eq!(ast.targets[0].name, "build");
        assert_eq!(ast.targets[1].name, "test");
    }

    #[test]
    fn test_parse_target_with_multiple_commands() {
        let (ast, _) = parse("build:\n\techo Starting\n\tmkdir -p dist\n\techo Done");

        assert_eq!(ast.targets[0].commands.len(), 3);
    }

    #[test]
    fn test_parse_empty_makefile() {
        let (ast, _) = parse("");

        assert_eq!(ast.variables.len(), 0);
        assert_eq!(ast.targets.len(), 0);
    }

    #[test]
    fn test_parse_comments_only() {
        let (ast, _) = parse("# This is a comment\n# Another comment");

        assert_eq!(ast.variables.len(), 0);
        assert_eq!(ast.targets.len(), 0);
    }

    #[test]
    fn test_parse_complex_makefile() {
        let content = r#".DEFAULT_GOAL := build

VAR1 := value1
VAR2 = value2
VERSION := $(shell git describe --tags)

.PHONY: build clean test

# Build the project
build: test
	echo "Building version $(VERSION)"
	mkdir -p dist

# Run tests
test:
	go test ./...

clean:
	rm -rf dist
"#;

        let (ast, diagnostics) = parse(content);

        assert_eq!(ast.default_goal, Some("build".to_string()));
        assert_eq!(ast.variables.len(), 3);
        assert_eq!(ast.phony_targets.len(), 3);
        assert_eq!(ast.targets.len(), 3);
        assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
    }

    // --- continuations -----------------------------------------------------

    #[test]
    fn test_variable_continuation_is_joined() {
        let (ast, _) = parse("SOURCES := a.c \\\n  b.c \\\n  c.c\n");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].value, "a.c b.c c.c");
    }

    #[test]
    fn test_space_indented_recipe_continuation_is_joined_not_invented() {
        let content = "build:\n\tdocker run --rm \\\n\t  -v /a:/b \\\n\t  --label foo\n";

        let (ast, _) = parse(content);

        assert_eq!(ast.targets.len(), 1, "the continuation must not become a second target");
        assert_eq!(
            ast.targets[0].commands,
            vec!["docker run --rm -v /a:/b --label foo".to_string()]
        );
    }

    #[test]
    fn test_dependency_list_continuation_is_joined() {
        let (ast, _) = parse("all: build \\\n     test\n\techo done\n");

        assert_eq!(ast.targets.len(), 1);
        assert_eq!(
            ast.targets[0].dependencies,
            vec!["build".to_string(), "test".to_string()]
        );
    }

    #[test]
    fn test_escaped_backslash_does_not_continue() {
        let (ast, _) = parse("VAR := back\\\\\nOTHER := two\n");

        assert_eq!(ast.variables.len(), 2);
        assert_eq!(ast.variables[1].name, "OTHER");
    }

    // --- errors ------------------------------------------------------------

    #[test]
    fn test_space_indented_recipe_is_rejected() {
        let mut parser = MakefileParser::new("build:\n    docker run -v /a:/b\n".to_string());

        let err = parser.parse().expect_err("a space-indented recipe must not convert");

        let message = err.to_string();
        assert!(message.contains("indented with spaces"), "{message}");
        assert!(message.contains("Makefile:2"), "{message}");
    }

    #[test]
    fn test_space_indented_recipe_without_colon_is_rejected() {
        let mut parser = MakefileParser::new("build:\n    echo hi\n".to_string());

        let err = parser.parse().expect_err("a space-indented recipe must not convert");

        assert!(err.to_string().contains("indented with spaces"), "{err}");
    }

    #[test]
    fn test_space_indented_assignment_is_not_an_error() {
        let (ast, _) = parse("build:\n\techo hi\n\n  INDENTED := ok\n");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(ast.variables[0].name, "INDENTED");
    }

    // --- constructs that used to corrupt silently --------------------------

    #[test]
    fn test_export_is_stripped_and_reported() {
        let (ast, diagnostics) = parse("export PATH := /usr/bin\n");

        assert_eq!(ast.variables.len(), 1);
        assert_eq!(
            ast.variables[0].name, "PATH",
            "`export` must not become part of the name"
        );
        assert!(
            messages(&diagnostics).contains("`export` on `PATH` dropped"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_target_specific_variable_is_not_a_dependency() {
        let (ast, diagnostics) = parse("build: CFLAGS=-g\n");

        assert!(ast.targets.is_empty(), "a target-specific variable is not a rule");
        assert!(
            messages(&diagnostics).contains("target-specific variable"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_multi_target_rule_becomes_one_task_each() {
        let (ast, diagnostics) = parse("test check: build\n\techo hi\n");

        assert_eq!(ast.targets.len(), 2);
        assert_eq!(ast.targets[0].name, "test");
        assert_eq!(ast.targets[1].name, "check");
        assert_eq!(ast.targets[1].dependencies, vec!["build".to_string()]);
        assert!(
            messages(&diagnostics).contains("declares 2 targets"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_double_colon_rule_keeps_its_dependencies() {
        let (ast, diagnostics) = parse("install:: build\n\techo hi\n");

        assert_eq!(ast.targets.len(), 1);
        assert_eq!(ast.targets[0].name, "install");
        assert_eq!(
            ast.targets[0].dependencies,
            vec!["build".to_string()],
            "`:` must not become a dependency"
        );
        assert!(
            messages(&diagnostics).contains("double-colon"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_pattern_rule_is_skipped_with_a_warning() {
        let (ast, diagnostics) = parse("%.o: %.c\n\tgcc -c $< -o $@\n\nbuild:\n\techo hi\n");

        assert_eq!(ast.targets.len(), 1, "the pattern rule must not become a task");
        assert_eq!(ast.targets[0].name, "build");
        assert!(
            messages(&diagnostics).contains("pattern rule"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_static_pattern_rule_is_skipped_with_a_warning() {
        let (ast, diagnostics) = parse("objs: %.o: %.c\n\tgcc -c $< -o $@\n");

        assert!(ast.targets.is_empty());
        assert!(
            messages(&diagnostics).contains("static pattern rule"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_inline_comment_is_not_a_dependency() {
        let (ast, _) = parse("help: ## Help me.\n\techo help\n");

        assert_eq!(ast.targets.len(), 1);
        assert!(
            ast.targets[0].dependencies.is_empty(),
            "`## Help me.` is a comment, not three deps"
        );
        assert_eq!(ast.targets[0].comment, Some("Help me.".to_string()));
    }

    #[test]
    fn test_inline_comment_is_not_part_of_a_value() {
        let (ast, _) = parse("VERSION := 1.0 # bump me\n");

        assert_eq!(ast.variables[0].value, "1.0");
    }

    #[test]
    fn test_blank_line_does_not_end_a_recipe() {
        let (ast, diagnostics) =
            parse("keygen:\n\tmkdir -p certs\n\n\topenssl genpkey -out certs/a.pem -pkeyopt bits:2048\n");

        assert_eq!(
            ast.targets.len(),
            1,
            "the second half of the recipe must not become a target"
        );
        assert_eq!(ast.targets[0].commands.len(), 2);
        assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
    }

    #[test]
    fn test_make_function_variable_is_reported_not_silently_dropped() {
        let (ast, diagnostics) = parse("OBJS := $(wildcard *.c)\n");

        assert!(ast.variables.is_empty());
        assert!(
            messages(&diagnostics).contains("make function"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_include_is_reported() {
        let (_, diagnostics) = parse("include other.mk\n");

        assert!(
            messages(&diagnostics).contains("`include` is not supported"),
            "{}",
            messages(&diagnostics)
        );
    }

    #[test]
    fn test_conditional_block_is_reported_once() {
        let (ast, diagnostics) = parse("ifeq ($(OS),Linux)\nCC := gcc\nendif\n");

        assert!(ast.variables.is_empty());
        assert_eq!(diagnostics.len(), 1, "{}", messages(&diagnostics));
        assert!(messages(&diagnostics).contains("conditional block skipped"));
    }

    #[test]
    fn test_define_block_is_skipped_whole() {
        let (ast, diagnostics) = parse("define greet\nfoo: bar\nendef\n\nbuild:\n\techo hi\n");

        assert_eq!(
            ast.targets.len(),
            1,
            "the define body must not be parsed as makefile syntax"
        );
        assert_eq!(ast.targets[0].name, "build");
        assert!(messages(&diagnostics).contains("define"), "{}", messages(&diagnostics));
    }
}
