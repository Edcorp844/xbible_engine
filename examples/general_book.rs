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

    let modules = engine.get_book_modules();
    info!("Available modules count: {}", modules.len());

    if let Some(module) = modules.get(1) {
        info!("Reading general book: {}", module.name);

        // 1. Fetch entire Tree hierarchy
        let root_structure = engine.get_general_book_structure(module);
        info!(
            "Root Node: Title='{}', Path='{}', Direct Children Count={}",
            root_structure.title,
            root_structure.path,
            root_structure.children.len()
        );

        // Print full hierarchy structure to logs
        print_tree_node(&root_structure, 0);

        // 2. Collect all leaf nodes (nodes that have no children)
        let leaf_nodes = collect_leaf_nodes(&root_structure);
        info!("Total leaf content nodes found: {}", leaf_nodes.len());

        // 3. Render content for each leaf node
        for leaf in leaf_nodes {
            info!(
                "--- Section: [{}] {} ('{}') ---",
                leaf.id, leaf.title, leaf.path
            );

            let sections = engine.get_genearl_book_content(module, leaf);
            for section in &sections {
                section.print();
            }
        }
    } else {
        error!("No Available general book modules found to execute search");
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
