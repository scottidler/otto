# Otto Clap-Based Two-Pass Parsing Design Document

## Overview

This document describes the sophisticated clap-based two-pass parsing approach implemented in Otto CLI revision `84577fd04069d136b44121195342862fed669fb3` (before nom was introduced). This implementation solved the challenge of parsing multiple task invocations in a single command line while supporting dynamic task definitions loaded from configuration files.

## The Challenge

Otto needed to support complex command lines like:
```bash
otto --ottofile=config.yml build --release=true test --verbose deploy --env=staging
```

This presents several challenges:
1. **Ottofile Discovery**: The `--ottofile` argument must be parsed first to load task definitions
2. **Dynamic Task Recognition**: Task names are not known until the configuration is loaded
3. **Argument Partitioning**: Arguments must be partitioned between tasks and validated against their individual parameter specifications
4. **Multiple Clap Parsers**: Different clap parsers needed for global options vs. task-specific arguments

## Architecture Overview

The solution implements a **two-pass parsing strategy** with sophisticated argument partitioning:

### Phase 1: Global Options Parsing with Clap
- **Purpose**: Parse global options like `--ottofile`, `--jobs`, etc.
- **Implementation**: Uses clap with `try_get_matches_from()` to allow external subcommands
- **Result**: Extracts ottofile path and loads configuration

### Phase 2: Manual Argument Partitioning
- **Purpose**: Identify task names and partition arguments between tasks
- **Implementation**: Manual parsing to separate global options from task arguments
- **Result**: Creates partitioned argument lists for each task

### Phase 3: Task-Specific Clap Parsing
- **Purpose**: Parse each task's arguments using dynamically generated clap parsers
- **Implementation**: Creates individual clap `Command` for each task based on configuration
- **Result**: Validated task arguments and parameters

## Detailed Implementation

### 1. Core Data Structures

**Location**: `src/cli/parse.rs`

```rust
pub struct Parser {
    prog: String,
    cwd: PathBuf,
    user: String,
    config_spec: ConfigSpec,
    hash: String,
    args: Vec<String>,
    pargs: Vec<Vec<String>>,  // Partitioned arguments
    ottofile: Option<PathBuf>,
}

pub struct Task {
    pub name: String,
    pub task_deps: Vec<String>,
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
}
```

### 2. Argument Partitioning Algorithm

**Core Functions**:

```rust
fn indices(args: &[String], task_names: &[&str]) -> Vec<usize> {
    let mut indices = vec![];
    for (i, arg) in args.iter().enumerate() {
        if task_names.contains(&arg.as_str()) {
            indices.push(i);
        }
    }
    indices
}

fn partitions(args: &Vec<String>, task_names: &[&str]) -> Vec<Vec<String>> {
    let task_indices = indices(args, task_names);
    if task_indices.is_empty() {
        return vec![];
    }

    let mut partitions = vec![];
    let mut end = args.len();

    for &index in task_indices.iter().rev() {
        partitions.insert(0, args[index..end].to_vec());
        end = index;
    }

    partitions
}
```

**Algorithm**:
1. Find indices of all task names in the argument list
2. Partition arguments between consecutive task names (reverse iteration)
3. Each partition contains one task name followed by its arguments

**Example**:
```
Input: ["build", "--release=true", "test", "--verbose", "deploy", "--env=staging"]
Task names: ["build", "test", "deploy"]
Partitions:
  - ["build", "--release=true"]
  - ["test", "--verbose"]
  - ["deploy", "--env=staging"]
```

### 3. Two-Pass Parsing Implementation

**Main Parse Function**: `Parser::parse()`

#### Stage 1: Global Options Parsing

```rust
// Stage 1: Parse global options with default config
let default_otto_spec = OttoSpec::default();
let otto_cmd = Self::otto_command(&default_otto_spec);

// Try to parse with allow_external_subcommands to capture remaining args
let matches = match otto_cmd.try_get_matches_from(&self.args) {
    Ok(m) => m,
    Err(e) => {
        use clap::error::ErrorKind;
        match e.kind() {
            ErrorKind::DisplayVersion | ErrorKind::DisplayHelp => {
                e.print().expect("clap error print failed");
                std::process::exit(0);
            }
            _ => return Err(eyre!(e)),
        }
    }
};

// Extract ottofile path and load config
let ottofile_value = matches.get_one::<String>("ottofile")
    .map(|s| s.clone())
    .unwrap_or_else(|| env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned()));

let ottofile_path = Self::divine_ottofile(ottofile_value)?;
let (config_spec, hash, ottofile) = Self::load_config_from_path(ottofile_path)?;
```

#### Stage 2: Manual Argument Extraction

```rust
// Stage 2: Extract remaining arguments manually from original args
let mut remaining_args = Vec::new();
let mut skip_next = false;
let mut in_task_args = false;

// Include both configured tasks AND built-in meta-tasks like "graph"
let mut task_names: Vec<&str> = self.config_spec.tasks.keys().map(String::as_str).collect();
task_names.push("graph"); // Always include graph as a built-in task name

for (_i, arg) in self.args.iter().enumerate().skip(1) { // Skip program name
    if skip_next {
        skip_next = false;
        continue;
    }

    // Check if this is a global option that takes a value
    if arg == "-o" || arg == "--ottofile" ||
       arg == "-a" || arg == "--api" ||
       arg == "-j" || arg == "--jobs" ||
       arg == "-H" || arg == "--home" ||
       arg == "-t" || arg == "--tasks" ||
       arg == "-v" || arg == "--verbosity" ||
       arg == "-T" || arg == "--timeout" {
        skip_next = true; // Skip the value
        continue;
    }

    // Check if this is a global flag
    if arg == "-h" || arg == "--help" {
        continue; // Already handled
    }

    // Check if this is a task name
    if task_names.contains(&arg.as_str()) {
        in_task_args = true;
    }

    if in_task_args {
        remaining_args.push(arg.clone());
    }
}
```

#### Stage 3: Task Argument Partitioning and Parsing

```rust
// Partition the remaining args by task names
let partitions = partitions(&remaining_args, &task_names);
self.pargs = partitions;

// Extract task names from partitions
let configured_tasks = self.pargs.iter()
    .filter_map(|p| if p.is_empty() { None } else { Some(p[0].clone()) })
    .collect::<Vec<String>>();

otto.tasks = configured_tasks;
```

### 4. Dynamic Clap Command Generation

**Location**: `src/cli/parse.rs`

#### Global Options Command

```rust
fn otto_command(otto_spec: &OttoSpec) -> Command {
    Command::new("otto")
        .version(env!("GIT_DESCRIBE"))
        .about("A task runner")
        .arg(
            Arg::new("ottofile")
                .short('o')
                .long("ottofile")
                .value_name("PATH")
                .help("path to the ottofile")
                .default_value(&otto_spec.home)
                .value_parser(value_parser!(String))
        )
        .arg(
            Arg::new("api")
                .short('a')
                .long("api")
                .value_name("URL")
                .help("api url")
                .default_value(&otto_spec.api)
                .value_parser(value_parser!(String))
        )
        .arg(
            Arg::new("jobs")
                .short('j')
                .long("jobs")
                .value_name("JOBS")
                .help("number of jobs to run in parallel")
                .default_value(&otto_spec.jobs.to_string())
                .value_parser(value_parser!(String))
        )
        // ... more global options
        .allow_external_subcommands(true)
}
```

#### Task-Specific Command Generation

```rust
fn task_to_command(task_spec: &TaskSpec) -> Command {
    let mut cmd = Command::new(&task_spec.name);

    if let Some(ref help) = task_spec.help {
        cmd = cmd.about(help);
    }

    for param_spec in task_spec.params.values() {
        let arg = Self::param_to_arg(param_spec);
        cmd = cmd.arg(arg);
    }

    cmd
}

fn param_to_arg(param_spec: &ParamSpec) -> Arg {
    let mut arg = Arg::new(&param_spec.name);

    if let Some(short) = param_spec.short {
        arg = arg.short(short);
    }

    if let Some(ref long) = param_spec.long {
        arg = arg.long(long);
    }

    if let Some(ref help) = param_spec.help {
        arg = arg.help(help);
    }

    arg.value_parser(value_parser!(String))
}
```

### 5. Task Processing and Validation

**Location**: `src/cli/parse.rs:process_tasks_with_filter()`

```rust
fn process_tasks_with_filter(&self, requested_tasks: &[String]) -> Result<DAG<Task>> {
    // Step 0: Evaluate global environment variables once
    let global_envs = if self.config_spec.otto.envs.is_empty() {
        HashMap::new()
    } else {
        env_eval::evaluate_envs(&self.config_spec.otto.envs, Some(&self.cwd))
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to evaluate global environment variables: {}", e);
                HashMap::new()
            })
    };

    // Step 1: Compute all task dependencies using simple linear algorithm
    let task_deps = self.compute_task_deps()?;

    // Step 2: Find all tasks we need (requested + their transitive dependencies)
    let mut tasks_needed = HashSet::new();
    for task_name in requested_tasks {
        self.collect_transitive_deps(task_name, &task_deps, &mut tasks_needed)?;
    }

    // Step 3: Build DAG from needed tasks
    let mut dag: DAG<Task> = DAG::new();
    let mut indices: HashMap<String, NodeIndex<u32>> = HashMap::new();

    // Add all needed tasks to DAG first
    for task_name in &tasks_needed {
        let task_spec = self.config_spec.tasks.get(task_name)
            .ok_or_else(|| eyre!("Task '{}' not found", task_name))?;

        let mut spec = Task::from_task_with_cwd_and_global_envs(task_spec, &self.cwd, &global_envs);

        // Find the partition for this task's arguments
        if let Some(task_args) = self.pargs.iter().find(|args| !args.is_empty() && args[0] == *task_name) {
            if task_args.len() > 1 {
                // Parse task arguments using clap
                let task_command = Self::task_to_command(task_spec);
                let matches = task_command.get_matches_from(task_args);

                for param_spec in task_spec.params.values() {
                    if let Some(value) = matches.get_one::<String>(param_spec.name.as_str()) {
                        spec.values.insert(param_spec.name.clone(), Value::Item(value.to_string()));
                        // Also add to environment variables
                        spec.envs.insert(param_spec.name.clone(), value.to_string());
                    }
                }
            }
        }

        // Override task_deps with computed dependencies
        spec.task_deps = task_deps.get(task_name)
            .map(|deps| deps.iter().cloned().collect())
            .unwrap_or_default();

        let index = dag.add_node(spec);
        indices.insert(task_name.clone(), index);
    }

    // Add edges based on computed dependencies
    for task_name in &tasks_needed {
        let task_index = indices[task_name];
        if let Some(deps) = task_deps.get(task_name) {
            for dep_name in deps {
                if let Some(&dep_index) = indices.get(dep_name) {
                    dag.add_edge(dep_index, task_index, ())?;
                }
            }
        }
    }

    Ok(dag)
}
```

### 6. Help System Integration

The implementation includes sophisticated help handling:

#### Global Help
```rust
// Check for top-level help first, before any parsing
if self.args.iter().any(|arg| arg == "--help" || arg == "-h") {
    // Load config for top-level help
    let ottofile_value = self.args.iter()
        .position(|arg| arg == "-o" || arg == "--ottofile")
        .and_then(|i| self.args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned()));

    let ottofile_path = Self::divine_ottofile(ottofile_value)?;
    let (config_spec, _hash, _ottofile) = Self::load_config_from_path(ottofile_path)?;

    let mut help_cmd = Self::help_command(&config_spec.otto, &config_spec.tasks);
    help_cmd.print_help()?;
    std::process::exit(0);
}
```

#### Task-Specific Help
```rust
// Check if help comes after a task name
for i in 1..self.args.len() {
    if (self.args[i] == "--help" || self.args[i] == "-h") && i > 1 {
        // Check if previous arg is a task name (we'll need to load config first)
        let ottofile_value = self.args.iter()
            .position(|arg| arg == "-o" || arg == "--ottofile")
            .and_then(|i| self.args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned()));

        let ottofile_path = Self::divine_ottofile(ottofile_value)?;
        let (config_spec, _hash, _ottofile) = Self::load_config_from_path(ottofile_path)?;

        task_name = self.args[i - 1].clone();
        // Check for both configured tasks and built-in meta-tasks
        if config_spec.tasks.contains_key(&task_name) || task_name == "graph" {
            help_after_task = true;
            self.config_spec = config_spec;
            self.inject_graph_meta_task();
            break;
        }
    }
}

if help_after_task {
    // Show task-specific help
    if let Some(task) = self.config_spec.tasks.get(&task_name) {
        let mut task_cmd = Self::task_to_command(task);
        task_cmd.print_help()?;
        std::process::exit(0);
    }
}
```

### 7. Configuration Loading and Ottofile Discovery

**Location**: `src/cli/parse.rs`

#### Ottofile Discovery
```rust
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

fn divine_ottofile(value: String) -> Result<Option<PathBuf>> {
    let mut path = expanduser(value)?;
    path = fs::canonicalize(path)?;
    if path.is_dir() {
        return Self::find_ottofile(&path);
    }
    Ok(Some(path))
}
```

#### Configuration Loading
```rust
fn load_config_from_path(ottofile_path: Option<PathBuf>) -> Result<(ConfigSpec, String, Option<PathBuf>)> {
    if let Some(ottofile) = ottofile_path {
        let content = fs::read_to_string(&ottofile)?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let result = hasher.finalize();
        let hash = hex::encode(&result)[..8].to_string();
        let config_spec: ConfigSpec = serde_yaml::from_str(&content)?;
        Ok((config_spec, hash, Some(ottofile)))
    } else {
        Err(eyre!("No ottofile found in this directory or any parent directory!"))
    }
}
```

### 8. Built-in Meta-Tasks

The implementation includes support for built-in meta-tasks like `graph`:

```rust
fn inject_graph_meta_task(&mut self) {
    // Add graph meta-task to the configuration
    let graph_task = TaskSpec {
        name: "graph".to_string(),
        help: Some("Visualize the task dependency graph".to_string()),
        after: vec![],
        before: vec![],
        input: vec![],
        output: vec![],
        envs: HashMap::new(),
        params: {
            let mut params = HashMap::new();

            // Add --format parameter
            params.insert("format".to_string(), ParamSpec {
                name: "format".to_string(),
                short: Some('f'),
                long: Some("format".to_string()),
                param_type: crate::cfg::param::ParamType::OPT,
                dest: None,
                metavar: None,
                default: Some("ascii".to_string()),
                constant: crate::cfg::param::Value::Empty,
                choices: vec!["ascii".to_string(), "dot".to_string(), "svg".to_string(), "png".to_string(), "pdf".to_string()],
                nargs: crate::cfg::param::Nargs::One,
                help: Some("Output format".to_string()),
                value: crate::cfg::param::Value::Empty,
            });

            // Add --output parameter
            params.insert("output".to_string(), ParamSpec {
                name: "output".to_string(),
                short: None,
                long: Some("output".to_string()),
                param_type: crate::cfg::param::ParamType::OPT,
                dest: None,
                metavar: None,
                default: None,
                constant: crate::cfg::param::Value::Empty,
                choices: vec![],
                nargs: crate::cfg::param::Nargs::One,
                help: Some("Output file path".to_string()),
                value: crate::cfg::param::Value::Empty,
            });

            params
        },
        action: "# Built-in graph command".to_string(),
        timeout: None,
    };

    self.config_spec.tasks.insert("graph".to_string(), graph_task);
}
```

### 9. Error Handling

**Location**: `src/cli/error.rs`

The implementation uses `eyre` for error handling with specific helper functions:

```rust
pub type OttoResult<T> = Result<T, Report>;

pub fn clap_error(source: clap::Error) -> Report {
    eyre!("Clap parse error: {}", source)
}

pub fn config_error(source: Report) -> Report {
    eyre!("config error: {}", source)
}

pub fn divine_error(path: PathBuf) -> Report {
    eyre!("divine error; unable to find ottofile from path=[{}]", path.display())
}
```

### 10. Testing Strategy

The implementation includes comprehensive tests covering:

#### Argument Partitioning Tests
```rust
#[test]
fn test_indices() {
    let args = vec!["task1".to_string(), "arg2".to_string(), "task2".to_string(), "arg3".to_string()];
    let task_names = &["task1", "task2"];
    let expected = vec![0, 2];
    assert_eq!(indices(&args, task_names), expected);
}

#[test]
fn test_partitions() {
    let args = vec!["task1".to_string(), "arg2".to_string(), "task2".to_string(), "arg3".to_string()];
    let task_names = &["task1", "task2"];
    let expected = vec![
        vec!["task1".to_string(), "arg2".to_string()],
        vec!["task2".to_string(), "arg3".to_string()]
    ];
    assert_eq!(partitions(&args, task_names), expected);
}
```

#### Multi-Task Parsing Tests
```rust
#[test]
fn test_multi_task_parsing() -> Result<()> {
    let args = vec![
        "otto".to_string(),
        "task1".to_string(),
        "--param1=value1".to_string(),
        "task2".to_string(),
        "--param2=value2".to_string(),
    ];

    let mut parser = Parser::new(args)?;
    let (otto, tasks, _hash, _ottofile) = parser.parse()?;

    assert_eq!(otto.tasks.len(), 2);
    assert_eq!(otto.tasks[0], "task1");
    assert_eq!(otto.tasks[1], "task2");

    Ok(())
}
```

#### Complex Argument Tests
```rust
#[test]
fn test_multiple_tasks_complex_args() {
    let args = vec![
        "otto".to_string(),
        "--jobs=4".to_string(),
        "build".to_string(),
        "--release".to_string(),
        "--target=x86_64-unknown-linux-gnu".to_string(),
        "test".to_string(),
        "--verbose".to_string(),
        "--filter=integration".to_string(),
        "deploy".to_string(),
        "--environment=staging".to_string(),
    ];

    let task_names = &["build", "test", "deploy"];
    let expected = vec![
        vec!["build", "--release", "--target=x86_64-unknown-linux-gnu"],
        vec!["test", "--verbose", "--filter=integration"],
        vec!["deploy", "--environment=staging"]
    ];

    assert_eq!(partitions(&args, task_names), expected);
}
```

## Key Design Decisions

### 1. Clap Integration Strategy
- **Rationale**: Leverage clap's excellent argument parsing and help generation
- **Implementation**: Use clap for both global options and individual task parsing
- **Benefits**: Consistent CLI experience, automatic help generation, robust validation

### 2. Two-Stage Parsing Approach
- **Rationale**: Ottofile must be loaded before task names are known
- **Implementation**: Parse global options first, then load config, then parse tasks
- **Benefits**: Enables dynamic task recognition and validation

### 3. Manual Argument Partitioning
- **Rationale**: Clap doesn't natively support multiple subcommands in one invocation
- **Implementation**: Custom partitioning algorithm based on task name recognition
- **Benefits**: Allows complex command lines with multiple tasks

### 4. Dynamic Command Generation
- **Rationale**: Task parameters are defined in YAML configuration
- **Implementation**: Generate clap `Command` objects from configuration at runtime
- **Benefits**: Flexible task definitions without code changes

### 5. Comprehensive Help System
- **Rationale**: CLI tools need excellent help and error messages
- **Implementation**: Integrated help for both global options and individual tasks
- **Benefits**: Better user experience and discoverability

### 6. Built-in Meta-Tasks
- **Rationale**: Some tasks are fundamental to the tool (like graph visualization)
- **Implementation**: Inject built-in tasks into configuration at runtime
- **Benefits**: Consistent interface for both configured and built-in tasks

## Performance Considerations

### 1. Configuration Loading
- **Optimization**: Load configuration only once during parsing
- **Caching**: Configuration is cached in the Parser struct
- **Impact**: Reduces file I/O and parsing overhead

### 2. Argument Partitioning
- **Complexity**: O(n) where n is number of arguments
- **Optimization**: Single pass through arguments with task name lookup
- **Impact**: Efficient even for complex command lines

### 3. Task Dependency Resolution
- **Algorithm**: Linear-time dependency computation
- **Optimization**: Uses HashSet for efficient lookups
- **Impact**: Scales well with number of tasks

## Migration Path and Extensibility

### From Previous Implementations
- **Backward Compatibility**: All existing command line syntax supported
- **Migration**: Gradual migration from simpler parsing approaches
- **Testing**: Comprehensive test suite ensures compatibility

### Future Extensibility
- **New Argument Types**: Easy to add new parameter types in configuration
- **New Global Options**: Simple to add new global options to clap command
- **New Meta-Tasks**: Built-in task injection system is extensible

## Conclusion

The clap-based two-pass parsing approach successfully solved Otto's unique parsing requirements:

1. **Dynamic Task Recognition**: Tasks are loaded from configuration and recognized dynamically
2. **Multiple Task Support**: Multiple tasks can be invoked in a single command line
3. **Robust Validation**: Comprehensive validation with excellent error messages using clap
4. **Maintainable Code**: Clear separation of concerns with well-tested components
5. **Excellent Help System**: Integrated help generation for all commands and tasks

This design provided a solid foundation for Otto's CLI parsing needs and demonstrated that complex parsing requirements could be met with creative use of clap's features.

## Implementation Files

- `src/cli/parse.rs`: Main parsing implementation (2024 lines)
- `src/cli/error.rs`: Error handling and reporting
- `src/cli/mod.rs`: Module organization
- `src/main.rs`: Integration with main application
- `src/cfg/`: Configuration parsing and data structures

## Key Functions and Their Purposes

### Core Parsing Functions
- `Parser::parse()`: Main two-pass parsing orchestration
- `Parser::process_tasks_with_filter()`: Task processing and DAG construction
- `indices()` / `partitions()`: Argument partitioning algorithm

### Configuration Functions
- `Parser::find_ottofile()`: Recursive ottofile discovery
- `Parser::divine_ottofile()`: Ottofile path resolution
- `Parser::load_config_from_path()`: Configuration loading and parsing

### Clap Integration Functions
- `Parser::otto_command()`: Global options clap command generation
- `Parser::task_to_command()`: Task-specific clap command generation
- `Parser::param_to_arg()`: Parameter to clap argument conversion

### Help System Functions
- `Parser::help_command()`: Global help command generation
- Help detection and routing in main parse loop

### Utility Functions
- `Parser::compute_task_deps()`: Task dependency computation
- `Parser::collect_transitive_deps()`: Transitive dependency resolution
- `Parser::inject_graph_meta_task()`: Built-in task injection

This implementation represents a sophisticated solution to the challenge of parsing complex command lines with dynamic task definitions, providing a robust foundation for Otto's CLI interface.
