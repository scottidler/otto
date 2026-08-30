# Sub-task Generation Design for Otto

## Overview

This document outlines the design for implementing PyDoit-style sub-task generation in Otto, allowing a single task definition to dynamically create multiple related tasks at runtime.

## Background: PyDoit's Approach

PyDoit achieves elegant sub-task generation through Python's dynamic typing and generators:

```python
def task_imports():
    """find imports from a python module"""
    for name, module in PKG_MODULES.by_name.items():
        yield {
            'name': name,
            'file_dep': [module.path],
            'actions': [(get_imports, (PKG_MODULES, module.path))],
        }

def task_dot():
    """generate a graphviz's dot graph from module imports"""
    return {
        'targets': ['requests.dot'],
        'actions': [module_to_dot],
        'getargs': {'imports': ('imports', 'modules')},
        'clean': True,
    }
```

### Key PyDoit Features

1. **Single Function Syntax**: Same function definition creates 1 or N tasks
2. **Runtime Generation**: Tasks created by executing Python code
3. **Dynamic Data**: Can iterate over computed values, API responses, file system scans
4. **Automatic Parent Tasks**: Creates group tasks automatically
5. **Sub-task Naming**: Uses `basename:subtask_name` convention
6. **Elegant Detection**: `generate_tasks()` detects generators vs single dicts

### PyDoit's Implementation Details

- Task creators return either `dict` (single task) or `generator` (multiple tasks)
- The `generate_tasks()` function uses `inspect.isgenerator()` to detect type
- Sub-tasks automatically get namespaced names (`parent:child`)
- Parent tasks are created automatically with `has_subtask=True`
- Empty generators create empty group tasks gracefully

## The Challenge: YAML vs Python

Otto faces a fundamental constraint that PyDoit doesn't:

- **PyDoit**: Configuration is executable Python code
- **Otto**: Configuration is declarative YAML

This means Otto cannot directly replicate PyDoit's approach of executing arbitrary code to generate tasks. We need architectural patterns that work within YAML's limitations while capturing the spirit of PyDoit's elegance.

## Architectural Approaches Considered

### Option 1: Enum Return Type
```rust
enum TaskResult {
    Single(TaskSpec),
    Multiple(impl Iterator<Item = TaskSpec>),
}
```
**Rejected**: Rust's type system makes this awkward with trait objects and generics.

### Option 2: Always Return Iterator (Recommended)
```rust
fn task_creator() -> impl Iterator<Item = TaskSpec>
```
- Single tasks return `std::iter::once(task)`
- Multi-tasks return actual iterators
- Caller always expects iterators
- Maintains conceptual elegance

### Option 3: Macro-Based DSL
```rust
task_creator! {
    fn create_files() {
        for i in 0..3 {
            yield task! { name: format!("file{}", i), ... };
        }
    }
}
```
**Rejected**: Too complex, moves away from YAML configuration.

## YAML Configuration Design

### Core Challenge
YAML is static declarative data, but we need dynamic task generation. The solution is to make task definitions **contextually aware** rather than structurally different.

### Option 1: Direct Field Templating (Recommended)
```yaml
data:
  modules:
    - name: "requests"
      path: "src/requests.py"
    - name: "urllib"
      path: "src/urllib.py"

tasks:
  imports:
    for_each: "modules"           # Only special key needed
    name: "${item.name}"          # Everything else is normal task fields
    file_dep: ["${item.path}"]
    action: "get_imports ${item.path}"
```

**Benefits**:
- Clean, flat structure
- Looks like normal task definition
- Only `for_each` indicates iteration
- Template variables are intuitive

### Option 2: Implicit Iteration from Data References
```yaml
data:
  modules: !include modules.json

tasks:
  imports:
    name: "${modules[].name}"      # [] signals iteration
    file_dep: ["${modules[].path}"]
    action: "get_imports ${modules[].path}"
```

**Benefits**:
- No special keys needed
- Iteration implied by `[]` syntax
- Very concise

**Drawbacks**:
- Less explicit about iteration intent
- Harder to parse and validate

### Option 3: Array Expansion
```yaml
tasks:
  imports:
    _expand: modules              # Special key for expansion source
    name: "${name}"
    file_dep: ["${path}"]
    action: "get_imports ${path}"
```

**Benefits**:
- Item fields directly available
- No nested object references

**Drawbacks**:
- Less clear about data structure
- Potential naming conflicts

### Option 4: Nested Template Structure (Rejected)
```yaml
tasks:
  imports:
    generator:
      type: data_iteration
      source: "modules"
      template:
        name: "${item.name}"
        file_dep: ["${item.path}"]
        action: "get_imports ${item.path}"
```

**Rejected**: Too verbose, creates unnecessary nesting, doesn't maintain YAML elegance.

## Recommended Implementation Architecture

### Core Components

1. **Task Creator Functions**: Return `impl Iterator<Item = TaskSpec>`
2. **Task Registry**: Maps names to creator functions
3. **Loading Pipeline**: Calls creators, flattens iterators, handles naming
4. **Template Engine**: Handles variable substitution in YAML
5. **Data Sources**: External data loading (JSON, YAML, command output)

### YAML Processing Pipeline

1. **Parse YAML**: Load configuration into structs
2. **Detect Iteration**: Look for `for_each` keys or array syntax
3. **Load Data**: Fetch data from specified sources
4. **Template Expansion**: Create one task spec per data item
5. **Name Generation**: Apply `basename:subtask` naming convention
6. **Parent Task Creation**: Generate group tasks automatically

### Task Naming Convention

Following PyDoit's pattern:
- Parent task: `imports` (group task, no actions)
- Sub-tasks: `imports:requests`, `imports:urllib`, etc.
- Command line selection: `otto imports:requests` or `otto "imports:*"`

### Data Sources

Support multiple data source types:
```yaml
data:
  # Static inline data
  modules:
    - name: "requests"
      path: "src/requests.py"

  # External files
  configs: !include "configs.json"

  # Command output (future)
  files: !command "find src -name '*.rs'"

  # Directory scanning (future)
  test_files: !glob "tests/**/*_test.rs"
```

## Migration Strategy

### Phase 1: Basic Implementation
- Implement `for_each` with static data
- Template variable substitution
- Parent task creation
- Basic command line selection

### Phase 2: Enhanced Data Sources
- External file loading (`!include`)
- JSON and YAML data sources
- Error handling and validation

### Phase 3: Advanced Features
- Command output data sources (`!command`)
- File glob data sources (`!glob`)
- Conditional task generation
- Dependency propagation between sub-tasks

### Phase 4: Optimization
- Lazy evaluation where possible
- Parallel task generation
- Caching of expensive data sources

## Example Configurations

### File Processing
```yaml
data:
  source_files: !glob "src/**/*.rs"

tasks:
  check-file:
    for_each: "source_files"
    name: "${item.stem}"
    file_dep: ["${item.path}"]
    action: |
      echo "Checking ${item.path}"
      cargo check --file ${item.path}
```

### Multi-Environment Deployment
```yaml
data:
  environments:
    - name: "staging"
      url: "https://staging.example.com"
      config: "staging.json"
    - name: "prod"
      url: "https://prod.example.com"
      config: "prod.json"

tasks:
  deploy:
    for_each: "environments"
    name: "${item.name}"
    file_dep: ["${item.config}"]
    action: |
      echo "Deploying to ${item.name}"
      deploy --config ${item.config} --url ${item.url}
```

### Test Suite Generation
```yaml
data:
  test_configs: !include "test-matrix.json"

tasks:
  test-matrix:
    for_each: "test_configs"
    name: "${item.rust_version}-${item.features}"
    action: |
      rustup run ${item.rust_version} cargo test --features "${item.features}"
```

## Benefits of This Approach

1. **Maintains YAML Elegance**: Task definitions look like normal tasks
2. **PyDoit-like Conceptual Model**: Same definition creates 1 or N tasks
3. **Type Safety**: Leverages Rust's type system where possible
4. **Extensible**: Easy to add new data source types
5. **Familiar**: Uses established templating patterns
6. **Performant**: Lazy evaluation and efficient iteration

## Open Questions

1. **Error Handling**: How to handle template expansion errors gracefully?
2. **Dependency Resolution**: How do sub-task dependencies interact with parent dependencies?
3. **Command Line Interface**: What's the best syntax for sub-task selection?
4. **Validation**: How to validate template variables before expansion?
5. **Debugging**: How to help users debug template expansion issues?

## Conclusion

The recommended approach uses `for_each` with direct field templating to achieve PyDoit-style sub-task generation while maintaining YAML's declarative nature. This provides the closest equivalent to PyDoit's elegance within Otto's architectural constraints.

The key insight is that we can't replicate PyDoit's runtime code execution, but we can capture its conceptual elegance through contextual YAML interpretation and template expansion.
