use vstd::prelude::*;

verus! {

pub fn max(a: u64, b: u64) -> (r: u64)
    ensures
        r >= a && r >= b && (r == a || r == b),
{
    if a >= b { a } else { b }
}

} // verus!
