class Counter {
    var value: Int = 0

    init(value: Int) {
        self.value = value
    }

    func bump() {
        value = step(value)
        self.log()
    }

    private func log() {}
}

func step(_ value: Int) -> Int {
    return value + 1
}
