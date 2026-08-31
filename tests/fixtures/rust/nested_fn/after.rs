pub fn total(items: &[u32]) -> u32 {
    fn triple(n: u32) -> u32 {
        n * 3
    }

    items.iter().map(|n| triple(*n)).sum()
}
