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
