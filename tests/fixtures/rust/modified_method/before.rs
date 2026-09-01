pub struct Counter {
    value: u32,
}

impl Counter {
    pub fn new() -> Self {
        Counter { value: 0 }
    }

    pub fn bump(&mut self) {
        self.value += 1;
    }
}
