/// Adds without overflowing.
pub fn add_saturating(a: u8, b: u8) -> u8 {
    a.saturating_add(b)
}

#[cfg(kani)]
mod proofs {
    use super::add_saturating;

    /// Holds: the sum never drops below `a`.
    #[kani::proof]
    fn never_smaller() {
        let a: u8 = kani::any();
        let b: u8 = kani::any();
        assert!(add_saturating(a, b) >= a);
    }

    /// Fails on purpose: with `b == 0` the sum equals `a`.
    #[kani::proof]
    fn always_larger() {
        let a: u8 = kani::any();
        let b: u8 = kani::any();
        assert!(add_saturating(a, b) > a);
    }
}
