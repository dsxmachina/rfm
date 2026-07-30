use toml::map::Entry;
use toml::Value;

/// Recursively merge `user` over `base`. Tables merge key-by-key; any
/// non-table value (scalars AND arrays) replaces the base value wholesale.
/// On type mismatch (e.g. user writes a scalar where the default has a
/// table) the user value wins; the typed deserialize downstream reports
/// it with a precise path.
pub fn deep_merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Table(b), Value::Table(u)) => {
            for (k, uv) in u {
                match b.entry(k) {
                    Entry::Occupied(mut e) => deep_merge(e.get_mut(), uv),
                    Entry::Vacant(e) => {
                        e.insert(uv);
                    }
                }
            }
        }
        (b, u) => *b = u,
    }
}

/// Dotted paths of user keys that do not exist in the defaults tree.
/// Subtrees whose top-level key is in WILDCARD_TABLES accept arbitrary
/// keys ([styles.*], [commands.*], [open.*]) and are not checked.
/// Type mismatches (key exists in defaults with a different shape) are
/// NOT reported here; the typed deserialize catches those later.
pub fn unknown_keys(defaults: &Value, user: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let (Value::Table(d), Value::Table(u)) = (defaults, user) {
        collect_unknown(d, u, "", true, &mut out);
    }
    out
}

const WILDCARD_TABLES: &[&str] = &["styles", "commands", "open"];

fn collect_unknown(
    defaults: &toml::map::Map<String, Value>,
    user: &toml::map::Map<String, Value>,
    prefix: &str,
    top_level: bool,
    out: &mut Vec<String>,
) {
    for (key, user_val) in user {
        if top_level && WILDCARD_TABLES.contains(&key.as_str()) {
            continue;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match defaults.get(key) {
            None => out.push(path),
            // recurse only through table/table pairs; a type mismatch is
            // neither unknown nor our business here
            Some(Value::Table(d)) => {
                if let Value::Table(u) = user_val {
                    collect_unknown(d, u, &path, false, out);
                }
            }
            Some(_) => {}
        }
    }
}

/// The minimal tree such that deep_merge(defaults, diff) == effective.
/// Returns None when effective adds nothing over defaults.
/// The minimality/round-trip guarantee assumes `effective` was produced
/// by `deep_merge(defaults, _)` — i.e. it is a recursive key-superset of
/// defaults; an `effective` missing default keys is out of contract.
pub fn diff_from_defaults(defaults: &Value, effective: &Value) -> Option<Value> {
    match (defaults, effective) {
        (Value::Table(d), Value::Table(e)) => {
            let mut out = toml::map::Map::new();
            for (key, eff_val) in e {
                match d.get(key) {
                    Some(def_val) => {
                        if let Some(diff) = diff_from_defaults(def_val, eff_val) {
                            out.insert(key.clone(), diff);
                        }
                    }
                    None => {
                        out.insert(key.clone(), eff_val.clone());
                    }
                }
            }
            (!out.is_empty()).then_some(Value::Table(out))
        }
        (d, e) => (d != e).then(|| e.clone()),
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

    #[test]
    fn nested_tables_merge_recursively() {
        // recursion deeper than one table level: inner sibling keys survive
        let mut base = v("[t.u]\nx = 1\ny = 2\n[t.w]\nz = 3");
        deep_merge(&mut base, v("[t.u]\ny = 9"));
        assert_eq!(base, v("[t.u]\nx = 1\ny = 9\n[t.w]\nz = 3"));
    }

    #[test]
    fn unknown_key_reported_with_path() {
        let d = v("[keys.movement]\npage_forward = [\"ctrl-f\"]");
        let u = v("[keys.movement]\npgae_forward = [\"ctrl-f\"]");
        assert_eq!(unknown_keys(&d, &u), vec!["keys.movement.pgae_forward"]);
    }

    #[test]
    fn wildcard_tables_accept_any_key() {
        let d = v("[styles]\n[commands]\n[open.text]\ndefault = { name = \"vim\", args = [], terminal = true }");
        let u = v("[styles.image]\ncolor = \"cyan\"\n[commands.checksum]\nkeys = [\"cs\"]\ncmd = \"sha256sum $@\"\n[open.video]\ndefault = { name = \"mpv\", args = [], terminal = true }");
        assert!(unknown_keys(&d, &u).is_empty());
    }

    #[test]
    fn diff_drops_values_equal_to_default() {
        let d = v("[general]\nuse_trash = true\nfancy_icons = false");
        let e = v("[general]\nuse_trash = true\nfancy_icons = true");
        assert_eq!(
            diff_from_defaults(&d, &e).unwrap(),
            v("[general]\nfancy_icons = true")
        );
    }

    #[test]
    fn diff_of_identical_trees_is_none() {
        let d = v("[general]\nuse_trash = true");
        assert!(diff_from_defaults(&d, &d.clone()).is_none());
    }

    #[test]
    fn diff_keeps_explicit_unbind() {
        // undo = [] differs from default ["u"] and must survive migrate-config
        let d = v("[keys.manipulation]\nundo = [\"u\"]");
        let e = v("[keys.manipulation]\nundo = []");
        assert_eq!(diff_from_defaults(&d, &e).unwrap(), e);
    }

    #[test]
    fn diff_round_trips_through_merge() {
        // the contract migrate-config depends on:
        //   deep_merge(defaults, diff_from_defaults(defaults, effective))
        //     == effective
        // for any effective produced by deep_merge(defaults, user).
        let defaults = v(concat!(
            "[general]\nuse_trash = true\nfancy_icons = false\n",
            "[keys.movement]\nundo = [\"u\"]\ndown = [\"j\"]\n",
            "[open]\nsize = 1\n",
        ));
        let user = v(concat!(
            "custom_key = 5\n", // (a) user-only key, absent from defaults
            "[keys.movement]\nundo = []\n", // (d) emptied array vs default ["u"]
            "[general]\nfancy_icons = true\n", // (b) nested table override
            "open = \"scalar\"\n", // (c) user scalar where default has a table
        ));

        let mut effective = defaults.clone();
        deep_merge(&mut effective, user);

        let diff = diff_from_defaults(&defaults, &effective).unwrap();
        let mut roundtrip = defaults.clone();
        deep_merge(&mut roundtrip, diff);
        assert_eq!(roundtrip, effective);
    }

    #[test]
    fn unknown_keys_root_path_and_type_mismatch() {
        // a top-level unknown key is reported with a bare (root) path...
        // ...and a type mismatch (user table where the default has a scalar)
        // is NOT reported, whatever keys the table holds inside.
        let d = v("[general]\nuse_trash = true\nsize = 1");
        let u = v("nosuch = 1\n[general.size]\nanything = 2\ngoes = 3");
        assert_eq!(unknown_keys(&d, &u), vec!["nosuch"]);
    }
}
