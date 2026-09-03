#![cfg(test)]

use super::*;
use crate::cfg::param::Value;
use hex;
use serial_test::serial;
use sha2::Digest;
use std::collections::HashMap;
use tempfile::TempDir;

/// Point this test's otto home at a scratch directory.
///
/// `OTTO_DB_PATH` is deliberately cleared rather than set: the database now
/// derives from `OTTO_HOME`, and pinning both was what let the derived
/// default stay broken while these tests passed.
fn setup_test_db(temp_dir: &std::path::Path) {
    let otto_home = temp_dir.join(".otto");
    // SAFETY: This is safe in tests because we control the execution environment
    // and tests are isolated. The env var is set before any StateManager is created.
    unsafe {
        std::env::remove_var("OTTO_DB_PATH");
        std::env::set_var("OTTO_HOME", &otto_home);
    }
}

#[tokio::test]
#[serial]
async fn test_bash_action_processing() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

    let mut task_envs = HashMap::new();
    task_envs.insert("greeting".to_string(), "hello".to_string());

    let mut task_values = HashMap::new();
    task_values.insert("greeting".to_string(), Value::Item("hello".to_string()));

    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![crate::executor::task::TaskEdge::success("dep_task")],
        vec![],
        vec![],
        task_envs,
        task_values,
        "#!/usr/bin/env bash\necho \"${greeting} world\"".to_string(),
    );

    // Process the action
    let result = processor.process(&task.action, &task)?;

    match result {
        ProcessedAction::Bash { path, script, hash } => {
            assert!(path.exists());
            assert!(script.contains("declare -a OTTO_INPUT"));
            assert!(script.contains("declare -a OTTO_OUTPUT"));
            assert!(script.contains("export OTTO_TASK_DIR"));
            assert!(script.contains("greeting='hello'"));
            assert!(script.contains("otto_deserialize_input 'dep_task'"));
            assert!(script.contains("echo \"${greeting} world\""));
            assert!(script.contains("otto_serialize_output 'test_task'"));

            assert_eq!(hash.len(), 8, "Hash should be 8 characters");
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "Hash should be hexadecimal"
            );

            let mut hasher = sha2::Sha256::new();
            hasher.update(script.as_bytes());
            let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
            assert_eq!(hash, expected_hash, "Hash should match generated script content");
        }
        _ => panic!("Expected Bash variant"),
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_python_action_processing() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

    let mut task_envs = HashMap::new();
    task_envs.insert("name".to_string(), "world".to_string());

    let mut task_values = HashMap::new();
    task_values.insert("name".to_string(), Value::Item("world".to_string()));

    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![crate::executor::task::TaskEdge::success("dep_task")],
        vec![],
        vec![],
        task_envs,
        task_values,
        "#!/usr/bin/env python3\nprint(f\"Hello {name}\")".to_string(),
    );

    // Process the action
    let result = processor.process(&task.action, &task)?;

    match result {
        ProcessedAction::Python3 { path, script, hash } => {
            assert!(path.exists());
            assert!(script.contains("OTTO_INPUT = {}"));
            assert!(script.contains("OTTO_OUTPUT = {}"));
            assert!(script.contains("os.environ['OTTO_TASK_DIR']"));
            assert!(script.contains("name = 'world'"));
            assert!(script.contains("otto_deserialize_input('dep_task')"));
            assert!(script.contains("print(f\"Hello {name}\")"));
            assert!(script.contains("otto_serialize_output('test_task')"));

            assert_eq!(hash.len(), 8, "Hash should be 8 characters");
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "Hash should be hexadecimal"
            );

            let mut hasher = sha2::Sha256::new();
            hasher.update(script.as_bytes());
            let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
            assert_eq!(hash, expected_hash, "Hash should match generated script content");
        }
        _ => panic!("Expected Python3 variant"),
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_default_bash_action_processing() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

    let mut task_envs = HashMap::new();
    task_envs.insert("message".to_string(), "hello".to_string());

    let mut task_values = HashMap::new();
    task_values.insert("message".to_string(), Value::Item("hello".to_string()));

    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        task_envs,
        task_values,
        "echo \"${message} from default bash\"".to_string(), // No shebang
    );

    // Process the action
    let result = processor.process(&task.action, &task)?;

    match result {
        ProcessedAction::Bash { path, script, hash } => {
            assert!(path.exists());
            assert!(script.contains("declare -a OTTO_INPUT"));
            assert!(script.contains("declare -a OTTO_OUTPUT"));
            assert!(script.contains("export OTTO_TASK_DIR"));
            assert!(script.contains("message='hello'"));
            assert!(script.contains("echo \"${message} from default bash\""));
            assert!(script.contains("otto_serialize_output 'test_task'"));

            assert_eq!(hash.len(), 8, "Hash should be 8 characters");
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "Hash should be hexadecimal"
            );

            let mut hasher = sha2::Sha256::new();
            hasher.update(script.as_bytes());
            let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
            assert_eq!(hash, expected_hash, "Hash should match generated script content");
        }
        _ => panic!("Expected Bash variant (default fallback)"),
    }

    Ok(())
}

/// The payload from the design doc's reproduction, verbatim. It closed the
/// generated `export K="..."` and ran `touch`; single quotes make it text.
const INJECTION: &str = r#"x"; touch /tmp/OTTO_PWNED; echo "y"#;

fn injection_task(action: &str) -> Task {
    let mut envs = HashMap::new();
    envs.insert("PAYLOAD".to_string(), INJECTION.to_string());
    envs.insert("QUOTED".to_string(), "it's here".to_string());

    let mut values = HashMap::new();
    values.insert("name".to_string(), Value::Item(INJECTION.to_string()));

    Task::new(
        "test_task".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        envs,
        values,
        action.to_string(),
    )
}

#[tokio::test]
#[serial]
async fn bash_generator_quotes_env_and_param_values_as_data() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;
    let task = injection_task("#!/usr/bin/env bash\necho hi");
    let ProcessedAction::Bash { script, .. } = processor.process(&task.action, &task)? else {
        panic!("Expected Bash variant");
    };

    assert!(
        script.contains(r#"export PAYLOAD='x"; touch /tmp/OTTO_PWNED; echo "y'"#),
        "env value must be one single-quoted word, got:\n{script}"
    );
    assert!(
        script.contains(r#"name='x"; touch /tmp/OTTO_PWNED; echo "y'"#),
        "param value must be one single-quoted word, got:\n{script}"
    );
    // The `touch` is inside quotes, so it is never a command of its own.
    assert!(
        !script.contains("\n touch") && !script.contains("; touch /tmp/OTTO_PWNED\n"),
        "payload must not reach the script as a command:\n{script}"
    );
    // A value containing the quote character survives it.
    assert!(
        script.contains(r"export QUOTED='it'\''s here'"),
        "embedded single quote must be escaped, got:\n{script}"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn python_generator_quotes_env_and_param_values_as_data() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;
    let task = injection_task("#!/usr/bin/env python3\nprint('hi')");
    let ProcessedAction::Python3 { script, .. } = processor.process(&task.action, &task)? else {
        panic!("Expected Python3 variant");
    };

    assert!(
        script.contains(r#"os.environ['PAYLOAD'] = 'x"; touch /tmp/OTTO_PWNED; echo "y'"#),
        "env value must be one string literal, got:\n{script}"
    );
    assert!(
        script.contains(r#"name = 'x"; touch /tmp/OTTO_PWNED; echo "y'"#),
        "param value must be one string literal, got:\n{script}"
    );
    assert!(
        script.contains(r"os.environ['QUOTED'] = 'it\'s here'"),
        "embedded single quote must be escaped, got:\n{script}"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn generator_rejects_an_env_key_that_is_not_an_identifier() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let mut envs = HashMap::new();
    envs.insert("EVIL; touch /tmp/OTTO_PWNED".to_string(), "x".to_string());
    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        envs,
        HashMap::new(),
        "#!/usr/bin/env bash\necho hi".to_string(),
    );

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;
    let err = match processor.process(&task.action, &task) {
        Ok(_) => panic!("an env key that is not an identifier must not generate a script"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("is not a valid identifier") && err.contains("test_task"),
        "error must name the task and the offending key, got: {err}"
    );

    Ok(())
}

/// Six runs of one unchanged task produced six cache files, because both
/// generators walked a HashMap. One script, one hash, one cache entry.
#[tokio::test]
#[serial]
async fn script_hash_is_stable_across_runs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());

    let mut envs = HashMap::new();
    let mut values = HashMap::new();
    for i in 0..6 {
        envs.insert(format!("VAR_{i}"), format!("value-{i}"));
        values.insert(format!("param{i}"), Value::Item(format!("v{i}")));
    }
    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        envs,
        values,
        "#!/usr/bin/env bash\necho hi".to_string(),
    );

    let mut hashes = std::collections::HashSet::new();
    for _ in 0..6 {
        let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
        workspace.init().await?;
        let processor = ActionProcessor::new(workspace.clone(), "test_task")?;
        let ProcessedAction::Bash { hash, .. } = processor.process(&task.action, &task)? else {
            panic!("Expected Bash variant");
        };
        hashes.insert(hash);
    }

    assert_eq!(hashes.len(), 1, "six runs must agree on one hash, got {hashes:?}");

    Ok(())
}

/// A truncated cache entry keeps its (content-addressed) name, so
/// write-only-if-absent re-executed the stump forever.
#[tokio::test]
#[serial]
async fn a_torn_cache_entry_is_rewritten_rather_than_reused() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = ActionProcessor::new(workspace.clone(), "test_task")?;
    let task = Task::new(
        "test_task".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "#!/usr/bin/env bash\necho hello\n".to_string(),
    );

    let ProcessedAction::Bash { script, hash, .. } = processor.process(&task.action, &task)? else {
        panic!("Expected Bash variant");
    };
    let cache_file = workspace.cache_dir().join(format!("{hash}.sh"));
    std::fs::write(&cache_file, b"#!/usr/bin/env bash\n# tor")?;

    processor.process(&task.action, &task)?;

    assert_eq!(
        std::fs::read_to_string(&cache_file)?,
        script,
        "the torn entry must be replaced by the real script"
    );

    Ok(())
}

/// macOS ships /bin/bash 3.2.57 and always will: Apple froze it at the last
/// GPLv2 release. The builtins prelude is `source`d into every bash task, so
/// one bash-4-only construct in it aborts the task before its body runs, with
/// nothing but `bad substitution` to go on. `${task_name^^}` shipped in
/// v2.0.0-v2.0.3 and did exactly that to every task with a dependency.
///
/// This is a text scan, not an execution test, on purpose: neither
/// `bash --posix` nor `BASH_COMPAT=3.1` rejects `${x^^}` under a bash 5
/// binary, so there is no way to reproduce the failure on a Linux runner.
#[tokio::test]
#[serial]
async fn builtins_contain_no_bash_4_only_constructs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;

    let processor = BashProcessor::new(workspace.clone(), "test_task");
    processor.create_builtins()?;
    let builtins = std::fs::read_to_string(workspace.task_dir("test_task").join("builtins.sh"))?;

    // (needle, what it is, the 3.2-safe spelling)
    let bash_4_only = [
        (
            "^^}",
            "case-fold parameter expansion",
            "printf | LC_ALL=C tr '[:lower:]' '[:upper:]'",
        ),
        (
            "^}",
            "case-fold parameter expansion",
            "printf | LC_ALL=C tr '[:lower:]' '[:upper:]'",
        ),
        (
            ",,}",
            "case-fold parameter expansion",
            "printf | LC_ALL=C tr '[:upper:]' '[:lower:]'",
        ),
        ("declare -A", "associative array", "two indexed arrays"),
        ("local -A", "associative array", "two indexed arrays"),
        ("mapfile", "mapfile builtin", "while IFS= read -r"),
        ("readarray", "readarray builtin", "while IFS= read -r"),
        ("wait -n", "wait -n", "wait on collected pids"),
        ("${EPOCHSECONDS", "EPOCHSECONDS", "date +%s"),
        ("${BASH_ARGV0", "BASH_ARGV0", "$0"),
    ];

    for (needle, what, instead) in bash_4_only {
        assert!(
            !builtins.contains(needle),
            "builtins.sh uses the bash-4-only {what} (`{needle}`), which fails on \
             macOS /bin/bash 3.2.57 with `bad substitution`. Use {instead} instead."
        );
    }

    Ok(())
}

/// The reader's fold must land on the byte-identical prefix the writer wrote.
/// `json_to_env` (executor/scheduler.rs) folds through `fold_to_var_name`; the
/// builtins do it in shell. The two spellings are not the same characters, so
/// this pins them together by result rather than by looking alike - and pins
/// the shell copy below to the shell that actually ships, so it cannot drift
/// into agreeing with a fold nobody runs.
#[tokio::test]
#[serial]
async fn builtins_input_fold_matches_the_writer_fold() -> Result<()> {
    // Verbatim from the generated builtins.sh, asserted to still be there.
    const SHELL_FOLD: &str = r#"        task_upper=$(printf '%s' "$task_name" \
            | LC_ALL=C tr '[:lower:]' '[:upper:]' \
            | LC_ALL=C tr -c '[:alnum:]_' '_')"#;

    let temp_dir = TempDir::new()?;
    setup_test_db(temp_dir.path());
    let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
    workspace.init().await?;
    let processor = BashProcessor::new(workspace.clone(), "test_task");
    processor.create_builtins()?;
    let builtins = std::fs::read_to_string(workspace.task_dir("test_task").join("builtins.sh"))?;
    assert!(
        builtins.contains(SHELL_FOLD),
        "the shell fold this test runs is no longer the one builtins.sh ships; \
         update SHELL_FOLD to the shipped text (and check it still matches the writer)"
    );

    let script = format!("task_name=\"$1\"\n{SHELL_FOLD}\nprintf '%s' \"OTTO_INPUT_${{task_upper}}_\"");

    for name in [
        "build",
        "Build",
        "BUILD",
        "pro.ducer",
        "my-task",
        "my-task.v2",
        "a-b.c-d",
        "task_9",
        // A foreach subtask name. The `-`/`.`-only fold left the `:` in place
        // and wrote `OTTO_INPUT_UP:ALPHA_K=v-alpha`, which bash read as a
        // command; the consumer of `up:alpha` died on `command not found`.
        "up:alpha",
        "up:alpha.beta",
        // A digit is fine anywhere in a fold: this is per byte, not the
        // whole-name identifier rule, which would fold the year to `____`.
        "up_2024",
        "2024",
        // Everything else outside the class, on both sides.
        "a b",
        "a+b/c",
    ] {
        let writer = format!("OTTO_INPUT_{}_", crate::executor::scheduler::fold_to_var_name(name));

        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .arg("bash")
            .arg(name)
            .output()
            .expect("bash is on PATH");
        assert!(out.status.success(), "shell fold failed for {name}: {out:?}");
        let reader = String::from_utf8(out.stdout).expect("utf8");

        assert_eq!(
            reader, writer,
            "reader and writer folds disagree for task name {name:?}"
        );
    }

    Ok(())
}
