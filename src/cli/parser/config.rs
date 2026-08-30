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

            let config_spec: ConfigSpec = serde_yaml::from_str(&content)?;

            // Validate that no tasks use reserved builtin param names
            Self::validate_no_builtin_params(&config_spec)?;

            // Validate foreach sources (a `command:` source is exclusive with
            // glob/items/range). Shape-only, executes nothing, so every
            // surface including `--help` reports the misconfiguration.
            Self::validate_foreach_sources(&config_spec)?;

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
