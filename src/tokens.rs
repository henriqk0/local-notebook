//! Character-based token estimate used for budgeting.

pub const CHARS_PER_TOKEN: usize = 4;

/// Conservative estimate: one token per four characters, rounded up.
pub fn estimate(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up() {
        assert_eq!(estimate(""), 0);
        assert_eq!(estimate("a"), 1);
        assert_eq!(estimate("abcd"), 1);
        assert_eq!(estimate("abcde"), 2);
        assert_eq!(estimate("ééééé"), 2); // chars, not bytes
    }
}
