# TUI Demo Examples

This directory contains tasks specifically designed to showcase Otto's TUI mode.

## Quick Start

```bash
# Run multiple parallel tasks with TUI
otto --tui all-parallel

# Run individual tasks to see different behaviors
otto --tui fast-task
otto --tui verbose-task
otto --tui failing-task
```

## Task Descriptions

### Parallel Tasks (run simultaneously)
- **fast-task**: Completes in ~2.5 seconds with minimal output
- **medium-task**: Takes ~10 seconds with moderate output
- **slow-task**: Takes ~20 seconds with steady output
- **verbose-task**: Produces lots of output to test scrolling and auto-scroll behavior

### Special Tasks
- **failing-task**: Deliberately fails to demonstrate error handling
- **dependent-task**: Runs sequentially after fast-task (shows dependencies)
- **all-parallel**: Meta-task that waits for all parallel tasks

## TUI Features to Observe

### Grid Layout
```bash
otto --tui all-parallel
```
Watch 4 tasks run in parallel in a 2x2 grid layout.

### Status Symbols
- `○` Pending (gray)
- `●` Running (green)
- `✓` Completed (green)
- `✗` Failed (red)
- `⊘` Skipped (yellow)

### Keyboard Navigation
- `Tab` / `→` : Next pane
- `Shift+Tab` / `←` : Previous pane
- `f` / `Enter` : Toggle fullscreen
- `↑` / `k` : Scroll up
- `↓` / `j` : Scroll down
- `Home` : Scroll to top
- `End` / `G` : Jump to the newest output and follow it again
- `q` / `Esc` : Quit

### Duration Tracking
Each pane shows live elapsed time while running and final duration when complete.

### Auto-Scroll
- Automatically scrolls to show latest output
- Disabled when you manually scroll up (the pane title says `[scroll paused]`)
- Re-enabled when you scroll to the bottom with `↓`/`j`, or in one jump with `End`/`G`

### Fullscreen Mode
Press `f` or `Enter` to view focused pane in fullscreen. Press again to exit.

## Example Commands

```bash
# See all parallel execution
otto --tui all-parallel

# Test fast completion
otto --tui fast-task

# Test scrolling with lots of output
otto --tui verbose-task

# See error handling
otto --tui failing-task

# Run just the working tasks
otto --tui fast-task medium-task slow-task

# Test sequential execution
otto --tui dependent-task
```

## What to Look For

1. **Real-time updates**: Status changes from pending → running → completed
2. **Live duration**: Watch the timer update every 100ms
3. **Output streaming**: See logs appear in real-time
4. **Auto-scroll**: Panes automatically scroll to show new output
5. **Border colors**: Yellow when focused, status color otherwise
6. **Status bar**: Context-sensitive help at the bottom
7. **Grid layout**: Automatic layout based on number of tasks
