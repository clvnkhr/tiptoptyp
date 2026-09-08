#[path = "private_workspace_probe.rs"] mod private_workspace;
mod compiler_probe;
fn main() { compiler_probe::audit_probe(std::path::Path::new(&std::env::args().nth(1).unwrap())); }
