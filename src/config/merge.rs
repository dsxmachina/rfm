use toml::Value;

/// Recursively merge `user` over `base`. Tables merge key-by-key; any
/// non-table value (scalars AND arrays) replaces the base value wholesale.
pub fn deep_merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Table(b), Value::Table(u)) => {
            for (k, uv) in u {
                match b.get_mut(&k) {
                    Some(bv) => deep_merge(bv, uv),
                    None => {
                        b.insert(k, uv);
                    }
                }
            }
        }
        (b, u) => *b = u,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        s.parse::<Value>().unwrap()
    }

    #[test]
    fn scalar_replaces() {
        let mut base = v("a = 1\nb = 2");
        deep_merge(&mut base, v("a = 9"));
        assert_eq!(base, v("a = 9\nb = 2"));
    }

    #[test]
    fn tables_merge_recursively() {
        let mut base = v("[t]\nx = 1\ny = 2");
        deep_merge(&mut base, v("[t]\ny = 9"));
        assert_eq!(base, v("[t]\nx = 1\ny = 9"));
    }

    #[test]
    fn arrays_replace_wholesale() {
        // jump_to semantics: a user list REPLACES the default list
        let mut base = v("a = [1, 2, 3]");
        deep_merge(&mut base, v("a = [9]"));
        assert_eq!(base, v("a = [9]"));
    }

    #[test]
    fn empty_array_survives_merge() {
        // `undo = []` (unbind) must not be treated as "absent"
        let mut base = v("undo = [\"u\"]");
        deep_merge(&mut base, v("undo = []"));
        assert_eq!(base, v("undo = []"));
    }

    #[test]
    fn user_only_keys_are_kept() {
        // unknown keys survive the merge; they are warned about separately
        let mut base = v("a = 1");
        deep_merge(&mut base, v("zz = 5"));
        assert_eq!(base, v("a = 1\nzz = 5"));
    }

    #[test]
    fn type_mismatch_user_wins() {
        // user writes a scalar where a table is expected: user wins here,
        // the typed deserialize reports it with a precise path later
        let mut base = v("[t]\nx = 1");
        deep_merge(&mut base, v("t = 3"));
        assert_eq!(base, v("t = 3"));
    }
}
