pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

/// Greets everyone, in order.
#[must_use]
pub fn greet_all(names: &[String]) -> Vec<String> {
    names.iter().map(|name| greet(name)).collect()
}
