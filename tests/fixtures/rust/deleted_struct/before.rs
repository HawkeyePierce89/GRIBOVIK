pub struct Legacy {
    pub id: u32,
}

impl Legacy {
    pub fn id(&self) -> u32 {
        self.id
    }
}

pub fn keep() -> u32 {
    Legacy { id: 1 }.id()
}
