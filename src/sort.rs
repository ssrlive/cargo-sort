use std::{cmp::Ordering, collections::BTreeMap, iter::FromIterator};

use toml_edit::{Array, Decor, DocumentMut, Item, RawString, Table, Value};

/// Each `Matcher` field when matched to a heading or key token
/// will be matched with `.contains()`.
pub struct Matcher<'a> {
    /// Toml headings with braces `[heading]`.
    pub heading: &'a [&'a str],
    /// Toml heading with braces `[heading]` and the key
    /// of the array to sort.
    pub heading_key: &'a [(&'a str, &'a str)],
}

pub const MATCHER: Matcher<'_> = Matcher {
    heading: &["dependencies", "dev-dependencies", "build-dependencies"],
    heading_key: &[
        ("workspace", "members"),
        ("workspace", "exclude"),
        ("workspace", "dependencies"),
        ("workspace", "dev-dependencies"),
        ("workspace", "build-dependencies"),
    ],
};

/// A state machine to track collection of headings.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Heading {
    /// After collecting heading segments we recurse into another table.
    Next(Vec<String>),
    /// We have found a completed heading.
    ///
    /// The the heading we are processing has key value pairs.
    Complete(Vec<String>),
}

// impl PartialOrd for Heading {
//     fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
//         Some(self.cmp(other))
//     }
// }

// impl Ord for Heading {
//     fn cmp(&self, other: &Self) -> Ordering {
//         fn split_rev(vec: &Vec<String>) -> Vec<String> {
//             // vec.iter()
//             //     .map(|s| s.as_str().split('.').rev().collect::<Vec<_>>().join("."))
//             //     .collect()
//             let v = vec
//                 .iter()
//                 .map(|s| s.as_str().split('.').rev().collect::<Vec<_>>().join("."))
//                 .collect();
//             println!("split_rev: {v:?}");
//             v
//         }
//         let (a_tag, a_segs) = match self {
//             Heading::Complete(segs) => (1, segs),
//             Heading::Next(segs) => (0, segs),
//         };
//         let (b_tag, b_segs) = match other {
//             Heading::Complete(segs) => (1, segs),
//             Heading::Next(segs) => (0, segs),
//         };
//         let ord = a_tag.cmp(&b_tag);
//         if ord == Ordering::Equal {
//             split_rev(a_segs).cmp(&split_rev(b_segs))
//         } else {
//             ord
//         }
//     }
// }

/// Returns a sorted toml `DocumentMut`.
pub fn sort_toml(
    input: &str,
    matcher: Matcher<'_>,
    group: bool,
    ordering: &[String],
) -> DocumentMut {
    let mut ordering = ordering.to_owned();
    let mut toml = input.parse::<DocumentMut>().unwrap();
    // This takes care of `[workspace] members = [...]`
    for (heading, key) in matcher.heading_key {
        // Since this `&mut toml[&heading]` is like
        // `SomeMap.entry(key).or_insert(Item::None)` we only want to do it if we
        // know the heading is there already
        if toml.as_table().contains_key(heading) {
            if let Item::Table(table) = &mut toml[heading] {
                if table.contains_key(key) {
                    match &mut table[key] {
                        Item::Value(Value::Array(arr)) => {
                            sort_array(arr);
                        }
                        Item::Table(table) => {
                            sort_table(table, group, &None);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let mut first_table = None;
    let mut heading_order: BTreeMap<_, Vec<Heading>> = BTreeMap::new();
    for (idx, (head, item)) in toml.as_table_mut().iter_mut().enumerate() {
        // println!("Processing heading: {head} at index: {idx}");

        // let mut table_path: Option<(&str, &Table)> = None;
        let mut table_path: Option<Vec<String>> = None;
        let item_key = head.get();
        if item_key == "target" {
            let mut special_tables = Vec::new();
            let mut path = vec![item_key];
            if let Some(table) = item.as_table() {
                special_tables.push((vec![item_key], table));
                collect_special_tables(table, &mut path, &mut special_tables, &matcher);
            }

            if let Some((path, _table)) = special_tables.pop() {
                // println!("Found special table at path: {:?}", path);
                if let Some(key) = path.last() {
                    if matcher.heading.contains(key) {
                        // table_path = Some((key, _table));
                        table_path =
                            Some(path.iter().map(|s| s.to_string()).collect::<_>());
                    }
                }
            }
        }

        // println!("table_path: {:?}", table_path);

        if !matcher.heading.contains(&item_key) && table_path.is_none() {
            if !ordering.contains(&head.to_owned()) && !ordering.is_empty() {
                ordering.push(head.to_owned());
            }
            continue;
        }
        match item {
            Item::Table(table) => {
                if first_table.is_none() {
                    // The root table is always index 0 which we ignore so add 1
                    first_table = Some(idx + 1);
                }
                // generate key likes: `dependencies.cfg(target_os="linux").target`
                let key = table_path
                    .as_ref()
                    .and_then(|t| {
                        Some(t.iter().rev().cloned().collect::<Vec<_>>().join("."))
                    })
                    .unwrap_or_else(|| head.to_string());
                // println!("Processing table: {key} at index: {idx}");
                let headings = heading_order.entry((idx, key.clone())).or_default();
                // Push a `Heading::Complete` here incase the tables are ordered
                // [heading.segs]
                // [heading]
                // It will just be ignored if not the case
                headings.push(Heading::Complete(vec![key]));

                gather_headings(table, headings, 1);
                headings.sort();
                sort_table(table, group, &table_path);
            }
            Item::None => continue,
            _ => {}
        }
    }

    if ordering.is_empty() {
        sort_lexicographical(first_table, &heading_order, &mut toml);
    } else {
        sort_by_ordering(&ordering, &heading_order, &mut toml);
    }

    toml
}

fn collect_special_tables<'a>(
    table: &'a Table,
    path: &mut Vec<&'a str>,
    result: &mut Vec<(Vec<&'a str>, &'a Table)>,
    matcher: &Matcher,
) {
    for (key, item) in table.iter() {
        if let Item::Table(inner) = item {
            path.push(key);
            // judge if the last level is dependencies/dev-dependencies/build-dependencies
            if matcher.heading.contains(&key) {
                result.push((path.clone(), inner));
            }
            // recursively collect special tables
            collect_special_tables(inner, path, result, matcher);
            path.pop();
        }
    }
}

fn sort_array(arr: &mut Array) {
    let mut all_strings = true;
    let trailing = arr.trailing().clone();
    let trailing_comma = arr.trailing_comma();

    let mut arr_copy = arr.iter().cloned().collect::<Vec<_>>();
    arr_copy.sort_by(|a, b| match (a, b) {
        (Value::String(a), Value::String(b)) => a.value().cmp(b.value()),
        _ => {
            all_strings = false;
            Ordering::Equal
        }
    });
    if all_strings {
        *arr = Array::from_iter(arr_copy);
    }

    arr.set_trailing(trailing);
    arr.set_trailing_comma(trailing_comma);
}

fn sort_table(table: &mut Table, group: bool, table_path: &Option<Vec<String>>) {
    if group {
        sort_by_group(table);
    } else if let Some(table_path) = table_path {
        if table_path.len() > 1 {
            sort_table_by_path(table, &table_path[1..]);
        }
    } else {
        table.sort_values();
    }
}

fn sort_table_by_path(table: &mut Table, path: &[String]) {
    if path.is_empty() {
        table.sort_values();
    } else if let Some(sub) = table.get_mut(&path[0]) {
        if let Item::Table(inner_table) = sub {
            sort_table_by_path(inner_table, &path[1..]);
        }
    }
}

fn gather_headings(table: &Table, keys: &mut Vec<Heading>, depth: usize) {
    if table.is_empty() && !table.is_implicit() {
        let next = match keys.pop().unwrap() {
            Heading::Next(segs) => Heading::Complete(segs),
            comp => comp,
        };
        keys.push(next);
    }
    for (head, item) in table.iter() {
        match item {
            Item::Value(_) => {
                if keys.last().is_some_and(|h| matches!(h, Heading::Complete(_))) {
                    continue;
                }
                let next = match keys.pop().unwrap() {
                    Heading::Next(segs) => Heading::Complete(segs),
                    _complete => unreachable!("the above if check prevents this"),
                };
                keys.push(next);
                continue;
            }
            Item::Table(table) => {
                let next = match keys.pop().unwrap() {
                    Heading::Next(mut segs) => {
                        segs.push(head.into());
                        Heading::Next(segs)
                    }
                    // This happens when
                    //
                    // [heading]       // transitioning from here to
                    // [heading.segs]  // here
                    Heading::Complete(segs) => {
                        let take = depth.max(1);
                        let mut next = segs[..take].to_vec();
                        next.push(head.into());
                        keys.push(Heading::Complete(segs));
                        Heading::Next(next)
                    }
                };
                keys.push(next);
                gather_headings(table, keys, depth + 1);
            }
            Item::ArrayOfTables(_arr) => unreachable!("no [[heading]] are sorted"),
            Item::None => unreachable!("an empty table will not be sorted"),
        }
    }
}

fn sort_by_group(table: &mut Table) {
    let table_clone = table.clone();
    table.clear();

    let mut groups = BTreeMap::new();
    let mut group_decor = BTreeMap::default();

    let mut curr = 0;
    for (idx, (k, _)) in table_clone.iter().enumerate() {
        let (k, v) = table_clone.get_key_value(k).unwrap();

        let blank_lines = k
            .leaf_decor()
            .prefix()
            .and_then(RawString::as_str)
            .unwrap_or("")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .count();

        if blank_lines > 0 {
            let decor = k.leaf_decor().clone();
            let k = k.clone().with_leaf_decor(Decor::default());

            groups.entry(idx).or_insert_with(|| vec![(k, v)]);
            group_decor.insert(idx, decor);
            curr = idx;
        } else {
            groups.entry(curr).or_default().push((k.clone(), v));
        }
    }

    for (idx, mut group) in groups {
        group.sort_by(|a, b| a.0.cmp(&b.0));
        let group_decor = group_decor.remove(&idx);

        for (idx, (mut k, v)) in group.into_iter().enumerate() {
            if idx == 0 {
                if let Some(group_decor) = group_decor.clone() {
                    k = k.with_leaf_decor(group_decor);
                }
            }

            table.insert_formatted(&k, v.clone());
        }
    }
}

fn sort_lexicographical(
    first_table: Option<usize>,
    heading_order: &BTreeMap<(usize, String), Vec<Heading>>,
    toml: &mut DocumentMut,
) {
    // Since the root table is always index 0 we add one
    let first_table_idx = first_table.unwrap_or_default() + 1;
    for (idx, heading) in heading_order.iter().flat_map(|(_, segs)| segs).enumerate() {
        if let Heading::Complete(segs) = heading {
            let mut nested = 0;
            let mut table = Some(toml.as_table_mut());
            for seg in segs {
                nested += 1;
                table = table.and_then(|t| t[seg].as_table_mut());
            }
            // Do not reorder the unsegmented tables
            if nested > 1 {
                if let Some(table) = table {
                    table.set_position(first_table_idx + idx);
                }
            }
        }
    }
}

fn sort_by_ordering(
    ordering: &[String],
    heading_order: &BTreeMap<(usize, String), Vec<Heading>>,
    toml: &mut DocumentMut,
) {
    let mut idx = 0;
    println!("Ordering: {:?}", ordering);
    println!("Heading order: {:?}", heading_order);
    for heading in ordering {
        if let Some((_, to_sort_headings)) =
            heading_order.iter().find(|((_, key), _)| key == heading)
        {
            for h in to_sort_headings {
                if let Heading::Complete(segs) = h {
                    let mut table = Some(toml.as_table_mut());
                    for seg in segs {
                        table = table.and_then(|t| t[seg].as_table_mut());
                    }
                    // Do not reorder the unsegmented tables
                    if let Some(table) = table {
                        table.set_position(idx);
                        idx += 1;
                    }
                }
            }
        } else if let Some(tab) = toml.as_table_mut()[heading].as_table_mut() {
            tab.set_position(idx);
            idx += 1;
            walk_tables_set_position(tab, &mut idx)
        } else if let Some(arrtab) = toml.as_table_mut()[heading].as_array_of_tables_mut()
        {
            for tab in arrtab.iter_mut() {
                tab.set_position(idx);
                idx += 1;
                walk_tables_set_position(tab, &mut idx);
            }
        }
    }
}

fn walk_tables_set_position(table: &mut Table, idx: &mut usize) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(tab) => {
                tab.set_position(*idx);
                *idx += 1;
                walk_tables_set_position(tab, idx)
            }
            Item::ArrayOfTables(arr) => {
                for tab in arr.iter_mut() {
                    tab.set_position(*idx);
                    *idx += 1;
                    walk_tables_set_position(tab, idx)
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod test {
    use std::fs;

    use similar_asserts::assert_eq;

    use super::MATCHER;

    #[test]
    fn toml_edit_check() {
        let input = fs::read_to_string("examp/workspace.toml").unwrap();
        let expected = fs::read_to_string("examp/workspace.sorted.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, false, &[]);
        assert_eq(expected, sorted);
    }

    #[test]
    fn toml_workspace_deps_edit_check() {
        let input = fs::read_to_string("examp/workspace_deps.toml").unwrap();
        let expected = fs::read_to_string("examp/workspace_deps.sorted.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, false, &[]);
        assert_eq(expected, sorted);
    }

    #[test]
    fn grouped_check() {
        let input = fs::read_to_string("examp/ruma.toml").unwrap();
        let expected = fs::read_to_string("examp/ruma.sorted.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_eq(expected, sorted);
    }

    #[test]
    fn sort_correct() {
        let input = fs::read_to_string("examp/right.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_eq(input, sorted);
    }

    #[test]
    fn sort_comments() {
        let input = fs::read_to_string("examp/comments.toml").unwrap();
        let expected = fs::read_to_string("examp/comments.sorted.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_eq(expected, sorted);
    }

    #[test]
    fn sort_tables() {
        let input = fs::read_to_string("examp/fend.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_ne!(input, sorted.to_string());
        // println!("{}", sorted.to_string());
    }

    #[test]
    fn sort_devfirst() {
        let input = fs::read_to_string("examp/reorder.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_eq(input, sorted);

        let input = fs::read_to_string("examp/noreorder.toml").unwrap();
        let sorted = super::sort_toml(&input, MATCHER, true, &[]);
        assert_eq(input, sorted);
    }

    #[test]
    fn reorder() {
        let input = fs::read_to_string("examp/clippy.toml").unwrap();
        let sorted = super::sort_toml(
            &input,
            MATCHER,
            true,
            &[
                "package".to_owned(),
                "features".to_owned(),
                "dependencies".to_owned(),
                "build-dependencies".to_owned(),
                "dev-dependencies".to_owned(),
            ],
        );
        assert_ne!(input, sorted.to_string());
    }

    fn assert_eq<L: ToString, R: ToString>(left: L, right: R) {
        let left = left.to_string();
        let right = right.to_string();

        #[cfg(windows)]
        assert_eq!(left.replace("\r\n", "\n"), right.replace("\r\n", "\n"));

        #[cfg(not(windows))]
        assert_eq!(left, right);
    }
}
