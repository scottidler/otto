//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::{eyre, Result};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::collections::{HashSet, HashMap, VecDeque};
use std::str;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use std::path::PathBuf;
use once_cell::sync::Lazy;
use expanduser::expanduser;
use log::{debug};

use crate::cli::parse::{TaskSpec, DAG};
use crate::cfg::param::Value;
use crate::cfg::otto::Otto;

static TIMESTAMP: Lazy<u64> = Lazy::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
});

pub struct Scheduler {
    pub otto: Otto,
    pub tasks: DAG<TaskSpec>,
    pub hash: String,
    pub timestamp: u64,
}

impl Scheduler {
    #[must_use]
    pub fn new(otto: Otto, tasks: DAG<TaskSpec>, hash: String) -> Self {
        Self {
            otto,
            tasks,
            hash,
            timestamp: *TIMESTAMP,
        }
    }

    /// Run the scheduler asynchronously.
    ///
    /// # Errors
    ///
    /// This function will return an error if it fails to lock a shared resource,
    /// fails to create a directory, fails to write an action to a file, or fails to execute a command.
    ///
    /// # Panics
    ///
    /// This function can panic if a task is missing from the task queue. This should never occur under normal circumstances.
    pub async fn run_async(&self) -> Result<()> {
        // Find the set of tasks to execute
        let tasks_to_execute = self.get_tasks_to_execute()?;
        let num_tasks = tasks_to_execute.len();

        let completed_tasks: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let task_queue: Arc<Mutex<VecDeque<TaskSpec>>> = Arc::new(Mutex::new(VecDeque::new()));

        let path = Arc::new(self.create_dir()?);  // wrap it in an Arc
        // Populate the task queue
        for node in self.tasks.raw_nodes() {
            let task = node.weight.clone();
            if tasks_to_execute.contains(&task.name) {
                task_queue.lock().unwrap().push_back(task);
            }
        }

        let mut handles = Vec::new();

        for _ in 0..self.otto.jobs {
            let completed_tasks = completed_tasks.clone();
            let task_queue = task_queue.clone();
            let path = Arc::clone(&path);  // clone the Arc, not the PathBuf

            let handle = tokio::spawn(async move {
                loop {
                    let task = {
                        let mut task_queue = task_queue.lock().unwrap();
                        if task_queue.is_empty() {
                            break;
                        }
                        task_queue.pop_front().unwrap()
                    };

                    let env = Self::setup_env(&task.clone());

                    let result = async {
                        let mut all_deps_completed = false;

                        while !all_deps_completed {
                            {
                                let completed_tasks = completed_tasks.lock().unwrap();
                                all_deps_completed = task.deps.iter().all(|dep| completed_tasks.contains(dep));
                            }
                            if !all_deps_completed {
                                continue;
                            }
                        }
                        let mut path = path.as_path().to_path_buf();  // clone the PathBuf, not the Arc
                        path.push(&task.name);

                        // Write the action to a file
                        tokio::fs::write(&path, &task.action).await.map_err(|e| eyre!("Failed to write action to file: {}", e))?;

                        // All dependencies are completed, now run the task
                        let output = tokio::task::spawn_blocking(move || {
                            Command::new("sh")
                                .envs(&env)
                                .arg(path) // execute the script
                                .output()
                        }).await
                        .map_err(|e| eyre!("Failed to execute command: {}", e))?;

                        let output = output.map_err(|e| eyre!("Failed to execute command: {}", e))?;

                        let stdout = str::from_utf8(&output.stdout)
                            .map_err(|e| eyre!("Failed to parse stdout as UTF-8: {}", e))?;
                        println!("{stdout}");

                        if !output.status.success() {
                            return Err(eyre!("Task {} failed with exit code {:?}", task.name, output.status.code()));
                        }

                        // Mark the task as completed
                        completed_tasks.lock().unwrap().insert(task.name.clone());

                        Ok(())
                    };

                    if let Err(err) = result.await {
                        eprintln!("Error executing task {}: {}", task.name, err);
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.await?;
        }

        let completed_tasks_count = completed_tasks.lock().unwrap().len();
        if completed_tasks_count != num_tasks{
            return Err(eyre!("Not all tasks were completed. Completed: {}, Expected: {}", completed_tasks_count, num_tasks));
        }

        Ok(())
    }

    fn create_dir(&self) -> Result<PathBuf> {
        // Construct the path
        debug!("Expanding home directory path: {}", &self.otto.home);
        let canonical = expanduser(&self.otto.home)
            .map_err(|e| eyre!("Failed to expand home directory: {}", e))?;
        debug!("Expanded home directory path: {:?}", canonical);

        let home_dir = PathBuf::from(&canonical);
        debug!("Home directory resolved to: {:?}", home_dir);

        // Ensure the home directory and all parent directories exist
        debug!("Ensuring home directory exists: {:?}", home_dir);
        fs::create_dir_all(&home_dir)
            .map_err(|e| eyre!("Failed to create home directory: {}", e))?;

        // Create the hidden directory
        let hidden_hash_dir = format!(".{}", &self.hash);
        let hidden_dir_path = home_dir.join(&hidden_hash_dir);
        debug!("Creating hidden hash directory: {:?}", hidden_dir_path);
        fs::create_dir_all(&hidden_dir_path)
            .map_err(|e| eyre!("Failed to create hidden hash directory: {}", e))?;

        // Create the timestamp directory
        let timestamp_dir_path = home_dir.join(self.timestamp.to_string());
        debug!("Creating timestamp directory: {:?}", timestamp_dir_path);
        fs::create_dir_all(&timestamp_dir_path)
            .map_err(|e| eyre!("Failed to create timestamp directory: {}", e))?;

        // Create a symlink from the <first-12-chars-of-hex-hash> -> .<64-char-hex-hash>
        let symlink_name = &self.hash[..12];
        let symlink_path = timestamp_dir_path.join(symlink_name);
        debug!(
            "Creating symlink from {:?} to {:?}",
            hidden_dir_path, symlink_path
        );
        if symlink_path.exists() {
            debug!("Removing existing symlink: {:?}", symlink_path);
            fs::remove_file(&symlink_path)
                .map_err(|e| eyre!("Failed to remove existing symlink: {}", e))?;
        }
        std::os::unix::fs::symlink(&hidden_dir_path, &symlink_path)
            .map_err(|e| eyre!("Failed to create symlink: {}", e))?;

        // Create or update the "latest" symlink -> home / timestamp
        let latest_symlink_path = home_dir.join("latest");
        debug!(
            "Creating or updating latest symlink from {:?} to {:?}",
            timestamp_dir_path, latest_symlink_path
        );
        if latest_symlink_path.exists() {
            debug!("Removing existing latest symlink: {:?}", latest_symlink_path);
            fs::remove_file(&latest_symlink_path)
                .map_err(|e| eyre!("Failed to remove existing latest symlink: {}", e))?;
        }
        std::os::unix::fs::symlink(&timestamp_dir_path, &latest_symlink_path)
            .map_err(|e| eyre!("Failed to create latest symlink: {}", e))?;

        debug!("Directory setup completed successfully: {:?}", timestamp_dir_path);
        Ok(timestamp_dir_path)
    }

    pub fn get_tasks_to_execute(&self) -> Result<HashSet<String>> {
        let mut tasks_to_execute = HashSet::new();
        let mut visited_tasks = HashSet::new();
        let mut path = HashSet::new();

        for task in &self.otto.tasks {
            if visited_tasks.contains(task) {
                return Err(eyre!("Circular dependency detected for task: {}", task));
            } else {
                visited_tasks.insert(task.clone());
                path.insert(task.clone());
                self.add_dependencies(task, &mut tasks_to_execute, &mut visited_tasks, &mut path)?;
                path.remove(task);
            }
        }

        Ok(tasks_to_execute)
    }

    fn add_dependencies(
        &self,
        taskname: &String,
        tasks_to_execute: &mut HashSet<String>,
        visited_tasks: &mut HashSet<String>,
        path: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(task) = self.tasks.raw_nodes().iter().find(|task| task.weight.name == *taskname) {
            for dep in &task.weight.deps {
                // If the dependency is already in the current path from the root task,
                // then we have a circular dependency.
                if path.contains(dep) {
                    return Err(eyre!(
                        "Circular dependency detected between tasks: {} and {}",
                        taskname, dep
                    ));
                }
                path.insert(dep.clone());
                self.add_dependencies(dep, tasks_to_execute, visited_tasks, path)?;
                path.remove(dep); // Remove the task from the current path once it's fully processed.
            }
        }
        visited_tasks.insert(taskname.clone());
        tasks_to_execute.insert(taskname.clone());
        Ok(())
    }

    fn setup_env(task: &TaskSpec) -> HashMap<String, String> {
        let mut env: HashMap<String, String> = HashMap::new();
        for (k, v) in &task.envs {
            env.insert(k.into(), v.into());
        }
        for (k, v) in &task.values {
            if let Value::Item(val) = v {
                env.insert(k.into(), val.into());
            }
        }
        env
    }

}

