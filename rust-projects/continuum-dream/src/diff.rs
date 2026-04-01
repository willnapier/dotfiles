use similar::{ChangeTag, TextDiff};
use std::fs;

use crate::types::{DreamResponse, MemoryState, ProposedChange};

/// Build a list of proposed changes from the AI response
pub fn build_changes(
    response: &DreamResponse,
    memory_state: &MemoryState,
) -> Vec<ProposedChange> {
    let mut changes = Vec::new();

    // File updates
    for update in &response.files_to_update {
        let old_content = memory_state
            .files
            .iter()
            .find(|f| f.filename == update.filename)
            .map(|f| {
                fs::read_to_string(&f.path).unwrap_or_default()
            })
            .unwrap_or_default();

        changes.push(ProposedChange::UpdateFile {
            filename: update.filename.clone(),
            old_content,
            new_content: update.content.clone(),
            reason: update.reason.clone(),
        });
    }

    // File creates
    for create in &response.files_to_create {
        changes.push(ProposedChange::CreateFile {
            filename: create.filename.clone(),
            content: create.content.clone(),
            reason: create.reason.clone(),
        });
    }

    // File deletes
    for delete in &response.files_to_delete {
        let old_content = memory_state
            .files
            .iter()
            .find(|f| f.filename == delete.filename)
            .map(|f| fs::read_to_string(&f.path).unwrap_or_default())
            .unwrap_or_default();

        changes.push(ProposedChange::DeleteFile {
            filename: delete.filename.clone(),
            old_content,
            reason: delete.reason.clone(),
        });
    }

    // Index update
    if response.memory_index != "UNCHANGED" {
        changes.push(ProposedChange::UpdateIndex {
            old_content: memory_state.index_content.clone(),
            new_content: response.memory_index.clone(),
        });
    }

    changes
}

/// Display proposed changes as a coloured unified diff
pub fn display(changes: &[ProposedChange], response: &DreamResponse, memory_state: &MemoryState) {
    if changes.is_empty() {
        println!("No changes proposed.");
        return;
    }

    let mut updates = 0;
    let mut creates = 0;
    let mut deletes = 0;

    for change in changes {
        match change {
            ProposedChange::UpdateFile {
                filename,
                old_content,
                new_content,
                reason,
            } => {
                updates += 1;
                println!("\n\x1b[36m--- {} (update) ---\x1b[0m", filename);
                println!("\x1b[90mReason: {}\x1b[0m\n", reason);
                print_diff(old_content, new_content);
            }
            ProposedChange::CreateFile {
                filename,
                content,
                reason,
            } => {
                creates += 1;
                println!("\n\x1b[32m+++ {} (create) +++\x1b[0m", filename);
                println!("\x1b[90mReason: {}\x1b[0m\n", reason);
                for line in content.lines() {
                    println!("\x1b[32m+ {}\x1b[0m", line);
                }
            }
            ProposedChange::DeleteFile {
                filename,
                old_content,
                reason,
            } => {
                deletes += 1;
                println!("\n\x1b[31m--- {} (delete) ---\x1b[0m", filename);
                println!("\x1b[90mReason: {}\x1b[0m\n", reason);
                for line in old_content.lines() {
                    println!("\x1b[31m- {}\x1b[0m", line);
                }
            }
            ProposedChange::UpdateIndex {
                old_content,
                new_content,
            } => {
                println!("\n\x1b[36m--- MEMORY.md (update) ---\x1b[0m\n");
                print_diff(old_content, new_content);

                let old_lines = old_content.lines().count();
                let new_lines = new_content.lines().count();
                let status = if new_lines <= 200 { "OK" } else { "OVER LIMIT" };
                println!(
                    "\x1b[90mMEMORY.md: {} -> {} lines ({})\x1b[0m",
                    old_lines, new_lines, status
                );
            }
        }
    }

    // Summary
    println!(
        "\n\x1b[1mDream summary:\x1b[0m {} updated, {} created, {} deleted",
        updates, creates, deletes
    );
    println!("\x1b[90m{}\x1b[0m", response.summary);

    // Show MEMORY.md line status
    if response.memory_index != "UNCHANGED" {
        let new_lines = response.memory_index.lines().count();
        if new_lines > 200 {
            println!(
                "\x1b[33mWARNING: MEMORY.md still over limit ({} > 200 lines)\x1b[0m",
                new_lines
            );
        }
    } else {
        println!(
            "\x1b[90mMEMORY.md: unchanged ({} lines)\x1b[0m",
            memory_state.index_line_count
        );
    }
}

fn print_diff(old: &str, new: &str) {
    let diff = TextDiff::from_lines(old, new);

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => print!("\x1b[31m- {}\x1b[0m", change),
            ChangeTag::Insert => print!("\x1b[32m+ {}\x1b[0m", change),
            ChangeTag::Equal => {
                // Only show a few context lines
                print!("  {}", change);
            }
        }
    }
}
