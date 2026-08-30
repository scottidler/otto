# TUI Implementation Plan

## Overview

Add an interactive terminal UI mode to Otto for real-time task monitoring during parallel execution. Users enable this with `otto --tui <tasks>`. The TUI displays each task in its own pane with live output streaming, status indicators, and navigation controls.

## Goals

1. **Opt-in**: Default behavior unchanged, TUI enabled via `--tui` flag
2. **Live Streaming**: Real-time output display per task using existing broadcast channels
3. **Dynamic Panes**: Panes appear/disappear as tasks start/complete
4. **Navigation**: Tab cycling, scrolling, fullscreen mode
5. **No Prefix Pollution**: Task names in pane borders, not prefixing every line
6. **Graceful Degradation**: Fall back to terminal mode if TUI init fails

## ⚠️ Critical: Non-Invasive Implementation

**This feature must be built WITHOUT modifying the existing execution system.**

### Core Principles

1. **Separate Output Path**: TUI is purely an alternative *display mechanism*, not a change to execution logic
2. **Zero Impact on Default**: Without `--tui` flag, behavior must be **byte-for-byte identical**
3. **Existing Tests Are Gospel**: All current tests must pass throughout development
4. **Branching, Not Refactoring**: Use `if/else` branching, not shared code paths that risk regressions

### What This Means

**✅ ALLOWED:**
- Add new TUI-specific modules (`src/tui/`)
- Add optional parameters to functions (with defaults preserving current behavior)
- Subscribe to existing broadcast channels as a new consumer
- Add a simple boolean flag to suppress terminal output when TUI is active

**❌ NOT ALLOWED:**
- Changing existing execution logic "to support TUI"
- Modifying output formats or timing in the default path
- Refactoring working code "while we're at it"
- Making existing code depend on TUI modules

### Safety Net: Baseline Testing

**Before starting TUI implementation**, we must establish a baseline test suite that proves the current system works. These tests become our regression suite - run them after every phase.

If ANY baseline test fails during TUI development, **stop and fix it immediately**. No exceptions. The TUI feature is not worth breaking existing functionality.

## Architecture

### Key Components

```
┌─────────────────────────────────────────────────────────────┐
│                          Main                                │
│  ┌────────────────┐                 ┌────────────────────┐  │
│  │ CLI Parser     │──tui_mode────→ │ Execution Branch   │  │
│  │ (--tui flag)   │                │ if/else            │  │
│  └────────────────┘                └────────────────────┘  │
│                                      │              │       │
│                                      │              │       │
│                    ┌─────────────────┘              └────┐  │
│                    ↓                                      ↓  │
│         ┌──────────────────────┐          ┌──────────────────────┐
│         │ execute_with_tui()   │          │ execute_with_terminal│
│         │ (NEW)                │          │ (EXISTING)           │
│         └──────────────────────┘          └──────────────────────┘
│                    ↓                                      ↓
│         ┌──────────────────────┐          ┌──────────────────────┐
│         │ TuiApp               │          │ TaskScheduler        │
│         │ - Event loop         │          │ - Current behavior   │
│         │ - Pane management    │          └──────────────────────┘
│         │ - Ratatui rendering  │
│         └──────────────────────┘
│                    ↓
│         ┌──────────────────────┐
│         │ TaskScheduler        │
│         │ - Spawn tasks        │
│         │ - TaskStreams        │
│         │ - Broadcast channels │
│         └──────────────────────┘
│                    ↓
│         ┌──────────────────────┐
│         │ TaskPane (per task)  │
│         │ - Subscribe to       │
│         │   broadcast::Receiver│
│         │ - Buffer output      │
│         │ - Render to screen   │
│         └──────────────────────┘
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

```
Task Execution
     ↓
TaskOutput { task_name: "build", content: "compiling...\n" }
     ↓
broadcast::Sender (TaskStreams.output_tx)
     ↓
     ├─→ TeeWriter → Terminal (Terminal mode)
     │                "[build] compiling...\n"
     │
     └─→ TaskPane.output_rx → TUI Pane (TUI mode)
         ┌─ build ────────────┐
         │ compiling...       │  ← No prefix!
         └────────────────────┘
```

## Phase 0: Establish Baseline (DO THIS FIRST)

**Before writing any TUI code, establish proof that the current system works.**

### 0.1 Run Existing Test Suite

```bash
# Run all existing tests - ALL must pass
cargo test

# Document the results
cargo test 2>&1 | tee baseline-test-results.txt
```

Expected: All tests pass. If any fail, fix them BEFORE starting TUI work.

### 0.2 Manual Baseline Tests

Create a baseline test script to verify current behavior:

**File**: `tests/baseline_verification.sh`

```bash
#!/bin/bash
set -e

echo "=== Otto Baseline Verification ==="
echo "Run this before and after TUI changes to ensure no regressions"
echo ""

# Build first
cargo build --release
OTTO="./target/release/otto"

# Test 1: Basic execution
echo "[TEST] Basic task execution..."
cd examples/ex1
$OTTO test > /tmp/otto-baseline-1.txt 2>&1
if [ $? -eq 0 ]; then
    echo "✓ Basic execution works"
else
    echo "✗ Basic execution FAILED"
    exit 1
fi
cd ../..

# Test 2: Parallel execution
echo "[TEST] Parallel execution with multiple tasks..."
cd examples/ex10
$OTTO build package > /tmp/otto-baseline-2.txt 2>&1
if [ $? -eq 0 ]; then
    echo "✓ Parallel execution works"
else
    echo "✗ Parallel execution FAILED"
    exit 1
fi
cd ../..

# Test 3: Failed task handling
echo "[TEST] Failed task handling..."
cd examples/ex1
$OTTO nonexistent-task > /tmp/otto-baseline-3.txt 2>&1
if [ $? -ne 0 ]; then
    echo "✓ Failure handling works"
else
    echo "✗ Failure handling BROKEN (should have failed)"
    exit 1
fi
cd ../..

# Test 4: Output file creation
echo "[TEST] Output files created correctly..."
cd examples/ex1
$OTTO test
if [ -f ".otto/output/test.stdout" ]; then
    echo "✓ Output files created"
else
    echo "✗ Output files NOT created"
    exit 1
fi
cd ../..

# Test 5: File dependencies
echo "[TEST] File dependencies trigger reruns..."
cd examples/ex8
# Clean first
rm -rf .otto/
$OTTO process_config > /tmp/otto-baseline-5a.txt 2>&1
# Run again - should skip
$OTTO process_config > /tmp/otto-baseline-5b.txt 2>&1
if grep -q "up-to-date" /tmp/otto-baseline-5b.txt || grep -q "Skipped" /tmp/otto-baseline-5b.txt; then
    echo "✓ File dependencies work"
else
    echo "✗ File dependencies BROKEN"
    exit 1
fi
cd ../..

# Test 6: Clean command
echo "[TEST] Clean command..."
cd examples/ex1
$OTTO test
$OTTO clean
if [ ! -d ".otto/" ]; then
    echo "✓ Clean command works"
else
    echo "✗ Clean command FAILED"
    exit 1
fi
cd ../..

# Test 7: History command
echo "[TEST] History command..."
cd examples/ex1
$OTTO test
$OTTO history > /tmp/otto-baseline-7.txt 2>&1
if grep -q "test" /tmp/otto-baseline-7.txt; then
    echo "✓ History command works"
else
    echo "✗ History command FAILED"
    exit 1
fi
cd ../..

# Test 8: Jobs parallelism
echo "[TEST] Jobs parameter..."
cd examples/ex10
$OTTO -j 2 build package > /tmp/otto-baseline-8.txt 2>&1
if [ $? -eq 0 ]; then
    echo "✓ Jobs parameter works"
else
    echo "✗ Jobs parameter FAILED"
    exit 1
fi
cd ../..

echo ""
echo "=== All baseline tests passed! ==="
echo "Save this output for comparison after TUI changes."
```

Make executable:
```bash
chmod +x tests/baseline_verification.sh
```

### 0.3 Run Baseline Tests

```bash
# Run baseline tests
./tests/baseline_verification.sh

# Save results
./tests/baseline_verification.sh 2>&1 | tee baseline-verification-results.txt

# Commit the baseline
git add baseline-test-results.txt baseline-verification-results.txt
git commit -m "Baseline test results before TUI implementation"
```

### 0.4 Document Current Output Format

Capture exact current output format for comparison:

```bash
cd examples/ex10

# Capture single task output
cargo run -- build 2>&1 | tee baseline-single-task.txt

# Capture parallel output
cargo run -- build package 2>&1 | tee baseline-parallel-tasks.txt

# Capture with jobs flag
cargo run -- -j 4 build test package 2>&1 | tee baseline-jobs-flag.txt

cd ../..

git add baseline-*.txt
git commit -m "Baseline output formats before TUI implementation"
```

### 0.5 Regression Testing Protocol

**After EVERY phase of TUI implementation:**

1. Run full test suite: `cargo test`
2. Run baseline verification: `./tests/baseline_verification.sh`
3. Manually verify default behavior unchanged: `cargo run -- build` (in examples/ex10)
4. Check output files still created correctly
5. Verify no new warnings or errors in compilation

**If ANY regression is detected:**
- **STOP** TUI work immediately
- Fix the regression
- Re-run ALL baseline tests
- Only continue TUI work after baseline is green again

### 0.6 Create Regression Test Issues

Before starting Phase 1, create GitHub issues for tracking:

- [ ] Issue: "Baseline tests must pass before TUI merge"
- [ ] Issue: "Verify default output unchanged after TUI"
- [ ] Issue: "Performance comparison: before/after TUI"

Tag these as **blocking** for the TUI feature.

---

## Phase 1: CLI Flag Integration

### 1.1 Add --tui Flag

**File**: `src/cli/parser.rs`

**Location**: In `otto_command()` function, add argument alongside `jobs`, `verbose`, `dry-run`:

```rust
Arg::new("tui")
    .short('t')
    .long("tui")
    .help("Enable interactive TUI dashboard for task monitoring")
    .action(clap::ArgAction::SetTrue)
    .global(true)
```

**Why global**: Works with all task invocations, not subcommand-specific.

### 1.2 Parse and Extract Flag

**File**: `src/cli/parser.rs`

**Location**: In `Parser::parse()` method, after extracting `jobs`:

```rust
// After line ~310 where jobs is extracted
let tui_mode = matches.get_flag("tui");
```

### 1.3 Return TUI Mode from Parser

**File**: `src/cli/parser.rs`

**Change**: Update return signature:

```rust
// OLD
pub fn parse(&mut self) -> Result<(Vec<Task>, String, Option<PathBuf>, usize)>

// NEW
pub fn parse(&mut self) -> Result<(Vec<Task>, String, Option<PathBuf>, usize, bool)>

// At the end of parse():
Ok((tasks, self.hash.clone(), self.ottofile.clone(), self.jobs, tui_mode))
```

### 1.4 Update Main to Handle TUI Mode

**File**: `src/main.rs`

**Change**: Update call site in `main()` function:

```rust
// OLD (line ~85)
let (tasks, hash, ottofile_path, jobs) = match parser.parse() {

// NEW
let (tasks, hash, ottofile_path, jobs, tui_mode) = match parser.parse() {
```

Update `execute_tasks` call:

```rust
// OLD (line ~94)
if let Err(e) = execute_tasks(tasks, hash, ottofile_path, jobs).await {

// NEW
if let Err(e) = execute_tasks(tasks, hash, ottofile_path, jobs, tui_mode).await {
```

### 1.5 Branch in Execute Function

**File**: `src/main.rs`

**Change**: Update `execute_tasks` signature and add branching:

```rust
async fn execute_tasks(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
    tui_mode: bool,  // NEW
) -> Result<(), Report> {
    if tui_mode {
        // Check if we have a TTY
        if !atty::is(atty::Stream::Stdout) {
            eprintln!("Warning: --tui requires a TTY, falling back to standard output");
            return execute_with_terminal_output(tasks, hash, ottofile_path, jobs).await;
        }

        execute_with_tui(tasks, hash, ottofile_path, jobs).await
    } else {
        execute_with_terminal_output(tasks, hash, ottofile_path, jobs).await
    }
}
```

### 1.6 Extract Current Logic to execute_with_terminal_output

**File**: `src/main.rs`

Create new function with existing execution logic:

```rust
async fn execute_with_terminal_output(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    // Move all existing execute_tasks body here
    // This is the current behavior - no changes to logic

    let workspace = Arc::new(Workspace::new(ottofile_path)?);
    let db_path = workspace.db_path();
    let execution_context = ExecutionContext::new(db_path, &hash)?;

    info!("Executing {} tasks with {} parallel jobs", tasks.len(), jobs);

    let scheduler = TaskScheduler::new(
        tasks,
        workspace.clone(),
        execution_context,
        jobs,
    ).await?;

    scheduler.execute_all().await?;

    Ok(())
}
```

### 1.7 Create Stub execute_with_tui

**File**: `src/main.rs`

```rust
async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    // Phase 2+ implementation goes here
    todo!("TUI mode not yet implemented")
}
```

### 1.8 Add atty Dependency

**File**: `Cargo.toml`

```toml
[dependencies]
atty = "0.2"  # For TTY detection
```

### Testing Phase 1

**First, ensure baseline still passes:**
```bash
# Critical: Run baseline tests BEFORE testing TUI
cargo test
./tests/baseline_verification.sh
```

If baseline fails, **STOP** and fix before continuing.

**Then test TUI flag:**
```bash
# Should work as before
cargo build
./target/debug/otto build

# Should show not implemented error
./target/debug/otto --tui build

# Should fall back with warning when piped
./target/debug/otto --tui build | cat

# Short form should work
./target/debug/otto -t build

# Help should show new flag
./target/debug/otto --help
```

**Verification**:
- ✅ `--tui` flag appears in help
- ✅ Without `--tui`, behavior unchanged
- ✅ With `--tui`, hits the `todo!()`
- ✅ Non-TTY falls back gracefully
- ✅ **ALL baseline tests still pass**

---

## Phase 2: Basic TUI Infrastructure

### 2.1 Add Ratatui Dependencies

**File**: `Cargo.toml`

```toml
[dependencies]
ratatui = "0.29"
crossterm = "0.29"
```

### 2.2 Create TUI Module Structure

**New directory**: `src/tui/`

**Files to create**:
```
src/tui/
├── mod.rs        # Module exports
├── app.rs        # TuiApp struct and main event loop
├── pane.rs       # Pane trait and TaskPane implementation
└── layout.rs     # Layout management for dynamic panes
```

### 2.3 Create TUI Module Entry Point

**File**: `src/tui/mod.rs`

```rust
mod app;
mod pane;
mod layout;

pub use app::TuiApp;
pub use pane::{Pane, TaskPane};
pub use layout::PaneLayout;

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::io;

/// Initialize the terminal for TUI mode
pub fn init_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

/// Restore the terminal after TUI mode
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
```

### 2.4 Create Pane Trait

**File**: `src/tui/pane.rs`

```rust
use crate::executor::output::TaskOutput;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use tokio::sync::broadcast;

/// Status of a task displayed in a pane
#[derive(Debug, Clone, PartialEq)]
pub enum PaneStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl PaneStatus {
    pub fn symbol(&self) -> &str {
        match self {
            PaneStatus::Pending => "○",
            PaneStatus::Running => "●",
            PaneStatus::Completed => "✓",
            PaneStatus::Failed => "✗",
            PaneStatus::Skipped => "⊘",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            PaneStatus::Pending => Color::Gray,
            PaneStatus::Running => Color::Green,
            PaneStatus::Completed => Color::Green,
            PaneStatus::Failed => Color::Red,
            PaneStatus::Skipped => Color::Yellow,
        }
    }
}

/// Trait for renderable panes
pub trait Pane {
    /// Render the pane to the given area
    fn render(&self, frame: &mut Frame, area: Rect, focused: bool);

    /// Get the pane's identifier (task name)
    fn id(&self) -> &str;

    /// Update pane state (receive from broadcast channel)
    fn update(&mut self);

    /// Get current status
    fn status(&self) -> PaneStatus;

    /// Scroll up
    fn scroll_up(&mut self);

    /// Scroll down
    fn scroll_down(&mut self, visible_height: u16);

    /// Reset scroll to top
    fn reset_scroll(&mut self);
}

/// A pane that displays output from a single task
pub struct TaskPane {
    task_name: String,
    status: PaneStatus,
    output_rx: broadcast::Receiver<TaskOutput>,
    output_buffer: VecDeque<String>,
    scroll_offset: u16,
    max_buffer_lines: usize,
}

impl TaskPane {
    pub fn new(
        task_name: String,
        output_tx: broadcast::Sender<TaskOutput>,
    ) -> Self {
        Self {
            task_name: task_name.clone(),
            status: PaneStatus::Pending,
            output_rx: output_tx.subscribe(),
            output_buffer: VecDeque::new(),
            scroll_offset: 0,
            max_buffer_lines: 1000, // Ring buffer
        }
    }

    pub fn set_status(&mut self, status: PaneStatus) {
        self.status = status;
    }
}

impl Pane for TaskPane {
    fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        // Create border with task name and status
        let title = format!(
            " {} {} ",
            self.task_name,
            self.status.symbol()
        );

        let border_color = if focused {
            Color::Yellow
        } else {
            self.status.color()
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Render output lines with scrolling
        let visible_height = inner_area.height as usize;
        let total_lines = self.output_buffer.len();

        let start_line = (self.scroll_offset as usize).min(total_lines.saturating_sub(visible_height));
        let end_line = (start_line + visible_height).min(total_lines);

        let visible_lines: Vec<Line> = self.output_buffer
            .iter()
            .skip(start_line)
            .take(end_line - start_line)
            .map(|s| Line::from(s.as_str()))
            .collect();

        let paragraph = Paragraph::new(visible_lines);
        frame.render_widget(paragraph, inner_area);
    }

    fn id(&self) -> &str {
        &self.task_name
    }

    fn update(&mut self) {
        // Non-blocking receive from broadcast channel
        while let Ok(output) = self.output_rx.try_recv() {
            // Only process output for this task
            if output.task_name == self.task_name {
                // Split content by lines and add to buffer
                for line in output.content.lines() {
                    self.output_buffer.push_back(line.to_string());

                    // Maintain ring buffer
                    if self.output_buffer.len() > self.max_buffer_lines {
                        self.output_buffer.pop_front();
                    }
                }

                // If content ends with newline, add empty line
                if output.content.ends_with('\n') && !output.content.trim().is_empty() {
                    // Already handled by lines()
                }
            }
        }
    }

    fn status(&self) -> PaneStatus {
        self.status.clone()
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn scroll_down(&mut self, visible_height: u16) {
        let total_lines = self.output_buffer.len() as u16;
        if total_lines > visible_height {
            let max_scroll = total_lines - visible_height;
            if self.scroll_offset < max_scroll {
                self.scroll_offset += 1;
            }
        }
    }

    fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }
}
```

### 2.5 Create Layout Manager

**File**: `src/tui/layout.rs`

```rust
use super::pane::Pane;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

/// Manages dynamic pane layout
pub struct PaneLayout {
    panes: Vec<Box<dyn Pane>>,
    focused_index: usize,
}

impl PaneLayout {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            focused_index: 0,
        }
    }

    pub fn add_pane(&mut self, pane: Box<dyn Pane>) {
        self.panes.push(pane);
    }

    pub fn remove_pane(&mut self, task_name: &str) {
        self.panes.retain(|p| p.id() != task_name);
        if self.focused_index >= self.panes.len() && !self.panes.is_empty() {
            self.focused_index = self.panes.len() - 1;
        }
    }

    pub fn update_all(&mut self) {
        for pane in &mut self.panes {
            pane.update();
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if self.panes.is_empty() {
            return;
        }

        let grid_areas = self.calculate_grid(area);

        for (i, pane) in self.panes.iter().enumerate() {
            if let Some(pane_area) = grid_areas.get(i) {
                let focused = i == self.focused_index;
                pane.render(frame, *pane_area, focused);
            }
        }
    }

    fn calculate_grid(&self, area: Rect) -> Vec<Rect> {
        let num_panes = self.panes.len();

        if num_panes == 0 {
            return vec![];
        }

        // Determine grid dimensions based on pane count
        let (rows, cols) = match num_panes {
            1 => (1, 1),
            2 => (1, 2),
            3..=4 => (2, 2),
            5..=6 => (2, 3),
            7..=9 => (3, 3),
            10..=12 => (3, 4),
            _ => (4, 4), // Max 16 visible panes
        };

        // Create row constraints
        let row_constraints: Vec<Constraint> = (0..rows)
            .map(|_| Constraint::Percentage(100 / rows as u16))
            .collect();

        let row_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(row_constraints)
            .split(area);

        // Create column constraints for each row
        let col_constraints: Vec<Constraint> = (0..cols)
            .map(|_| Constraint::Percentage(100 / cols as u16))
            .collect();

        let mut grid_areas = Vec::new();
        for row_area in row_layout {
            let col_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(&col_constraints)
                .split(row_area);

            grid_areas.extend(col_layout.iter().copied());
        }

        // Return only as many areas as we have panes
        grid_areas.truncate(num_panes);
        grid_areas
    }

    pub fn focus_next(&mut self) {
        if !self.panes.is_empty() {
            self.focused_index = (self.focused_index + 1) % self.panes.len();
        }
    }

    pub fn focus_prev(&mut self) {
        if !self.panes.is_empty() {
            self.focused_index = if self.focused_index == 0 {
                self.panes.len() - 1
            } else {
                self.focused_index - 1
            };
        }
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut Box<dyn Pane>> {
        self.panes.get_mut(self.focused_index)
    }

    pub fn pane_ids(&self) -> Vec<String> {
        self.panes.iter().map(|p| p.id().to_string()).collect()
    }
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::new()
    }
}
```

### 2.6 Create TUI App

**File**: `src/tui/app.rs`

```rust
use super::layout::PaneLayout;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{backend::Backend, Terminal};
use std::io;
use std::time::{Duration, Instant};

const TUI_TICK_RATE_MS: u64 = 100; // 10 FPS

/// Main TUI application
pub struct TuiApp {
    layout: PaneLayout,
    should_quit: bool,
    last_tick: Instant,
    tick_rate: Duration,
}

impl TuiApp {
    pub fn new() -> Self {
        Self {
            layout: PaneLayout::new(),
            should_quit: false,
            last_tick: Instant::now(),
            tick_rate: Duration::from_millis(TUI_TICK_RATE_MS),
        }
    }

    pub fn layout_mut(&mut self) -> &mut PaneLayout {
        &mut self.layout
    }

    /// Run the TUI event loop
    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            // Draw UI
            terminal.draw(|f| self.layout.render(f, f.area()))?;

            // Handle events with timeout
            let timeout = self.tick_rate
                .checked_sub(self.last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if crossterm::event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key_event(key.code);
                    }
                }
            }

            // Update tick
            if self.last_tick.elapsed() >= self.tick_rate {
                self.on_tick();
                self.last_tick = Instant::now();
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    fn on_tick(&mut self) {
        // Update all panes (receive from broadcast channels)
        self.layout.update_all();
    }

    fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Tab | KeyCode::Right => {
                self.layout.focus_next();
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.layout.focus_prev();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.scroll_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    // TODO: Get visible height from render area
                    pane.scroll_down(20);
                }
            }
            KeyCode::Home => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.reset_scroll();
                }
            }
            _ => {}
        }
    }
}
```

### 2.7 Expose TUI Module in lib.rs

**File**: `src/lib.rs`

Add:

```rust
pub mod tui;
```

### Testing Phase 2

**First, ensure baseline still passes:**
```bash
cargo test
./tests/baseline_verification.sh
```

If baseline fails, **STOP** and fix before continuing.

**Then test TUI infrastructure:**

Create a test binary to verify TUI infrastructure:

**File**: `examples/tui_test.rs`

```rust
use otto::tui::{TuiApp, TaskPane};
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize terminal
    let mut terminal = otto::tui::init_terminal()?;

    // Create app
    let mut app = TuiApp::new();

    // Create dummy broadcast channel
    let (tx, _rx) = broadcast::channel(100);

    // Add some test panes
    app.layout_mut().add_pane(Box::new(TaskPane::new("test-task-1".to_string(), tx.clone())));
    app.layout_mut().add_pane(Box::new(TaskPane::new("test-task-2".to_string(), tx.clone())));
    app.layout_mut().add_pane(Box::new(TaskPane::new("test-task-3".to_string(), tx.clone())));

    // Run TUI
    let result = app.run(&mut terminal);

    // Restore terminal
    otto::tui::restore_terminal(&mut terminal)?;

    result?;
    Ok(())
}
```

```bash
cargo run --example tui_test
```

**Verification**:
- ✅ Terminal switches to alternate screen
- ✅ Three empty panes render in grid
- ✅ Tab cycles focus (yellow border)
- ✅ q/Esc exits cleanly
- ✅ Terminal restores properly

---

## Phase 3: Integrate TUI with Task Execution

### 3.1 Make TaskScheduler TUI-Aware

**File**: `src/executor/scheduler.rs`

Add field to struct:

```rust
pub struct TaskScheduler {
    task_statuses: Arc<Mutex<HashMap<String, TaskStatus>>>,
    semaphore: Arc<Semaphore>,
    workspace: Arc<Workspace>,
    execution_context: ExecutionContext,
    tasks: Vec<Task>,
    tui_mode: bool,  // NEW
}
```

Update constructor:

```rust
pub async fn new(
    tasks: Vec<Task>,
    workspace: Arc<Workspace>,
    execution_context: ExecutionContext,
    max_parallel: usize,
    tui_mode: bool,  // NEW
) -> Result<Self> {
    // ... existing code ...

    Ok(Self {
        task_statuses,
        semaphore: Arc::new(Semaphore::new(max_parallel)),
        workspace,
        execution_context,
        tasks,
        tui_mode,  // NEW
    })
}
```

### 3.2 Suppress Terminal Output in TUI Mode

**File**: `src/executor/output.rs`

Modify `TeeWriter` to conditionally print:

```rust
pub struct TeeWriter {
    file: File,
    is_stderr: bool,
    task_name: String,
    suppress_terminal: bool,  // NEW
}

impl TeeWriter {
    pub async fn new(
        file: File,
        is_stderr: bool,
        task_name: String,
        suppress_terminal: bool,  // NEW
    ) -> Self {
        Self {
            file,
            is_stderr,
            task_name,
            suppress_terminal,
        }
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        // Always write to file
        self.file.write_all(data).await?;

        // Conditionally write to terminal
        if !self.suppress_terminal {
            let colored_prefix = colorize_task_prefix(&self.task_name);
            let terminal_output = format!("{} {}", colored_prefix, String::from_utf8_lossy(data));
            if self.is_stderr {
                eprint!("{terminal_output}");
            } else {
                print!("{terminal_output}");
            }

            // Flush
            if self.is_stderr {
                io::stderr().flush()?;
            } else {
                io::stdout().flush()?;
            }
        }

        Ok(())
    }
}
```

Update `TaskStreams::process_output`:

```rust
pub async fn process_output(
    &self,
    task_name: String,
    output_type: OutputType,
    mut reader: impl AsyncBufReadExt + Unpin,
    suppress_terminal: bool,  // NEW
) -> Result<()> {
    let output_file = match output_type {
        OutputType::Stdout => &self.stdout_file,
        OutputType::Stderr => &self.stderr_file,
    };

    let file = File::create(output_file).await?;
    let mut writer = TeeWriter::new(
        file,
        matches!(output_type, OutputType::Stderr),
        task_name.clone(),
        suppress_terminal,  // NEW
    ).await;

    // ... rest unchanged
}
```

### 3.3 Pass Suppression Flag Through Scheduler

**File**: `src/executor/scheduler.rs`

In `execute_task` method, when spawning output processors:

```rust
// Around line 300-350 where output processors are spawned
let suppress_terminal = self.tui_mode;

tokio::spawn({
    let streams = streams.clone();
    let task_name = task.name.clone();
    async move {
        let stdout = child.stdout.take().expect("stdout not captured");
        let reader = BufReader::new(stdout);
        if let Err(e) = streams
            .process_output(
                task_name,
                OutputType::Stdout,
                reader,
                suppress_terminal  // NEW
            )
            .await
        {
            error!("Error processing stdout: {}", e);
        }
    }
});

// Similar for stderr
```

### 3.4 Return TaskStreams Collection from Scheduler

We need a way to get the broadcast senders for each task to wire up TUI panes.

**File**: `src/executor/scheduler.rs`

Add method:

```rust
impl TaskScheduler {
    /// Get a map of task names to their output broadcast senders
    pub fn task_output_channels(&self) -> HashMap<String, broadcast::Sender<TaskOutput>> {
        // This requires storing the senders during task creation
        // We'll need to refactor execute_task to save them
        todo!("Implement in next step")
    }
}
```

**Better approach**: Change scheduler to store and provide access to TaskStreams:

```rust
pub struct TaskScheduler {
    // ... existing fields ...
    task_streams: Arc<Mutex<HashMap<String, Arc<TaskStreams>>>>,  // NEW
}
```

Update `execute_task` to store TaskStreams:

```rust
async fn execute_task(&self, task: Task, tx: mpsc::Sender<Result<String>>) -> Result<JoinHandle<()>> {
    // ... existing setup ...

    let streams = Arc::new(TaskStreams::new(&task.name, &output_dir).await?);

    // Store for TUI access
    {
        let mut streams_map = self.task_streams.lock().await;
        streams_map.insert(task.name.clone(), streams.clone());
    }

    // ... rest of execution ...
}
```

Add accessor:

```rust
impl TaskScheduler {
    pub fn get_task_stream(&self, task_name: &str) -> Option<Arc<TaskStreams>> {
        // This will be called from async context
        // Need to use tokio::runtime::Handle::current().block_on() or make it async
        // For now, document this needs refinement
        todo!("Needs async accessor")
    }
}
```

**Alternative simpler approach**: Have `execute_with_tui` create its own TaskStreams and pass them to scheduler:

We'll use this approach - see next section.

### 3.5 Implement execute_with_tui

**File**: `src/main.rs`

```rust
async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    use otto::executor::output::TaskStreams;
    use otto::tui::{TuiApp, TaskPane};
    use std::path::Path;

    // Set up workspace
    let workspace = Arc::new(Workspace::new(ottofile_path)?);
    let db_path = workspace.db_path();
    let execution_context = ExecutionContext::new(db_path, &hash)?;

    // Create task streams for each task (before starting scheduler)
    let output_dir = workspace.output_dir();
    let mut task_streams_map = std::collections::HashMap::new();

    for task in &tasks {
        let streams = TaskStreams::new(&task.name, &output_dir).await?;
        task_streams_map.insert(task.name.clone(), streams);
    }

    // Initialize TUI
    let mut terminal = otto::tui::init_terminal()
        .map_err(|e| eyre::eyre!("Failed to initialize TUI: {}", e))?;

    let mut app = TuiApp::new();

    // Create pane for each task
    for task in &tasks {
        let streams = task_streams_map.get(&task.name).unwrap();
        let pane = TaskPane::new(task.name.clone(), streams.output_tx.clone());
        app.layout_mut().add_pane(Box::new(pane));
    }

    // Start scheduler in background
    let scheduler = TaskScheduler::new(
        tasks,
        workspace.clone(),
        execution_context,
        jobs,
        true,  // tui_mode = true
    ).await?;

    let scheduler_handle = tokio::spawn(async move {
        scheduler.execute_all().await
    });

    // Run TUI (blocks until user quits)
    let tui_result = app.run(&mut terminal);

    // Restore terminal
    otto::tui::restore_terminal(&mut terminal)?;

    // Handle TUI errors
    tui_result.map_err(|e| eyre::eyre!("TUI error: {}", e))?;

    // Wait for scheduler to complete or propagate errors
    match scheduler_handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(eyre::eyre!("Scheduler panicked: {}", e)),
    }
}
```

**Problem**: This approach creates TaskStreams but scheduler creates its own. We need to pass them in.

**Revised approach**: Refactor scheduler to accept pre-created TaskStreams.

### 3.6 Refactor Scheduler to Accept TaskStreams

**File**: `src/executor/scheduler.rs`

Add to constructor:

```rust
pub async fn new(
    tasks: Vec<Task>,
    workspace: Arc<Workspace>,
    execution_context: ExecutionContext,
    max_parallel: usize,
    tui_mode: bool,
    task_streams: Option<HashMap<String, Arc<TaskStreams>>>,  // NEW
) -> Result<Self> {
    // Store task_streams in struct
    // ...
}
```

Add field:

```rust
pub struct TaskScheduler {
    // ... existing ...
    task_streams: Option<HashMap<String, Arc<TaskStreams>>>,
}
```

In `execute_task`, use provided streams if available:

```rust
async fn execute_task(&self, task: Task, tx: mpsc::Sender<Result<String>>) -> Result<JoinHandle<()>> {
    // ...

    let streams = if let Some(ref streams_map) = self.task_streams {
        // Use pre-created streams (TUI mode)
        streams_map.get(&task.name)
            .ok_or_else(|| eyre!("TaskStreams not found for {}", task.name))?
            .clone()
    } else {
        // Create streams on-demand (terminal mode)
        Arc::new(TaskStreams::new(&task.name, &output_dir).await?)
    };

    // ... rest unchanged ...
}
```

### 3.7 Wire Up Status Updates

TaskPanes need to update their status when tasks start/complete.

**Option A**: Use existing task_statuses from scheduler

Problem: Scheduler's `task_statuses` is private and accessed internally.

**Option B**: Add status to broadcast channel

Modify `TaskOutput` to optionally include status updates:

**File**: `src/executor/output.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskMessage {
    Output(TaskOutput),
    StatusChange(String, TaskStatus),  // task_name, new_status
}
```

Then broadcast status changes:

**File**: `src/executor/scheduler.rs`

```rust
// When task starts
streams.output_tx.send(TaskMessage::StatusChange(
    task.name.clone(),
    TaskStatus::Running
))?;

// When task completes
streams.output_tx.send(TaskMessage::StatusChange(
    task.name.clone(),
    TaskStatus::Completed
))?;
```

Update TaskPane to handle both:

**File**: `src/tui/pane.rs`

```rust
fn update(&mut self) {
    while let Ok(msg) = self.output_rx.try_recv() {
        match msg {
            TaskMessage::Output(output) => {
                if output.task_name == self.task_name {
                    // ... handle output ...
                }
            }
            TaskMessage::StatusChange(task_name, status) => {
                if task_name == self.task_name {
                    self.status = status.into(); // Convert TaskStatus to PaneStatus
                }
            }
        }
    }
}
```

**Better approach**: Keep it simple for now, infer status from output:
- If receiving output → Running
- If "finished successfully" seen → Completed
- If error messages → Failed

Implement proper status tracking in Phase 4.

### Testing Phase 3

**First, ensure baseline still passes:**
```bash
cargo test
./tests/baseline_verification.sh
```

If baseline fails, **STOP** and fix before continuing.

**Then test TUI integration:**

```bash
# Build
cargo build

# Test TUI with actual tasks
./target/debug/otto --tui test

# Should see:
# - TUI starts
# - Pane for "test" task appears
# - Live output streams in (no [test] prefix)
# - Can scroll with up/down
# - Tab to cycle (if multiple tasks)
# - q to quit
```

**Verification**:
- ✅ TUI initializes
- ✅ Panes created for each task
- ✅ Output appears in real-time
- ✅ No `[task-name]` prefixes in panes
- ✅ File logs still written correctly
- ✅ Tasks execute to completion
- ✅ TUI can be quit with 'q'

---

## Phase 4: Enhanced UX & Polish

### 4.1 Proper Status Tracking

Implement status message broadcasting:

**File**: `src/executor/output.rs`

Create enum for messages:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskMessage {
    Output {
        task_name: String,
        stream_type: OutputType,
        timestamp: SystemTime,
        content: String,
    },
    StatusChange {
        task_name: String,
        status: TaskStatusUpdate,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatusUpdate {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}
```

Update TaskStreams:

```rust
pub struct TaskStreams {
    pub stdout_file: PathBuf,
    pub stderr_file: PathBuf,
    pub output_tx: broadcast::Sender<TaskMessage>,  // Changed type
}

impl TaskStreams {
    pub fn broadcast_status(&self, status: TaskStatusUpdate, task_name: String) -> Result<()> {
        self.output_tx.send(TaskMessage::StatusChange {
            task_name,
            status,
        })?;
        Ok(())
    }
}
```

**File**: `src/executor/scheduler.rs`

Broadcast status at key points:

```rust
// Before starting task
if let Some(streams) = task_streams_map.get(&task.name) {
    let _ = streams.broadcast_status(
        TaskStatusUpdate::Running,
        task.name.clone()
    );
}

// After task completes successfully
let _ = streams.broadcast_status(
    TaskStatusUpdate::Completed,
    task.name.clone()
);

// On task failure
let _ = streams.broadcast_status(
    TaskStatusUpdate::Failed(error_msg),
    task.name.clone()
);

// When skipped
let _ = streams.broadcast_status(
    TaskStatusUpdate::Skipped,
    task.name.clone()
);
```

Update TaskPane to handle:

**File**: `src/tui/pane.rs`

```rust
fn update(&mut self) {
    while let Ok(msg) = self.output_rx.try_recv() {
        match msg {
            TaskMessage::Output { task_name, content, .. } => {
                if task_name == self.task_name {
                    for line in content.lines() {
                        self.output_buffer.push_back(line.to_string());
                        if self.output_buffer.len() > self.max_buffer_lines {
                            self.output_buffer.pop_front();
                        }
                    }
                }
            }
            TaskMessage::StatusChange { task_name, status } => {
                if task_name == self.task_name {
                    self.status = match status {
                        TaskStatusUpdate::Pending => PaneStatus::Pending,
                        TaskStatusUpdate::Running => PaneStatus::Running,
                        TaskStatusUpdate::Completed => PaneStatus::Completed,
                        TaskStatusUpdate::Failed(_) => PaneStatus::Failed,
                        TaskStatusUpdate::Skipped => PaneStatus::Skipped,
                    };
                }
            }
        }
    }
}
```

### 4.2 Fullscreen Mode

Add fullscreen toggle to TuiApp:

**File**: `src/tui/app.rs`

```rust
pub struct TuiApp {
    layout: PaneLayout,
    should_quit: bool,
    last_tick: Instant,
    tick_rate: Duration,
    fullscreen_pane: Option<usize>,  // NEW: index of fullscreen pane
}

impl TuiApp {
    fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            // ... existing cases ...

            KeyCode::Enter | KeyCode::Char('f') => {
                // Toggle fullscreen
                if self.fullscreen_pane.is_some() {
                    self.fullscreen_pane = None;
                } else {
                    self.fullscreen_pane = Some(self.layout.focused_index());
                }
            }

            // ... rest ...
        }
    }

    fn draw(&self, frame: &mut Frame) {
        if let Some(fullscreen_idx) = self.fullscreen_pane {
            // Render only the fullscreen pane
            if let Some(pane) = self.layout.get_pane(fullscreen_idx) {
                pane.render(frame, frame.area(), true);
            }
        } else {
            // Normal multi-pane layout
            self.layout.render(frame, frame.area());
        }
    }
}
```

Update `run` to use new draw method:

```rust
pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
    loop {
        terminal.draw(|f| self.draw(f))?;
        // ... rest unchanged ...
    }
}
```

### 4.3 Add Status Bar

**File**: `src/tui/app.rs`

Add helper to render status bar:

```rust
fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
    use ratatui::widgets::{Paragraph};
    use ratatui::text::Span;

    let help_text = if self.fullscreen_pane.is_some() {
        "ESC: Exit fullscreen  q: Quit  ↑↓: Scroll"
    } else {
        "Tab: Next  Enter: Fullscreen  ↑↓: Scroll  q: Quit"
    };

    let status = Paragraph::new(Span::styled(
        help_text,
        Style::default().fg(Color::DarkGray)
    ));

    frame.render_widget(status, area);
}

fn draw(&self, frame: &mut Frame) {
    let area = frame.area();

    // Split area into main + status bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    // Main content area
    if let Some(fullscreen_idx) = self.fullscreen_pane {
        if let Some(pane) = self.layout.get_pane(fullscreen_idx) {
            pane.render(frame, chunks[0], true);
        }
    } else {
        self.layout.render(frame, chunks[0]);
    }

    // Status bar
    self.render_status_bar(frame, chunks[1]);
}
```

### 4.4 Improve Scroll Behavior

**Auto-scroll to bottom**: When new output arrives, optionally follow.

**File**: `src/tui/pane.rs`

```rust
pub struct TaskPane {
    // ... existing fields ...
    auto_scroll: bool,  // NEW
}

impl TaskPane {
    fn update(&mut self) {
        let had_new_output = false;

        while let Ok(msg) = self.output_rx.try_recv() {
            match msg {
                TaskMessage::Output { task_name, content, .. } => {
                    if task_name == self.task_name {
                        // ... add to buffer ...
                        had_new_output = true;
                    }
                }
                // ... status handling ...
            }
        }

        // Auto-scroll to bottom if enabled and new output arrived
        if self.auto_scroll && had_new_output {
            self.scroll_offset = u16::MAX; // Will be clamped in render
        }
    }

    fn scroll_up(&mut self) {
        // Disable auto-scroll when user manually scrolls up
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn scroll_down(&mut self, visible_height: u16) {
        let total_lines = self.output_buffer.len() as u16;
        if total_lines > visible_height {
            let max_scroll = total_lines - visible_height;
            if self.scroll_offset < max_scroll {
                self.scroll_offset += 1;
            } else {
                // At bottom, re-enable auto-scroll
                self.auto_scroll = true;
            }
        }
    }
}
```

### 4.5 Handle Completed Tasks

Option to keep or hide completed panes:

**File**: `src/tui/app.rs`

Add field:

```rust
pub struct TuiApp {
    // ... existing ...
    hide_completed: bool,  // NEW
}

impl TuiApp {
    fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            // ... existing ...

            KeyCode::Char('h') => {
                // Toggle hide completed tasks
                self.hide_completed = !self.hide_completed;
            }

            // ... rest ...
        }
    }
}
```

**File**: `src/tui/layout.rs`

Filter panes when rendering:

```rust
impl PaneLayout {
    pub fn render(&self, frame: &mut Frame, area: Rect, hide_completed: bool) {
        let visible_panes: Vec<&Box<dyn Pane>> = if hide_completed {
            self.panes.iter()
                .filter(|p| p.status() != PaneStatus::Completed)
                .collect()
        } else {
            self.panes.iter().collect()
        };

        if visible_panes.is_empty() {
            return;
        }

        let grid_areas = self.calculate_grid(area, visible_panes.len());

        for (i, pane) in visible_panes.iter().enumerate() {
            if let Some(pane_area) = grid_areas.get(i) {
                let focused = self.is_focused(pane);
                pane.render(frame, *pane_area, focused);
            }
        }
    }
}
```

### 4.6 Add Duration Display

Show how long each task has been running:

**File**: `src/tui/pane.rs`

```rust
pub struct TaskPane {
    // ... existing ...
    start_time: Option<Instant>,
}

impl TaskPane {
    fn update(&mut self) {
        while let Ok(msg) = self.output_rx.try_recv() {
            match msg {
                TaskMessage::StatusChange { task_name, status } => {
                    if task_name == self.task_name {
                        match status {
                            TaskStatusUpdate::Running => {
                                self.start_time = Some(Instant::now());
                                self.status = PaneStatus::Running;
                            }
                            // ... other statuses ...
                        }
                    }
                }
                // ... output handling ...
            }
        }
    }

    fn duration_str(&self) -> String {
        if let Some(start) = self.start_time {
            let elapsed = start.elapsed();
            format!("{:.1}s", elapsed.as_secs_f64())
        } else {
            String::new()
        }
    }
}

impl Pane for TaskPane {
    fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let duration = self.duration_str();
        let title = if duration.is_empty() {
            format!(" {} {} ", self.task_name, self.status.symbol())
        } else {
            format!(" {} {} {} ", self.task_name, self.status.symbol(), duration)
        };

        // ... rest of render ...
    }
}
```

### Testing Phase 4

**First, ensure baseline still passes:**
```bash
cargo test
./tests/baseline_verification.sh
```

If baseline fails, **STOP** and fix before continuing.

**Then test enhanced UX:**

```bash
cargo build
./target/debug/otto --tui build test docs
```

**Verification**:
- ✅ Status updates reflect task state (○ → ● → ✓)
- ✅ Duration shows elapsed time
- ✅ Enter toggles fullscreen
- ✅ Status bar shows help
- ✅ Auto-scroll to bottom works
- ✅ Manual scroll disables auto-scroll
- ✅ Scrolling back to bottom re-enables auto-scroll
- ✅ 'h' toggles hiding completed tasks

---

## Phase 5: Edge Cases & Robustness

### 5.1 Handle TUI Initialization Failures

**File**: `src/main.rs`

Wrap TUI init in error handling:

```rust
async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    // Try to initialize TUI
    let mut terminal = match otto::tui::init_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Warning: Failed to initialize TUI ({}), falling back to terminal output", e);
            return execute_with_terminal_output(tasks, hash, ottofile_path, jobs).await;
        }
    };

    // ... rest of TUI execution ...

    // Ensure terminal is restored even on error
    let result = {
        // ... TUI execution logic ...
    };

    // Always restore terminal
    if let Err(e) = otto::tui::restore_terminal(&mut terminal) {
        eprintln!("Warning: Failed to restore terminal: {}", e);
    }

    result
}
```

### 5.2 Handle Task Completion Before TUI Starts

If tasks complete very quickly, they might finish before TUI renders.

**File**: `src/main.rs`

Ensure all status updates are captured:

```rust
// In execute_with_tui, set up panes with initial status
for task in &tasks {
    let streams = task_streams_map.get(&task.name).unwrap();
    let mut pane = TaskPane::new(task.name.clone(), streams.output_tx.clone());

    // Set initial status (tasks haven't started yet)
    pane.set_status(PaneStatus::Pending);

    app.layout_mut().add_pane(Box::new(pane));
}
```

### 5.3 Handle Very Long Lines

Terminal width is limited, long lines should wrap or truncate:

**File**: `src/tui/pane.rs`

```rust
fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
    // ... existing block rendering ...

    // Wrap long lines to fit width
    let inner_width = inner_area.width as usize;
    let wrapped_lines: Vec<Line> = self.output_buffer
        .iter()
        .skip(start_line)
        .take(end_line - start_line)
        .flat_map(|line| {
            // Wrap lines that exceed width
            if line.len() > inner_width {
                line.chars()
                    .collect::<Vec<_>>()
                    .chunks(inner_width)
                    .map(|chunk| Line::from(chunk.iter().collect::<String>()))
                    .collect::<Vec<_>>()
            } else {
                vec![Line::from(line.as_str())]
            }
        })
        .collect();

    let paragraph = Paragraph::new(wrapped_lines);
    frame.render_widget(paragraph, inner_area);
}
```

### 5.4 Handle Terminal Resize

Ratatui handles this automatically, but verify grid recalculation:

**File**: `src/tui/layout.rs`

No changes needed - layout is recalculated every frame based on current terminal size.

Test by resizing terminal during execution.

### 5.5 Handle Ctrl+C Gracefully

Ensure terminal is restored on interrupt:

**File**: `src/main.rs`

```rust
async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    // ... setup ...

    let mut terminal = otto::tui::init_terminal()?;

    // Set up Ctrl+C handler
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        let _ = shutdown_tx.send(()).await;
    });

    // Run TUI with shutdown signal
    let tui_result = tokio::select! {
        result = app.run(&mut terminal) => result,
        _ = shutdown_rx.recv() => {
            // Ctrl+C received, quit TUI
            Ok(())
        }
    };

    // Always restore terminal
    otto::tui::restore_terminal(&mut terminal)?;

    tui_result.map_err(|e| eyre::eyre!("TUI error: {}", e))?;

    // Note: scheduler tasks will be automatically cancelled when dropped
    Ok(())
}
```

### 5.6 Handle Zero Tasks

Edge case: `otto --tui` with no tasks in graph.

**File**: `src/main.rs`

```rust
async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    if tasks.is_empty() {
        eprintln!("No tasks to execute");
        return Ok(());
    }

    // ... rest ...
}
```

### 5.7 Handle Many Tasks (>16)

Current layout maxes out at 4x4 grid. For more tasks:

**File**: `src/tui/layout.rs`

Add pagination:

```rust
pub struct PaneLayout {
    panes: Vec<Box<dyn Pane>>,
    focused_index: usize,
    page: usize,  // NEW
    panes_per_page: usize,  // NEW
}

impl PaneLayout {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            focused_index: 0,
            page: 0,
            panes_per_page: 16,
        }
    }

    pub fn next_page(&mut self) {
        let total_pages = (self.panes.len() + self.panes_per_page - 1) / self.panes_per_page;
        if total_pages > 0 {
            self.page = (self.page + 1) % total_pages;
        }
    }

    pub fn prev_page(&mut self) {
        let total_pages = (self.panes.len() + self.panes_per_page - 1) / self.panes_per_page;
        if total_pages > 0 {
            self.page = if self.page == 0 {
                total_pages - 1
            } else {
                self.page - 1
            };
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let start_idx = self.page * self.panes_per_page;
        let end_idx = (start_idx + self.panes_per_page).min(self.panes.len());
        let visible_panes: Vec<&Box<dyn Pane>> = self.panes[start_idx..end_idx].iter().collect();

        // ... render only visible panes ...
    }
}
```

**File**: `src/tui/app.rs`

Add page navigation:

```rust
fn handle_key_event(&mut self, code: KeyCode) {
    match code {
        // ... existing ...

        KeyCode::PageDown => {
            self.layout.next_page();
        }
        KeyCode::PageUp => {
            self.layout.prev_page();
        }

        // ... rest ...
    }
}
```

### Testing Phase 5

**First, ensure baseline still passes:**
```bash
cargo test
./tests/baseline_verification.sh
```

If baseline fails, **STOP** and fix before continuing.

**Then test edge cases:**

Create stress tests:

```bash
# Test with many tasks
./target/debug/otto --tui task1 task2 ... task20

# Test terminal resize
./target/debug/otto --tui build
# Resize terminal window during execution

# Test Ctrl+C
./target/debug/otto --tui long-running-task
# Press Ctrl+C, verify terminal restores

# Test no TTY
./target/debug/otto --tui build | cat
# Should fall back to terminal output

# Test zero tasks
./target/debug/otto --tui
```

**Verification**:
- ✅ >16 tasks paginate correctly
- ✅ PgUp/PgDn cycle pages
- ✅ Terminal resize recalculates layout
- ✅ Ctrl+C restores terminal
- ✅ Non-TTY falls back gracefully
- ✅ Zero tasks handled
- ✅ Long lines wrap/truncate
- ✅ Fast-completing tasks show correct status

---

## Implementation Checklist

### Phase 0: Establish Baseline ⚠️ MUST DO FIRST ✅ COMPLETE
- [x] Run `cargo test` and document results
- [x] Create `tests/baseline_verification.sh` script (implemented as `otto baseline` task)
- [x] Run baseline verification and save results
- [x] Document current output formats
- [x] Commit baseline results to git
- [x] Create blocking issues for regression tracking

**✅ ALL BASELINE TESTS PASS**

### Phase 1: CLI Flag Integration ✅ COMPLETE
**After each step, run: `cargo test && ./tests/baseline_verification.sh`**

- [x] Add `--tui` flag to `otto_command()`
- [x] Parse flag in `Parser::parse()`
- [x] Update return signature to include `tui_mode`
- [x] Update `main.rs` to receive and pass flag
- [x] Add branching in `execute_tasks()`
- [x] Extract current logic to `execute_with_terminal_output()`
- [x] Create stub `execute_with_tui()` (fully implemented)
- [x] Add `atty` dependency
- [x] Test flag parsing and branching

### Phase 2: Basic TUI Infrastructure ✅ COMPLETE
**After each step, run: `cargo test && ./tests/baseline_verification.sh`**

- [x] Add ratatui and crossterm dependencies
- [x] Create `src/tui/` module structure
- [x] Implement terminal init/restore functions
- [x] Create `Pane` trait with `TaskPane` implementation
- [x] Implement `PaneLayout` with dynamic grid
- [x] Create `TuiApp` with event loop
- [x] Expose TUI module in `lib.rs`
- [x] Create and test `examples/tui-demo/` (better than tui_test.rs)

### Phase 3: Integrate with Task Execution ✅ COMPLETE
**After each step, run: `cargo test && ./tests/baseline_verification.sh`**

- [x] Add `tui_mode` field to `TaskScheduler`
- [x] Add `suppress_terminal` to `TeeWriter`
- [x] Update `TaskStreams::process_output` signature
- [x] Pass suppression flag through scheduler
- [x] Refactor scheduler to accept pre-created `TaskStreams`
- [x] Implement `execute_with_tui()` fully
- [x] Create TaskStreams before scheduler
- [x] Wire up broadcast channels to TaskPanes
- [x] Test end-to-end with real tasks

### Phase 4: Enhanced UX & Polish ✅ COMPLETE
**After each step, run: `cargo test && ./tests/baseline_verification.sh`**

- [x] Implement `TaskMessage` enum for status updates
- [x] Broadcast status changes from scheduler
- [x] Update TaskPane to handle status messages
- [x] Add fullscreen mode toggle
- [x] Implement status bar with help text
- [x] Add auto-scroll behavior
- [x] Add duration display to panes (hide completed toggle deferred)
- [x] Test all UX features

### Phase 5: Edge Cases & Robustness ✅ COMPLETE
**After each step, run: `cargo test && ./tests/baseline_verification.sh`**

- [x] Add TUI init failure fallback
- [x] Handle fast-completing tasks
- [x] Add line wrapping for long output
- [x] Verify terminal resize handling (automatic via Ratatui)
- [x] Add Ctrl+C signal handling with proper cleanup
- [x] Handle zero tasks case
- [x] Add pagination for >16 tasks (PgUp/PgDn navigation)
- [x] Comprehensive edge case testing
- [x] **BONUS**: Fixed critical test isolation bug in env variable resolution

---

## Testing Strategy

### Unit Tests

**File**: `src/tui/pane.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_status_symbols() {
        assert_eq!(PaneStatus::Pending.symbol(), "○");
        assert_eq!(PaneStatus::Running.symbol(), "●");
        assert_eq!(PaneStatus::Completed.symbol(), "✓");
    }

    #[test]
    fn test_output_buffer_ring() {
        let (tx, _) = broadcast::channel(10);
        let mut pane = TaskPane::new("test".to_string(), tx);
        pane.max_buffer_lines = 5;

        // Add more lines than buffer size
        for i in 0..10 {
            pane.output_buffer.push_back(format!("line {}", i));
            if pane.output_buffer.len() > pane.max_buffer_lines {
                pane.output_buffer.pop_front();
            }
        }

        assert_eq!(pane.output_buffer.len(), 5);
        assert_eq!(pane.output_buffer.front().unwrap(), "line 5");
    }
}
```

### Integration Tests

**File**: `tests/tui_integration_test.rs`

```rust
use otto::cli::Parser;
use std::env;

#[test]
fn test_tui_flag_parsing() {
    let args = vec![
        "otto".to_string(),
        "--tui".to_string(),
        "build".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (_, _, _, _, tui_mode) = parser.parse().unwrap();

    assert!(tui_mode);
}

#[test]
fn test_tui_short_flag() {
    let args = vec![
        "otto".to_string(),
        "-t".to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (_, _, _, _, tui_mode) = parser.parse().unwrap();

    assert!(tui_mode);
}
```

### Manual Test Cases

Create test ottofile:

**File**: `examples/tui-test/otto.yml`

```yaml
tasks:
  fast:
    action: |
      echo "Fast task"
      sleep 1
      echo "Done"

  slow:
    action: |
      for i in {1..10}; do
        echo "Slow task iteration $i"
        sleep 1
      done

  many-lines:
    action: |
      for i in {1..100}; do
        echo "Line $i: Lorem ipsum dolor sit amet consectetur adipiscing elit"
      done

  fails:
    action: |
      echo "This will fail"
      sleep 2
      exit 1
```

Test scenarios:

```bash
cd examples/tui-test

# Basic TUI
otto --tui fast

# Multiple tasks
otto --tui fast slow

# Many lines (scrolling)
otto --tui many-lines

# Task failure
otto --tui fails

# All tasks
otto --tui fast slow many-lines fails
```

---

## Known Limitations & Future Enhancements

### Current Limitations

1. **Max 16 visible panes** - Pagination required for more
2. **Fixed tick rate** - 100ms may be too slow for high-output tasks
3. **No output filtering** - Shows all stdout/stderr mixed
4. **No search** - Can't search within pane output
5. **No export** - Can't save TUI view to file (logs are saved though)

### Future Enhancements (Not in Scope)

- **Configurable layouts** - User-defined grid sizes
- **Output filtering** - Show only errors, or filter by regex
- **Search functionality** - Search within focused pane
- **Color support** - Preserve ANSI colors from task output
- **Follow mode toggle** - Per-pane auto-scroll control
- **Minimize/maximize panes** - Variable pane sizes
- **Mouse support** - Click to focus, scroll with mouse wheel
- **Watch mode** - Auto-rerun tasks on file changes + TUI

---

## Dependencies Reference

```toml
[dependencies]
ratatui = "0.29"
crossterm = "0.29"
atty = "0.2"
tokio = { version = "1", features = ["full"] }
eyre = "0.6"
log = "0.4"
# ... existing dependencies ...
```

---

## Architecture Diagrams

### Component Relationships

```
┌─────────────────────────────────────────────────────────────┐
│                        Otto Binary                           │
├─────────────────────────────────────────────────────────────┤
│                         main.rs                              │
│  ┌────────────┐  ┌──────────────────────┐  ┌─────────────┐ │
│  │ CLI Parser │─→│ execute_tasks()      │  │ TuiApp      │ │
│  │ (--tui)    │  │  ├─ Terminal mode    │  │ ├─ Layout   │ │
│  └────────────┘  │  └─ TUI mode         │  │ ├─ Panes    │ │
│                  └───────────┬───────────┘  │ └─ Events   │ │
│                              │              └─────────────┘ │
│                              ↓                               │
│                  ┌─────────────────────────┐                │
│                  │   TaskScheduler         │                │
│                  │  ├─ spawn tasks         │                │
│                  │  ├─ TaskStreams         │                │
│                  │  └─ broadcast::Sender   │                │
│                  └───────────┬─────────────┘                │
│                              │                               │
│                    ┌─────────┴──────────┐                   │
│                    ↓                    ↓                    │
│         ┌──────────────────┐  ┌──────────────────┐         │
│         │ TeeWriter        │  │ TaskPane         │         │
│         │ (terminal)       │  │ (TUI subscriber) │         │
│         └──────────────────┘  └──────────────────┘         │
└─────────────────────────────────────────────────────────────┘
```

---

## Reference Implementation Files

Key files from scan-new to reference:

- `src/tui/mod.rs` - Module structure, init/restore
- `src/tui/pane.rs` - Pane trait pattern
- `src/tui/layout.rs` - Grid layout calculations
- `src/tui/security.rs` - Example pane with scrolling
- `src/main.rs` - TUI vs non-TUI branching

---

## Completion Criteria

The TUI feature is complete when:

- ✅ `otto --tui <tasks>` launches TUI mode
- ✅ Default behavior without flag unchanged
- ✅ Each task displays in its own pane
- ✅ Output streams in real-time without `[task]` prefixes
- ✅ Status indicators (pending/running/complete/failed) work
- ✅ Navigation (Tab, arrows) works
- ✅ Scrolling works per-pane
- ✅ Fullscreen mode works
- ✅ Status bar shows help
- ✅ Terminal restores properly on exit/error
- ✅ Non-TTY environments fall back gracefully
- ✅ All edge cases handled (0 tasks, >16 tasks, fast tasks, etc.)
- ✅ File logs still written correctly
- ✅ Documentation updated
- ✅ Tests pass

---

## Notes for Future Maintainers

- **Why suppress terminal in TUI mode**: To avoid mixed output (TUI drawing + task output)
- **Why broadcast channels**: Allows multiple subscribers (file writer + TUI pane)
- **Why ring buffers**: Memory bound for long-running tasks with lots of output
- **Why TaskMessage enum**: Allows both output and status on same channel
- **Why graceful fallback**: CI/CD and non-interactive environments
- **Grid layout algorithm**: Dynamic based on task count, max 4x4 for readability

---

## Phase 6 (Optional): Multi-Page View for Active/Completed Tasks

### Overview

This optional enhancement adds a two-page system to separate active (running/pending) tasks from completed tasks, similar to virtual desktops in Ubuntu. This keeps the active view clean and focused while preserving the ability to review completed work.

### Motivation

**Problem**: As tasks complete, their panes clutter the active view, reducing space for currently running tasks.

**Solution**: Automatically move completed tasks to a separate "Completed" page, keeping the active view focused on work in progress.

### Visual Design

**Page 1 - Active Tasks**:
```
┌─────────────────┬─────────────────┬─────────────────┐
│ build [●] 5.2s  │ test [●] 3.1s   │ lint [○] waiting│
│ Compiling...    │ Running tests...│                 │
└─────────────────┴─────────────────┴─────────────────┘

[Active: 3] Completed: 7  |  Space: Switch | 1: Active | 2: Completed | q: Quit
```

**Page 2 - Completed Tasks**:
```
┌─────────────────┬─────────────────┬─────────────────┐
│ format [✓] 1.2s │ docs [✓] 2.8s  │ deps [✓] 0.5s  │
│ Formatted 42    │ Generated docs  │ Resolved deps   │
│ files           │ successfully    │ successfully    │
└─────────────────┴─────────────────┴─────────────────┘

Active: 3 [Completed: 7]  |  Space: Switch | 1: Active | 2: Completed | q: Quit
```

### User Experience

**Auto-switching behavior**:
- Tasks start on Active page (Pending → Running)
- When task completes/fails/skips, automatically moves to Completed page
- User can press `Space` or `p` to toggle between pages
- Press `1` to jump directly to Active page
- Press `2` to jump directly to Completed page
- Status bar shows counts and current page

**Benefits**:
1. Clean active view - maximizes screen space for running tasks
2. Preserve history - review completed tasks anytime
3. Better scalability - handles workflows with many tasks
4. Focus - see only what needs attention

### Implementation

#### 6.1 Add Page Enum and Fields

**File**: `src/tui/layout.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Active,
    Completed,
}

pub struct PaneLayout {
    active_panes: Vec<Box<dyn Pane>>,
    completed_panes: Vec<Box<dyn Pane>>,
    current_page: Page,
    focused_index: usize,
    max_completed_panes: usize,  // Cap history to prevent unbounded growth
}

impl PaneLayout {
    pub fn new() -> Self {
        Self {
            active_panes: Vec::new(),
            completed_panes: Vec::new(),
            current_page: Page::Active,
            focused_index: 0,
            max_completed_panes: 50,  // Keep last 50 completed tasks
        }
    }

    pub fn active_count(&self) -> usize {
        self.active_panes.len()
    }

    pub fn completed_count(&self) -> usize {
        self.completed_panes.len()
    }

    pub fn current_page(&self) -> Page {
        self.current_page
    }
}
```

#### 6.2 Page Switching Logic

**File**: `src/tui/layout.rs`

```rust
impl PaneLayout {
    pub fn switch_page(&mut self) {
        self.current_page = match self.current_page {
            Page::Active => Page::Completed,
            Page::Completed => Page::Active,
        };
        self.focused_index = 0;  // Reset focus when switching pages
    }

    pub fn goto_page(&mut self, page: Page) {
        self.current_page = page;
        self.focused_index = 0;
    }
}
```

#### 6.3 Auto-Move Completed Tasks

**File**: `src/tui/layout.rs`

```rust
impl PaneLayout {
    pub fn update_all(&mut self) {
        // Update panes on current page
        let panes = match self.current_page {
            Page::Active => &mut self.active_panes,
            Page::Completed => &mut self.completed_panes,
        };

        for pane in panes.iter_mut() {
            pane.update();
        }

        // Move finished tasks from active to completed
        let mut to_move_indices = Vec::new();

        for (idx, pane) in self.active_panes.iter().enumerate() {
            match pane.status() {
                PaneStatus::Completed | PaneStatus::Failed | PaneStatus::Skipped => {
                    to_move_indices.push(idx);
                }
                _ => {}
            }
        }

        // Remove from active and add to completed (in reverse to preserve indices)
        for idx in to_move_indices.into_iter().rev() {
            let pane = self.active_panes.remove(idx);
            self.completed_panes.push(pane);

            // Cap completed history
            if self.completed_panes.len() > self.max_completed_panes {
                self.completed_panes.remove(0);
            }
        }

        // Adjust focus if needed
        if self.focused_index >= self.active_panes.len() && !self.active_panes.is_empty() {
            self.focused_index = self.active_panes.len() - 1;
        }
    }
}
```

#### 6.4 Render Current Page

**File**: `src/tui/layout.rs`

```rust
impl PaneLayout {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let panes = match self.current_page {
            Page::Active => &self.active_panes,
            Page::Completed => &self.completed_panes,
        };

        if panes.is_empty() {
            self.render_empty_page(frame, area);
            return;
        }

        // Calculate grid and render panes
        let grid_areas = self.calculate_grid(area, panes.len());

        for (i, pane) in panes.iter().enumerate() {
            if let Some(pane_area) = grid_areas.get(i) {
                let focused = i == self.focused_index;
                pane.render(frame, *pane_area, focused);
            }
        }
    }

    fn render_empty_page(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::Paragraph;
        use ratatui::layout::Alignment;

        let message = match self.current_page {
            Page::Active => "No active tasks",
            Page::Completed => "No completed tasks yet",
        };

        let paragraph = Paragraph::new(message)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);

        // Center vertically
        let vertical_center = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Length(1),
                Constraint::Percentage(60),
            ])
            .split(area);

        frame.render_widget(paragraph, vertical_center[1]);
    }
}
```

#### 6.5 Add Pane to Correct Page

**File**: `src/tui/layout.rs`

```rust
impl PaneLayout {
    pub fn add_pane(&mut self, pane: Box<dyn Pane>) {
        // New tasks always start on active page
        self.active_panes.push(pane);
    }
}
```

#### 6.6 Update Status Bar with Page Indicator

**File**: `src/tui/app.rs`

```rust
fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
    let active_count = self.layout.active_count();
    let completed_count = self.layout.completed_count();
    let current_page = self.layout.current_page();

    // Format page indicator with current page highlighted
    let page_indicator = match current_page {
        Page::Active => {
            format!("[Active: {}] Completed: {}", active_count, completed_count)
        }
        Page::Completed => {
            format!("Active: {} [Completed: {}]", active_count, completed_count)
        }
    };

    let help_text = if self.fullscreen_pane.is_some() {
        "ESC: Exit fullscreen  q: Quit  ↑↓: Scroll"
    } else {
        "Space: Switch Page | 1: Active | 2: Completed | Tab: Next | Enter: Fullscreen | q: Quit"
    };

    let status_text = format!("{}  |  {}", page_indicator, help_text);

    let paragraph = Paragraph::new(Span::styled(
        status_text,
        Style::default().fg(Color::DarkGray)
    ));

    frame.render_widget(paragraph, area);
}
```

#### 6.7 Add Keyboard Shortcuts

**File**: `src/tui/app.rs`

```rust
impl TuiApp {
    fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            // ... existing keys ...

            KeyCode::Char(' ') | KeyCode::Char('p') => {
                // Toggle between Active and Completed pages
                self.layout.switch_page();
            }

            KeyCode::Char('1') => {
                // Jump to Active page
                self.layout.goto_page(Page::Active);
            }

            KeyCode::Char('2') => {
                // Jump to Completed page
                self.layout.goto_page(Page::Completed);
            }

            // ... rest ...
        }
    }
}
```

### Configuration Options

Optional: Allow users to configure behavior via environment variables or config file:

```rust
// In PaneLayout::new()
let max_completed_panes = std::env::var("OTTO_TUI_MAX_HISTORY")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(50);

let auto_switch_to_completed = std::env::var("OTTO_TUI_AUTO_SWITCH")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(false);  // Don't auto-switch page when viewing active
```

### Testing

**File**: `examples/tui-test-pages/otto.yml`

```yaml
tasks:
  quick-1:
    action: |
      echo "Quick task 1"
      sleep 1

  quick-2:
    action: |
      echo "Quick task 2"
      sleep 1

  quick-3:
    action: |
      echo "Quick task 3"
      sleep 1

  slow-1:
    action: |
      for i in {1..10}; do
        echo "Slow task 1: iteration $i"
        sleep 1
      done

  slow-2:
    action: |
      for i in {1..10}; do
        echo "Slow task 2: iteration $i"
        sleep 1
      done
```

**Test scenarios**:

```bash
cd examples/tui-test-pages

# Test page switching
otto --tui quick-1 quick-2 quick-3 slow-1 slow-2

# Expected behavior:
# 1. All 5 tasks start on Active page
# 2. quick-1, quick-2, quick-3 complete quickly, move to Completed
# 3. Active page shows only slow-1 and slow-2
# 4. Press Space to see Completed page with quick tasks
# 5. Press Space again to return to Active page
# 6. Press 1 or 2 to jump directly to pages
```

### Verification Checklist

- [ ] Tasks start on Active page
- [ ] Completed tasks auto-move to Completed page
- [ ] Space toggles between pages
- [ ] `1` jumps to Active page
- [ ] `2` jumps to Completed page
- [ ] Status bar shows correct counts
- [ ] Current page highlighted in status bar
- [ ] Empty page shows appropriate message
- [ ] Focus resets when switching pages
- [ ] History capped at configured max (default 50)
- [ ] Tab/navigation works on both pages
- [ ] Fullscreen works on both pages

### Alternative Designs Considered

#### Three-Page System
```
Page 1: Active (Running + Pending)
Page 2: Completed (Successful only)
Page 3: Failed (Failed + Skipped)
```

**Trade-off**: More granular but adds complexity. Consider if workflow has many failures.

#### Split-Screen View
```
┌─ ACTIVE ──────────┬─ COMPLETED ──┐
│ Running tasks     │ Done tasks   │
└───────────────────┴──────────────┘
```

**Trade-off**: See both at once but less space per pane. Better for small task counts.

#### Auto-Hide Completed
Simple filter on single page instead of separate pages.

**Trade-off**: Simpler but completed tasks disappear completely (can't review).

### When to Implement

Implement this phase **after** core TUI functionality is stable (Phases 1-5 complete). This is an enhancement that improves usability for workflows with many tasks but isn't required for basic functionality.

**Recommended timing**:
- After Phase 5 is fully tested
- When user feedback indicates need for better completed task management
- Before tackling >16 task pagination (Phase 5.7) - pages may be a better solution

### Integration with Existing Phases

**Phase 5.7 (>16 tasks pagination)**: If implementing pages, you may not need pagination on Active view if tasks are automatically moved to Completed. However, Completed page may still need pagination for long-running workflows.

**Phase 4.5 (hide completed toggle)**: This becomes redundant with pages - remove or repurpose as "show/hide pending tasks" on Active page.

---

## Summary: Success Criteria

The TUI feature implementation will be considered **successful** when ALL of the following are true:

### 1. Baseline Tests Pass ✅
- [ ] ALL existing tests pass: `cargo test`
- [ ] Baseline verification script passes: `./tests/baseline_verification.sh`
- [ ] No new compiler warnings introduced
- [ ] No performance regressions in default mode

### 2. Default Behavior Unchanged ✅
- [ ] Without `--tui` flag, behavior is **identical** to pre-TUI implementation
- [ ] Output format matches baseline (byte-for-byte when possible)
- [ ] File creation, timing, and error handling unchanged
- [ ] All existing examples work exactly as before

### 3. TUI Features Work ✅
- [ ] `otto --tui <tasks>` launches TUI successfully
- [ ] Real-time output streaming works
- [ ] Status indicators work (pending/running/completed/failed)
- [ ] Navigation works (Tab, arrows, scrolling)
- [ ] Fullscreen mode works
- [ ] Clean exit restores terminal
- [ ] Graceful fallback when no TTY

### 4. Code Quality ✅
- [ ] TUI code isolated in `src/tui/` module
- [ ] No TUI-specific logic in existing execution paths
- [ ] Clean separation via branching in `main.rs`
- [ ] Optional parameters have sensible defaults
- [ ] Comprehensive error handling

### 5. Documentation ✅
- [ ] README updated with `--tui` flag
- [ ] Help text includes TUI option
- [ ] Examples demonstrate TUI usage
- [ ] Migration notes (none needed - purely additive)

---

## Final Pre-Merge Checklist

Before merging TUI feature to main:

- [ ] Run full test suite on clean branch: `cargo test --all`
- [ ] Run baseline verification: `./tests/baseline_verification.sh`
- [ ] Manual smoke tests on 5+ different otto.yml examples
- [ ] Test in CI environment (non-TTY fallback)
- [ ] Test on different terminal emulators (if possible)
- [ ] Code review by maintainer
- [ ] Performance benchmarks show no regression in default mode
- [ ] Git history is clean (no "fix regression" commits - those should be squashed)

**If ANY item fails, do NOT merge. Fix and re-test.**

---

End of implementation plan.
