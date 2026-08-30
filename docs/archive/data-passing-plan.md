# Otto Data Passing Implementation Plan

## Overview
We're implementing a comprehensive action processing system for Otto that handles script generation, language detection, dependency management, and data passing between tasks.

## Current State (✅ Completed)

### 1. Core ActionProcessor System
- **Location**: `src/executor/action.rs`
- **Status**: ✅ Implemented and working
- **Features**:
  - `ActionProcessor` struct that coordinates script processing
  - `ProcessedAction` enum with `Bash` and `Python3` variants including hash
  - Language detection based on shebang lines (`#!/usr/bin/env bash`, `#!/bin/bash`, `#!/usr/bin/env python3`, `#!/usr/bin/python3`)
  - Fallback to bash when no shebang detected
  - Hash calculation of complete generated script content (8-character hex)

### 2. Language Processors
- **Status**: ✅ Implemented and working
- **Bash Processor**:
  - Generates bash prologue with `declare -A OTTO_INPUT/OUTPUT`, parameter parsing with `getopts`
  - Environment variable exports
  - Input loading from dependencies with jq
  - Epilogue with safe JSON serialization using jq
- **Python Processor**:
  - Generates python prologue with imports, dictionaries, argument parsing
  - Environment setup
  - Input loading from dependencies
  - Epilogue with atomic JSON file writing

### 3. Integration with Scheduler
- **Location**: `src/executor/scheduler.rs`
- **Status**: ✅ Integrated and working
- **Features**:
  - ActionProcessor integrated into task execution flow
  - Dependency file linking via symlinks
  - Input/output directory creation and management

### 4. JSON Handling
- **Status**: ✅ Implemented correctly
- **Features**:
  - `hash jq >/dev/null` checks only when needed (dependencies exist or output has content)
  - Clear error messages when jq missing
  - No fallback mechanisms (fails fast if jq unavailable)
  - Safe JSON serialization using `jq -n --arg`

### 5. Testing
- **Status**: ✅ All 62 tests passing
- **Coverage**:
  - Unit tests for bash and python action processing
  - Hash verification tests
  - Language detection tests (bash, python, default fallback)
  - Integration tests with real execution

## Current Issues (🚨 Need Fixing)

### 1. Script Formatting Problems
**Issue**: Generated scripts have poor formatting
- ❌ Shebang embedded in middle of script instead of first line
- ❌ Excessive spacing between sections making ugly scripts
- ❌ Environment exports scattered throughout instead of grouped at top

**Example of current bad output**:
```bash
# Otto-generated bash prologue
set -euo pipefail

declare -A OTTO_INPUT
declare -A OTTO_OUTPUT


# Otto-generated parameter parsing (getopt)
GREETING="howdy"

while getopts "g:" opt; do
    # ... case statements
done


export OTTO_TASK_NAME="hello"
export OTTO_INPUT_DIR="/path/to/inputs"
export OTTO_OUTPUT_FILE="/path/to/outputs/hello.output.json"
export GREETING="howdy"
export OTTO_PARAM_GREETING="${GREETING}"




#!/bin/bash    # ❌ SHEBANG IN WRONG PLACE!
sleep 1
echo "${greeting:-hello}"
```

### 2. Directory Structure Problems
**Issue**: Creating unnecessary subdirectories
- ❌ Creating `inputs/` and `outputs/` subdirectories
- ❌ Output file in wrong location (`outputs/hello.output.json` instead of `hello.output.json`)
- ❌ Input directory created even when no dependencies exist

**Current structure (WRONG)**:
```
~/.otto/otto-553a8582/1749515023/tasks/hello/
├── inputs/          # ❌ Shouldn't exist if no dependencies
├── outputs/         # ❌ Shouldn't exist
│   └── hello.output.json  # ❌ Should be at task level
├── script.sh
├── stderr.log
└── stdout.log
```

### 3. Missing Caching Implementation
**Issue**: No script caching system
- ❌ Scripts written directly as `script.sh` instead of cached
- ❌ No symlink from `script.sh` to `.cache/<hash>`
- ❌ Missing deduplication of identical scripts

## Target Architecture (🎯 Goal)

### Correct Directory Structure
```
~/.otto/otto-<workspace-hash>/
├── .cache/
│   └── <script-content-hash>.sh    # Actual script content
└── <timestamp>/
    ├── run.yaml
    └── tasks/
        ├── hello/
        │   ├── script.sh -> ../../.cache/<script-content-hash>.sh
        │   ├── hello.output.json    # Output file at task level
        │   ├── stderr.log
        │   └── stdout.log
        └── world/
            ├── hello.input.json -> ../hello/hello.output.json  # Symlink to dependency
            ├── script.sh -> ../../.cache/<script-content-hash>.sh
            ├── world.output.json
            ├── stderr.log
            └── stdout.log
```

### Correct Script Format
```bash
#!/bin/bash
# Otto-generated bash prologue
set -euo pipefail

declare -A OTTO_INPUT
declare -A OTTO_OUTPUT

# Global exports
export OTTO_TASK_NAME="hello"
export OTTO_INPUT_DIR="/path/to/task"
export OTTO_OUTPUT_FILE="/path/to/task/hello.output.json"
export GREETING="howdy"
export OTTO_PARAM_GREETING="${GREETING}"

# Parameter parsing (if needed)
GREETING="howdy"
while getopts "g:" opt; do
    case $opt in
        g) GREETING="${OPTARG}" ;;
        \?) echo "Invalid option: -$OPTARG" >&2; exit 1 ;;
    esac
done

# Input loading (if dependencies exist)
# ... jq-based loading ...

# User action content (cleaned of shebang)
sleep 1
echo "${greeting:-hello}"

# Otto-generated bash epilogue
# ... output serialization ...
```

## Implementation Plan (📋 Next Steps)

### Phase 1: Fix Script Formatting (IN PROGRESS)
**Files**: `src/executor/action.rs`
**Status**: 🟡 Partially started

#### 1.1 Fix Shebang Placement ✅ DONE
- Extract shebang from user action
- Place shebang as first line of generated script
- Remove shebang from user content before embedding

#### 1.2 Clean Up Spacing 🟡 IN PROGRESS
- Remove excessive newlines between sections
- Create clean, readable script format
- Group related sections together

#### 1.3 Reorganize Exports ⏳ TODO
- Move all environment exports to top of prologue (after shebang and set flags)
- Group global declarations together
- Clean up parameter parsing formatting

### Phase 2: Fix Directory Structure
**Files**: `src/executor/workspace.rs`, `src/executor/scheduler.rs`
**Status**: ⏳ TODO

#### 2.1 Update Workspace Methods
- Remove `task_input_dir()` and `task_output_dir()` methods
- Update `task_output_file()` to return `<task-dir>/<task-name>.output.json`
- Add `task_input_file()` method for dependency symlinks
- Add `cache_dir()` and `cached_script_path()` methods

#### 2.2 Update Scheduler Integration
- Remove creation of `inputs/` and `outputs/` subdirectories
- Create dependency symlinks directly in task directory
- Only create symlinks when dependencies actually exist
- Fix output file path to be at task level

### Phase 3: Implement Script Caching
**Files**: `src/executor/action.rs`, `src/executor/workspace.rs`, `src/executor/scheduler.rs`
**Status**: ⏳ TODO

#### 3.1 Cache Infrastructure
- Create `.cache/` directory in workspace
- Write generated scripts to `.cache/<hash>.sh` or `.cache/<hash>.py`
- Make cached files executable

#### 3.2 Symlink Management
- Create symlink from `tasks/<task>/script.{sh,py}` to `../../.cache/<hash>.{sh,py}`
- Handle both bash (.sh) and python (.py) extensions
- Ensure symlinks are created correctly

### Phase 4: Update Tests
**Files**: `src/executor/action.rs`, `tests/` directory
**Status**: ⏳ TODO

#### 4.1 Update Unit Tests
- Update test expectations for new script format
- Update directory structure assertions
- Add caching tests

#### 4.2 Integration Tests
- Verify end-to-end functionality with new structure
- Test dependency symlink creation
- Test script caching and reuse

### Phase 5: Final Verification
**Status**: ⏳ TODO

#### 5.1 Manual Testing
- Run real examples (`examples/ex1`, `examples/lang-test.yml`)
- Verify directory structure matches target
- Verify script formatting is clean
- Verify caching works correctly

#### 5.2 Performance Testing
- Verify script reuse works (same script = same cache file)
- Verify symlink creation is fast
- Verify no unnecessary work is done

## Key Files and Their Roles

### `src/executor/action.rs`
- **Role**: Core action processing, script generation, language detection
- **Current Issues**: Script formatting problems
- **Next**: Fix shebang placement, spacing, export organization

### `src/executor/workspace.rs`
- **Role**: File path management, directory structure
- **Current Issues**: Wrong directory methods for inputs/outputs
- **Next**: Update methods to match target structure, add caching support

### `src/executor/scheduler.rs`
- **Role**: Task execution orchestration, dependency management
- **Current Issues**: Creates wrong directories, wrong file paths
- **Next**: Update to use correct paths, implement caching, fix symlinks

### Test Files
- **Role**: Verification of functionality
- **Current Issues**: Tests expect old structure
- **Next**: Update for new directory structure and script format

## Dependencies and Requirements

### External Dependencies
- **jq**: Required for JSON handling (already implemented with proper checks)
- **bash**: Default shell for script execution
- **python3**: For python script execution

### Internal Dependencies
- Workspace initialization must happen before ActionProcessor use
- Scheduler must create task directories before ActionProcessor writes scripts
- Hash calculation must happen after script generation for caching

## Risk Areas

### 1. Backward Compatibility
- Existing examples and tests expect current structure
- Need careful migration to avoid breaking existing functionality

### 2. Cross-Platform Compatibility
- Symlink creation may behave differently on Windows
- File permissions for cached scripts

### 3. Concurrent Access
- Multiple tasks might try to create same cached script
- Need atomic operations for cache management

## Success Criteria

### ✅ Phase 1 Complete When:
- Generated scripts have shebang as first line
- Clean, minimal spacing between sections
- Environment exports grouped at top
- All tests still pass

### ✅ Phase 2 Complete When:
- No `inputs/` or `outputs/` subdirectories created
- Output files at task level (`<task>/<task>.output.json`)
- Dependency symlinks created only when needed
- Symlinks point to correct output files

### ✅ Phase 3 Complete When:
- Scripts written to `.cache/<hash>.{sh,py}`
- Task scripts are symlinks to cached files
- Identical scripts reuse same cache file
- Cache files have correct permissions

### ✅ Project Complete When:
- All tests pass with new structure
- Real examples work correctly
- Generated scripts are clean and professional
- Caching provides performance benefits
- Directory structure matches specification

## Current Status Summary

**Overall Progress**: ~70% complete
- ✅ Core functionality working
- ✅ Language detection working
- ✅ JSON handling working
- ✅ Integration working
- 🟡 Script formatting partially fixed
- ❌ Directory structure needs fixing
- ❌ Caching not implemented
- ❌ Tests need updating

**Next Immediate Actions**:
1. Finish script formatting fixes in `action.rs`
2. Update workspace methods for correct directory structure
3. Fix scheduler to use correct paths and implement caching
4. Update tests to match new structure
5. Verify end-to-end functionality
