use std::ffi::{CStr, CString};

use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::{
    engines::{
        module_engine::{
            module_engine::ModuleEngine,
            module_engine_extensions::module_engine_module_content_ext::Section,
            sword_module::module::SwordModule,
        },
        osis_translation_engine::engine::OsisTransilationEngine,
    },
    ffi::{
        org_crosswire_sword_SWMgr_getModuleByName, org_crosswire_sword_SWModule_begin,
        org_crosswire_sword_SWModule_getKeyText, org_crosswire_sword_SWModule_getRawEntry,
        org_crosswire_sword_SWModule_next, org_crosswire_sword_SWModule_popError,
        org_crosswire_sword_SWModule_setKeyText,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct TreeNode {
    pub id: u32,
    pub title: String,
    pub path: String, // e.g., "/Book 1/Chapter 2/Section 1"
    pub depth: i32,
    pub children: Vec<TreeNode>,
}

impl Default for TreeNode {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            path: String::new(),
            depth: 0,
            children: Vec::new(),
        }
    }
}

impl ModuleEngine {
    /// Constructs a hierarchical `TreeNode` representation for General Book modules (`RawGenBook` / TreeKey).
    pub fn get_general_book_structure(&self, book_module: &SwordModule) -> TreeNode {
        // SWORD configurations can label these as "Generic Books" or "General Books"
        if !book_module.category.eq_ignore_ascii_case("Generic Books")
            && !book_module.category.eq_ignore_ascii_case("General Books")
        {
            error!("Module is not a general book, returning default empty tree");
            return TreeNode::default();
        }

        let c_mod_name = match CString::new(book_module.name.as_str()) {
            Ok(name) => name,
            Err(e) => {
                error!("Invalid module name string: {e}");
                return TreeNode::default();
            }
        };

        let inner = self.inner.lock().unwrap();

        let mut root = TreeNode {
            id: 0,
            title: book_module.name.clone(),
            path: "/".to_string(),
            depth: 0,
            children: Vec::new(),
        };

        unsafe {
            let h_module =
                org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, c_mod_name.as_ptr());
            if h_module == 0 {
                error!(
                    "Failed to retrieve C SWModule handle for {}",
                    book_module.name
                );
                return root;
            }

            org_crosswire_sword_SWModule_begin(h_module);

            let mut node_id_counter: u32 = 1;

            // Stack storing mutable pointers to active parent nodes along the current tree branch
            let mut stack: Vec<*mut TreeNode> = vec![&mut root as *mut TreeNode];

            loop {
                if org_crosswire_sword_SWModule_popError(h_module) != 0 {
                    break;
                }

                let key_ptr = org_crosswire_sword_SWModule_getKeyText(h_module);
                if key_ptr.is_null() {
                    break;
                }

                let key = CStr::from_ptr(key_ptr).to_string_lossy();
                let trimmed_key = key.trim_matches('/');

                if !trimmed_key.is_empty() {
                    let parts: Vec<&str> = trimmed_key.split('/').collect();
                    let depth = parts.len() as i32;
                    let title = parts.last().unwrap_or(&"").to_string();

                    let new_node = TreeNode {
                        id: node_id_counter,
                        title,
                        path: key.to_string(),
                        depth,
                        children: Vec::new(),
                    };
                    node_id_counter += 1;

                    // Adjust the parent stack depth so it matches the current node's parent level
                    let target_depth = depth as usize;
                    while stack.len() > target_depth {
                        stack.pop();
                    }

                    // Push child into active parent's children vector
                    if let Some(&parent_ptr) = stack.last() {
                        let parent = &mut *parent_ptr;
                        parent.children.push(new_node);

                        // Push the newly appended child onto the stack as potential parent for next loop iteration
                        let added_child_ptr = parent.children.last_mut().unwrap() as *mut TreeNode;
                        stack.push(added_child_ptr);
                    }
                }

                org_crosswire_sword_SWModule_next(h_module);
            }
        }

        root
    }

    /// Fetches and parses OSIS content for a General Book tree node (or subtree) into structured Sections.
    /// Fetches and parses OSIS content for a General Book tree node (or subtree) into structured Sections.
    pub fn get_genearl_book_content(
        &self,
        book_module: &SwordModule,
        node: TreeNode,
    ) -> Vec<Section> {
        let mut raw_entries: Vec<(String, String)> = Vec::new();

        // 1. Enable global options (Headings, Footnotes, etc.)
        let options = ["Headings"];
        unsafe { self.set_global_options(&options, "On") };

        // 2. Lock module handle
        let inner = self.inner.lock().unwrap();
        let mod_name = match CString::new(book_module.name.as_str()) {
            Ok(name) => name,
            Err(e) => {
                error!("[SWORD ERROR]: Invalid module name: {e}");
                return Vec::new();
            }
        };

        unsafe {
            let h_mod = org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, mod_name.as_ptr());
            if h_mod == 0 {
                error!("[SWORD ERROR]: Module '{}' not found!", book_module.name);
                return Vec::new();
            }

            // Pass h_mod (isize) directly into the helper
            self.collect_raw_osis_entries(h_mod, &node, &mut raw_entries);
        }

        info!(
            "[SWORD] Collected {} raw OSIS entries for GenBook path '{}'",
            raw_entries.len(),
            node.path
        );

        if raw_entries.is_empty() {
            return Vec::new();
        }

        info!("Raw Entry: {:?}", raw_entries);

        // 3. Hand off raw OSIS entries directly to the translation engine
        let engine = OsisTransilationEngine::new();
        engine.parse_osis_list_to_sections(book_module.language.clone(), raw_entries)
    }

    /// Helper method to recursively collect `(path_key, raw_xml)` tuples for leaf and branch nodes.
    unsafe fn collect_raw_osis_entries(
        &self,
        h_mod: isize,
        node: &TreeNode,
        entries: &mut Vec<(String, String)>,
    ) {
        if let Ok(c_path) = CString::new(node.path.as_str()) {
            unsafe {
                org_crosswire_sword_SWModule_setKeyText(h_mod, c_path.as_ptr());

                if org_crosswire_sword_SWModule_popError(h_mod) == 0 {
                    let raw_ptr = org_crosswire_sword_SWModule_getRawEntry(h_mod);
                    if let Some(raw_xml) = self.sword_ptr_to_string(raw_ptr) {
                        let trimmed = raw_xml.trim();
                        if !trimmed.is_empty() {
                            entries.push((node.path.clone(), trimmed.to_string()));
                        }
                    }
                }
            }
        }

        // Recursively traverse children if target node is a parent folder (e.g. '/Enoch')
        for child in &node.children {
            unsafe {
                self.collect_raw_osis_entries(h_mod, child, entries);
            }
        }
    }
}
