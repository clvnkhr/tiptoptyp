#[path = "/Users/calvinkhor/Documents/GitHub/mytypst/src/search.rs"] mod search;
fn main() {
 let text="a".repeat(50_000); let query="a".repeat(999)+"b";
 for case_sensitive in [true, false] {
  let start=std::time::Instant::now();
  let count=search::find_all_with_options(std::hint::black_box(&text),std::hint::black_box(&query),case_sensitive,false).len();
  println!("case_sensitive={case_sensitive} count={count} duration={:?}", start.elapsed());
 }
}
