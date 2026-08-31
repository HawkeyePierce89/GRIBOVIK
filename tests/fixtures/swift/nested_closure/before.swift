func total(values: [Int]) -> Int {
    func double(_ n: Int) -> Int {
        return n * 2
    }
    let doubled = values.map { double($0) }
    return doubled.reduce(0, +)
}
