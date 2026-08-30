use colored::{Color, Colorize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// All 15 possible color combinations (bracket_color, text_color) where bracket ≠ text
/// This gives us 15 unique visual patterns before cycling
/// Ordered to ensure good bracket color distribution for the first several tasks
const COLOR_COMBINATIONS: [(Color, Color); 15] = [
    (Color::BrightRed, Color::BrightGreen),      // 0 - Red brackets
    (Color::BrightBlue, Color::BrightYellow),    // 1 - Blue brackets
    (Color::BrightGreen, Color::BrightBlue),     // 2 - Green brackets
    (Color::BrightYellow, Color::BrightCyan),    // 3 - Yellow brackets
    (Color::BrightCyan, Color::BrightMagenta),   // 4 - Cyan brackets
    (Color::BrightRed, Color::BrightBlue),       // 5 - Red brackets
    (Color::BrightBlue, Color::BrightCyan),      // 6 - Blue brackets
    (Color::BrightGreen, Color::BrightYellow),   // 7 - Green brackets
    (Color::BrightYellow, Color::BrightMagenta), // 8 - Yellow brackets
    (Color::BrightRed, Color::BrightYellow),     // 9 - Red brackets
    (Color::BrightBlue, Color::BrightMagenta),   // 10 - Blue brackets
    (Color::BrightGreen, Color::BrightCyan),     // 11 - Green brackets
    (Color::BrightRed, Color::BrightCyan),       // 12 - Red brackets
    (Color::BrightGreen, Color::BrightMagenta),  // 13 - Green brackets
    (Color::BrightRed, Color::BrightMagenta),    // 14 - Red brackets
];

/// Global task ordering context for consistent color assignment
static TASK_ORDER: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

pub fn set_global_task_order(task_names: Vec<String>) {
    let mut sorted_names = task_names;
    sorted_names.sort();
    let task_order = TASK_ORDER.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut order) = task_order.lock() {
        *order = sorted_names;
    }
}

pub fn get_task_color_combination(task_name: &str) -> (Color, Color) {
    if let Some(task_order) = TASK_ORDER.get()
        && let Ok(order) = task_order.lock()
        && let Some(position) = order.iter().position(|name| name == task_name)
    {
        return COLOR_COMBINATIONS[position % COLOR_COMBINATIONS.len()];
    }

    // Fallback to hash-based assignment
    let mut hasher = DefaultHasher::new();
    task_name.hash(&mut hasher);
    let hash = hasher.finish();
    COLOR_COMBINATIONS[(hash as usize) % COLOR_COMBINATIONS.len()]
}

/// Get a consistent color for a task name using alphabetical ordering (legacy function for backwards compatibility)
pub fn get_task_color(task_name: &str) -> Color {
    // Return just the bracket color for backwards compatibility
    get_task_color_combination(task_name).0
}

/// Format a task name with its assigned color
pub fn colorize_task_name(task_name: &str) -> String {
    if colored::control::SHOULD_COLORIZE.should_colorize() {
        task_name.color(get_task_color(task_name)).to_string()
    } else {
        task_name.to_string()
    }
}

/// Format a task prefix (e.g., "\[task_name\]") with two-color system: colored brackets + colored text
pub fn colorize_task_prefix(task_name: &str) -> String {
    if colored::control::SHOULD_COLORIZE.should_colorize() {
        let (bracket_color, text_color) = get_task_color_combination(task_name);
        format!(
            "{}{}{}",
            "[".color(bracket_color),
            task_name.color(text_color),
            "]".color(bracket_color)
        )
    } else {
        format!("[{task_name}]")
    }
}

#[path = "colors_tests.rs"]
mod tests;
