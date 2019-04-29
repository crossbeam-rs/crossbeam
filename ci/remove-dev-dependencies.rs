// https://github.com/rust-lang-nursery/futures-rs/blob/master/ci/remove-dev-dependencies/src/main.rs

use std::{env, error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    for file in env::args().skip(1) {
        let content = fs::read_to_string(&file)?;
        let mut doc: toml_edit::Document = content.parse()?;
        doc.as_table_mut().remove("dev-dependencies");
        fs::write(file, doc.to_string())?;
    }

    Ok(())
}
