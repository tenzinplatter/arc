pub fn step_wrapping<Id: PartialEq + Copy>(ids: &[Id], current: Id, forward: bool) -> Option<Id> {
    let pos = ids.iter().position(|&x| x == current)?;
    let len = ids.len();
    let next = if forward {
        (pos + 1) % len
    } else if pos == 0 {
        len - 1
    } else {
        pos - 1
    };
    Some(ids[next])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_both_ways_and_is_none_when_current_missing() {
        let ids = [10, 20, 30];
        assert_eq!(step_wrapping(&ids, 10, true), Some(20));
        assert_eq!(step_wrapping(&ids, 30, true), Some(10));
        assert_eq!(step_wrapping(&ids, 10, false), Some(30));
        assert_eq!(step_wrapping(&ids, 20, false), Some(10));
        assert_eq!(step_wrapping(&ids, 99, true), None);
        assert_eq!(step_wrapping::<i32>(&[], 1, true), None);
    }
}
