#[path = "/Users/calvinkhor/Documents/GitHub/mytypst/src/project_index.rs"] mod project_index;
use std::{collections::BTreeMap, path::PathBuf};
fn main() {
 let root = PathBuf::from(std::env::args().nth(1).unwrap());
 let main = root.join("main.typ");
 std::fs::write(root.join("child.typ"), "= Child\n").unwrap();
 let mut overrides = BTreeMap::new();
 overrides.insert(main.clone(), r###"/*
= Comment heading
#let comment_definition = 1
#include "child.typ"
*/
```typ
= Sample heading
#let sample_definition = 1
```
#let real = 2
"###.to_owned());
 println!("{:?}", project_index::analyze_project(&root, &main, &overrides));
}
