struct Point {
    var x: Int
    var y: Int
}

extension Point {
    func magnitude() -> Int {
        return x + y
    }
}
