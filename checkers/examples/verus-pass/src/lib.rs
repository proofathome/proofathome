use vstd::prelude::*;

pub mod spec;

verus! {

pub fn max(a: u64, b: u64) -> (r: u64)
    ensures
        spec::is_max(a, b, r),
{
    if a >= b {
        a
    } else {
        b
    }
}

} // verus!
