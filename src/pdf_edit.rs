use crate::data::{self, ScoreFile, Store};
use crate::ink::Mark;
use lopdf::{
    Dictionary, Document, Object, ObjectId, Stream, decode_text_string, dictionary, text_string,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::fs::File;
use std::io::{self, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Foundation::RECT;

type EditResult<T> = Result<T, Box<dyn std::error::Error>>;

pub type Snapshots = HashMap<PathBuf, Arc<Vec<Mark>>>;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Bookmarks {
    pub entries: Vec<Bookmark>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Bookmark {
    pub title: String,
    pub page: Option<u32>,
    pub depth: usize,
    id: Option<ObjectId>,
    retargeted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookmarkPosition {
    First,
    Last,
    After(usize),
    Child(usize),
}

impl Bookmarks {
    pub fn read(source: &ScoreFile, store: &Store) -> EditResult<Self> {
        let file = source.open_checked()?;
        let path = store.bookmarks_path(source);
        if path.try_exists()? {
            return Ok(data::read_or_default(&path)?);
        }
        let doc = Document::load_from(file)?;
        Self::from_pdf(&doc)
    }

    fn from_pdf(doc: &Document) -> EditResult<Self> {
        let mut bookmarks = Self::default();
        let Some(root) = doc
            .catalog()?
            .get(b"Outlines")
            .ok()
            .and_then(|value| resolve(doc, value))
            .and_then(|value| value.as_dict().ok())
        else {
            return Ok(bookmarks);
        };
        let pages = doc
            .get_pages()
            .into_iter()
            .map(|(page, id)| (id, page))
            .collect();
        let mut outline = OutlineReader {
            doc,
            pages,
            visited: HashSet::new(),
            entries: &mut bookmarks.entries,
        };
        outline.read(root, 0);
        Ok(bookmarks)
    }

    pub fn insert(&mut self, position: BookmarkPosition, title: String, page: u32) -> Option<usize> {
        let (index, depth) = match position {
            BookmarkPosition::First => (0, 0),
            BookmarkPosition::Last => (self.entries.len(), 0),
            BookmarkPosition::After(selected) => {
                let depth = self.entries.get(selected)?.depth;
                (self.subtree_end(selected), depth)
            },
            BookmarkPosition::Child(selected) => {
                let depth = self.entries.get(selected)?.depth + 1;
                if depth >= 128 { return None; }
                (self.subtree_end(selected), depth)
            }
        };
        self.entries.insert(index, Bookmark { title, page: Some(page), depth, id: None, retargeted: true });
        Some(index)
    }

    pub fn retarget(&mut self, index: usize, page: u32) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.page = Some(page);
            entry.retargeted = true;
        }
    }

    pub fn remove(&mut self, index: usize) {
        let end = self.subtree_end(index);
        for child in &mut self.entries[index + 1..end] {
            child.depth -= 1;
        }
        self.entries.remove(index);
    }

    #[cfg(test)]
    pub fn reorder(&mut self, index: usize, direction: i32) -> usize {
        let end = self.subtree_end(index);
        let depth = self.entries[index].depth;
        if direction > 0
            && self
                .entries
                .get(end)
                .is_some_and(|item| item.depth == depth)
        {
            let following = self.subtree_end(end);
            self.entries[index..following].rotate_left(end - index);
            return index + following - end;
        }
        if direction < 0
            && let Some(previous) = (0..index)
                .rev()
                .find(|&other| self.entries[other].depth <= depth)
            && self.entries[previous].depth == depth
        {
            self.entries[previous..end].rotate_right(end - index);
            return previous;
        }
        index
    }

    fn subtree_end(&self, index: usize) -> usize {
        let depth = self.entries[index].depth;
        index
            + 1
            + self.entries[index + 1..]
                .iter()
                .take_while(|entry| entry.depth > depth)
                .count()
    }

    fn write_pdf(&self, doc: &mut Document) -> EditResult<()> {
        if self.entries.is_empty() {
            doc.catalog_mut()?.remove(b"Outlines");
            return Ok(());
        }
        let pages = doc.get_pages();
        let ids: Vec<_> = self.entries.iter().map(|_| doc.new_object_id()).collect();
        let root = doc.new_object_id();
        let root_index = ids.len();
        let mut children = vec![Vec::new(); root_index + 1];
        let mut parents = Vec::new();
        let mut ancestry = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            ancestry.truncate(entry.depth);
            let parent = ancestry.last().copied().unwrap_or(root_index);
            children[parent].push(index);
            parents.push(parent);
            ancestry.push(index);
        }
        let node_ids = |index: usize| {
            if index == root_index {
                root
            } else {
                ids[index]
            }
        };
        let mut siblings = vec![(None, None); root_index];
        for group in &children {
            for (position, &index) in group.iter().enumerate() {
                siblings[index] = (
                    position.checked_sub(1).map(|i| ids[group[i]]),
                    group.get(position + 1).map(|&i| ids[i]),
                );
            }
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let mut node = entry
                .id
                .and_then(|id| doc.get_dictionary(id).ok())
                .cloned()
                .unwrap_or_default();
            let collapsed = node
                .get(b"Count")
                .and_then(Object::as_i64)
                .is_ok_and(|v| v < 0);
            for key in [
                b"Parent".as_slice(),
                b"Prev",
                b"Next",
                b"First",
                b"Last",
                b"Count",
            ] {
                node.remove(key);
            }
            node.set("Title", text_string(&entry.title));
            node.set("Parent", Object::Reference(node_ids(parents[index])));
            if let Some(previous) = siblings[index].0 {
                node.set("Prev", Object::Reference(previous));
            }
            if let Some(next) = siblings[index].1 {
                node.set("Next", Object::Reference(next));
            }
            if let (Some(&first), Some(&last)) = (children[index].first(), children[index].last()) {
                node.set("First", Object::Reference(ids[first]));
                node.set("Last", Object::Reference(ids[last]));
                let count = (self.subtree_end(index) - index - 1) as i64;
                node.set("Count", if collapsed { -count } else { count });
            }
            if entry.retargeted {
                let dest = entry.page.and_then(|page| pages.get(&page)).ok_or(tr!(
                    "La destinazione del segnalibro non esiste più.",
                    "The bookmark destination no longer exists."
                ))?;
                node.remove(b"A");
                node.set(
                    "Dest",
                    vec![Object::Reference(*dest), Object::Name(b"Fit".to_vec())],
                );
            }
            doc.set_object(ids[index], node);
        }
        let tops = &children[root_index];
        doc.set_object(
            root,
            dictionary! {
                "Type" => "Outlines",
                "First" => Object::Reference(ids[tops[0]]),
                "Last" => Object::Reference(ids[tops[tops.len() - 1]]),
                "Count" => self.entries.len() as i64,
            },
        );
        doc.catalog_mut()?.set("Outlines", Object::Reference(root));
        Ok(())
    }
}

struct OutlineReader<'a> {
    doc: &'a Document,
    pages: HashMap<ObjectId, u32>,
    visited: HashSet<ObjectId>,
    entries: &'a mut Vec<Bookmark>,
}

impl OutlineReader<'_> {
    fn read(&mut self, parent: &Dictionary, depth: usize) {
        if depth >= 128 {
            return;
        }
        let mut next = parent.get(b"First").and_then(Object::as_reference).ok();
        while let Some(id) = next {
            if !self.visited.insert(id) {
                break;
            }
            let Ok(node) = self.doc.get_dictionary(id) else {
                break;
            };
            let title = node
                .get(b"Title")
                .ok()
                .and_then(|value| resolve(self.doc, value))
                .and_then(|value| decode_text_string(value).ok())
                .unwrap_or_else(|| tr!("(Senza titolo)", "(Untitled)").into());
            let dest = node.get(b"Dest").ok().or_else(|| {
                let action = resolve(self.doc, node.get(b"A").ok()?)?.as_dict().ok()?;
                (action.get(b"S").and_then(Object::as_name).ok()? == b"GoTo")
                    .then(|| action.get(b"D").ok())
                    .flatten()
            });
            let page = dest.and_then(|dest| self.page(dest, 0));
            self.entries.push(Bookmark {
                title,
                page,
                depth,
                id: Some(id),
                retargeted: false,
            });
            self.read(node, depth + 1);
            next = node.get(b"Next").and_then(Object::as_reference).ok();
        }
    }

    fn page(&self, dest: &Object, depth: u8) -> Option<u32> {
        if depth >= 16 {
            return None;
        }
        let dest = resolve(self.doc, dest)?;
        match dest {
            Object::Array(array) => match array.first()? {
                Object::Reference(id) => self.pages.get(id).copied(),
                Object::Integer(index) => u32::try_from(*index).ok()?.checked_add(1),
                _ => None,
            },
            Object::Dictionary(dict) => self.page(dict.get(b"D").ok()?, depth + 1),
            Object::Name(name) | Object::String(name, _) => {
                let catalog = self.doc.catalog().ok()?;
                let old = catalog
                    .get(b"Dests")
                    .ok()
                    .and_then(|obj| resolve(self.doc, obj))
                    .and_then(|obj| obj.as_dict().ok())
                    .and_then(|dict| dict.get(name).ok());
                let named = old.or_else(|| {
                    let names = resolve(self.doc, catalog.get(b"Names").ok()?)?
                        .as_dict()
                        .ok()?;
                    find_name(self.doc, names.get(b"Dests").ok()?, name, 0)
                })?;
                self.page(named, depth + 1)
            }
            _ => None,
        }
    }
}

fn resolve<'a>(doc: &'a Document, value: &'a Object) -> Option<&'a Object> {
    doc.dereference(value).ok().map(|(_, value)| value)
}

fn find_name<'a>(
    doc: &'a Document,
    root: &'a Object,
    name: &[u8],
    depth: u8,
) -> Option<&'a Object> {
    if depth >= 16 {
        return None;
    }
    let dict = resolve(doc, root)?.as_dict().ok()?;
    if let Ok(names) = dict.get(b"Names").and_then(Object::as_array) {
        return names
            .as_chunks::<2>()
            .0
            .iter()
            .find(|pair| pair[0].as_str().ok() == Some(name))
            .map(|pair| &pair[1]);
    }
    let kids = dict.get(b"Kids").and_then(Object::as_array).ok()?;
    kids.iter()
        .find_map(|child| find_name(doc, child, name, depth + 1))
}

pub fn export(
    source: &ScoreFile,
    destination: &Path,
    store: &Store,
    bookmarks: Option<&Bookmarks>,
    snapshots: Option<&Snapshots>,
) -> EditResult<()> {
    if destination.canonicalize().ok().as_deref() == Some(&source.path) {
        return Err(tr!(
            "Scegli un nome diverso per la copia PDF.",
            "Choose a different filename for the PDF copy."
        )
        .into());
    }
    let (mut input, mut doc, changed) = prepare_export(source, store, bookmarks, snapshots)?;
    data::atomic_write(destination, |file| {
        write_copy(&mut input, &mut doc, changed, file)
    })?;
    Ok(())
}

pub fn export_to_folder(
    source: &ScoreFile,
    directory: &Path,
    store: &Store,
    bookmarks: Option<&Bookmarks>,
    snapshots: &Snapshots,
) -> EditResult<PathBuf> {
    let (mut input, mut doc, changed) = prepare_export(source, store, bookmarks, Some(snapshots))?;
    let stem = source.path.file_stem().unwrap_or_default();
    for number in 1..=u32::MAX {
        let mut name = stem.to_os_string();
        if number == 1 {
            name.push(" - annotato.pdf");
        } else {
            name.push(format!(" - annotato ({number}).pdf"));
        }
        let path = directory.join(name);
        if path.try_exists()? {
            continue;
        }
        match data::atomic_write_new(&path, |file| {
            write_copy(&mut input, &mut doc, changed, file)
        }) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        tr!(
            "Non è disponibile un nome per la copia PDF.",
            "No filename is available for the PDF copy."
        ),
    )
    .into())
}

pub fn finalize_original(
    source: &ScoreFile,
    store: &Store,
    bookmarks: Option<&Bookmarks>,
    snapshots: &Snapshots,
) -> EditResult<bool> {
    let (_input, mut doc, changed) = prepare_export(source, store, bookmarks, Some(snapshots))?;
    if changed {
        store.publish_pdf(source, |file| save(&mut doc, file))?;
    }
    Ok(changed)
}

fn write_copy(
    input: &mut File,
    doc: &mut Document,
    changed: bool,
    file: &mut std::fs::File,
) -> io::Result<()> {
    if changed {
        save(doc, file)
    } else {
        input.rewind()?;
        io::copy(input, file).map(|_| ())
    }
}

fn save(doc: &mut Document, file: &mut std::fs::File) -> io::Result<()> {
    doc.save_to(file).map(|_| ()).map_err(io::Error::other)
}

fn prepare_export(
    source: &ScoreFile,
    store: &Store,
    bookmarks: Option<&Bookmarks>,
    snapshots: Option<&Snapshots>,
) -> EditResult<(File, Document, bool)> {
    let mut input = source.open_checked()?;
    let mut doc = Document::load_from(&mut input)?;
    if doc.is_encrypted() || doc.trailer.has(b"Encrypt") {
        return Err(tr!(
            "La scrittura dei PDF protetti non è disponibile.",
            "Protected PDFs can’t be updated."
        )
        .into());
    }
    let mut changed = bookmarks.is_some();
    if let Some(bookmarks) = bookmarks {
        bookmarks.write_pdf(&mut doc)?;
    }
    if let Some(snapshots) = snapshots {
        for (page, id) in doc.get_pages() {
            let path = store.page_path(source, page - 1);
            if let Some(marks) = snapshots.get(&path) {
                changed |= !marks.is_empty();
                append_layer(&mut doc, id, marks)?;
            } else {
                let (marks, _, _) = data::read_page::<Vec<Mark>>(&path)?;
                changed |= !marks.is_empty();
                append_layer(&mut doc, id, &marks)?;
            }
        }
        if doc
            .version
            .parse::<f32>()
            .is_ok_and(|version| version < 1.4)
        {
            doc.version = "1.4".into();
        }
    }
    Ok((input, doc, changed))
}

fn inherited<'a>(doc: &'a Document, mut id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    for _ in 0..128 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return resolve(doc, value);
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

fn page_box(doc: &Document, id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    let numbers = inherited(doc, id, key)?.as_array().ok()?;
    let [a, b, c, d] = numbers.as_slice() else {
        return None;
    };
    let [a, b, c, d] = [
        a.as_float().ok()?,
        b.as_float().ok()?,
        c.as_float().ok()?,
        d.as_float().ok()?,
    ];
    [a, b, c, d]
        .iter()
        .all(|n| n.is_finite())
        .then_some([a.min(c), b.min(d), a.max(c), b.max(d)])
}

fn append_layer(doc: &mut Document, id: ObjectId, marks: &[Mark]) -> EditResult<()> {
    if marks.is_empty() {
        return Ok(());
    }
    let [mut x0, mut y0, mut x1, mut y1] = page_box(doc, id, b"MediaBox").ok_or(tr!(
        "Le dimensioni della pagina PDF non sono valide.",
        "The PDF has invalid page dimensions."
    ))?;
    if let Some([a, b, c, d]) = page_box(doc, id, b"CropBox") {
        (x0, y0, x1, y1) = (x0.max(a), y0.max(b), x1.min(c), y1.min(d));
    }
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0.0 || h <= 0.0 {
        return Err(tr!(
            "L’area visibile del PDF è vuota.",
            "The PDF has an empty visible area."
        )
        .into());
    }
    let rotation = inherited(doc, id, b"Rotate")
        .and_then(|v| v.as_i64().ok())
        .unwrap_or(0)
        .rem_euclid(360);
    let (vw, vh) = if rotation % 180 == 0 { (w, h) } else { (h, w) };
    let right = (vw * 2600.0 / vw.max(vh)).round().max(1.0) as i32;
    let bottom = (vh * 2600.0 / vw.max(vh)).round().max(1.0) as i32;
    let (rw, rh) = (right as f32, bottom as f32);
    let (a, b, c, d, e, f) = match rotation {
        90 => (0.0, h / rw, w / rh, 0.0, x0, y0),
        180 => (-w / rw, 0.0, 0.0, h / rh, x1, y0),
        270 => (0.0, -h / rw, -w / rh, 0.0, x1, y1),
        _ => (w / rw, 0.0, 0.0, -h / rh, x0, y1),
    };
    let mut content = format!("{a:.9} {b:.9} {c:.9} {d:.9} {e:.6} {f:.6} cm\n");
    let mut states = Dictionary::new();
    for mark in marks {
        let opacity = mark.opacity();
        states.set(
            format!("SD{opacity}"),
            dictionary! { "Type" => "ExtGState", "ca" => f32::from(opacity) / 255.0 },
        );
        mark.append_pdf(
            &mut content,
            RECT {
                left: 0,
                top: 0,
                right,
                bottom,
            },
        )?;
    }
    let bbox = [x0, y0, x1, y1]
        .into_iter()
        .map(Object::Real)
        .collect::<Vec<_>>();
    let mut form = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => bbox, "Resources" => dictionary! { "ExtGState" => states },
        },
        content.into_bytes(),
    );
    form.compress()?;
    let form_id = doc.add_object(form);
    let mut resources = inherited(doc, id, b"Resources")
        .and_then(|obj| obj.as_dict().ok())
        .cloned()
        .unwrap_or_default();
    let mut xobjects = resources
        .get(b"XObject")
        .ok()
        .and_then(|obj| resolve(doc, obj))
        .and_then(|obj| obj.as_dict().ok())
        .cloned()
        .unwrap_or_default();
    let mut name = format!("ScoreDrawInk{}", form_id.0);
    while xobjects.has(name.as_bytes()) {
        name.push('x');
    }
    xobjects.set(name.clone(), Object::Reference(form_id));
    resources.set("XObject", xobjects);
    let prior = doc.get_page_contents(id);
    let mut contents = Vec::new();
    let mut tail = String::new();
    if !prior.is_empty() {
        contents.push(Object::Reference(
            doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec())),
        ));
        contents.extend(prior.into_iter().map(Object::Reference));
        tail.push_str("Q\n");
    }
    writeln!(tail, "q /{name} Do Q")?;
    contents.push(Object::Reference(
        doc.add_object(Stream::new(Dictionary::new(), tail.into_bytes())),
    ));
    let page = doc.get_dictionary_mut(id)?;
    page.set("Resources", resources);
    page.set("Contents", contents);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_insertion_preserves_the_selected_subtree_and_depth() {
        use BookmarkPosition as Position;
        let mut bookmarks = Bookmarks::default();
        assert_eq!(bookmarks.insert(Position::After(0), "Invalid".into(), 1), None);
        bookmarks.insert(Position::Last, "Act I".into(), 1);
        bookmarks.insert(Position::Child(0), "Scene I".into(), 2);
        bookmarks.insert(Position::Child(1), "Song".into(), 3);
        assert_eq!(bookmarks.insert(Position::After(1), "Scene II".into(), 4), Some(3));
        bookmarks.insert(Position::First, "Prelude".into(), 1);
        bookmarks.insert(Position::Last, "Finale".into(), 5);
        let actual: Vec<_> = bookmarks.entries.iter().map(|entry| (entry.title.as_str(), entry.depth)).collect();
        assert_eq!(actual, [("Prelude", 0), ("Act I", 0), ("Scene I", 1), ("Song", 2), ("Scene II", 1), ("Finale", 0)]);
        bookmarks.remove(2);
        assert_eq!(bookmarks.entries[2].depth, 1);
    }

    #[test]
    fn exports_a_native_outline_and_a_vector_layer_on_a_rotated_cropped_page() {
        let scratch = data::Scratch::new();
        let store = Store::at(scratch.0.join("state"));
        let source = scratch.0.join("score.pdf");
        let output = scratch.0.join("export.pdf");
        let mut original = Document::with_version("1.3");
        let parents = original.new_object_id();
        let stream =
            original.add_object(Stream::new(Dictionary::new(), b"0 0 10 10 re f\n".to_vec()));
        let page = original.add_object(dictionary! {
            "Type" => "Page", "Parent" => Object::Reference(parents), "Contents" => Object::Reference(stream),
        });
        original.set_object(parents, dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 600.into()],
            "CropBox" => vec![10.into(), 20.into(), 290.into(), 580.into()],
            "Rotate" => 90, "Resources" => dictionary! { "ProcSet" => vec![Object::Name(b"PDF".to_vec())] },
        });
        let catalog = original
            .add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(parents) });
        original.trailer.set("Root", Object::Reference(catalog));
        let mut bookmarks = Bookmarks::default();
        bookmarks.insert(BookmarkPosition::Last, "Primo movimento".into(), 1);
        bookmarks.insert(BookmarkPosition::Last, "Épisode · ♫".into(), 1);
        bookmarks.entries[1].depth = 1;
        bookmarks.insert(BookmarkPosition::Last, "Finale".into(), 1);
        bookmarks.write_pdf(&mut original).unwrap();
        original.save(&source).unwrap();
        let (source, _) = ScoreFile::open(&source).unwrap();
        let mut bookmarks = Bookmarks::read(&source, &store).unwrap();
        assert_eq!(bookmarks.entries[1].title, "Épisode · ♫");
        assert_eq!(bookmarks.reorder(0, 1), 1);
        assert_eq!(bookmarks.entries[2].depth, 1);
        bookmarks.retarget(2, 1);
        let marks: Vec<Mark> = serde_json::from_value(serde_json::json!([{
            "kind": "shape", "staff": false,
            "style": { "color": 16711680, "opacity": 96, "width": 0.0 },
            "bounds": { "x": 0.2, "y": 0.3, "w": 0.3, "h": 0.25 }
        }]))
        .unwrap();
        let local_page = store.page_path(&source, 0);
        let snapshots: Snapshots = [(local_page.clone(), Arc::new(marks))]
            .into_iter()
            .collect();
        export(&source, &output, &store, Some(&bookmarks), Some(&snapshots)).unwrap();
        let siblings = scratch.0.join("altri-spartiti");
        std::fs::create_dir(&siblings).unwrap();
        let sibling = siblings.join("score.pdf");
        std::fs::copy(&source.path, &sibling).unwrap();
        let (sibling, _) = ScoreFile::open(&sibling).unwrap();
        let copies = scratch.0.join("copie");
        std::fs::create_dir(&copies).unwrap();
        let collision = copies.join("score - annotato.pdf");
        std::fs::write(&collision, b"existing").unwrap();
        let first =
            export_to_folder(&source, &copies, &store, Some(&bookmarks), &snapshots).unwrap();
        let second = export_to_folder(&sibling, &copies, &store, None, &snapshots).unwrap();
        assert_eq!(first, copies.join("score - annotato (2).pdf"));
        assert_eq!(second, copies.join("score - annotato (3).pdf"));
        assert_eq!(std::fs::read(&collision).unwrap(), b"existing");
        let first_copy = Document::load(first).unwrap();
        assert_eq!(
            Bookmarks::from_pdf(&first_copy).unwrap().entries[0].title,
            "Finale"
        );
        let second_copy = Document::load(second).unwrap();
        assert_eq!(
            Bookmarks::from_pdf(&second_copy).unwrap().entries[0].title,
            "Primo movimento"
        );
        assert_eq!(
            second_copy.get_page_contents(second_copy.get_pages()[&1]),
            vec![stream]
        );
        assert!(export(&source, &source.path, &store, None, None).is_err());
        let result = Document::load(&output).unwrap();
        assert_eq!(result.version, "1.4");
        let outline = Bookmarks::from_pdf(&result).unwrap();
        assert_eq!(outline.entries[0].title, "Finale");
        assert_eq!(outline.entries[2].page, Some(1));
        assert_eq!(outline.entries[2].depth, 1);
        let page = result.get_dictionary(page).unwrap();
        let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
        assert!(resources.has(b"ProcSet"));
        let objects = resources.get(b"XObject").unwrap().as_dict().unwrap();
        let (_, layer) = objects.iter().next().unwrap();
        let layer = resolve(&result, layer).unwrap().as_stream().unwrap();
        assert_eq!(
            layer.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form"
        );
        let content = String::from_utf8(layer.get_plain_content().unwrap()).unwrap();
        assert!(content.contains(" 10.000000 20.000000 cm"));
        assert!(content.contains("/SD96 gs"));
        assert_eq!(
            Document::load(&source.path)
                .unwrap()
                .get_page_contents(original.get_pages()[&1]),
            vec![stream]
        );
        assert!(finalize_original(&source, &store, Some(&bookmarks), &snapshots).unwrap());
        assert!(!local_page.parent().unwrap().join("backup").exists());
        assert!(finalize_original(&source, &store, Some(&bookmarks), &snapshots).is_err());
        let (source, _) = ScoreFile::open(&source.path).unwrap();
        assert_ne!(store.page_path(&source, 0), local_page);
        let baked = Document::load(&source.path).unwrap();
        assert_eq!(
            Bookmarks::from_pdf(&baked).unwrap().entries[0].title,
            "Finale"
        );
        let contents = baked.get_page_contents(original.get_pages()[&1]);
        let file = std::fs::read(&source.path).unwrap();
        assert!(!finalize_original(&source, &store, None, &Snapshots::new()).unwrap());
        assert_eq!(std::fs::read(&source.path).unwrap(), file);
        assert_eq!(
            Document::load(&source.path)
                .unwrap()
                .get_page_contents(original.get_pages()[&1]),
            contents
        );
    }
}
