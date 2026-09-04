use log::{error, info};
use xbible_engine::engines::{
    module_engine::module_engine_extensions::{
        module_engine_genaralbook_content_ext::TreeNode, module_engine_module_content_ext::Section,
    },
    xbible_engine::engine::XBibleEngine,
};

trait Render {
    fn print(&self);
}

impl Render for Section {
    fn print(&self) {
        println!();

        // 1. Title
        for word in self.title.clone() {
            if word.is_punctuation {
                print!("{}", word.text);
            } else {
                print!(" {}", word.text);
            }
        }
        println!();

        // 2. Section Header Notes (Enoch 108 intro note, section notes, etc.)
        if !self.notes.is_empty() {
            println!("\n  [Section Notes]:");
            for note in &self.notes {
                println!(
                    "   • [{}] (n={:?}, ref={:?}): {}",
                    note.note_type, note.n, note.osis_ref, note.text
                );
            }
        }

        // 3. Verses & Verse Notes
        for verse in self.verses.clone() {
            println!();
            for word in verse.words {
                if word.is_punctuation {
                    print!("{}", word.text);
                } else {
                    print!(" {}", word.text);
                }
            }
            println!();

            if !verse.notes.is_empty() {
                println!("  [Verse Notes]:");
                for note in &verse.notes {
                    println!(
                        "   -> [{}] (n={:?}, ref={:?}): {}",
                        note.note_type, note.n, note.osis_ref, note.text
                    );
                }
            }
        }
        println!();
    }
}

fn main() {
    xbible_engine::init_logging();
    let engine = XBibleEngine::new();

    let modules = engine.get_bible_modules();
    info!("Available modules : {:?}", modules);

    if let Some(module) = modules.get(6) {
         info!("[Test] Found module {}", module.name);
        let books = engine.get_books(&module.name);
       

        let Some(first_book) = books.first() else {
            info!("[Test] Module '{}' has no books available!", module.name);
            return;
        };

        info!("[Test] Using first book: {}", first_book.name);

        let first_chapter_num = first_book.chapters.first().map(|c| c.number).unwrap_or(1);

        let reference = format!("{} {}", first_book.name, first_chapter_num);
        info!(
            "[Test] Querying chapter sections for reference: '{}'...",
            reference
        );

        let sections = engine.get_chapter_content(&module.name, &reference);
        info!(
            "[Test] Successfully retrieved {} section(s) for '{}'",
            sections.len(),
            reference
        );
        for section in &sections {
            section.print();
        }
    }
}

/// Recursively collects all nodes that have no children (leaf nodes containing content)
fn collect_leaf_nodes(node: &TreeNode) -> Vec<TreeNode> {
    let mut leaves = Vec::new();

    if node.children.is_empty() {
        // Skip root "/" node if empty
        if node.path != "/" {
            leaves.push(node.clone());
        }
    } else {
        for child in &node.children {
            leaves.extend(collect_leaf_nodes(child));
        }
    }

    leaves
}

/// Helper function to visually print tree structure in logs
fn print_tree_node(node: &TreeNode, indent_level: usize) {
    let indent = "  ".repeat(indent_level);
    let is_leaf = node.children.is_empty();

    info!(
        "{}- [{}] {} (Path: '{}') {}",
        indent,
        node.id,
        node.title,
        node.path,
        if is_leaf { "[LEAF]" } else { "" }
    );

    for child in &node.children {
        print_tree_node(child, indent_level + 1);
    }
}
