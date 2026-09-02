impl Parser {
    fn find_ottofile(path: &Path) -> Result<Option<PathBuf>> {
        for ottofile in OTTOFILES {
            let ottofile_path = path.join(ottofile);
            if ottofile_path.exists() {
                return Ok(Some(ottofile_path));
            }
        }
        // If we've reached the root, stop searching
        if let Some(parent) = path.parent() {
            if parent == path {
                return Ok(None);
            }
            // Recurse up
            Self::find_ottofile(parent)
        } else {
            Ok(None)
        }
    }

    fn divine_ottofile(source: crate::cli::parser::OttofileSource) -> Result<Option<PathBuf>> {
        let value = source.as_start_path();
        // Both failures name the path. `otto -o /nope/nothere.yml` used to fail
        // with a bare "No such file or directory (os error 2)", which says
        // nothing about which file otto was looking for.
        let expanded = expanduser(value).wrap_err_with(|| format!("could not expand ottofile path '{source}'"))?;
        let path = fs::canonicalize(&expanded)
            .wrap_err_with(|| format!("ottofile path '{}' does not exist", expanded.display()))?;
        if path.is_dir() {
            return Self::find_ottofile(&path);
        }
        Ok(Some(path))
    }

    fn load_config_from_path(ottofile_path: Option<PathBuf>) -> Result<(ConfigSpec, String, Option<PathBuf>)> {
        if let Some(ottofile) = ottofile_path {
            // Named, for the same reason `divine_ottofile` names its two
            // failures: the not-exists branch already said which file it was
            // looking for, while an unreadable one failed with a bare
            // "Permission denied (os error 13)" and no path at all.
            let content = fs::read_to_string(&ottofile)
                .wrap_err_with(|| format!("could not read ottofile '{}'", ottofile.display()))?;
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let result = hasher.finalize();
            let hash = hex::encode(result)[..8].to_string();

            // Version gate BEFORE the typed parse, against the same string:
            // reversed, a file from a newer otto reports whichever key it
            // added instead of telling the operator to upgrade.
            crate::cfg::otto::check_api_version(&content)?;

            // `deny_unknown_fields` already names the offending key and path.
            // `wrap_unknown_field_error` adds a trailing line naming the
            // likely cause (a newer-than-this-binary key, or a typo) and the
            // fix, without an api bump (design doc `2026-09-01-cancellation-
            // reaping-and-foreach-concurrency.md`, Phase 4).
            let config_spec: ConfigSpec =
                serde_yaml::from_str(&content).map_err(crate::cfg::otto::wrap_unknown_field_error)?;

            // Validate that no tasks use reserved builtin param names
            Self::validate_no_builtin_params(&config_spec)?;

            // Validate foreach sources (a `command:` source is exclusive with
            // glob/items/range). Shape-only, executes nothing, so every
            // surface including `--help` reports the misconfiguration.
            Self::validate_foreach_sources(&config_spec)?;

            // Validate `required: true` combinations (FLG, default:,
            // zero-capable nargs) and required-positional ordering.
            // Shape-only, executes nothing.
            Self::validate_required_params(&config_spec)?;

            // Reject `foreach.buffer: true` combined with `tty: true` on the
            // same task. Shape-only, executes nothing.
            Self::validate_foreach_buffer(&config_spec)?;

            // Reject `foreach.jobs` combined with `foreach.parallel: false`.
            // Shape-only, executes nothing.
            Self::validate_foreach_jobs(&config_spec)?;

            Ok((config_spec, hash, Some(ottofile)))
        } else {
            Err(eyre!("{}", ottofile_not_found_message()))
        }
    }

    fn validate_foreach_sources(config: &ConfigSpec) -> Result<()> {
        for (task_name, task_spec) in &config.tasks {
            if let Some(foreach) = &task_spec.foreach {
                foreach.validate_sources(task_name)?;
            }
        }
        Ok(())
    }

    /// `required: true` rejections: each param's own shape (`ParamSpec::validate_required`)
    /// plus the one cross-param property that needs declaration order - a
    /// required positional after an optional one, which clap panics building
    /// (design doc Phase 1).
    fn validate_required_params(config: &ConfigSpec) -> Result<()> {
        for (task_name, task_spec) in &config.tasks {
            let mut last_optional_positional: Option<&str> = None;
            for param_spec in task_spec.params.values() {
                param_spec.validate_required(task_name)?;

                if param_spec.param_type != ParamType::POS {
                    continue;
                }
                if param_spec.required {
                    if let Some(earlier) = last_optional_positional {
                        return Err(eyre!(
                            "Task '{task_name}': required positional param '{}' is declared after \
                             optional positional param '{earlier}'; clap panics building a command \
                             with that shape, so declare every required positional before any \
                             optional one",
                            param_spec.name
                        ));
                    }
                } else {
                    last_optional_positional = Some(param_spec.name.as_str());
                }
            }
        }
        Ok(())
    }

    /// `foreach.buffer: true` together with `tty: true` on the same task is a
    /// load error: a `tty` task owns the terminal exclusively and runs
    /// exclusively (design doc `2026-08-31-buffered-foreach-computed-envs-
    /// required-params.md`, Phase 3), so there is nothing left to buffer.
    /// `tty: true` on a foreach task WITHOUT `buffer` is unaffected and keeps
    /// printing its today's unprefixed contiguous blocks.
    fn validate_foreach_buffer(config: &ConfigSpec) -> Result<()> {
        for (task_name, task_spec) in &config.tasks {
            let Some(foreach) = &task_spec.foreach else {
                continue;
            };
            if foreach.buffer && task_spec.tty == Some(true) {
                return Err(eyre!(
                    "Task '{task_name}': foreach.buffer cannot be combined with tty; \
                     a tty task owns the terminal exclusively, so there is nothing to buffer"
                ));
            }
        }
        Ok(())
    }

    /// `foreach.jobs` together with `foreach.parallel: false` is a load
    /// error: serial means one item at a time, so a per-group concurrency
    /// override is incoherent (design doc `2026-09-01-cancellation-reaping-
    /// and-foreach-concurrency.md`, Phase 2). `jobs: 0`, negatives, and
    /// non-integers are rejected during deserialization itself
    /// (`ForeachJobs`'s `Deserialize` impl), so this function only has the
    /// one cross-field shape left to catch.
    fn validate_foreach_jobs(config: &ConfigSpec) -> Result<()> {
        for (task_name, task_spec) in &config.tasks {
            let Some(foreach) = &task_spec.foreach else {
                continue;
            };
            if foreach.jobs.is_some() && !foreach.parallel {
                return Err(eyre!(
                    "Task '{task_name}': foreach.jobs cannot be combined with parallel: false; \
                     serial means one item at a time, so a concurrency override is incoherent"
                ));
            }
        }
        Ok(())
    }

    fn validate_no_builtin_params(config: &ConfigSpec) -> Result<()> {
        use crate::cli::builtins::is_builtin_param;

        for (task_name, task_spec) in &config.tasks {
            for param_name in task_spec.params.keys() {
                if is_builtin_param(param_name) {
                    return Err(eyre!(
                        "Task '{}' defines reserved builtin param '--{}'. \
                         Capitalized params are reserved for otto builtins.",
                        task_name,
                        param_name
                    ));
                }
            }
        }
        Ok(())
    }
}
