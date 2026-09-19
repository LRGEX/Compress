//! View-window tree model (pure logic, no Slint — unit-testable).
//!
//! The view window lists archive contents as a TREE: folder rows can be
//! expanded/collapsed and carry a tri-state checkbox — clicking a folder's
//! checkbox selects/deselects the ENTIRE subtree (WinRAR behavior). Without
//! this, extracting one folder out of 3,000 files means clicking 3,000 rows.
//!
//! Extraction itself is untouched: we still emit a flat list of checked FILE
//! paths to the `--only` listfile (the engine creates parent dirs itself).
//!
//! Expansion defaults are Explorer-style: folders start COLLAPSED (a 3-folder
//! archive opens as 3 rows, not 9,000), except single-child chains and a
//! single top-level wrapper folder — those auto-expand so the user sees the
//! archive's real top level immediately.

/// One row as shown in the ListView (a flattened view of the arena tree).
#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    /// Display label (file or folder name, no path).
    pub label: String,
    /// Full archive-relative path, forward slashes (files only used for extraction).
    pub path: String,
    /// Nesting depth (0 = top level).
    pub depth: usize,
    /// Folder rows expand/collapse; file rows never.
    pub is_folder: bool,
    /// Selection state (folders: aggregated over descendants).
    pub checked: bool,
    /// Some (not all) descendants are checked — visual half-state.
    pub partial: bool,
    /// Folder is currently expanded (children visible).
    pub expanded: bool,
    /// Arena node index — callbacks (toggle / expand) address nodes, not rows,
    /// because row indexes shift whenever expansion changes.
    pub node: usize,
}

#[derive(Clone, Debug)]
struct Node {
    label: String,
    path: String,
    is_folder: bool,
    parent: Option<usize>,
    children: Vec<usize>,
    checked: bool,
    partial: bool,
    expanded: bool,
}

/// Arena-backed selection tree over sorted archive paths.
#[derive(Clone, Debug)]
pub struct ViewTree {
    nodes: Vec<Node>,
    /// Children of the virtual root (depth-0 rows).
    roots: Vec<usize>,
    total_files: usize,
    /// folder path -> node idx (interning map; only folders, bounded by dir count).
    dirs: std::collections::HashMap<String, usize>,
}

impl ViewTree {
    /// Build from the archive's file list (forward-slash relative paths,
    /// any order — internally sorted). Directory entries are implicit:
    /// derived from path prefixes, so callers never need dir rows.
    pub fn build(paths: Vec<String>) -> ViewTree {
        let mut sorted = paths;
        sorted.sort();
        // The v3 path index stores DIRECTORY records too (empty-folder preservation,
        // walk_tree pushes EntKind::Dir). A path that is a strict prefix of its
        // sorted successor (successor starts with "P/") is one of those dir
        // records → DROP it: folders materialize as tree nodes from their files'
        // paths. (Without this, every folder shows TWICE: a dead file-look row
        // that can't expand + the real folder row.) A dir record with NO successor
        // under it (empty folder) is kept — it represents a real extractable entry.
        let files: Vec<String> = sorted
            .iter()
            .enumerate()
            .filter(|(i, p)| match sorted.get(i + 1) {
                Some(next) => {
                    !(next.starts_with(p.as_str()) && next.as_bytes().get(p.len()) == Some(&b'/'))
                }
                None => true,
            })
            .map(|(_, p)| p.clone())
            .collect();
        let mut tree = ViewTree {
            nodes: Vec::new(),
            roots: Vec::new(),
            total_files: 0,
            dirs: std::collections::HashMap::new(),
        };
        for full in files {
            let mut parent: Option<usize> = None;
            let mut prefix = String::new();
            // Create a folder node for every missing path component.
            let comps: Vec<&str> = full.split('/').filter(|c| !c.is_empty()).collect();
            for (i, comp) in comps.iter().enumerate() {
                let is_last = i + 1 == comps.len();
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(comp);
                if is_last {
                    // File leaf.
                    let idx = tree.push_node(comp.to_string(), prefix.clone(), false, parent);
                    if let Some(p) = parent {
                        tree.nodes[p].children.push(idx);
                    } else {
                        tree.roots.push(idx);
                    }
                    tree.total_files += 1;
                } else {
                    // Folder interior node (create once, reuse on later files).
                    let idx = match tree.dirs.get(&prefix) {
                        Some(&existing) => existing,
                        None => {
                            let idx = tree.push_node(comp.to_string(), prefix.clone(), true, parent);
                            tree.dirs.insert(prefix.clone(), idx);
                            if let Some(p) = parent {
                                tree.nodes[p].children.push(idx);
                            } else {
                                tree.roots.push(idx);
                            }
                            idx
                        }
                    };
                    parent = Some(idx);
                }
            }
        }
        // Explorer-style initial expansion:
        //  1. A folder with exactly ONE child auto-expands (chain unfolding —
        //     "a/b/c/file" shows as a,b,c nested instead of one cryptic row).
        //  2. A single top-level wrapper folder ("ArchiveName/...") expands so
        //     the user sees the archive's real top level right away.
        //  Everything else starts collapsed — 3 folders × 3,000 files opens
        //  as 3 rows, ready to check/uncheck with one click each.
        for n in tree.nodes.iter_mut() {
            if n.is_folder && n.children.len() == 1 {
                n.expanded = true;
            }
        }
        if tree.roots.len() == 1 && tree.nodes[tree.roots[0]].is_folder {
            tree.nodes[tree.roots[0]].expanded = true;
        }
        tree
    }

    /// Node index of a path (visible or not) — tests + wiring convenience.
    pub fn node_by_path(&self, path: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.path == path)
    }

    fn push_node(&mut self, label: String, path: String, is_folder: bool, parent: Option<usize>) -> usize {
        self.nodes.push(Node {
            label,
            path,
            is_folder,
            parent,
            children: Vec::new(),
            checked: true,
            partial: false,
            expanded: false,
        });
        self.nodes.len() - 1
    }

    /// Toggle a node's checkbox. Folder: if ANY descendant is selected,
    /// deselect the whole subtree; otherwise select all of it. File: flip.
    pub fn toggle(&mut self, idx: usize) {
        let target = if self.nodes[idx].is_folder {
            !(self.nodes[idx].checked || self.nodes[idx].partial)
        } else {
            !self.nodes[idx].checked
        };
        self.set_subtree(idx, target);
    }

    /// Select / deselect everything (header checkbox).
    pub fn set_all(&mut self, checked: bool) {
        for n in self.nodes.iter_mut() {
            n.checked = checked;
            n.partial = false;
        }
    }

    fn set_subtree(&mut self, idx: usize, checked: bool) {
        self.nodes[idx].checked = checked;
        self.nodes[idx].partial = false;
        let kids: Vec<usize> = self.nodes[idx].children.clone();
        for c in kids {
            self.set_subtree(c, checked);
        }
        self.refresh_ancestors(idx);
    }

    /// After a change at `start`, walk upward recomputing each folder's
    /// checked/partial state from its children (checked = all, partial = some).
    fn refresh_ancestors(&mut self, start: usize) {
        let mut cur = self.nodes[start].parent;
        while let Some(p) = cur {
            let (all, any) = {
                let mut all = true;
                let mut any = false;
                for &c in &self.nodes[p].children {
                    let n = &self.nodes[c];
                    if n.checked || n.partial {
                        any = true;
                    }
                    if !(n.checked && !n.partial) {
                        all = false;
                    }
                }
                (all, any)
            };
            self.nodes[p].checked = all;
            self.nodes[p].partial = any && !all;
            cur = self.nodes[p].parent;
        }
    }

    /// Expand/collapse a folder node (no-op for files).
    pub fn toggle_expand(&mut self, idx: usize) {
        if self.nodes[idx].is_folder {
            self.nodes[idx].expanded = !self.nodes[idx].expanded;
        }
    }

    /// Expand ALL folders (legacy "show everything" view).
    pub fn expand_all(&mut self) {
        for n in self.nodes.iter_mut() {
            if n.is_folder {
                n.expanded = true;
            }
        }
    }

    /// Flatten to the currently visible rows (DFS, skipping collapsed subtrees).
    pub fn flatten_visible(&self) -> Vec<RowView> {
        let mut out = Vec::new();
        fn walk(nodes: &Vec<Node>, kids: &[usize], depth: usize, out: &mut Vec<RowView>) {
            for &i in kids {
                let n = &nodes[i];
                out.push(RowView {
                    label: n.label.clone(),
                    path: n.path.clone(),
                    depth,
                    is_folder: n.is_folder,
                    checked: n.checked,
                    partial: n.partial,
                    expanded: n.expanded,
                    node: i,
                });
                if n.is_folder && n.expanded {
                    walk(nodes, &n.children.clone(), depth + 1, out);
                }
            }
        }
        walk(&self.nodes, &self.roots.clone(), 0, &mut out);
        out
    }

    /// Checked FILE paths (full, forward-slash) for the `--only` listfile.
    /// Folders are never emitted — the engine creates parent dirs itself.
    pub fn collect_checked_files(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| !n.is_folder && n.checked)
            .map(|n| n.path.clone())
            .collect()
    }

    /// (selected files, total files) for the status line.
    pub fn selection_stats(&self) -> (usize, usize) {
        let sel = self.nodes.iter().filter(|n| !n.is_folder && n.checked).count();
        (sel, self.total_files)
    }

    /// True when every file is selected (header checkbox state).
    pub fn all_selected(&self) -> bool {
        let (sel, total) = self.selection_stats();
        total > 0 && sel == total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixture: single root wrapper, 3 top folders (one nested), files.
    // Creation order (sorted paths): root=0, Folder1=1, a=2, b=3,
    // Folder2=4, deep=5, x=6, y=7, top.txt=8.
    fn tree() -> ViewTree {
        ViewTree::build(vec![
            "root/Folder2/y.txt".into(),
            "root/Folder1/a.txt".into(),
            "root/top.txt".into(),
            "root/Folder2/deep/x.txt".into(),
            "root/Folder1/b.txt".into(),
        ])
    }

    #[test]
    fn single_root_wrapper_auto_expands_others_collapsed() {
        let t = tree();
        let rows = t.flatten_visible();
        // root expanded (single wrapper) → depth-1 rows: Folder1, Folder2, top.txt
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["root", "Folder1", "Folder2", "top.txt"]);
        assert!(rows.iter().all(|r| r.checked && !r.partial), "all selected initially");
        // Folder1/2 collapsed: their files NOT visible
        assert!(!rows.iter().any(|r| r.label == "a.txt"));
    }

    #[test]
    fn three_root_folders_open_as_three_rows() {
        // The user's exact scenario: 3 folders, thousands of files each.
        let mut paths = Vec::new();
        for f in 1..=3 {
            for i in 0..50 {
                paths.push(format!("Folder{}/file{}.txt", f, i));
            }
        }
        let t = ViewTree::build(paths);
        let rows = t.flatten_visible();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Folder1", "Folder2", "Folder3"], "opens as 3 rows");
        assert_eq!(t.collect_checked_files().len(), 150);
    }

    #[test]
    fn unchecking_folder_deselects_whole_subtree() {
        let mut t = tree();
        let f2 = t.node_by_path("root/Folder2").unwrap();
        t.toggle(f2); // fully selected → one click deselects its 2 files
        let mut got = t.collect_checked_files();
        got.sort();
        assert_eq!(got, vec!["root/Folder1/a.txt", "root/Folder1/b.txt", "root/top.txt"]);
        // Parent "root" becomes partial
        let root_row = &t.flatten_visible()[0];
        assert!(!root_row.checked && root_row.partial, "root must be partial");
    }

    #[test]
    fn clicking_partial_folder_selects_everything_again() {
        let mut t = tree();
        let f2 = t.node_by_path("root/Folder2").unwrap();
        t.toggle(f2); // deselect
        t.toggle(f2); // empty → select all
        assert_eq!(t.collect_checked_files().len(), 5);
    }

    #[test]
    fn one_deep_file_deselect_makes_ancestors_partial() {
        let mut t = tree();
        let x = t.node_by_path("root/Folder2/deep/x.txt").unwrap();
        t.toggle(x);
        // Hidden nodes still carry correct aggregate state
        let deep = t.node_by_path("root/Folder2/deep").unwrap();
        // deep contains ONLY x.txt → fully unchecked, not partial
        assert!(!t.nodes[deep].partial && !t.nodes[deep].checked, "deep fully unchecked");
        let f2 = t.node_by_path("root/Folder2").unwrap();
        assert!(t.nodes[f2].partial && !t.nodes[f2].checked, "Folder2 must be partial");
        // Clicking partial Folder2 → deselect everything inside it
        t.toggle(f2);
        assert!(!t.collect_checked_files().iter().any(|p| p.starts_with("root/Folder2")));
    }

    #[test]
    fn collapse_hides_subtree_selection_survives() {
        let mut t = tree();
        let root = t.node_by_path("root").unwrap();
        t.toggle_expand(root); // collapse the wrapper
        let rows = t.flatten_visible();
        assert_eq!(rows.len(), 1, "collapsed root shows only itself");
        assert_eq!(t.collect_checked_files().len(), 5, "selection survives collapse");
        // And a collapsed checked folder still yields its files on extract
        t.toggle(root); // deselect everything under root
        assert_eq!(t.collect_checked_files().len(), 0);
    }

    #[test]
    fn select_all_and_stats() {
        let mut t = tree();
        assert_eq!(t.selection_stats(), (5, 5));
        assert!(t.all_selected());
        t.set_all(false);
        assert_eq!(t.selection_stats(), (0, 5));
        assert!(!t.all_selected());
        t.set_all(true);
        assert!(t.all_selected());
    }

    #[test]
    fn empty_and_flat_archives() {
        let empty = ViewTree::build(vec![]);
        assert_eq!(empty.flatten_visible().len(), 0);
        assert_eq!(empty.collect_checked_files().len(), 0);
        // Flat archive (no folders): files at root, visible immediately
        let flat = ViewTree::build(vec!["b.txt".into(), "a.txt".into()]);
        let flat_rows = flat.flatten_visible();
        let labels: Vec<&str> = flat_rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["a.txt", "b.txt"]);
        assert!(flat.flatten_visible().iter().all(|r| !r.is_folder && r.depth == 0));
    }

    #[test]
    fn single_child_chain_auto_expands() {
        let t = ViewTree::build(vec!["a/b/c/file.txt".into()]);
        let rows = t.flatten_visible();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["a", "b", "c", "file.txt"], "1-child chain unfolds");
        let depths: Vec<usize> = rows.iter().map(|r| r.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 3]);
    }

    #[test]
    fn expand_all_shows_everything() {
        let mut t = tree();
        t.expand_all();
        let rows = t.flatten_visible();
        assert_eq!(rows.len(), 9, "all 5 files + 4 folders");
    }

    #[test]
    fn dir_records_in_index_do_not_become_phantom_file_rows() {
        // Real-world shape: walk_tree pushes dir records for empty-folder
        // preservation, so the index contains BOTH "F" (dir record) and
        // "F/a.txt". The dir record must NOT appear as a file row — it showed
        // up as a dead duplicate "F" row that couldn't expand (user report:
        // "New folder" listed twice, once as a file).
        let t = ViewTree::build(vec![
            "New folder".into(),
            "New folder/New folder".into(),
            "New folder/New folder/New folder/x.txt".into(),
            "New folder/a.bmp".into(),
            "New folder/b.txt".into(),
        ]);
        let rows = t.flatten_visible();
        let names: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        // Wrapper expanded (single root); single-child chain auto-unfolds:
        // New folder → New folder → New folder → x.txt, then a.bmp, b.txt.
        assert_eq!(names, vec!["New folder", "New folder", "New folder", "x.txt", "a.bmp", "b.txt"],
            "dir records must vanish — every 'New folder' row is a FOLDER row");
        assert!(rows[0].is_folder && rows[1].is_folder && rows[2].is_folder);
        // no FILE row named "New folder" anywhere
        assert!(rows.iter().filter(|r| !r.is_folder).all(|r| r.label != "New folder"));
        // selection still covers all 3 files
        assert_eq!(t.collect_checked_files().len(), 3);
    }

    #[test]
    fn empty_folder_record_is_kept_as_a_row() {
        // A dir record with NO files under it ("empty/") survives the filter —
        // it is a real archive entry the user can extract.
        let t = ViewTree::build(vec!["empty".into(), "z.txt".into()]);
        let t_rows = t.flatten_visible();
        let names: Vec<&str> = t_rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(names, vec!["empty", "z.txt"]);
        // "empty" has nothing under it → treated as an entry row (not expandable)
        assert!(!t.flatten_visible()[0].is_folder);
    }

    #[test]
    fn all_dir_records_filtered_in_nested_tree() {
        // Every level contributes a dir record; none may survive as file rows.
        let t = ViewTree::build(vec![
            "A".into(),
            "A/B".into(),
            "A/B/C".into(),
            "A/B/C/f.txt".into(),
            "A/g.txt".into(),
        ]);
        let rows = t.flatten_visible();
        let files: Vec<&str> = rows.iter().filter(|r| !r.is_folder).map(|r| r.label.as_str()).collect();
        assert_eq!(files, vec!["f.txt", "g.txt"], "only real files appear as file rows (chain auto-expands so f.txt is visible)");
        // A/B/C single-child chain: A expanded (single root) → B visible, expanded → C visible, expanded → f.txt
        assert_eq!(t.collect_checked_files(), vec!["A/B/C/f.txt", "A/g.txt"]);
    }

    #[test]
    fn dup_dir_and_file_names_do_not_confuse_build() {
        // "notes.txt" file record + "notes.txt/inner.md": the file record is a
        // strict prefix of the folder path → filtered as a dir record (a tar
        // writing both would clobber at extract time anyway — folder wins).
        let t = ViewTree::build(vec![
            "notes.txt".into(),
            "notes.txt/inner.md".into(),
        ]);
        let rows = t.flatten_visible();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "notes.txt");
        assert_eq!(rows[0].is_folder, true, "folder row only — no dead duplicate");
        // folder has 1 child → auto-expanded
        assert_eq!(rows[1].label, "inner.md");
        assert_eq!(rows[1].is_folder, false);
    }

    #[test]
    fn row_node_index_addresses_hidden_nodes() {
        // Row.node must be stable regardless of visibility so callbacks can
        // toggle folders that are currently collapsed.
        let mut t = tree();
        let f1 = t.node_by_path("root/Folder1").unwrap();
        t.toggle_expand(f1);
        let rows = t.flatten_visible();
        let row = rows.iter().find(|r| r.label == "a.txt").unwrap();
        t.toggle(row.node);
        assert!(!t.collect_checked_files().contains(&"root/Folder1/a.txt".to_string()));
    }
}
