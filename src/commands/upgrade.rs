use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt::Display,
    io::{self, ErrorKind},
    path::{Path, PathBuf, absolute},
};

use fs_err::tokio as fs;
use miette::Diagnostic;
use owo_colors::OwoColorize;
use supports_color::Stream;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, ThemeSet},
    parsing::SyntaxSet,
    util::as_24_bit_terminal_escaped,
};
use thiserror::Error;
use tokio::task::JoinSet;
use toml_edit::{DocumentMut, Value, table, value};

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
    println!("{}", files.display(true, highlight).await);

    Ok(())
}

/// Updates the user's Cargo config to use the Rust `armv7a-vex-v5` target
/// and deletes their old target JSON file.
async fn update_cargo_config(root: &Path, files: &mut FileOperationStore) -> Result<(), CliError> {
    let cargo_config = root.join(".cargo").join("config.toml");
    let existing_config = files.read_to_string(&cargo_config).await;

    // If the config file is missing, make a new one.
    let mut document = match existing_config {
        Ok(contents) => contents
            .parse::<DocumentMut>()
            .map_err(UpgradeError::from)?,
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
        _ => {}
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

    async fn display(&self, show_contents: bool, highlight: bool) -> FileOperationsDisplay<'_> {
        let old_files = if show_contents {
            let mut read_tasks = JoinSet::new();

            for (file, change) in &self.changes {
                if matches!(change, FileChange::Change(_)) {
                    let file = file.clone();

                    read_tasks.spawn(async move {
                        let contents = fs::read_to_string(&file).await;
                        (file, contents.ok())
                    });
                }
            }

            Some(
                read_tasks
                    .join_all()
                    .await
                    .into_iter()
                    .filter_map(|(path, contents)| Some((path, contents?)))
                    .collect(),
            )
        } else {
            None
        };

        FileOperationsDisplay {
            store: self,
            highlight,
            old_files,
        }
    }
}

/// Prints created files, deleted files, and modified files.
struct FileOperationsDisplay<'a> {
    store: &'a FileOperationStore,
    /// The contents of the files before the pending changes
    old_files: Option<BTreeMap<PathBuf, String>>,
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

            if let Some(old_files) = &self.old_files
                && let FileChange::Change(contents) = change
            {
                let mut fmt = if self.highlight {
                    ps.find_syntax_by_extension("rs")
                        .map(|syntax| HighlightLines::new(syntax, theme))
                } else {
                    None
                };

                let left = old_files.get(path).map(Cow::from).unwrap_or_default();

                for (idx, line_diff) in diff::lines(&left, contents).iter().enumerate() {
                    let line = match line_diff {
                        diff::Result::Left(line) => line,
                        diff::Result::Both(line, _) => line,
                        diff::Result::Right(line) => line,
                    };
                    let diff_indicator = match line_diff {
                        diff::Result::Left(..) => format!("{}", "-".red()),
                        diff::Result::Both(..) => " ".to_string(),
                        diff::Result::Right(..) => format!("{}", "+".green()),
                    };

                    let prefix = format!("{:2}", idx + 1);

                    if let Some(fmt) = &mut fmt {
                        let ranges: Vec<(Style, &str)> = fmt.highlight_line(line, &ps).unwrap();
                        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);

                        write!(f, "{diff_indicator}  {}  {escaped}", prefix.bright_black())?;
                    } else {
                        write!(f, "{diff_indicator}  {prefix} {line}")?;
                    }

                    writeln!(f)?;
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
