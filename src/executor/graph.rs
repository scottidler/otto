use eyre::{Result, eyre};
use log::debug;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::Path;
use std::process::Command;

use super::task::{DAG, Task};
use crate::cfg::task::{ForeachSpec, TaskSpecs};
use crate::cli::Parser;

/// Graph visualization options
#[derive(Debug, Clone)]
pub struct GraphOptions {
    /// Include task details in nodes
    pub show_details: bool,
    /// Show file dependencies
    pub show_file_deps: bool,
    /// Output format preference
    pub format: GraphFormat,
    /// Node styling
    pub style: NodeStyle,
    /// Output file path (optional)
    pub output_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
pub enum GraphFormat {
    /// Generate SVG image (requires graphviz)
    Svg,
    /// Generate PNG image (requires graphviz)
    Png,
    /// Generate PDF (requires graphviz)
    Pdf,
    /// Raw DOT format
    Dot,
    /// ASCII art for terminal
    Ascii,
    /// Auto-detect based on file extension
    Auto,
}

#[derive(Debug, Clone)]
pub enum NodeStyle {
    Simple,
    Detailed,
    Compact,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            show_details: true,
            show_file_deps: true,
            format: GraphFormat::Svg,
            style: NodeStyle::Detailed,
            output_path: None,
        }
    }
}

/// Info about a collapsed task for graph rendering
struct CollapsedTaskInfo {
    /// Display name (e.g., "examples:* [8 items]" for foreach, or just "build" for regular)
    display_name: String,
    /// Dependencies (task names this depends on)
    deps: Vec<String>,
    /// File inputs count
    file_deps_count: usize,
    /// File outputs count
    output_deps_count: usize,
}

/// DAG visualizer for Otto tasks
pub struct DagVisualizer {
    options: GraphOptions,
}

impl DagVisualizer {
    pub fn new(options: GraphOptions) -> Self {
        Self { options }
    }

    pub fn with_defaults() -> Self {
        Self::new(GraphOptions::default())
    }

    pub async fn execute_command(task: &crate::cli::parser::Task) -> Result<()> {
        // Parse graph command arguments
        let format = task
            .values
            .get("format")
            .and_then(|v| match v {
                crate::cfg::config::Value::Item(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("ascii");

        let output_path = task.values.get("output").and_then(|v| match v {
            crate::cfg::config::Value::Item(s) => Some(std::path::PathBuf::from(s)),
            _ => None,
        });

        let graph_format = match format {
            "ascii" => GraphFormat::Ascii,
            "dot" => GraphFormat::Dot,
            "svg" => GraphFormat::Svg,
            "png" => GraphFormat::Png,
            "pdf" => GraphFormat::Pdf,
            _ => GraphFormat::Ascii,
        };

        let options = GraphOptions {
            show_details: true,
            show_file_deps: true,
            format: graph_format,
            style: NodeStyle::Detailed,
            output_path,
        };

        // We need to reload the parser to get all tasks for the graph
        let args: Vec<String> = env::args().collect();
        let mut parser = Parser::new(args)?;
        let (all_tasks, _, _) = parser.parse_all_tasks()?;

        // Get original task specs for foreach metadata lookup
        let original_specs = parser.original_task_specs();

        let dag = Self::from_tasks(all_tasks)?;

        let visualizer = DagVisualizer::new(options);
        let result = visualizer.visualize(&dag, original_specs)?;

        println!("{result}");

        Ok(())
    }

    pub fn from_tasks(tasks: Vec<crate::cli::parser::Task>) -> Result<DAG<Task>> {
        // Convert parser tasks to executor tasks
        let executor_tasks: Vec<Task> = tasks
            .into_iter()
            .filter(|task| task.name != "graph") // Exclude graph task itself
            .map(Task::from)
            .collect();

        Self::create_dag_from_tasks(executor_tasks)
    }

    /// Reject a task set whose `after:`/`before:` edges form a cycle.
    ///
    /// Reuses the same daggy construction `otto Graph` uses, so the two can
    /// never disagree about what is a DAG. The scheduler calls this at init:
    /// without it a cycle is not detected on the execution path at all, and
    /// `a -> b -> a` printed two skip lines and exited 0.
    pub fn validate_acyclic(tasks: &[Task]) -> Result<()> {
        if Self::create_dag_from_tasks(tasks.to_vec()).is_ok() {
            return Ok(());
        }
        // daggy decides *whether* there is a cycle; this walk supplies the path,
        // because "WouldCycle" alone does not tell an operator what to edit.
        let path = Self::find_cycle_path(tasks).unwrap_or_else(|| "<could not resolve the cycle>".to_string());
        Err(eyre!(
            "dependency cycle detected: {path}. otto cannot order these tasks; break the cycle in the ottofile"
        ))
    }

    /// The first dependency cycle in `tasks`, rendered as `a -> b -> a`.
    fn find_cycle_path(tasks: &[Task]) -> Option<String> {
        let deps: HashMap<&str, Vec<&str>> = tasks
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.task_deps.iter().map(|d| d.task.as_str()).collect::<Vec<_>>(),
                )
            })
            .collect();

        let mut settled: HashSet<&str> = HashSet::new();
        for task in tasks {
            let mut on_path: Vec<&str> = Vec::new();
            if let Some(cycle) = Self::walk_for_cycle(task.name.as_str(), &deps, &mut settled, &mut on_path) {
                return Some(cycle);
            }
        }
        None
    }

    /// Depth-first walk marking the current path; a revisit of a node already on
    /// the path is the cycle, and the path from that node onward is its body.
    fn walk_for_cycle<'a>(
        node: &'a str,
        deps: &HashMap<&'a str, Vec<&'a str>>,
        settled: &mut HashSet<&'a str>,
        on_path: &mut Vec<&'a str>,
    ) -> Option<String> {
        if let Some(start) = on_path.iter().position(|n| *n == node) {
            let mut cycle: Vec<&str> = on_path[start..].to_vec();
            cycle.push(node);
            return Some(cycle.join(" -> "));
        }
        if settled.contains(node) {
            return None;
        }
        on_path.push(node);
        if let Some(children) = deps.get(node) {
            for child in children {
                if let Some(cycle) = Self::walk_for_cycle(child, deps, settled, on_path) {
                    return Some(cycle);
                }
            }
        }
        on_path.pop();
        settled.insert(node);
        None
    }

    fn create_dag_from_tasks(mut tasks: Vec<Task>) -> Result<DAG<Task>> {
        use daggy::Dag;

        let mut dag: DAG<Task> = Dag::new();
        let mut task_indices = HashMap::new();

        // Sort tasks alphabetically for consistent ordering
        tasks.sort_by(|a, b| a.name.cmp(&b.name));

        for task in tasks {
            let index = dag.add_node(task.clone());
            task_indices.insert(task.name.clone(), index);
        }

        let mut edges_to_add = Vec::new();
        for (node_index, node_data) in dag.raw_nodes().iter().enumerate() {
            let task = &node_data.weight;
            let current_index = daggy::NodeIndex::new(node_index);

            for dep in &task.task_deps {
                match task_indices.get(&dep.task) {
                    Some(&dep_index) => edges_to_add.push((dep_index, current_index, task.name.clone())),
                    // Legitimate: `collect_transitive_deps` can hand the scheduler a
                    // task whose dep is outside the run set. Dropping the edge is
                    // right; dropping it without a trace is what made a missing
                    // dependency indistinguishable from a satisfied one.
                    None => debug!(
                        "Task {}: dependency {} is not in the task set; edge omitted from the DAG",
                        task.name, dep.task
                    ),
                }
            }
        }

        for (dep_index, current_index, task_name) in edges_to_add {
            dag.add_edge(dep_index, current_index, ())
                .map_err(|e| eyre!("Failed to add edge to {}: {:?}", task_name, e))?;
        }

        Ok(dag)
    }

    /// Visualize the DAG and save to file or display
    pub fn visualize(&self, dag: &DAG<Task>, original_specs: &TaskSpecs) -> Result<String> {
        match self.options.format {
            GraphFormat::Ascii => self.generate_ascii(dag, original_specs),
            GraphFormat::Dot => self.generate_dot(dag),
            GraphFormat::Svg | GraphFormat::Png | GraphFormat::Pdf | GraphFormat::Auto => {
                self.generate_image(dag, original_specs)
            }
        }
    }

    pub fn generate_image(&self, dag: &DAG<Task>, original_specs: &TaskSpecs) -> Result<String> {
        let dot_content = self.generate_dot(dag)?;

        // Determine output format and path
        let (format, output_path) = self.determine_output_format()?;

        if !self.is_graphviz_available() {
            return Err(eyre!(
                "Graphviz not found. Please install graphviz to generate images.\n\
                On Ubuntu/Debian: sudo apt install graphviz\n\
                On macOS: brew install graphviz\n\
                On Windows: Download from https://graphviz.org/download/\n\
                \n\
                Falling back to ASCII output:\n{}",
                self.generate_ascii(dag, original_specs)?
            ));
        }

        let temp_dir = tempfile::tempdir()?;
        let dot_file = temp_dir.path().join("otto_graph.dot");
        std::fs::write(&dot_file, &dot_content)?;

        // Run graphviz to generate image
        let output = Command::new("dot")
            .arg(format!("-T{format}"))
            .arg(&dot_file)
            .arg("-o")
            .arg(&output_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Graphviz failed: {}", stderr));
        }

        Ok(format!(
            "Graph visualization saved to: {}\n\
            Format: {}\n\
            Open with your preferred image viewer or browser.",
            output_path.display(),
            format.to_uppercase()
        ))
    }

    /// Consecutive (predecessor, successor) pairs of every serial foreach group in the
    /// graph, derived from each task's group name and order index.
    fn serial_order_pairs(dag: &DAG<Task>) -> Vec<(String, String)> {
        let mut groups: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for node in dag.raw_nodes() {
            let task = &node.weight;
            if let Some(group) = &task.serial_group {
                groups
                    .entry(group.clone())
                    .or_default()
                    .push((task.serial_index, task.name.clone()));
            }
        }

        let mut pairs = Vec::new();
        let mut group_names: Vec<String> = groups.keys().cloned().collect();
        group_names.sort();
        for group in group_names {
            let members = groups.get_mut(&group).expect("group name came from the map");
            members.sort();
            for window in members.windows(2) {
                pairs.push((window[0].1.clone(), window[1].1.clone()));
            }
        }
        pairs
    }

    pub fn generate_dot(&self, dag: &DAG<Task>) -> Result<String> {
        let mut dot = String::new();

        // Start digraph
        dot.push_str("digraph otto_dag {\n");
        dot.push_str("  label=\"Otto Task DAG\";\n");
        dot.push_str("  labelloc=\"t\";\n");
        dot.push_str("  fontsize=\"16\";\n");
        dot.push_str("  fontname=\"Helvetica\";\n");
        dot.push_str("  rankdir=\"TB\";\n");
        dot.push_str("  bgcolor=\"white\";\n");
        dot.push_str("  \n");

        // Default node attributes
        dot.push_str("  node [\n");
        dot.push_str("    shape=\"box\",\n");
        dot.push_str("    style=\"rounded,filled\",\n");
        dot.push_str("    fontname=\"Helvetica\",\n");
        dot.push_str("    fontsize=\"12\"\n");
        dot.push_str("  ];\n");
        dot.push_str("  \n");

        // Default edge attributes
        dot.push_str("  edge [\n");
        dot.push_str("    fontname=\"Helvetica\",\n");
        dot.push_str("    fontsize=\"10\"\n");
        dot.push_str("  ];\n");
        dot.push_str("  \n");

        let mut task_to_id = HashMap::new();
        let mut file_nodes = std::collections::HashSet::new();

        for (idx, node) in dag.raw_nodes().iter().enumerate() {
            let task = &node.weight;
            let node_id = format!("task_{idx}");
            task_to_id.insert(task.name.clone(), node_id.clone());

            let label = self.create_node_label(task);
            let escaped_label = self.escape_dot_string(&label);

            // Determine node color based on task characteristics
            let color = if !task.file_deps.is_empty() || !task.output_deps.is_empty() {
                "lightblue"
            } else {
                "lightgray"
            };

            dot.push_str(&format!(
                "  {node_id} [label=\"{escaped_label}\", fillcolor=\"{color}\"];\n"
            ));
        }

        dot.push_str("  \n");

        for node in dag.raw_nodes() {
            let task = &node.weight;
            if let Some(target_id) = task_to_id.get(&task.name) {
                for dep in &task.task_deps {
                    if let Some(source_id) = task_to_id.get(&dep.task) {
                        let (label, color, style) = match dep.when {
                            crate::cfg::edge::When::Success => ("depends", "black", "solid"),
                            crate::cfg::edge::When::Failure => ("on-failure", "red", "dashed"),
                            crate::cfg::edge::When::Always => ("always", "darkgreen", "solid"),
                        };
                        dot.push_str(&format!(
                            "  {source_id} -> {target_id} [label=\"{label}\", color=\"{color}\", style=\"{style}\"];\n"
                        ));
                    }
                }
            }
        }

        // Serial foreach ordering is a task property, not a dependency edge, so it has
        // no entry in task_deps. Render it as a dashed `order` edge between consecutive
        // group members so the ordering stays visible without claiming to be a `depends`.
        for (source, target) in Self::serial_order_pairs(dag) {
            if let (Some(source_id), Some(target_id)) = (task_to_id.get(&source), task_to_id.get(&target)) {
                dot.push_str(&format!(
                    "  {source_id} -> {target_id} [label=\"order\", color=\"gray40\", style=\"dashed\"];\n"
                ));
            }
        }

        if self.options.show_file_deps {
            self.add_file_dependencies_to_dot(&mut dot, dag, &task_to_id, &mut file_nodes)?;
        }

        dot.push_str("}\n");

        Ok(dot)
    }

    pub fn generate_ascii(&self, dag: &DAG<Task>, original_specs: &TaskSpecs) -> Result<String> {
        let mut output = String::new();

        output.push_str("┌─────────────────────────────────────┐\n");
        output.push_str("│           Otto Task DAG             │\n");
        output.push_str("└─────────────────────────────────────┘\n\n");

        // Collapse foreach subtasks: group tasks like "examples:foo", "examples:bar" into "examples:{...}"
        let collapsed_tasks = Self::collapse_foreach_subtasks(dag, original_specs);

        // Find leaf tasks (tasks that nothing depends on - the top-level tasks you'd run)
        let mut leaf_tasks: Vec<_> = collapsed_tasks
            .iter()
            .filter(|(name, _)| {
                // A task is a leaf if no other task depends on it
                !collapsed_tasks.values().any(|info| info.deps.contains(*name))
            })
            .collect();

        // Sort leaf tasks alphabetically for consistent output
        leaf_tasks.sort_by(|a, b| a.0.cmp(b.0));

        if leaf_tasks.is_empty() {
            output.push_str("No leaf tasks found (possible circular dependencies)\n");
            return Ok(output);
        }

        for (i, (name, info)) in leaf_tasks.iter().enumerate() {
            let is_last_leaf = i == leaf_tasks.len() - 1;
            Self::render_collapsed_ascii_subtree(
                &mut output,
                name,
                info,
                &collapsed_tasks,
                0,
                &mut std::collections::HashSet::new(),
                is_last_leaf,
            )?;
        }

        output.push_str("\n┌─────────────────────────────────────┐\n");
        output.push_str("│ Legend:                             │\n");
        output.push_str("│ ├─ Task name [inputs:N] [outputs:M] │\n");
        output.push_str("│ └─ Dependencies flow top to bottom  │\n");
        output.push_str("└─────────────────────────────────────┘\n");

        Ok(output)
    }

    /// Collapse foreach subtasks into a single display entry
    ///
    /// Uses original_specs to determine display format based on ForeachSpec:
    /// - For items: show `{item1,item2,...}` for small lists, `{...} [N items]` for large
    /// - For glob: show `pattern [N items]`
    /// - For range: show `start..end`
    fn collapse_foreach_subtasks(dag: &DAG<Task>, original_specs: &TaskSpecs) -> HashMap<String, CollapsedTaskInfo> {
        let mut result: HashMap<String, CollapsedTaskInfo> = HashMap::new();
        let mut foreach_groups: HashMap<String, Vec<&Task>> = HashMap::new();

        // First pass: identify foreach subtasks and group them
        for node in dag.raw_nodes() {
            let task = &node.weight;
            if let Some((parent, _subtask)) = task.name.split_once(':') {
                // This is a foreach subtask
                foreach_groups.entry(parent.to_string()).or_default().push(task);
            }
        }

        // Second pass: process all tasks
        for node in dag.raw_nodes() {
            let task = &node.weight;

            if let Some((parent, _)) = task.name.split_once(':') {
                // This is a foreach subtask - only add the parent once
                if !result.contains_key(parent) {
                    let subtasks = foreach_groups.get(parent).unwrap();
                    let count = subtasks.len();

                    // Try to get display name from original ForeachSpec, fall back to inference
                    let display_name = if let Some(parent_spec) = original_specs.get(parent) {
                        if let Some(foreach_spec) = parent_spec.foreach.as_ref() {
                            Self::format_foreach_display(parent, foreach_spec, count)
                        } else {
                            // No foreach spec - use inference fallback
                            let pattern = Self::infer_subtask_pattern(subtasks);
                            format!("{}:{} [{} items]", parent, pattern, count)
                        }
                    } else {
                        // Parent not found in original specs - use inference fallback
                        let pattern = Self::infer_subtask_pattern(subtasks);
                        format!("{}:{} [{} items]", parent, pattern, count)
                    };

                    // Get deps from first subtask (they should all be the same, minus internal deps)
                    let deps: Vec<String> = subtasks
                        .first()
                        .map(|t| {
                            t.task_deps
                                .iter()
                                .filter(|d| !d.task.starts_with(&format!("{}:", parent)))
                                .map(|d| d.task.clone())
                                .collect()
                        })
                        .unwrap_or_default();

                    result.insert(
                        parent.to_string(),
                        CollapsedTaskInfo {
                            display_name,
                            deps,
                            file_deps_count: subtasks.first().map(|t| t.file_deps.len()).unwrap_or(0),
                            output_deps_count: subtasks.first().map(|t| t.output_deps.len()).unwrap_or(0),
                        },
                    );
                }
            } else if !foreach_groups.contains_key(&task.name) {
                // Regular task (not a foreach parent that has subtasks)
                result.insert(
                    task.name.clone(),
                    CollapsedTaskInfo {
                        display_name: task.name.clone(),
                        deps: task.task_deps.iter().map(|d| d.task.clone()).collect(),
                        file_deps_count: task.file_deps.len(),
                        output_deps_count: task.output_deps.len(),
                    },
                );
            }
        }

        result
    }

    /// Format the display name for a foreach task based on its ForeachSpec
    ///
    /// - For items: `parent:{item1,item2,...}` for ≤6 items, `parent:{...} [N items]` for more
    /// - For glob: `parent:pattern [N items]`
    /// - For range: `parent:start..end`
    fn format_foreach_display(parent: &str, foreach: &ForeachSpec, count: usize) -> String {
        if let Some(ref glob) = foreach.glob {
            // Glob notation: examples:*.sh [8 items]
            format!("{}:{} [{} items]", parent, glob, count)
        } else if !foreach.items.is_empty() {
            // Check for special characters that would break brace notation
            let has_special_chars = foreach
                .items
                .iter()
                .any(|item| item.contains(',') || item.contains('{') || item.contains('}'));

            if has_special_chars || foreach.items.len() > 6 {
                // Too many items or special chars: install:{...} [15 items]
                format!("{}:{{...}} [{} items]", parent, foreach.items.len())
            } else {
                // Brace notation for small lists: install:{td,ts,cs}
                format!("{}:{{{}}}", parent, foreach.items.join(","))
            }
        } else if let Some(ref range) = foreach.range {
            // Range notation: batch:1..10
            format!("{}:{}", parent, range)
        } else {
            // Fallback
            format!("{}:* [{} items]", parent, count)
        }
    }

    /// Infer the glob pattern from subtask names
    ///
    /// If all subtask identifiers share a common extension (e.g., .rs, .sh), returns `*.<ext>`
    /// Otherwise returns `*`
    fn infer_subtask_pattern(subtasks: &[&Task]) -> String {
        if subtasks.is_empty() {
            return "*".to_string();
        }

        // Extract the identifier part (after the colon) from each subtask name
        let identifiers: Vec<&str> = subtasks
            .iter()
            .filter_map(|t| t.name.split_once(':').map(|(_, id)| id))
            .collect();

        if identifiers.is_empty() {
            return "*".to_string();
        }

        // Check for common file extension
        let first_ext = identifiers[0].rsplit_once('.').map(|(_, ext)| ext);

        if let Some(ext) = first_ext {
            // Verify all identifiers share this extension
            let all_match = identifiers
                .iter()
                .all(|id| id.rsplit_once('.').map(|(_, e)| e == ext).unwrap_or(false));

            if all_match {
                return format!("*.{}", ext);
            }
        }

        "*".to_string()
    }

    fn render_collapsed_ascii_subtree(
        output: &mut String,
        task_name: &str,
        info: &CollapsedTaskInfo,
        all_tasks: &HashMap<String, CollapsedTaskInfo>,
        depth: usize,
        visited: &mut std::collections::HashSet<String>,
        is_last: bool,
    ) -> Result<()> {
        let indent = "  ".repeat(depth);
        let connector = if is_last { "└─" } else { "├─" };

        if visited.contains(task_name) {
            output.push_str(&format!(
                "{}{} {} (circular ref)\n",
                indent, connector, info.display_name
            ));
            return Ok(());
        }

        visited.insert(task_name.to_string());

        // Show task info
        output.push_str(&format!("{}{} {}", indent, connector, info.display_name));
        if info.file_deps_count > 0 {
            output.push_str(&format!(" [inputs:{}]", info.file_deps_count));
        }
        if info.output_deps_count > 0 {
            output.push_str(&format!(" [outputs:{}]", info.output_deps_count));
        }
        output.push('\n');

        // Find tasks that this task depends on
        let dependencies: Vec<_> = info
            .deps
            .iter()
            .filter_map(|dep_name| {
                // Handle collapsed names - deps might reference subtasks but we show parents
                let lookup_name = if let Some((parent, _)) = dep_name.split_once(':') {
                    parent.to_string()
                } else {
                    dep_name.clone()
                };
                all_tasks.get(&lookup_name).map(|info| (lookup_name, info))
            })
            .collect();

        for (i, (dep_name, dep_info)) in dependencies.iter().enumerate() {
            let is_last_dependency = i == dependencies.len() - 1;
            Self::render_collapsed_ascii_subtree(
                output,
                dep_name,
                dep_info,
                all_tasks,
                depth + 1,
                visited,
                is_last_dependency,
            )?;
        }

        visited.remove(task_name);
        Ok(())
    }

    fn create_node_label(&self, task: &Task) -> String {
        match self.options.style {
            NodeStyle::Simple => task.name.clone(),
            NodeStyle::Compact => {
                format!("{}\n[{}]", task.name, &task.hash[..6])
            }
            NodeStyle::Detailed => {
                let mut label = task.name.clone();

                if self.options.show_details {
                    if !task.file_deps.is_empty() {
                        label.push_str(&format!("\nInputs: {}", task.file_deps.len()));
                    }
                    if !task.output_deps.is_empty() {
                        label.push_str(&format!("\nOutputs: {}", task.output_deps.len()));
                    }
                    if !task.envs.is_empty() {
                        label.push_str(&format!("\nEnvs: {}", task.envs.len()));
                    }
                }

                label.push_str(&format!("\n[{}]", &task.hash[..6]));
                label
            }
        }
    }

    /// Escape strings for DOT format
    fn escape_dot_string(&self, s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
            .replace('$', "\\$") // Escape dollar signs to prevent variable expansion
            .replace('{', "\\{") // Escape braces
            .replace('}', "\\}")
    }

    fn add_file_dependencies_to_dot(
        &self,
        dot: &mut String,
        dag: &DAG<Task>,
        task_to_id: &HashMap<String, String>,
        file_nodes: &mut std::collections::HashSet<String>,
    ) -> Result<()> {
        dot.push_str("  \n  // File dependencies\n");

        for node in dag.raw_nodes() {
            let task = &node.weight;
            if let Some(task_id) = task_to_id.get(&task.name) {
                for file_dep in &task.file_deps {
                    let file_id = format!("file_{}", file_dep.replace(['/', '.', '*', '-', '$', '{', '}'], "_"));

                    if !file_nodes.contains(&file_id) {
                        let escaped_label = self.escape_dot_string(file_dep);
                        dot.push_str(&format!(
                            "  {file_id} [label=\"{escaped_label}\", shape=\"ellipse\", fillcolor=\"lightgreen\"];\n"
                        ));
                        file_nodes.insert(file_id.clone());
                    }

                    dot.push_str(&format!(
                        "  {file_id} -> {task_id} [label=\"input\", color=\"green\", style=\"dashed\"];\n"
                    ));
                }

                for output_dep in &task.output_deps {
                    let file_id = format!(
                        "output_{}",
                        output_dep.replace(['/', '.', '*', '-', '$', '{', '}'], "_")
                    );

                    if !file_nodes.contains(&file_id) {
                        let escaped_label = self.escape_dot_string(output_dep);
                        dot.push_str(&format!(
                            "  {file_id} [label=\"{escaped_label}\", shape=\"ellipse\", fillcolor=\"lightyellow\"];\n"
                        ));
                        file_nodes.insert(file_id.clone());
                    }

                    dot.push_str(&format!(
                        "  {task_id} -> {file_id} [label=\"output\", color=\"orange\", style=\"dashed\"];\n"
                    ));
                }
            }
        }

        Ok(())
    }

    fn is_graphviz_available(&self) -> bool {
        Command::new("dot")
            .arg("-V")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn determine_output_format(&self) -> Result<(String, std::path::PathBuf)> {
        let (format, extension) = match &self.options.format {
            GraphFormat::Svg => ("svg", "svg"),
            GraphFormat::Png => ("png", "png"),
            GraphFormat::Pdf => ("pdf", "pdf"),
            GraphFormat::Auto => {
                if let Some(ref path) = self.options.output_path {
                    match path.extension().and_then(|s| s.to_str()) {
                        Some("svg") => ("svg", "svg"),
                        Some("png") => ("png", "png"),
                        Some("pdf") => ("pdf", "pdf"),
                        _ => ("svg", "svg"), // default
                    }
                } else {
                    ("svg", "svg") // default
                }
            }
            _ => return Err(eyre!("Invalid format for image generation")),
        };

        let output_path = if let Some(ref path) = self.options.output_path {
            path.clone()
        } else {
            std::env::current_dir()?.join(format!("otto_graph.{extension}"))
        };

        Ok((format.to_string(), output_path))
    }

    pub fn write_dot_file(&self, dag: &DAG<Task>, path: &Path) -> Result<()> {
        let dot_content = self.generate_dot(dag)?;
        std::fs::write(path, dot_content)?;
        Ok(())
    }

    pub fn write_ascii_file(&self, dag: &DAG<Task>, original_specs: &TaskSpecs, path: &Path) -> Result<()> {
        let ascii_content = self.generate_ascii(dag, original_specs)?;
        std::fs::write(path, ascii_content)?;
        Ok(())
    }
}

#[path = "graph_tests.rs"]
mod tests;
