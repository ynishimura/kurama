//! Where the tables of config.toml's document print and which comments belong to them: the positions of headers, the first header of a unit, and a unit removed with the comments it owns.

use toml_edit::{DocumentMut, Item, Key, Table};

/// Remove the unit `keys` names with the comments it owns: the lines right
/// above its first header after the last blank line there, and the lines
/// right after its last key before the next blank line (none when no blank
/// line follows them: those sit right above the next header and are its).
/// The comments above that, a file's opening comment included, stay where
/// they were, and a blank line still separates the sections around it.
pub(super) fn remove_unit(doc: &mut DocumentMut, keys: &[Key]) {
    let table = keys[0].get();
    let unit = match keys.get(1) {
        None => doc.get(table),
        Some(name) => doc
            .get(table)
            .and_then(Item::as_table_like)
            .and_then(|entries| entries.get(name.get())),
    };
    let owned: Vec<isize> = unit
        .map(|unit| headers(unit).filter_map(Table::position).collect())
        .unwrap_or_default();
    let (Some(&first), Some(&last)) = (owned.iter().min(), owned.iter().max()) else {
        remove_item(doc, keys);
        return;
    };
    let above = unit
        .and_then(|unit| headers(unit).find(|header| header.position() == Some(first)))
        .and_then(|header| header.decor().prefix())
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_owned();
    let others: Vec<isize> = headers(doc.as_item())
        .filter_map(Table::position)
        .filter(|position| !owned.contains(position))
        .collect();
    let at_top = !others.iter().any(|position| *position < first);
    let next = others.into_iter().filter(|position| *position > last).min();
    remove_item(doc, keys);
    let kept = before_last_blank(&above);
    match next.map(|next| header_at_mut(doc.as_item_mut(), next)) {
        // A header this reading cannot reach: its comments stay as they are.
        Some(None) => {}
        Some(Some(header)) => {
            let below = header
                .decor()
                .prefix()
                .and_then(|prefix| prefix.as_str())
                .unwrap_or_default();
            let rest = match first_blank(below) {
                Some(blank) => &below[blank..],
                None if at_top && kept.is_empty() => below,
                None => &format!("\n{below}"),
            };
            let rest = if at_top && kept.is_empty() {
                rest.trim_start_matches(['\n', '\r'])
            } else {
                rest
            };
            let prefix = format!("{kept}{rest}");
            header.decor_mut().set_prefix(prefix);
        }
        None => {
            let trailing = doc.trailing().as_str().unwrap_or_default();
            let rest = first_blank(trailing).map_or("", |blank| &trailing[blank..]);
            let trailing = format!("{kept}{rest}");
            doc.set_trailing(trailing);
        }
    }
}

/// Remove the unit `keys` names, and its table when it was the last entry.
fn remove_item(doc: &mut DocumentMut, keys: &[Key]) {
    let table = keys[0].get();
    match keys.get(1) {
        None => {
            doc.remove(table);
        }
        Some(name) => {
            let entries = doc[table].as_table_like_mut().expect("the unit was found");
            entries.remove(name.get());
            if entries.is_empty() {
                doc.remove(table);
            }
        }
    }
}

/// Where the first blank line of `text` starts.
fn first_blank(text: &str) -> Option<usize> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            return Some(start);
        }
        start += line.len();
    }
    None
}

/// The lines of `text` before its last blank line; none without one.
fn before_last_blank(text: &str) -> &str {
    let mut end = None;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            end = Some(start);
        }
        start += line.len();
    }
    &text[..end.unwrap_or(0)]
}

/// The tables under `item` that print a header of their own.
fn headers(item: &Item) -> impl Iterator<Item = &Table> {
    tables(item)
        .into_iter()
        .filter(|table| !table.is_implicit() && !table.is_dotted())
}

/// The first table under `item` to print a header.
pub(super) fn first_header(item: &Item) -> Option<&Table> {
    headers(item).min_by_key(|table| table.position())
}

/// The table printing a header that the input puts first, where the
/// replacement's comments go.
pub(super) fn first_header_mut(item: &mut Item) -> Option<&mut Table> {
    let table = item.as_table_mut()?;
    if !table.is_implicit() {
        return Some(table);
    }
    table
        .iter_mut()
        .find_map(|(_, child)| first_header_mut(child))
}

/// The table printing a header at `position`, `[[array]]` entries included.
fn header_at_mut(item: &mut Item, position: isize) -> Option<&mut Table> {
    let at = |table: &Table| {
        table.position() == Some(position) && !table.is_implicit() && !table.is_dotted()
    };
    match item {
        Item::Table(table) => {
            if at(table) {
                return Some(table);
            }
            table
                .iter_mut()
                .find_map(|(_, child)| header_at_mut(child, position))
        }
        Item::ArrayOfTables(array) => array.iter_mut().find_map(|table| {
            if at(table) {
                return Some(table);
            }
            table
                .iter_mut()
                .find_map(|(_, child)| header_at_mut(child, position))
        }),
        _ => None,
    }
}

/// Every table under `item`, itself included.
fn tables(item: &Item) -> Vec<&Table> {
    let mut found = Vec::new();
    match item {
        Item::Table(table) => {
            found.push(table);
            for (_, child) in table.iter() {
                found.extend(tables(child));
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter() {
                found.push(table);
                for (_, child) in table.iter() {
                    found.extend(tables(child));
                }
            }
        }
        _ => {}
    }
    found
}

/// Where the first header of `item` is in the file.
pub(super) fn first_position(item: &Item) -> Option<isize> {
    tables(item).into_iter().filter_map(Table::position).min()
}

/// A position after every header of the document.
pub(super) fn next_position(root: &Item) -> isize {
    tables(root)
        .into_iter()
        .filter_map(Table::position)
        .max()
        .map_or(0, |last| last + 1)
}

/// Forget where the input put its headers: the tables then follow the one
/// they are placed after.
pub(super) fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(table) => {
            table.set_position(None);
            for (_, child) in table.iter_mut() {
                clear_positions(child);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                table.set_position(None);
                for (_, child) in table.iter_mut() {
                    clear_positions(child);
                }
            }
        }
        _ => {}
    }
}
