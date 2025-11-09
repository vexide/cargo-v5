use std::{
    collections::{HashMap, HashSet}, fmt::Debug, str::FromStr
};

use cargo_metadata::Edition;
use ra_ap_syntax::{
    AstNode, SourceFile, SyntaxNode,
    ast::{HasModuleItem, Item, Path, PathSegment, Use, UseTree, UseTreeList, make},
    syntax_editor::SyntaxEditor,
};
use tokio::task::{block_in_place, spawn_blocking};

use crate::{commands::upgrade::ChangesCtx, errors::CliError};

pub async fn update_project(ctx: &mut ChangesCtx) -> Result<(), CliError> {
    let root = ctx.fs.root().to_owned();

    let Some(metadata) = spawn_blocking(|| {
        cargo_metadata::MetadataCommand::new()
            .no_deps()
            .current_dir(root)
            .exec()
            .ok()
    })
    .await
    .unwrap() else {
        return Ok(());
    };

    for package in metadata.workspace_packages() {
        let edition =
            ra_ap_syntax::Edition::from_str(package.edition.as_str()).expect("unknown edition");

        for target in &package.targets {
            let entrypoint = target.src_path.as_path();
            println!("{entrypoint}:");
            let file_contents = ctx.fs.read_to_string(entrypoint).await?;
            let root = ra_ap_syntax::SourceFile::parse(&file_contents, edition);

            let mut editor = SyntaxEditor::new(root.syntax_node());

            // println!("{}: {}", target.name, parsed);
            rewrite_imports(ctx, root.syntax_node(), &mut editor);
        }
    }

    Ok(())
}

pub fn rewrite_imports(_ctx: &mut ChangesCtx, root: SyntaxNode, _editor: &mut SyntaxEditor) {
    for old_use in root.descendants().filter_map(Use::cast) {
        let Some(tree) = old_use.use_tree() else {
            continue;
        };

        let node = ImportNode::from(tree);
        println!("{node:?}");

        // rewrite_use_tree(&tree, &moved_items);
        println!();
    }
}

// fn rewrite_use_tree(tree: &UseTree, moved_items: &HashMap<&str, Path>) {
//     println!("Node");

//     if let Some(path) = tree.path() {
//         println!("path: {path}");
//     }

//     if let Some(rename) = tree.rename() {
//         println!(" -> {rename}");
//     }

//     if let Some(star) = tree.star_token() {
//         println!(" -> {star}");
//     }

//     if let Some(list) = tree.use_tree_list() {
//         println!("{{");
//         for tree in list.use_trees() {
//             rewrite_use_tree(&tree, moved_items);
//         }
//         println!("}}");
//     }
// }

struct ImportNode {
    kind: ImportKind,
    syntax: Option<SyntaxNode>,
}

impl ImportNode {
    const STAR: Self = ImportNode {
        kind: ImportKind::Star,
        syntax: None,
    };
}

impl Debug for ImportNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.syntax.is_some() {
            write!(f, "@")?;
        }

        write!(f, "{:#?}", self.kind)
    }
}

enum ImportKind {
    Star,
    Module {
        ident: String,
        tail: Option<Box<ImportNode>>,
    },
    List {
        subnodes: Vec<ImportNode>,
    },
    Unknown,
}

impl Debug for ImportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportKind::Star => write!(f, "*"),
            ImportKind::Module { ident, tail } => {
                write!(f, "{ident:?}")?;
                if let Some(tail) = tail {
                    write!(f, "::")?;
                    tail.fmt(f)?;
                }
                Ok(())
            }
            ImportKind::List { subnodes } => {
                f.debug_set()
                    .entries(subnodes)
                    .finish()
            }
            ImportKind::Unknown => write!(f, "???")
        }
    }
}

impl ImportKind {
    fn tail_mut(&mut self) -> Option<&mut Option<Box<ImportNode>>> {
        if let Self::Module { tail, .. } = self {
            Some(tail)
        } else {
            None
        }
    }
}

impl From<UseTreeList> for ImportNode {
    fn from(list: UseTreeList) -> Self {
        let nodes = list.use_trees().map(Self::from).collect();

        ImportNode {
            kind: ImportKind::List { subnodes: nodes },
            syntax: Some(list.syntax().clone()),
        }
    }
}

impl From<UseTree> for ImportNode {
    fn from(tree: UseTree) -> Self {
        // No path + no star + no list means just `use ;` perhaps?
        // We'll keep track of the syntax for when we write it back, but there's probably not much we can
        // determine structurally here.
        let fallback = ImportNode {
            kind: ImportKind::Unknown,
            syntax: Some(tree.syntax().clone()),
        };

        if let Some(path) = tree.path() {
            // Split multi-segment paths like vexide::devices::smart into multiple nodes.
            // This will make it much easier to do operations like renaming/flattening single modules.
            let mut nodes = path
                .segments()
                .map(|segment| ImportNode {
                    kind: ImportKind::Module {
                        ident: segment.to_string(),
                        tail: None,
                    },
                    syntax: None,
                })
                .collect::<Vec<_>>();

            let Some(mut top) = nodes.pop() else {
                // Extremely messed up path without any segments somehow...?
                // Can't do much with this.
                return fallback;
            };

            // The last submodule imported owns the final tail (::*/::{}) of the import - add that
            // on before we build everything into a tree & return that.
            let tail = top.kind.tail_mut().unwrap();

            if tree.star_token().is_some() {
                *tail = Some(Box::new(ImportNode::STAR));
            }

            if let Some(list) = tree.use_tree_list() {
                *tail = Some(Box::new(Self::from(list)));
            }

            // Build the nodes into a tree - the last notes in the list are the deepest,
            // so we construct them first and continually build up the tree until we reach the
            // top-level module in the path.
            let mut head = top;
            for mut next in nodes.into_iter().rev() {
                let tail = next.kind.tail_mut().unwrap();
                *tail = Some(Box::new(head));
                head = next;
            }

            head.syntax = fallback.syntax;

            return head;
        }

        // No path = either `*` in `use vexide::{*}` or `{}` in `use {vexide}`;`

        if tree.star_token().is_some() {
            return ImportNode {
                kind: ImportKind::Star,
                syntax: fallback.syntax,
            };
        }

        if let Some(list) = tree.use_tree_list() {
            return Self::from(list);
        }

        fallback
    }
}
