//! Virtual file system for pending changes

use core::fmt;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::{Display, Formatter},
    io::{self, ErrorKind},
    path::{Path, PathBuf, absolute},
    sync::LazyLock,
};

use fs_err::tokio as fs;
use owo_colors::OwoColorize;
use owo_colors::Style as OwoStyle;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, ThemeSet},
    parsing::SyntaxSet,
    util::as_24_bit_terminal_escaped,
};
use tokio::task::JoinSet;

/// Stores pending operations on the file system.
#[derive(Debug)]
pub struct FileOperationStore {
    changes: HashMap<PathBuf, FileChange>,
    root: PathBuf,
}

impl FileOperationStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            changes: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonicalize the given relative path.
    async fn resolve(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let full = self.root.join(relative);
        fs::canonicalize(&full).await.or_else(|_| absolute(&full))
    }

    pub async fn delete_if_exists(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = self.resolve(path).await?;

        if matches!(self.changes.get(&path), Some(FileChange::Delete)) {
            return Ok(());
        }

        let exists = tokio::fs::try_exists(&path).await.unwrap_or(true);
        if !exists {
            return Ok(());
        }

        self.changes.insert(path, FileChange::Delete);

        Ok(())
    }

    pub async fn write(&mut self, path: impl AsRef<Path>, contents: String) -> io::Result<()> {
        let path = self.resolve(path).await?;

        self.changes.insert(path, FileChange::Change(contents));

        Ok(())
    }

    pub async fn read_to_string(&self, path: impl AsRef<Path>) -> io::Result<String> {
        let path = self.resolve(path).await?;

        if let Some(change) = self.changes.get(&path) {
            return match change {
                FileChange::Change(contents) => Ok(contents.clone()),
                FileChange::Delete => Err(io::Error::from(ErrorKind::NotFound)),
            };
        }

        fs::read_to_string(path).await
    }

    pub async fn display(&self, show_contents: bool, highlight: bool) -> FileOperationsDisplay<'_> {
        FileOperationsDisplay::new(self, show_contents, highlight).await
    }

    pub async fn apply(&mut self) -> std::io::Result<()> {
        for (path, change) in self.changes.drain() {
            match change {
                FileChange::Delete => {
                    fs::remove_file(path).await?;
                }
                FileChange::Change(new_contents) => {
                    fs::write(path, new_contents).await?;
                }
            }
        }

        Ok(())
    }
}

/// Prints created files, deleted files, and modified files.
pub struct FileOperationsDisplay<'a> {
    store: &'a FileOperationStore,
    /// The contents of the files before the pending changes
    old_files: BTreeMap<PathBuf, String>,
    highlight: bool,
    show_contents: bool,
}

impl<'a> FileOperationsDisplay<'a> {
    async fn new(store: &'a FileOperationStore, show_contents: bool, highlight: bool) -> Self {
        let mut read_tasks = JoinSet::new();

        for (file, change) in &store.changes {
            if matches!(change, FileChange::Change(_)) {
                let file = file.clone();

                read_tasks.spawn(async move {
                    let contents = fs::read_to_string(&file).await;
                    (file, contents.ok())
                });
            }
        }

        let old_files = read_tasks
            .join_all()
            .await
            .into_iter()
            .filter_map(|(path, contents)| Some((path, contents?)))
            .collect();

        Self {
            store,
            highlight,
            old_files,
            show_contents,
        }
    }

    fn write_header(
        &self,
        f: &mut Formatter<'_>,
        path: &Path,
        change: &FileChange,
        is_new: bool,
    ) -> fmt::Result {
        let mut style = owo_colors::Style::new();

        if self.highlight {
            match change {
                FileChange::Delete => style = style.on_red(),
                FileChange::Change(_) if is_new => style = style.on_green(),
                FileChange::Change(_) => style = style.on_yellow(),
            }

            style = style.bold();
        }

        let label = match change {
            FileChange::Delete => "File deleted",
            FileChange::Change(_) if is_new => "File created",
            FileChange::Change(_) => "File modified",
        };

        write!(f, "{}", label.style(style))?;
        if !self.highlight {
            write!(f, ":")?;
        }

        writeln!(f, " {}", path.display())?;
        writeln!(f)?;

        Ok(())
    }

    fn render_diff(
        &self,
        f: &mut Formatter<'_>,
        changes: &FileChange,
        old_contents: Option<&str>,
        mut highlighter: Option<HighlightLines>,
        syntaxes: &SyntaxSet,
    ) -> fmt::Result {
        let FileChange::Change(left_new) = changes else {
            // Intentionally not implemented for deletes to make output more brief.
            return Ok(());
        };

        let right_old = old_contents.unwrap_or_default();

        let mut left_num = 0;
        let mut right_num = 0;

        struct DiffLine<'a> {
            num: usize,
            text: &'a str,
            icon: &'a str,
            gutter_style: OwoStyle,
            code_style: OwoStyle,
        }

        for comparison in diff::lines(left_new, right_old) {
            let line = match comparison {
                diff::Result::Right(text) => {
                    right_num += 1;

                    DiffLine {
                        num: right_num,
                        text,
                        icon: "-",
                        gutter_style: OwoStyle::new().red(),
                        code_style: OwoStyle::new().dimmed(),
                    }
                }
                diff::Result::Both(text, _) => {
                    left_num += 1;
                    right_num += 1;

                    DiffLine {
                        num: left_num,
                        text,
                        icon: " ",
                        gutter_style: OwoStyle::new().bright_black(),
                        code_style: OwoStyle::new(),
                    }
                }
                diff::Result::Left(text) => {
                    left_num += 1;

                    DiffLine {
                        num: left_num,
                        text,
                        icon: "+",
                        gutter_style: OwoStyle::new().green(),
                        code_style: OwoStyle::new(),
                    }
                }
            };

            let prefix = format!("{:2}", line.num);

            if let Some(fmt) = highlighter.as_mut() {
                let ranges: Vec<(Style, &str)> = fmt.highlight_line(line.text, syntaxes).unwrap();
                let escaped = as_24_bit_terminal_escaped(&ranges[..], false);

                write!(
                    f,
                    "{}  {}  {}",
                    line.icon.style(line.gutter_style),
                    prefix.style(line.gutter_style),
                    escaped.style(line.code_style)
                )?;
            } else {
                write!(f, "{}  {} {}", line.icon, prefix, line.text)?;
            }

            writeln!(f)?;
        }

        write!(f, "\n\n")?;

        Ok(())
    }
}

impl Display for FileOperationsDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        static SYNTAXES_DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntax.dump"));
        static SYNTAXES: LazyLock<SyntaxSet> =
            LazyLock::new(|| syntect::dumps::from_uncompressed_data(SYNTAXES_DUMP).unwrap());
        static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

        let theme = &THEMES.themes["Solarized (dark)"];

        for (path, change) in &self.store.changes {
            let old_contents = self.old_files.get(path).map(|s| s.as_str());

            self.write_header(f, path, change, old_contents.is_none())?;

            if !self.show_contents {
                continue;
            }

            let highlighter = if self.highlight {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(|ext| SYNTAXES.find_syntax_by_extension(ext))
                    .map(|syntax| HighlightLines::new(syntax, theme))
            } else {
                None
            };

            self.render_diff(f, change, old_contents, highlighter, &SYNTAXES)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
enum FileChange {
    Delete,
    Change(String),
}
