// lib.rs

pub mod cfg;
#[macro_use]
pub mod cli;
pub mod cmd;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
