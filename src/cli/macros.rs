#![allow(unused_macros)]
macro_rules! str_tuple2 {
    ($app:ident, $value:ident, $method:ident) => {{
        if let Some(vec) = $value.as_vec() {
            for ys in vec {
                if let Some(tup) = ys.as_vec() {
                    debug_assert_eq!(2, tup.len());
                    $app = $app.$method(str_str!(tup[0]), str_str!(tup[1]));
                } else {
                    panic!("Failed to convert YAML value to vec");
                }
            }
        } else {
            panic!("Failed to convert YAML value to vec");
        }
        $app
    }};
}

macro_rules! str_tuple3 {
    ($app:ident, $value:ident, $method:ident) => {{
        if let Some(vec) = $value.as_vec() {
            for ys in vec {
                if let Some(tup) = ys.as_vec() {
                    debug_assert_eq!(3, tup.len());
                    $app = $app.$method(str_str!(tup[0]), str_opt_str!(tup[1]), str_opt_str!(tup[2]));
                } else {
                    panic!("Failed to convert YAML value to vec");
                }
            }
        } else {
            panic!("Failed to convert YAML value to vec");
        }
        $app
    }};
}

macro_rules! str_vec_or_str {
    ($app:ident, $value:ident, $method:ident) => {{
        let maybe_vec = $value.as_vec();
        if let Some(vec) = maybe_vec {
            for ys in vec {
                if let Some(s) = ys.as_str() {
                    $app = $app.$method(s);
                } else {
                    panic!("Failed to convert YAML value {:?} to a string", ys);
                }
            }
        } else {
            if let Some(s) = $value.as_str() {
                $app = $app.$method(s);
            } else {
                panic!("Failed to convert YAML value {:?} to either a vec or string", $value);
            }
        }
        $app
    }};
}

macro_rules! str_vec {
    ($app:ident, $value:ident, $method:ident) => {{
        let maybe_vec = $value.as_vec();
        if let Some(vec) = maybe_vec {
            let content = vec.into_iter().map(|ys| {
                if let Some(s) = ys.as_str() {
                    s
                } else {
                    panic!("Failed to convert YAML value {:?} to a string", ys);
                }
            });
            $app = $app.$method(content)
        } else {
            panic!("Failed to convert YAML value {:?} to a vec", $value);
        }
        $app
    }};
}

macro_rules! str_opt_str {
    ($value:expr) => {{
        if !$value.is_null() {
            Some(
                $value
                    .as_str()
                    .unwrap_or_else(|| panic!("failed to convert YAML {:?} value to a string", $value)),
            )
        } else {
            None
        }
    }};
}

macro_rules! str_char {
    ($value:expr) => {{
        $value
            .as_str()
            .unwrap_or_else(|| panic!("failed to convert YAML {:?} value to a string", $value))
            .chars()
            .next()
            .unwrap_or_else(|| panic!("Expected char"))
    }};
}

macro_rules! str_str {
    ($value:expr) => {{
        $value
            .as_str()
            .unwrap_or_else(|| panic!("failed to convert YAML {:?} value to a string", $value))
    }};
}

macro_rules! str_to_char {
    ($app:ident, $value:ident, $method:ident) => {{
        $app.$method(str_char!($value))
    }};
}

macro_rules! str_to_str {
    ($app:ident, $value:ident, $method:ident) => {{
        $app.$method(str_str!($value))
    }};
}

macro_rules! str_to_bool {
    ($app:ident, $value:ident, $method:ident) => {{
        $app.$method(
            $value
                .as_bool()
                .unwrap_or_else(|| panic!("failed to convert YAML {:?} value to a string", $value)),
        )
    }};
}

macro_rules! str_to_usize {
    ($app:ident, $value:ident, $method:ident) => {{
        $app.$method(
            $value
                .as_i64()
                .unwrap_or_else(|| panic!("failed to convert YAML {:?} value to a string", $value))
                as usize,
        )
    }};
}

macro_rules! vec_of_strings {
    ($($x:expr),* $(,)?) => {{
        let mut temp_vec = Vec::new();
        $(
            temp_vec.push(String::from($x));
        )*
        temp_vec
    }}
}
