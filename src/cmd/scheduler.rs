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
    pub fn new(otto: Otto, tasks: DAG<TaskSpec>, hash: String) -> Self {
        Self {
            otto,
            tasks,
            hash,
            timestamp: *TIMESTAMP,
        }
    }

    fn create_dir(&self) -> Result<PathBuf> {
        // Construct the path
        let canonical = expanduser(&self.otto.home)
            .map_err(|e| eyre!("Failed to expand home directory: {}", e))?;
        let home_dir = PathBuf::from(&canonical);

        // Create the hidden directory if it doesn't already exist
        let hidden_hash_dir = format!(".{}", &self.hash);
        let hidden_dir_path = home_dir.join(&hidden_hash_dir);
        if !hidden_dir_path.exists() {
            fs::create_dir(&hidden_dir_path)?;
        }

        // Create the timestamp directory
        let timestamp_dir_path = home_dir.join(self.timestamp.to_string());
        fs::create_dir(&timestamp_dir_path)?;

        // Create a symlink from the <first-12-chars-of-hex-hash> -> .<64-char-hex-hash>
        let symlink_name = &self.hash[..12];
        let symlink_path = timestamp_dir_path.join(symlink_name);
        if symlink_path.exists() {
            fs::remove_file(&symlink_path)?;
        }
        std::os::unix::fs::symlink(&hidden_dir_path, &symlink_path)?;

        // Create or update the "latest" symlink -> home / timestamp
        let latest_symlink_path = home_dir.join("latest");
        if latest_symlink_path.exists() {
            fs::remove_file(&latest_symlink_path)?;
        }
        std::os::unix::fs::symlink(&timestamp_dir_path, &latest_symlink_path)?;

        Ok(timestamp_dir_path)
    }

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
                        println!("{}", stdout);

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


/*
    pub async fn run_async(&self) -> Result<()> {
        // Find the set of tasks to execute
        let tasks_to_execute = self.get_tasks_to_execute()?;
        let num_tasks = tasks_to_execute.len();

        let completed_tasks: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let task_queue: Arc<Mutex<VecDeque<TaskSpec>>> = Arc::new(Mutex::new(VecDeque::new()));

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

                        // All dependencies are completed, now run the task
                        let output = tokio::task::spawn_blocking(move || {
                            Command::new("sh")
                                .envs(&env)
                                .arg("-c")
                                .arg(&task.action)
                                .output()
                        }).await
                        .map_err(|e| eyre!("Failed to execute command: {}", e))?;

                        let output = output.map_err(|e| eyre!("Failed to execute command: {}", e))?;

                        let stdout = str::from_utf8(&output.stdout)
                            .map_err(|e| eyre!("Failed to parse stdout as UTF-8: {}", e))?;
                        println!("{}", stdout);

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
*/

    pub fn get_tasks_to_execute(&self) -> Result<HashSet<String>> {
        let mut tasks_to_execute = HashSet::new();
        let mut visited_tasks = HashSet::new();
        let mut path = HashSet::new();

        for task in &self.otto.tasks {
            if !visited_tasks.contains(task) {
                visited_tasks.insert(task.clone());
                path.insert(task.clone());
                self.add_dependencies(task, &mut tasks_to_execute, &mut visited_tasks, &mut path)?;
                path.remove(task);
            } else {
                return Err(eyre!("Circular dependency detected for task: {}", task));
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

