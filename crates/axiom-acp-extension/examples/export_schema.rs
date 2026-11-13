use std::{env, fs, path::PathBuf};

fn main() {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: export_schema <output-path>");
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("create schema directory");
    }
    fs::write(output, axiom_acp_extension::schema_json()).expect("write protocol schema");
}
