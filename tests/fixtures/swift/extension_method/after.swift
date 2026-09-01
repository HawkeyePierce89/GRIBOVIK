struct Point {
    var x: Int
    var y: Int
}

extension Point {
    func magnitude() -> Int {
        return abs(x) + abs(y)
    }

    func moved(by delta: Int) -> Point {
        return Point(x: x + delta, y: y + delta)
    }
}
