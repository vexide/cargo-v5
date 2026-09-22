use std::{env, path::PathBuf};

use syntect::{
    dumps::dump_to_uncompressed_file,
    parsing::{SyntaxDefinition, SyntaxSet},
};

const TOML_SYNTAX: &str = include_str!("src/commands/migrate/TOML.sublime-syntax");

fn main() {
    // Create dump with custom languages for syntax highlighting.

    let toml =
        SyntaxDefinition::load_from_str(TOML_SYNTAX, true, None).expect("TOML syntax is valid");

    let mut syntaxes = SyntaxSet::load_defaults_newlines().into_builder();
    syntaxes.add(toml);
    let syntaxes = syntaxes.build();

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    dump_to_uncompressed_file(&syntaxes, out_dir.join("syntax.dump")).unwrap();
}
