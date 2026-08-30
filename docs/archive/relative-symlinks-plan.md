# Relative Symlinks Implementation Plan

## TL;DR

**Problem**: Otto creates symlinks with absolute paths, making workspace directories less portable and harder to read.

**Solution**: Convert to relative symlinks in 3 locations:
1. Script symlinks: `tasks/<task>/script.sh` → `../../../.cache/<hash>.sh`
2. Dependency symlinks: `tasks/<task>/input.<dep>.json` → `../<dep>/output.<dep>.json`
3. (Optional) Backup symlinks: `otto-latest.backup` → `otto-<version>-<timestamp>.backup`

**Effort**: 4-6 hours for core functionality

**Files to modify**:
- `src/executor/workspace.rs` (add helper methods)
- `src/executor/action.rs` (script symlinks)
- `src/executor/scheduler.rs` (dependency symlinks)
- `src/cli/commands/upgrade.rs` (optional, backup symlinks)

## Overview

Otto currently creates symlinks with absolute paths. This plan outlines the changes needed to convert them to relative paths for better portability and cleaner directory structures.

## Before/After Comparison

### Current (Absolute Paths) ❌
```bash
# Script symlinks
tasks/ci/script.sh -> /home/saidler/.otto/otto-26bf0c7a/.cache/f2deb29f.sh

# Dependency symlinks
tasks/all/input.ci.json -> /home/saidler/.otto/otto-26bf0c7a/1762469962/tasks/ci/output.ci.json

# Issues:
# - Not portable across machines/users
# - Verbose and cluttered
# - Workspace can't be moved
```

### Proposed (Relative Paths) ✅
```bash
# Script symlinks
tasks/ci/script.sh -> ../../../.cache/f2deb29f.sh

# Dependency symlinks
tasks/all/input.ci.json -> ../ci/output.ci.json

# Benefits:
# - Portable across machines/users
# - Clean and readable
# - Workspace can be moved/copied
```

## Current State Analysis

### Symlink Types

Otto creates two types of symlinks with absolute paths:

#### 1. Script Symlinks (Cache → Task Directory)
**Location**: `src/executor/action.rs:133`

```rust
fs::symlink(&cache_file, &script_path)?;
```

**Current behavior**:
- **Source**: `~/.otto/<project>-<hash>/<timestamp>/tasks/<task>/script.{sh,py}`
- **Target**: Absolute path to `~/.otto/<project>-<hash>/.cache/<hash>.{sh,py}`

**Example**:
```
tasks/ci/script.sh -> /home/saidler/.otto/otto-26bf0c7a/.cache/f2deb29f.sh
```

#### 2. Dependency Input Symlinks (Task → Task)
**Location**: `src/executor/scheduler.rs:448`

```rust
fs::symlink(&dep_output_file, &current_input_file)?;
```

**Current behavior**:
- **Source**: `~/.otto/<project>-<hash>/<timestamp>/tasks/<task>/input.<dep>.json`
- **Target**: Absolute path to `~/.otto/<project>-<hash>/<timestamp>/tasks/<dep>/output.<dep>.json`

**Example**:
```
tasks/all/input.ci.json -> /home/saidler/.otto/otto-26bf0c7a/1762469962/tasks/ci/output.ci.json
```

#### 3. Upgrade Backup Symlinks
**Location**: `src/cli/commands/upgrade.rs:375`

```rust
unix_fs::symlink(&backup_path, &latest_link).ok();
```

**Current behavior**:
- **Source**: `~/.otto/backups/otto-latest.backup`
- **Target**: Absolute path to `~/.otto/backups/otto-<version>-<timestamp>.backup`

**Note**: This is less critical for the portability goal but should be converted for consistency.

### Directory Structure

```
~/.otto/
  <project>-<hash>/                          # e.g. otto-26bf0c7a
    .cache/
      <script-hash>.sh                       # Cached scripts (content-addressable)
      <script-hash>.py
    <timestamp>/                             # e.g. 1762469962
      run.yaml
      tasks/
        <task-name>/
          script.sh                          # Symlink to cache
          input.<dep>.json                   # Symlink to dependency output
          output.<task-name>.json            # Task output
          stdout.log
          stderr.log
          builtins.sh
```

## Target State

### 1. Script Symlinks (Relative)

**Target**: `../../../.cache/<hash>.{sh,py}`

**Path calculation**:
- From task directory: `tasks/<task>/script.sh`
- Up 1 level: `tasks/`
- Up 1 level: `<timestamp>/`
- Up 1 level: `<project>-<hash>/`
- Down into `.cache/`

**Example**:
```
tasks/ci/script.sh -> ../../../.cache/f2deb29f.sh
```

### 2. Dependency Input Symlinks (Relative)

**Target**: `../<dep>/output.<dep>.json`

**Path calculation**:
- From task directory: `tasks/<task>/input.<dep>.json`
- Up 1 level: `tasks/`
- Down into dependency: `<dep>/`

**Example**:
```
tasks/all/input.ci.json -> ../ci/output.ci.json
```

### 3. Upgrade Backup Symlinks (Relative)

**Target**: `./<backup-file>`

**Path calculation**:
- Both files in same directory: `~/.otto/backups/`
- Relative path is just the filename

**Example**:
```
otto-latest.backup -> otto-0.1.0-1699999999.backup
```

## Implementation Plan

### Phase 1: Create Helper Functions

**File**: `src/executor/workspace.rs`

Add utility methods to the `Workspace` struct:

```rust
impl Workspace {
    /// Calculate relative path from source to target
    fn make_relative_path(source: &Path, target: &Path) -> Result<PathBuf> {
        // Use pathdiff crate or implement custom logic
        pathdiff::diff_paths(target, source.parent().unwrap())
            .ok_or_else(|| eyre!("Failed to calculate relative path"))
    }

    /// Get relative path from task script to cache file
    pub fn relative_script_cache_path(&self, cache_file: &Path, script_path: &Path) -> Result<PathBuf> {
        Self::make_relative_path(script_path, cache_file)
    }

    /// Get relative path from task input to dependency output
    pub fn relative_task_dependency_path(&self, task_name: &str, dep_name: &str) -> PathBuf {
        // Simple case: just ../dep_name/output.dep_name.json
        PathBuf::from("..").join(dep_name).join(format!("output.{}.json", dep_name))
    }
}
```

### Phase 2: Update Script Symlink Creation

**File**: `src/executor/action.rs`

**Method**: `ActionProcessor::write_script()`

**Current code** (lines 126-134):
```rust
if script_path.exists() {
    std::fs::remove_file(&script_path)?;
}

#[cfg(unix)]
{
    use std::os::unix::fs;
    fs::symlink(&cache_file, &script_path)?;
}
```

**New code**:
```rust
if script_path.exists() {
    std::fs::remove_file(&script_path)?;
}

#[cfg(unix)]
{
    use std::os::unix::fs;
    // Calculate relative path from script location to cache file
    let relative_cache = self.workspace.relative_script_cache_path(&cache_file, &script_path)?;
    fs::symlink(&relative_cache, &script_path)?;
}
```

### Phase 3: Update Dependency Input Symlink Creation

**File**: `src/executor/scheduler.rs`

**Method**: `Scheduler::execute_task()`

**Current code** (lines 440-449):
```rust
if current_input_file.exists() {
    tokio::fs::remove_file(&current_input_file).await.ok();
}

// Create symlink from dependency output to current task input
#[cfg(unix)]
{
    use std::os::unix::fs;
    fs::symlink(&dep_output_file, &current_input_file)?;
}
```

**New code**:
```rust
if current_input_file.exists() {
    tokio::fs::remove_file(&current_input_file).await.ok();
}

// Create symlink from dependency output to current task input
#[cfg(unix)]
{
    use std::os::unix::fs;
    // Use relative path: ../dep_name/output.dep_name.json
    let relative_dep_path = workspace.relative_task_dependency_path(&task_name, dep_name);
    fs::symlink(&relative_dep_path, &current_input_file)?;
}
```

### Phase 4: Update Upgrade Backup Symlink

**File**: `src/cli/commands/upgrade.rs`

**Method**: `create_backup_copy()`

**Current code** (lines 369-376):
```rust
// Update "latest" symlink on Unix systems
#[cfg(unix)]
{
    use std::os::unix::fs as unix_fs;
    let latest_link = backup_dir.join("otto-latest.backup");
    let _ = fs::remove_file(&latest_link);
    unix_fs::symlink(&backup_path, &latest_link).ok();
}
```

**New code**:
```rust
// Update "latest" symlink on Unix systems
#[cfg(unix)]
{
    use std::os::unix::fs as unix_fs;
    let latest_link = backup_dir.join("otto-latest.backup");
    let _ = fs::remove_file(&latest_link);

    // Use relative path (just the filename since both are in same directory)
    if let Some(backup_name) = backup_path.file_name() {
        unix_fs::symlink(backup_name, &latest_link).ok();
    }
}
```

### Phase 5: Add `pathdiff` Dependency (if needed)

**File**: `Cargo.toml`

If we use the `pathdiff` crate for robust relative path calculation:

```toml
[dependencies]
pathdiff = "0.2"
```

**Alternative**: Implement custom relative path calculation if we want to avoid the dependency.

### Alternative Approaches

#### Option A: Use `pathdiff` crate (Recommended)
**Pros**:
- Well-tested library
- Handles edge cases
- Clean API

**Cons**:
- Additional dependency

#### Option B: Manual relative path calculation
**Pros**:
- No dependencies
- Full control

**Cons**:
- Need to handle edge cases
- More testing required

**Implementation**:
```rust
fn make_relative_path(from: &Path, to: &Path) -> Result<PathBuf> {
    let from_abs = from.canonicalize()?;
    let to_abs = to.canonicalize()?;

    let from_components: Vec<_> = from_abs.parent().unwrap().components().collect();
    let to_components: Vec<_> = to_abs.components().collect();

    // Find common prefix
    let common = from_components.iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Build relative path: ../ for each remaining from component, then to components
    let mut result = PathBuf::new();
    for _ in 0..(from_components.len() - common) {
        result.push("..");
    }
    for component in &to_components[common..] {
        result.push(component);
    }

    Ok(result)
}
```

#### Option C: Hardcoded relative paths (Simple but less flexible)
Since we know the exact directory structure:
- Script symlinks always: `../../../.cache/<hash>.{sh,py}`
- Dependency symlinks always: `../<dep>/output.<dep>.json`

**Pros**:
- Simplest implementation
- No dependencies
- Fast

**Cons**:
- Brittle if directory structure changes
- Harder to maintain

**Recommendation**: Use Option C for the dependency symlinks (they're simple) and Option A or B for script symlinks.

## Testing Strategy

### Unit Tests

**File**: `src/executor/workspace.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_task_dependency_path() {
        let ws = Workspace::new_with_hash(
            PathBuf::from("/test"),
            "test".to_string(),
            "abcd1234".to_string()
        ).await.unwrap();

        let rel_path = ws.relative_task_dependency_path("all", "ci");
        assert_eq!(rel_path, PathBuf::from("../ci/output.ci.json"));
    }
}
```

### Integration Tests

**File**: `tests/symlink_tests.rs` (new file)

```rust
#[tokio::test]
async fn test_script_symlink_is_relative() {
    // Create workspace and task
    // Verify symlink target is relative
    // Verify symlink resolves correctly
}

#[tokio::test]
async fn test_dependency_symlink_is_relative() {
    // Create tasks with dependencies
    // Verify symlinks are relative
    // Verify data flows correctly
}
```

### Manual Testing

1. **Clean run test**:
   ```bash
   rm -rf ~/.otto/otto-*
   otto run ci
   ls -la ~/.otto/otto-*/*/tasks/*/script.sh
   ls -la ~/.otto/otto-*/*/tasks/*/input.*.json
   ```

2. **Symlink verification**:
   ```bash
   # Check symlinks are relative
   find ~/.otto -type l -exec sh -c 'echo "{}"; readlink "{}"' \;

   # Verify no absolute paths
   find ~/.otto -type l -exec readlink {} \; | grep "^/"
   # Should return nothing
   ```

3. **Functional test**:
   ```bash
   # Move the workspace directory to verify portability
   mv ~/.otto ~/.otto.backup
   mkdir ~/.otto
   mv ~/.otto.backup/otto-* ~/.otto/
   otto run ci  # Should still work
   ```

## Edge Cases & Considerations

### 1. Symlink Resolution
- **Issue**: Ensure relative paths resolve correctly from the symlink location
- **Solution**: Always calculate from the symlink's parent directory

### 2. Cross-Timestamp Symlinks
- **Issue**: Tasks in one run might reference another run's outputs
- **Current**: Not supported in current implementation
- **Impact**: No change needed (dependencies are always within same run)

### 3. Windows Compatibility
- **Issue**: Symlinks behave differently on Windows
- **Current**: Falls back to file copying on non-Unix
- **Impact**: No change needed (relative paths work in copies too)

### 4. Broken Symlinks
- **Issue**: If cache is cleaned, symlinks could break
- **Current**: Already a potential issue with absolute paths
- **Impact**: Same behavior, but now more portable

### 5. Symlink Readback
- **Issue**: Code that reads symlinks (e.g., `workspace.verify_task()`)
- **Current**: Uses `tokio::fs::read_link()` at line 203
- **Impact**: Should work fine with relative paths (might need testing)

## Implementation Priority

The three symlink types have different levels of importance:

1. **High Priority**: Script symlinks (Phase 2) and Dependency input symlinks (Phase 3)
   - These are created on every task execution
   - Most visible in the directory structure shown by the user
   - Core to Otto's task execution functionality

2. **Low Priority**: Upgrade backup symlinks (Phase 4)
   - Only created during `otto upgrade` command
   - Less frequently used
   - Can be done in a follow-up PR

**Recommendation**: Implement Phases 1-3 first, defer Phase 4 unless doing comprehensive cleanup.

## Rollout Plan

### Step 1: Development
1. Add helper functions to `Workspace`
2. Update `ActionProcessor::write_script()`
3. Update `Scheduler::execute_task()`
4. (Optional) Update `create_backup_copy()` in upgrade command
5. Add unit tests

### Step 2: Testing
1. Run existing test suite
2. Add new symlink-specific tests
3. Manual testing with various scenarios
4. Test on different systems (if possible)

### Step 3: Deployment
1. Merge to main branch
2. Note in changelog/release notes
3. Existing workspaces will continue to work (old absolute symlinks still resolve)
4. New runs will use relative symlinks

### Step 4: Validation
1. Monitor for issues
2. Verify symlinks in production usage
3. Document the change

## Benefits

1. **Portability**: Workspace can be moved/copied without breaking symlinks
2. **Cleaner output**: Shorter, more readable symlink targets
3. **Better for version control**: If workspace ever needs to be tracked
4. **Easier debugging**: Less cluttered paths in logs and output
5. **More Unix-idiomatic**: Relative symlinks are standard practice

## Potential Issues

1. **Backward compatibility**: Old symlinks will still be absolute (but functional)
2. **Testing coverage**: Need to ensure all symlink-related code is tested
3. **Path calculation bugs**: Edge cases in relative path calculation
4. **Platform differences**: Symlink behavior varies (but already handled)

## Dependencies

### Required
- None (can use stdlib `PathBuf` methods)

### Optional
- `pathdiff = "0.2"` for robust relative path calculation

## Estimated Effort

### Core Implementation (Phases 1-3)
- **Helper functions**: 30-60 minutes
- **Script symlink update**: 30 minutes
- **Dependency symlink update**: 30 minutes
- **Unit tests**: 1-2 hours
- **Integration tests**: 1-2 hours
- **Manual testing**: 1 hour
- **Documentation update**: 30 minutes
- **Subtotal**: ~4-6 hours

### Optional (Phase 4 - Upgrade Symlinks)
- **Implementation**: 15 minutes
- **Testing**: 30 minutes
- **Subtotal**: ~45 minutes

### Total Estimate
- **Core functionality**: 4-6 hours
- **With upgrade symlinks**: 5-7 hours

## Success Criteria

1. ✅ All new symlinks use relative paths
2. ✅ Script symlinks correctly resolve to cache
3. ✅ Dependency symlinks correctly resolve to outputs
4. ✅ All existing tests pass
5. ✅ New tests verify relative symlink behavior
6. ✅ Manual testing confirms functionality
7. ✅ No performance degradation

## References

- **Current implementation**: `src/executor/action.rs:133`, `src/executor/scheduler.rs:448`
- **Workspace structure**: `src/executor/workspace.rs`
- **Documentation**: `docs/directory-layout.md`, `docs/data-passing-plan.md`
- **Related**: Upgrade command also creates symlinks (`src/cli/commands/upgrade.rs:375`)
