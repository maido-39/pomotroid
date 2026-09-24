//! Picking things at random without the repeats a person notices.
//!
//! Two features need this — the background folder and the sound pools — and
//! both need the same unobvious bit: uniform random *feels* broken. With five
//! items, one switch in five shows what is already there, and the user reads
//! that as "it didn't work". So both avoid the immediate repeat, and the
//! background walks a permutation so every picture is seen before any returns.
//!
//! No `rand` dependency. Entropy comes the same way the pairing token gets it,
//! from a `RandomState` hasher, because which picture or which chime comes next
//! is not a security question. Keeping this out of the shared dependency table
//! also keeps the Android build honest.

/// Seed a generator. Never zero, which xorshift cannot leave.
pub fn seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    h.finish() | 1
}

/// Advance `state` and return the new value. xorshift64.
pub fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// An order to walk `len` items in, shuffled or left alone.
///
/// `avoid_first` is the item that was showing when the previous order ran out.
/// Keeping it out of the first slot stops a walk showing the same thing twice
/// across the seam.
pub fn permutation(len: usize, shuffle: bool, avoid_first: Option<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if !shuffle || len < 2 {
        return order;
    }
    let mut state = seed();
    for i in (1..order.len()).rev() {
        let j = (next(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    if let Some(avoid) = avoid_first {
        if order[0] == avoid {
            let swap_with = 1 + (next(&mut state) % (order.len() as u64 - 1)) as usize;
            order.swap(0, swap_with);
        }
    }
    order
}

/// One of `len` items, never `avoid` unless it is the only one.
///
/// Used where a full pass is not the point — an alert only has to not repeat
/// back to back.
pub fn pick(len: usize, avoid: Option<usize>) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if len == 1 {
        return Some(0);
    }
    let mut state = seed();
    match avoid.filter(|&a| a < len) {
        // Pick among the others, then map back past the avoided index. This
        // gives every other item an equal chance, which rejection sampling in
        // a loop would only approximate.
        Some(avoid) => {
            let k = (next(&mut state) % (len as u64 - 1)) as usize;
            Some(if k >= avoid { k + 1 } else { k })
        }
        None => Some((next(&mut state) % len as u64) as usize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_order_is_left_alone() {
        assert_eq!(permutation(4, false, None), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_shuffled_order_holds_every_item_once() {
        let order = permutation(12, true, None);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn a_shuffled_order_does_not_open_on_the_item_that_just_finished() {
        for _ in 0..200 {
            assert_ne!(permutation(6, true, Some(3))[0], 3);
        }
    }

    #[test]
    fn one_item_is_always_itself() {
        assert_eq!(permutation(1, true, Some(0)), vec![0]);
        assert_eq!(pick(1, Some(0)), Some(0), "with one sound, repeating it is the only option");
    }

    #[test]
    fn nothing_to_pick_from() {
        assert_eq!(pick(0, None), None);
    }

    #[test]
    fn pick_never_returns_the_one_to_avoid() {
        for _ in 0..500 {
            let got = pick(4, Some(2)).unwrap();
            assert!(got < 4);
            assert_ne!(got, 2);
        }
    }

    #[test]
    fn pick_reaches_every_other_item() {
        // The mapping past the avoided index must not leave a hole: with four
        // items and one avoided, the other three all have to be reachable.
        let mut seen = [false; 4];
        for _ in 0..2000 {
            seen[pick(4, Some(1)).unwrap()] = true;
        }
        assert_eq!(seen, [true, false, true, true]);
    }

    #[test]
    fn pick_is_in_range_without_an_avoid() {
        for _ in 0..500 {
            assert!(pick(3, None).unwrap() < 3);
        }
    }
}
