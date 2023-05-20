use std::process::Command;
use std::sync::{Arc, Mutex, Condvar};
use std::collections::{HashSet, HashMap};

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

/*
    pub fn run(&self) {
        let num_jobs = self.jobs.raw_nodes().len();
        println!("num_jobs: {}", num_jobs);

        let completed_jobs: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let condvar: Arc<Condvar> = Arc::new(Condvar::new());

        rayon::scope(|s| {
            for node in self.jobs.raw_nodes() {
                let job = node.weight.clone();
                println!("job: {:?}", job);
                let completed_jobs = completed_jobs.clone();
                let condvar = condvar.clone();

                s.spawn(move |_| {
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
                        .expect("Failed to execute command");

                    println!("output: {:?}", output);

                    if !output.status.success() {
                        panic!("Job {} failed with exit code {:?}", job.name, output.status.code());
                    }

                    // Mark the job as completed
                    completed_jobs.lock().unwrap().insert(job.name.clone());

                    // Notify other jobs that a job has finished.
                    condvar.notify_all();
                });
            }
        });

        assert_eq!(completed_jobs.lock().unwrap().len(), num_jobs, "All jobs should be completed");
        println!("All jobs completed");
    }
*/



    pub fn run(&self) {
        // Find the set of tasks to execute
        let tasks_to_execute = self.get_tasks_to_execute();
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

                println!("job: {:?}", job);
                let completed_jobs = completed_jobs.clone();
                let condvar = condvar.clone();

                s.spawn(move |_| {
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
                        .expect("Failed to execute command");

                    println!("output: {:?}", output);

                    if !output.status.success() {
                        panic!("Job {} failed with exit code {:?}", job.name, output.status.code());
                    }

                    // Mark the job as completed
                    completed_jobs.lock().unwrap().insert(job.name.clone());

                    // Notify other jobs that a job has finished.
                    condvar.notify_all();
                });
            }
        });

        assert_eq!(completed_jobs.lock().unwrap().len(), num_jobs, "All jobs should be completed");
        println!("All jobs completed");
    }

    pub fn get_tasks_to_execute(&self) -> HashSet<String> {
        let mut tasks_to_execute = HashSet::new();

        // Add the tasks specified in Otto.tasks to the set.
        for task in &self.otto.tasks {
            tasks_to_execute.insert(task.clone());
        }

        // For each task, add its dependencies to the set.
        for task in &self.otto.tasks {
            if let Some(job) = self.jobs.raw_nodes().iter().find(|node| node.weight.name == *task) {
                for dep in &job.weight.deps {
                    tasks_to_execute.insert(dep.clone());
                }
            }
        }

        tasks_to_execute
    }



}
