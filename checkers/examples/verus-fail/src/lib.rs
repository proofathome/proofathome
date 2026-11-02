use vstd::prelude::*;

pub mod spec;

verus! {

// Wrong on purpose: returns `a` even when `b` is larger, so the `ensures`
// does not hold and Verus must reject it.
pub fn max(a: u64, b: u64) -> (r: u64)
    ensures
        spec::is_max(a, b, r),
{
    a
}

} // verus!
