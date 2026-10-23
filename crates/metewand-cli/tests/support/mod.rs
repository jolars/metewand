pub fn replace_yaml(source: &str, before: &str, after: &str) -> String {
    if !before.contains('\n') && !after.contains('\n') {
        assert!(
            source.contains(before),
            "missing {before:?} in YAML:\n{source}"
        );
        return source.replace(before, after);
    }
    let first_line = before.lines().next().unwrap();
    let mut updated = String::new();
    let mut previous = 0;
    for (offset, _) in source.match_indices(first_line) {
        if offset < previous {
            continue;
        }
        let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
        let indentation = &source[line_start..offset];
        if !indentation.chars().all(|character| character == ' ') {
            continue;
        }
        let indent = |text: &str| {
            text.split_inclusive('\n')
                .enumerate()
                .map(|(index, line)| {
                    if index == 0 || line == "\n" {
                        line.to_owned()
                    } else {
                        format!("{indentation}{line}")
                    }
                })
                .collect::<String>()
        };
        let expected = indent(before);
        if source[offset..].starts_with(&expected) {
            updated.push_str(&source[previous..offset]);
            updated.push_str(&indent(after));
            previous = offset + expected.len();
        }
    }
    assert!(previous > 0, "missing {before:?} in YAML:\n{source}");
    updated.push_str(&source[previous..]);
    updated
}
