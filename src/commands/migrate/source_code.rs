use std::str::FromStr;

use cargo_metadata::Metadata;
use ra_ap_syntax::{
    AstNode, SourceFile,
    ast::{Attr, ExternCrate, HasAttrs},
};

use crate::{commands::migrate::ChangesCtx, errors::CliError};

/// Perform updates that require knowledge of Rust workspace layout & syntax.
pub async fn update_targets(ctx: &mut ChangesCtx, metadata: &Metadata) -> Result<(), CliError> {
    for package in metadata.workspace_packages() {
        let edition =
            ra_ap_syntax::Edition::from_str(package.edition.as_str()).expect("unknown edition");

        for target in &package.targets {
            let entrypoint = target.src_path.as_path();
            log::debug!(
                "Parsing & updating {entrypoint} in target {}.{}",
                package.name,
                target.name
            );

            let file_contents = ctx.fs.read_to_string(entrypoint).await?;
            let root = ra_ap_syntax::SourceFile::parse(&file_contents, edition);

            let errors = root.errors();
            if !errors.is_empty() {
                log::warn!(
                    "{entrypoint:?} has syntax errors; make sure to review any suggested edits for this file."
                );
            }

            let root_node = root.syntax_node().clone_for_update();
            let Some(root_node) = SourceFile::cast(root_node) else {
                // Can't parse as file due to egregious syntax errors; skip.
                continue;
            };

            remove_no_std(root_node.clone());

            let mut new_contents = root_node.to_string();
            // Avoid registering this as a "changed file" if there were no changes.
            // This keeps it from showing up in the diffs.
            if new_contents == file_contents {
                continue;
            }

            ctx.describe(format!("Enabled importing from the Standard Library (for {})", target.name));

            // Removing nodes can leave the line they are on, so remove any prefixed whitespace.
            let trimmed_len = new_contents.len() - new_contents.trim_start().len();
            new_contents.drain(..trimmed_len);

            ctx.fs.write(entrypoint, new_contents).await?;
        }
    }

    Ok(())
}

/// Remove all no_std/no_main attributes from the given syntax node.
pub fn remove_no_std(node: SourceFile) {
    let mut to_remove = vec![];

    for child in node.syntax().descendants() {
        let mut remove = false;

        if let Some(attr) = Attr::cast(child.clone())
            && let Some(name) = attr.simple_name()
        {
            remove = attr.kind().is_inner() && matches!(name.as_str(), "no_std" | "no_main");
            log::debug!("{attr} ({name:?}, {:?}): remove = {remove}", attr.kind());
        }

        if let Some(extern_crate) = ExternCrate::cast(child.clone())
            && let Some(name) = extern_crate.name_ref()
            && let Some(name_ident) = name.ident_token()
        {
            remove = name_ident.text() == "alloc" && extern_crate.attrs().count() == 0;
            log::debug!("{extern_crate} ({name}): remove = {remove}");
        }

        if remove {
            to_remove.push(child.clone());
        }
    }

    for attr in to_remove {
        attr.detach();
    }
}
