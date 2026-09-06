impl Parser {
    /// Give every builtin a `TaskSpec`, so its name is reserved, `otto --help`
    /// lists it, and `otto help <NAME>` can render its flags.
    fn inject_builtin_commands(&mut self) {
        for command in builtin_clap_commands() {
            let task = builtin_task(&command);
            self.config_spec.tasks.insert(task.name.clone(), task);
        }
    }
}

/// Every builtin, as the clap `Command` its own parser uses.
///
/// This is the whole point of the derivation below: the `Command` here is the
/// same one `main`'s early route parses the invocation with, so a flag exists
/// in exactly one place. `otto help Clean` and `otto Clean --help` describe
/// the same command because they are the same declaration. The hand-written
/// `TaskSpec` literals this replaced had drifted: meta said `--keep DAYS`
/// while clap said `--keep-days` and also took `--keep-last`, `--keep-failed`
/// and `--no-db`; `History`/`Stats` meta declared `-t|--task TASK` while clap
/// took `TASK` positionally; `Upgrade` meta was missing `--backup-dir` and
/// `--github-token`.
///
/// Each `#[command(name = ...)]` is the builtin's task name (`Clean`, not
/// `clean`), because that name is what the derived `TaskSpec` is keyed by and
/// what `BUILTIN_COMMANDS` reserves.
fn builtin_clap_commands() -> Vec<Command> {
    use crate::cli::commands::{
        CleanCommand, ConvertCommand, GraphCommand, HistoryCommand, StatsCommand, UpgradeCommand,
    };
    use clap::CommandFactory;

    vec![
        CleanCommand::command(),
        ConvertCommand::command(),
        GraphCommand::command(),
        HistoryCommand::command(),
        StatsCommand::command(),
        UpgradeCommand::command(),
    ]
}

/// The `TaskSpec` for a builtin, derived from its clap `Command`.
///
/// `help` is that `Command`'s own `about` with the `[built-in]` marker in
/// front, the marker being what tells a reader the name is otto's and not
/// their ottofile's. It used to be a second, hand-written sentence per
/// builtin, and three of the six had drifted from the `about` clap renders:
/// `otto --help` said "Clean old runs from ~/.otto/" where `otto Clean --help`
/// said "Clean old otto run directories". One declaration, two renderings.
///
/// The fields clap has no way to express are identical for all six and live
/// here: a builtin is dispatched by name (`app::dispatch_builtin`), so its
/// `action` is never executed and exists only to be printed by a serialized
/// spec; and none of them is a foreach parent, a virtual parent, a tty task,
/// or an `on-failure:` target.
fn builtin_task(cmd: &Command) -> TaskSpec {
    let name = cmd.get_name().to_string();
    let params = cmd
        .get_arguments()
        // A hidden arg is one help does not show and the meta task must not
        // advertise either.
        .filter(|arg| !arg.is_hide_set())
        .map(|arg| {
            let param = builtin_param(arg);
            (param.name.clone(), param)
        })
        .collect();

    TaskSpec {
        help: Some(format!("[built-in] {}", cmd.get_about().map(|about| about.to_string()).unwrap_or_default())),
        after: vec![],
        before: vec![],
        input: vec![],
        output: vec![],
        envs: HashMap::new(),
        params,
        action: format!("# Built-in {name} command"),
        foreach: None,
        virtual_parent: false,
        tty: None,
        on_failure: vec![],
        name,
    }
}

/// One clap `Arg` as the `ParamSpec` otto's own parser and help renderer read.
fn builtin_param(arg: &Arg) -> ParamSpec {
    // A flag is the action, not the type: `SetTrue` is what clap's derive
    // gives a `bool` field, and it is the one shape that consumes no value.
    let is_flag = matches!(arg.get_action(), clap::ArgAction::SetTrue | clap::ArgAction::SetFalse);

    let long = arg.get_long().map(str::to_string);
    // The name is what a user types, so a long option is named by its long.
    // A positional has neither long nor short, so it falls back to clap's id,
    // which is the Rust field name (`task_name` -> `task-name`) - otto's param
    // names are kebab-case and the env var it exports is derived from them.
    let name = long
        .clone()
        .unwrap_or_else(|| arg.get_id().as_str().replace('_', "-"));

    let param_type = if arg.is_positional() {
        ParamType::POS
    } else if is_flag {
        ParamType::FLG
    } else {
        ParamType::OPT
    };

    // `get_num_args()` is `None` until clap builds the command, and building
    // it would also inject clap's own `--help` as an arg to derive a param
    // from. So the count comes from the action, with an explicit `num_args`
    // honored when the derive set one (a `Vec` field does).
    let nargs = match arg.get_num_args() {
        Some(range) => num_args_to_nargs(range),
        None if is_flag => Nargs::Zero,
        None => Nargs::One,
    };

    ParamSpec {
        name,
        short: arg.get_short(),
        long,
        param_type,
        metavar: arg
            .get_value_names()
            .and_then(|names| names.first())
            .map(ToString::to_string),
        default: arg
            .get_default_values()
            .first()
            .map(|value| value.to_string_lossy().to_string()),
        // No builtin has a dynamic value set: `choices-command:` is an
        // ottofile-only feature with no clap equivalent, so it is the one
        // `ParamSpec` field this derivation always leaves empty.
        choices_command: None,
        choices: if is_flag {
            // A flag consumes no value, so it has no value set. clap reports
            // `true`/`false` for one here only because `get_possible_values()`
            // asks the arg's value parser, and an unbuilt `SetTrue` arg still
            // carries the `bool` parser; after `Command::build` the same call
            // returns nothing.
            vec![]
        } else {
            arg.get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_string())
                .collect()
        },
        nargs,
        help: arg.get_help().map(ToString::to_string),
        value: Value::Empty,
        required: arg.is_required_set(),
    }
}

/// The `nargs:` a clap value count means. Inverse of [`nargs_to_num_args`].
fn num_args_to_nargs(range: clap::builder::ValueRange) -> Nargs {
    match (range.min_values(), range.max_values()) {
        (0, 0) => Nargs::Zero,
        (1, 1) => Nargs::One,
        (0, 1) => Nargs::OneOrZero,
        (1, usize::MAX) => Nargs::OneOrMore,
        (0, usize::MAX) => Nargs::ZeroOrMore,
        (min, max) => Nargs::Range(min, max),
    }
}
