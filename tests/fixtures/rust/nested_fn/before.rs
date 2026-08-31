pub fn total(items: &[u32]) -> u32 {
    fn double(n: u32) -> u32 {
        n * 2
    }

    items.iter().map(|n| double(*n)).sum()
}
