use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    io::{self, ErrorKind},
    path::{absolute, Path, PathBuf},
};

use miette::Diagnostic;
use owo_colors::OwoColorize;
use supports_color::Stream;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, ThemeSet},
    parsing::SyntaxSet,
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};
use thiserror::Error;
use fs_err::tokio as fs;
use toml_edit::{table, value, DocumentMut, Value};

use crate::errors::CliError;

#[derive(Debug, Error, Diagnostic)]
pub enum UpgradeError {
    #[error("failed to parse toml file")]
    #[diagnostic(code(cargo_v5::upgrade::invalid_toml_file))]
    TomlParse(#[from] toml_edit::TomlError),
}

/// Applies all available upgrades to the workspace.
pub async fn upgrade_workspace(root: &Path) -> Result<(), CliError> {
    let mut files = FileOperationStore::default();

    update_cargo_config(root, &mut files).await?;

    // Print pending changes - in the future we will apply them too.
    let highlight = supports_color::on_cached(Stream::Stdout).is_some();

    println!();
    println!("{}", files.display(true, highlight));

    Ok(())
}

/// Updates the user's Cargo config to use the Rust `armv7a-vex-v5` target
/// and deletes their old target JSON file.
async fn update_cargo_config(root: &Path, files: &mut FileOperationStore) -> Result<(), CliError> {
    let cargo_config = root.join(".cargo").join("config.toml");
    let existing_config = files.read_to_string(&cargo_config).await;

    // If the config file is missing, make a new one.
    let mut document = match existing_config {
        Ok(contents) => contents.parse::<DocumentMut>().map_err(UpgradeError::from)?,
        Err(err) if err.kind() == ErrorKind::NotFound => DocumentMut::new(),
        Err(other) => return Err(other)?,
    };

    let mut build = table();
    build["target"] = value("armv7a-vex-v5");
    document["build"] = build;


    let mut unstable = table();

    let build_std = Value::from_iter(vec!["std", "panic_abort"]);
    unstable["build-std"] = value(build_std);
    document["unstable"] = unstable;

    files.write(cargo_config, document.to_string()).await?;

    match files.delete(root.join("armv7a-vex-v5.json")).await {
        Err(err) if err.kind() != ErrorKind::NotFound => return Err(err)?,
        _ => {},
    }

    Ok(())
}

/// Stores pending operations on the file system.
#[derive(Debug, Default)]
struct FileOperationStore {
    changes: BTreeMap<PathBuf, FileChange>,
}

impl FileOperationStore {
    async fn delete(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.changes
            .insert(fs::canonicalize(&path).await?, FileChange::Delete);

        Ok(())
    }

    async fn write(&mut self, path: impl AsRef<Path>, contents: String) -> io::Result<()> {
        let path = path.as_ref();
        let path = fs::canonicalize(path).await.or_else(|_| absolute(path))?;

        self.changes.insert(path, FileChange::Change(contents));

        Ok(())
    }

    async fn read_to_string(&self, path: impl AsRef<Path>) -> io::Result<String> {
        let path = path.as_ref();
        let path = fs::canonicalize(path).await.or_else(|_| absolute(path))?;

        if let Some(change) = self.changes.get(&path) {
            return match change {
                FileChange::Change(contents) => Ok(contents.clone()),
                FileChange::Delete => Err(io::Error::from(ErrorKind::NotFound)),
            };
        }

        fs::read_to_string(path).await
    }

    fn display(&self, show_contents: bool, highlight: bool) -> FileOperationsDisplay<'_> {
        FileOperationsDisplay {
            store: self,
            show_contents,
            highlight,
        }
    }
}

/// Prints created files, deleted files, and modified files.
struct FileOperationsDisplay<'a> {
    store: &'a FileOperationStore,
    show_contents: bool,
    highlight: bool,
}

impl Display for FileOperationsDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ps = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        let theme = &ts.themes["Solarized (dark)"];

        for (path, change) in &self.store.changes {
            if self.highlight {
                match change {
                    FileChange::Delete => {
                        write!(f, "{}", "File deleted".on_red().bold())?;
                    }
                    FileChange::Change(_) => {
                        write!(f, "{}", "File created".on_green().bold())?;
                    }
                }
            } else {
                match change {
                    FileChange::Delete => {
                        write!(f, "File deleted:")?;
                    }
                    FileChange::Change(_) => {
                        write!(f, "File created:")?;
                    }
                }
            }

            writeln!(f, " {}", path.display())?;
            writeln!(f)?;

            if self.show_contents
                && let FileChange::Change(contents) = change
            {
                let mut fmt = if self.highlight {
                    ps.find_syntax_by_extension("rs")
                        .map(|syntax| HighlightLines::new(syntax, theme))
                } else {
                    None
                };

                for (idx, line) in LinesWithEndings::from(contents).enumerate() {
                    // writeln!(f, "{line:?}")?;
                    let line_num = format!("{:2}", idx + 1);

                    if let Some(fmt) = &mut fmt {
                        let ranges: Vec<(Style, &str)> = fmt.highlight_line(line, &ps).unwrap();
                        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);

                        write!(f, " {}  {escaped}", line_num.bright_black())?;
                    } else {
                        write!(f, " {line_num} {line}")?;
                    }
                }

                write!(f, "\n\n")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
enum FileChange {
    Delete,
    Change(String),
}
