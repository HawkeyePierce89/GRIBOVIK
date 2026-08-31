func greet(name: String) -> String {
    return "hi \(name)"
}

/// Greets everyone in turn.
/// One line per name.
public func greetAll(names: [String]) -> [String] {
    return names.map { greet(name: $0) }
}
