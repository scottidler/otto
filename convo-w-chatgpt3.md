are you familiar with pydoit?

Yes, I am familiar with pydoit. pydoit is a task management and automation tool written in Python. It is designed to be a simple, flexible, and extensible alternative to other task runners such as make, rake, or ant.

pydoit allows you to define tasks in Python code, and then run those tasks from the command line or from within your Python code. You can use pydoit to automate a wide variety of tasks, such as building and testing software, deploying code, or running data processing pipelines.

One of the key features of pydoit is its ability to automatically figure out dependencies between tasks, so that tasks are only run when their dependencies have been met. This can make it easier to build and maintain complex automation systems, as you don't have to manually specify the order in which tasks should be run.

pydoit also has a number of other features, such as the ability to run tasks in parallel, to store and retrieve task output, and to define custom task actions. You can find more information about pydoit and how to use it in the pydoit documentation.
scott idler
Let's rewrite it in Rust

Rewriting pydoit in Rust is certainly possible, and Rust is a good choice for building command-line tools and other kinds of automation software. Here are a few things to consider if you are planning to rewrite pydoit in Rust:

    Familiarize yourself with Rust: If you are not already familiar with Rust, it would be a good idea to spend some time learning the basics of the language and its ecosystem. This will help you get started with your rewrite and ensure that you are using Rust effectively.

    Understand the features and capabilities of pydoit: Take some time to understand how pydoit works and what it is capable of. This will help you determine which features you will need to implement in your Rust version of pydoit, and how you might go about doing so.

    Break the project down into smaller pieces: Rewriting pydoit in Rust is likely to be a large project, so it will be helpful to break it down into smaller pieces that you can tackle one at a time. This could involve starting with a subset of pydoit's functionality and gradually adding more features as you go.

    Consider using existing Rust libraries: There are many libraries and frameworks available in Rust that could help you implement some of the features of pydoit. For example, you might be able to use the tokio or async-std libraries to implement task execution and parallelism, or the serde library for storing and loading task output.

Overall, rewriting pydoit in Rust will likely be a challenging but rewarding project that will give you the opportunity to learn and grow as a Rust developer.
scott idler
Let's do it!

It's great that you are excited about the idea of rewriting pydoit in Rust! Here are a few steps you can take to get started:

    Familiarize yourself with Rust: If you are not already familiar with Rust, take some time to learn the basics of the language and its ecosystem. This will help you understand how Rust works and how to use it effectively.

    Understand the features and capabilities of pydoit: Review the pydoit documentation and try out the tool to get a good understanding of its features and how it works. This will help you determine which features you need to implement in your Rust version of pydoit.

    Break the project down into smaller pieces: As mentioned earlier, rewriting pydoit in Rust is likely to be a large project, so it will be helpful to break it down into smaller pieces that you can tackle one at a time. This could involve starting with a subset of pydoit's functionality and gradually adding more features as you go.

    Consider using existing Rust libraries: There are many libraries and frameworks available in Rust that could help you implement some of the features of pydoit. For example, you might be able to use the tokio or async-std libraries to implement task execution and parallelism, or the serde library for storing and loading task output.

    Start coding: With these steps in mind, you can begin writing code for your Rust version of pydoit. Be sure to take advantage of the resources available to Rust developers, such as the Rust documentation, online tutorials, and the Rust community forums.

I hope this helps get you started on your project! Good luck, and I'm here if you have any questions along the way.
scott idler
you already said that. I want to see some rust code

Certainly! Here is an example of how you might implement a simple task in Rust using pydoit:

use std::error::Error;

fn my_task() -> Result<(), Box<dyn Error>> {
    // Perform some action here

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    // Run the task
    my_task()
}

This code defines a function my_task that performs some action, and then a main function that calls my_task. The Result type is used to indicate the success or failure of the task, and the Box<dyn Error> type is used to represent any error that might occur.

Note that this is just a simple example, and it does not include all of the features of pydoit such as task dependencies and parallel execution. However, it should give you an idea of how you might go about implementing tasks in Rust using pydoit.

I hope this helps! Let me know if you have any questions or if you would like to see more examples.
scott idler
Let's get more into the details and show some code of the key areas: task execution, parallelism, dependency tracking, etc

Certainly! Here is an example of how you might implement task execution, parallelism, and dependency tracking in Rust using pydoit:

use std::error::Error;
use tokio::runtime::Runtime;

struct Task {
    name: String,
    dependencies: Vec<String>,
    action: Box<dyn Fn() -> Result<(), Box<dyn Error>>>,
}

impl Task {
    fn new<F>(name: &str, dependencies: Vec<String>, action: F) -> Self
        where F: 'static + Fn() -> Result<(), Box<dyn Error>>
    {
        Self {
            name: name.to_string(),
            dependencies,
            action: Box::new(action),
        }
    }

    fn run(&self) -> Result<(), Box<dyn Error>> {
        (self.action)().map_err(|e| eprintln!("Task {} failed: {}", self.name, e))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let tasks = vec![
        Task::new("task1", vec![], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task2", vec!["task1"], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task3", vec!["task1"], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task4", vec!["task2", "task3"], || {
            // Perform some action here
            Ok(())
        }),
    ];

    let mut runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut tasks_to_run = tasks.clone();

        while !tasks_to_run.is_empty() {
            let mut next_batch = vec![];

            for task in tasks_to_run {
                let ready = task.dependencies.iter().all(|dep| {
                    tasks.iter().any(|t| t.name == *dep && t.run().is_ok())
                });

                if ready {
                    task.run()?;
                } else {
                    next_batch.push(task);
                }
            }

            tasks_to_run = next_batch;
        }

        Ok(())
    })?;

    Ok(())
}

This code defines a Task struct that contains the name of the task, its dependencies, and the action to be performed when the task is run. The Task struct also implements a run method that runs the task's action and handles any errors that might occur.

The main function creates a vector of Task instances and then runs them using the Tokio runtime. The tasks are run in batches, with each batch containing only the tasks that have all of their dependencies satisfied. This allows the tasks to be run in the correct order and in parallel when possible.

I hope this gives you an idea of how you might implement some of the key features of pydoit in Rust. Let me know if you have any questions or if you would like to see more examples.
scott idler
Assuming you have some limit to how many tasks can run in parallel (probably based upon numcpus) how do you launch a new task, after one of the running tasks completes so that you can keep the amount of parallelism at the limit which the program executes its DAG of tasks?

To launch a new task after one of the running tasks completes, so that you can keep the amount of parallelism at the limit, you can use a combination of a queue and a fixed-size pool of worker tasks. Here is an example of how you might do this in Rust using the tokio crate:

use std::error::Error;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

struct Task {
    name: String,
    dependencies: Vec<String>,
    action: Box<dyn Fn() -> Result<(), Box<dyn Error>>>,
}

impl Task {
    fn new<F>(name: &str, dependencies: Vec<String>, action: F) -> Self
        where F: 'static + Fn() -> Result<(), Box<dyn Error>>
    {
        Self {
            name: name.to_string(),
            dependencies,
            action: Box::new(action),
        }
    }

    fn run(&self) -> Result<(), Box<dyn Error>> {
        (self.action)().map_err(|e| eprintln!("Task {} failed: {}", self.name, e))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let tasks = vec![
        Task::new("task1", vec![], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task2", vec!["task1"], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task3", vec!["task1"], || {
            // Perform some action here
            Ok(())
        }),
        Task::new("task4", vec!["task2", "task3"], || {
            // Perform some action here
            Ok(())
        }),
    ];

    let num_cpus = num_cpus::get();

    let (task_sender, task_receiver) = mpsc::channel(num_cpus);
    let mut runtime = Runtime::new()?;

    for _ in 0..num_cpus {
        let task_receiver = task_receiver.clone();
        runtime.spawn(async move {
            while let Some(task) = task_receiver.recv().await {
                task.run().unwrap_or_else(|e| eprintln!("Task {} failed: {}", task.name, e));
            }
        });
    }

    let mut tasks_to_run = tasks.clone();

    while !tasks_to_run.is_empty() {
        let mut next_batch = vec![];

        for task in tasks_to_run {
            let ready = task.dependencies.iter().all(|dep| {
                tasks.iter().any(|t| t.name == *dep && t.run().is_ok())
            });

            if ready {
                task_sender.send(task).await?;
            } else {
                next_batch.push(task);
            }
        }

        tasks_to_run = next_batch;
    }

    Ok(())
}

In this example, the main function creates a
scott idler
continue

In this example, the main function creates a fixed-size pool of worker tasks and a queue for sending tasks to the workers. The main function then adds the tasks to the queue, and the worker tasks pick tasks from the queue and run them.

This approach allows you to limit the amount of parallelism to the number of CPU cores, as specified by the num_cpus variable. The worker tasks will only run a new task when a previous task has completed, which ensures that the amount of parallelism stays at the desired level.

I hope this helps! Let me know if you have any questions or if you would like to see more examples.
