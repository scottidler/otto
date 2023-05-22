use std::process::Command;
use std::sync::{Arc, Mutex, Condvar};
use std::collections::{HashSet, HashMap};
use eyre::{Result, eyre};
use std::str;

use crate::cli::parse::{Job, DAG};
use crate::cfg::param::Value;
use crate::cfg::otto::Otto;

pub struct Scheduler {
    pub otto: Otto,
    pub jobs: DAG<Job>,
}

impl Scheduler {
    pub fn new(otto: Otto, jobs: DAG<Job>) -> Self {
        Self {
            otto,
            jobs,
        }
    }

    pub fn run(&self) -> Result<()> {
        // Find the set of tasks to execute
        let tasks_to_execute = self.get_tasks_to_execute()?;
        let num_jobs = tasks_to_execute.len();

        let completed_jobs: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let condvar: Arc<Condvar> = Arc::new(Condvar::new());

        rayon::scope(|s| {
            for node in self.jobs.raw_nodes() {
                let job = node.weight.clone();

                // Skip this job if it's not in the set of tasks to execute
                if !tasks_to_execute.contains(&job.name) {
                    continue;
                }

                let completed_jobs = completed_jobs.clone();
                let condvar = condvar.clone();

                s.spawn(move |_| {
                    let result = || -> Result<()> {
                        let mut all_deps_completed = false;

                        while !all_deps_completed {
                            {
                                let completed_jobs = completed_jobs.lock().unwrap();
                                all_deps_completed = job.deps.iter().all(|dep| completed_jobs.contains(dep));
                            }
                            if !all_deps_completed {
                                continue;
                            }
                        }

                        let mut env: HashMap<String, String> = HashMap::new();
                        for (k, v) in &job.envs {
                            env.insert(k.into(), v.into());
                        }
                        for (k, v) in &job.values {
                            if let Value::Item(val) = v {
                                env.insert(k.into(), val.into());
                            }
                        }

                        // All dependencies are completed, now run the job
                        let output = Command::new("sh")
                            .envs(&env)
                            .arg("-c")
                            .arg(&job.action)
                            .output()
                            .map_err(|e| eyre!("Failed to execute command: {}", e))?;

                        let stdout = str::from_utf8(&output.stdout)
                            .map_err(|e| eyre!("Failed to parse stdout as UTF-8: {}", e))?;
                        println!("{}", stdout);

                        if !output.status.success() {
                            return Err(eyre!("Job {} failed with exit code {:?}", job.name, output.status.code()));
                        }

                        // Mark the job as completed
                        completed_jobs.lock().unwrap().insert(job.name.clone());

                        // Notify other jobs that a job has finished.
                        condvar.notify_all();

                        Ok(())
                    };

                    if let Err(err) = result() {
                        eprintln!("Error executing job {}: {}", job.name, err);
                    }
                });
            }
        });

        let completed_jobs_count = completed_jobs.lock().unwrap().len();
        if completed_jobs_count != num_jobs {
            return Err(eyre!("Not all jobs were completed. Completed: {}, Expected: {}", completed_jobs_count, num_jobs));
        }

        Ok(())
    }

/*
    pub fn get_tasks_to_execute(&self) -> Result<HashSet<String>> {
        let mut tasks_to_execute = HashSet::new();
        let mut visited_tasks = HashSet::new();

        for task in &self.otto.tasks {
            if !visited_tasks.contains(task) {
                visited_tasks.insert(task.clone());
                self.add_dependencies(task, &mut tasks_to_execute, &mut visited_tasks)?;
            } else {
                return Err(eyre!("Circular dependency detected for task: {}", task));
            }
        }

        Ok(tasks_to_execute)
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
        task: &String,
        tasks_to_execute: &mut HashSet<String>,
        visited_tasks: &mut HashSet<String>,
        path: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(job) = self.jobs.raw_nodes().iter().find(|node| node.weight.name == *task) {
            for dep in &job.weight.deps {
                // If the dependency is already in the current path from the root task,
                // then we have a circular dependency.
                if path.contains(dep) {
                    return Err(eyre!(
                        "Circular dependency detected between tasks: {} and {}",
                        task, dep
                    ));
                }
                path.insert(dep.clone());
                self.add_dependencies(dep, tasks_to_execute, visited_tasks, path)?;
                path.remove(dep); // Remove the task from the current path once it's fully processed.
            }
        }
        visited_tasks.insert(task.clone());
        tasks_to_execute.insert(task.clone());
        Ok(())
    }



/*
    fn add_dependencies(
        &self,
        task: &String,
        tasks_to_execute: &mut HashSet<String>,
        visited_tasks: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(job) = self.jobs.raw_nodes().iter().find(|node| node.weight.name == *task) {
            for dep in &job.weight.deps {
                if visited_tasks.contains(dep) {
                    return Err(eyre!(
                        "Circular dependency detected between tasks: {} and {}",
                        task, dep
                    ));
                }
                visited_tasks.insert(dep.clone());
                self.add_dependencies(dep, tasks_to_execute, visited_tasks)?;
            }
        }
        tasks_to_execute.insert(task.clone());
        Ok(())
    }
*/

/*
    fn add_dependencies(
        &self,
        task: &String,
        tasks_to_execute: &mut HashSet<String>,
        visited_tasks: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(job) = self.jobs.raw_nodes().iter().find(|node| node.weight.name == *task) {
            for dep in &job.weight.deps {
                if visited_tasks.contains(dep) {
                    return Err(eyre!(
                        "Circular dependency detected between tasks: {} and {}",
                        task, dep
                    ));
                }

                // The recursive call was moved up to ensure we visit the dependencies first
                self.add_dependencies(dep, tasks_to_execute, visited_tasks)?;

                // Moved this line here, after the dependencies are visited, to prevent
                // a false circular dependency detection. Previously, it was inserted
                // into the visited_tasks set before its dependencies were visited.
                visited_tasks.insert(dep.clone());
            }
        }
        tasks_to_execute.insert(task.clone());
        Ok(())
    }
*/

}
