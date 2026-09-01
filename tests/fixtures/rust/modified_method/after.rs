pub struct Counter {
    value: u32,
}

impl Counter {
    pub fn new() -> Self {
        Counter { value: 0 }
    }

    pub fn bump(&mut self) {
        self.value = step(self.value);
        self.log();
    }

    fn log(&self) {}
}

fn step(value: u32) -> u32 {
    value + 1
}
