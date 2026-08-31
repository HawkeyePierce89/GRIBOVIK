class Counter {
    var value: Int = 0

    init(value: Int) {
        self.value = value
    }

    func bump() {
        value += 1
    }
}
