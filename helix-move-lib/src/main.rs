use helix_move_lib::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter()
        .map(|x| x.to_string())
        .collect()
}

fn main() {
    let original = s(&[
        "./testdel/abc/d",
        "./testdel/abc/d/test3",
    ]);

    let new = s(&[
        "./testdel/abxc/d",
        "./testdel/abxc/d/test3",
    ]);

    let rules = build_rules(&original, &new);
    let normalized_rules = normalize_rules(&rules);
    println!("{:#?}", normalized_rules);

    assert_eq!(normalized_rules, normalize_rules(&normalized_rules));

    // let applied = apply_rules_to_list(&normalized_rules);
    // println!("{:#?}", applied);

    // let full_rules = add_missing_directories(&normalized_rules);
    // println!("{:#?}", full_rules);

    let original = s(&[
        "./testdel/abc/d",
        "./testdel/abc/d/test3",
    ]);

    let new = s(&[
        "./testdel/abxc/d",
        "./testdel/abc/d/test3",
    ]);

    let rules = build_rules(&original, &new);
    let normalized_rules = normalize_rules(&rules);
    println!("{:#?}", normalized_rules);

    assert_eq!(normalized_rules, normalize_rules(&normalized_rules));
}
