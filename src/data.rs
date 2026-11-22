use crate::ink::Tool;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FileBasicInfo, FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
};

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Setlist {
    pub id: String,
    pub name: String,
    pub scores: Vec<PathBuf>,
}

impl Setlist {
    pub fn new(name: String) -> Self {
        Self {
            id: unique_name(),
            name,
            scores: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub language: crate::language::LanguagePreference,
    pub tap_turn: bool,
    pub pdf_default_prompt_seen: bool,
    pub bpm: u16,
    pub beats: u8,
    pub pen: Style,
    pub highlighter: Style,
    pub rectangle: Style,
    pub staff: Style,
    pub active_tool: Tool,
    pub active_preset: Option<u8>,
    pub(crate) pen_presets: [Option<Style>; 3],
    pub(crate) highlighter_presets: [Option<Style>; 3],
    #[serde(skip_serializing)]
    pub(crate) favorites: [Option<Favorite>; 4],
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct Favorite {
    highlighter: bool,
    style: Style,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetTool {
    Pen,
    Highlighter,
}

impl Settings {
    pub fn preset(&self, tool: PresetTool, slot: u8) -> Style {
        let index = usize::from(slot);
        let saved = match tool {
            PresetTool::Pen => self.pen_presets[index],
            PresetTool::Highlighter => self.highlighter_presets[index],
        };
        let settings = Settings::default();
        saved.unwrap_or_else(|| match tool {
            PresetTool::Pen => Style {
                color: [0x8a481b, 0x4e2a92, 0x425c1e][index],
                ..settings.pen
            },
            PresetTool::Highlighter => Style {
                color: [0x48d7ff, 0x96e17c, 0xf6b0d9][index],
                ..settings.highlighter
            },
        })
    }

    pub fn preset_mut(&mut self, tool: PresetTool, slot: u8) -> &mut Style {
        let initial = self.preset(tool, slot);
        let slots = match tool {
            PresetTool::Pen => &mut self.pen_presets,
            PresetTool::Highlighter => &mut self.highlighter_presets,
        };
        slots[usize::from(slot)].get_or_insert(initial)
    }

    pub fn selected_preset(&self) -> Option<(PresetTool, u8)> {
        let slot = self.active_preset.filter(|slot| *slot < 3)?;
        Some((self.active_tool.preset_group()?, slot))
    }

    fn migrate_presets(&mut self) {
        for (index, favorite) in std::mem::take(&mut self.favorites).into_iter().enumerate() {
            let Some(favorite) = favorite else { continue };
            let slots = if favorite.highlighter {
                &mut self.highlighter_presets
            } else {
                &mut self.pen_presets
            };
            if let Some(slot) = slots.get_mut(index).filter(|slot| slot.is_none()) {
                *slot = Some(favorite.style);
            } else if let Some(slot) = slots.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(favorite.style);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Style {
    pub color: u32,
    pub opacity: u8,
    pub width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Default::default(),
            tap_turn: false,
            pdf_default_prompt_seen: false,
            bpm: 96,
            beats: 4,
            pen: Style {
                color: 0xb04820,
                opacity: 255,
                width: 0.0035,
            },
            highlighter: Style {
                color: 0x32eaff,
                opacity: 100,
                width: 0.024,
            },
            rectangle: Style {
                color: 0xffffff,
                opacity: 255,
                width: 0.0,
            },
            staff: Style {
                color: 0x302b28,
                opacity: 255,
                width: 0.0015,
            },
            active_tool: Tool::default(),
            active_preset: None,
            pen_presets: [None; 3],
            highlighter_presets: [None; 3],
            favorites: [None; 4],
        }
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn open() -> io::Result<Self> {
        let root = std::env::var_os("SCOREDRAW_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|path| PathBuf::from(path).join("ScoreDraw"))
            })
            .ok_or_else(|| {
                io::Error::other(tr!(
                    "La cartella dei dati di ScoreDraw non è disponibile.",
                    "The ScoreDraw data folder is unavailable."
                ))
            })?;
        Ok(Self { root })
    }

    pub fn settings(&self) -> io::Result<Settings> {
        let mut settings: Settings = read_or_default(&self.root.join("settings.json"))?;
        settings.migrate_presets();
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json(&self.root.join("settings.json"), settings)
    }

    pub fn setlists(&self) -> io::Result<Vec<Setlist>> {
        let mut lists = Vec::new();
        let folder = self.root.join("setlists");
        let files = match fs::read_dir(&folder) {
            Ok(files) => files,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(lists),
            Err(error) => return Err(error),
        };
        for entry in files {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let list: Setlist = serde_json::from_slice(&fs::read(&path)?)?;
                if self.setlist_path(&list.id)? != path {
                    return Err(io::Error::other(trf!(
                        "Identificativo scaletta incongruente in {}.",
                        "Setlist ID mismatch in {}.",
                        path.display()
                    )));
                }
                lists.push(list);
            }
        }
        lists.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(lists)
    }

    pub fn save_setlist(&self, list: &Setlist) -> io::Result<()> {
        write_json(&self.setlist_path(&list.id)?, list)
    }

    pub fn delete_setlist(&self, id: &str) -> io::Result<()> {
        fs::remove_file(self.setlist_path(id)?)
    }

    fn setlist_path(&self, id: &str) -> io::Result<PathBuf> {
        if id.is_empty() || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err(io::Error::other(tr!(
                "Identificativo scaletta non valido.",
                "Invalid setlist ID."
            )));
        }
        Ok(self.root.join("setlists").join(format!("{id}.json")))
    }

    pub fn page_path(&self, document: &ScoreFile, page: u32) -> PathBuf {
        self.annotation_directory(document)
            .join(format!("{page}.json"))
    }

    fn annotation_directory(&self, document: &ScoreFile) -> PathBuf {
        let stamp = document.stamp;
        let mut digest = Sha256::new();
        digest.update(stamp.volume.to_le_bytes());
        digest.update(stamp.id);
        digest.update(stamp.revision.created.to_le_bytes());
        digest.update(stamp.revision.written.to_le_bytes());
        digest.update(stamp.revision.changed.to_le_bytes());
        digest.update(stamp.revision.bytes.to_le_bytes());
        // Legacy sidecars only identify a path and cannot establish which PDF they belong to.
        self.root
            .join("markup-by-file")
            .join(format!("{:x}", digest.finalize()))
    }

    pub fn publish_pdf(
        &self,
        source: &ScoreFile,
        write: impl FnOnce(&mut fs::File) -> io::Result<()>,
    ) -> io::Result<()> {
        let root = self.annotation_directory(source);
        fs::create_dir_all(&root)?;
        let _lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(root.join("incorporazione.lock"))?;
        let _original = source.open_checked()?;
        staged_write(&source.path, write, |stage| {
            source.open_checked()?;
            fs::rename(stage, &source.path)
        })
    }

    pub fn bookmarks_path(&self, document: &ScoreFile) -> PathBuf {
        self.annotation_directory(document).join("bookmarks.json")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreFile {
    pub path: PathBuf,
    stamp: FileStamp,
}

impl ScoreFile {
    pub fn open(path: &Path) -> io::Result<(Self, fs::File)> {
        let path = path.canonicalize()?;
        let file = fs::File::open(&path)?;
        let stamp = FileStamp::read(&file)?;
        Ok((Self { path, stamp }, file))
    }

    pub fn open_checked(&self) -> io::Result<fs::File> {
        // Keep the bytes stable while reading for bookmarks or export. Atomic replacements
        // may still occur, so publishing also checks that the path still points to this file.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
            .open(&self.path)?;
        if FileStamp::read(&file)? != self.stamp {
            return Err(pdf_changed());
        }
        Ok(file)
    }

    pub fn verify_revision(&self, file: &fs::File) -> io::Result<()> {
        if FileRevision::read(file)? != self.stamp.revision {
            return Err(pdf_changed());
        }
        Ok(())
    }
}

fn pdf_changed() -> io::Error {
    io::Error::other(tr!(
        "Il PDF è cambiato. Riaprilo prima di continuare.",
        "The PDF has changed. Reopen it before continuing."
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    volume: u64,
    id: [u8; 16],
    revision: FileRevision,
}

impl FileStamp {
    fn read(file: &fs::File) -> io::Result<Self> {
        let handle = HANDLE(file.as_raw_handle());
        let mut info = FILE_ID_INFO::default();
        let (volume, id) = if unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&raw mut info).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        }
        .is_ok()
        {
            (info.VolumeSerialNumber, info.FileId.Identifier)
        } else {
            let mut legacy = BY_HANDLE_FILE_INFORMATION::default();
            unsafe { GetFileInformationByHandle(handle, &mut legacy) }.map_err(io::Error::other)?;
            let index = (u64::from(legacy.nFileIndexHigh) << 32) | u64::from(legacy.nFileIndexLow);
            let mut id = [0; 16];
            id[..8].copy_from_slice(&index.to_le_bytes());
            (u64::from(legacy.dwVolumeSerialNumber), id)
        };
        if id == [0; 16] {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                tr!(
                    "Non è possibile identificare questo file.",
                    "This file cannot be uniquely identified."
                ),
            ));
        }
        Ok(Self {
            volume,
            id,
            revision: FileRevision::read(file)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileRevision {
    created: i64,
    written: i64,
    changed: i64,
    bytes: u64,
}

impl FileRevision {
    fn read(file: &fs::File) -> io::Result<Self> {
        let mut basic = FILE_BASIC_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileBasicInfo,
                (&raw mut basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        }
        .map_err(io::Error::other)?;
        Ok(Self {
            created: basic.CreationTime,
            written: basic.LastWriteTime,
            changed: basic.ChangeTime,
            bytes: file.metadata()?.len(),
        })
    }
}

pub fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error),
    }
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    atomic_write(path, |file| Ok(serde_json::to_writer(file, value)?))
}

pub type Revision = Option<[u8; 32]>;

pub fn read_page<T: DeserializeOwned + Default>(path: &Path) -> io::Result<(T, Revision, bool)> {
    let Some(bytes) = read_optional(path)? else {
        return Ok((T::default(), None, false));
    };
    let version = Some(Sha256::digest(&bytes).into());
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok((value, version, false)),
        Err(error) => {
            let backup = read_optional(&path.with_extension("bak"))?;
            if let Some(backup) = backup
                && let Ok(value) = serde_json::from_slice(&backup)
            {
                Ok((value, version, true))
            } else {
                Err(error.into())
            }
        }
    }
}

pub fn write_page<T: Serialize + DeserializeOwned>(
    path: &Path,
    expected: Revision,
    value: &T,
) -> io::Result<Revision> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::other(tr!(
            "Cartella delle annotazioni non valida.",
            "Invalid annotations folder."
        ))
    })?;
    fs::create_dir_all(parent)?;
    // Keep the same lock file across writes. Windows closes the exclusive handle if the process exits.
    let _lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path.with_extension("lock"))?;
    let current = read_optional(path)?;
    let actual = current
        .as_deref()
        .map(|content| Sha256::digest(content).into());
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            tr!(
                "Questa pagina è stata salvata da un’altra istanza di ScoreDraw.",
                "This page was saved by another instance of ScoreDraw."
            ),
        ));
    }
    if let Some(bytes) = current.as_deref()
        && serde_json::from_slice::<T>(bytes).is_ok()
    {
        atomic_write(&path.with_extension("bak"), |file| file.write_all(bytes))?;
    }
    let bytes = serde_json::to_vec(value)?;
    atomic_write(path, |file| file.write_all(&bytes))?;
    Ok(Some(Sha256::digest(&bytes).into()))
}

pub fn write_recovery<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<PathBuf> {
    let filename = path
        .file_stem()
        .and_then(|part| part.to_str())
        .unwrap_or("pagina");
    let recovery = path.with_file_name(format!("{filename}.recupero-{}.json", unique_name()));
    write_json(&recovery, value)?;
    Ok(recovery)
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    staged_write(path, write, |stage| fs::rename(stage, path))
}

pub(crate) fn atomic_write_new(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
    use windows::core::HSTRING;

    staged_write(path, write, |stage| unsafe {
        MoveFileExW(
            &HSTRING::from(stage.as_os_str()),
            &HSTRING::from(path.as_os_str()),
            MOVE_FILE_FLAGS(0),
        )
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))
    })
}

fn staged_write(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
    publish: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::other(tr!(
            "Cartella di destinazione non valida.",
            "Invalid destination folder."
        ))
    })?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!("{}.tmp", unique_name()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        write(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        publish(&temp)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn unique_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let process = std::process::id();
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{time:x}-{process:x}-{next:x}")
}

#[cfg(test)]
pub(crate) struct Scratch(pub PathBuf);

#[cfg(test)]
impl Scratch {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("scoredraw-test-{}", unique_name()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_or_recreating_a_pdf_does_not_reuse_its_local_edits() {
        use std::os::windows::ffi::OsStrExt;

        let scratch = Scratch::new();
        let path = scratch.0.join("score.pdf");
        let store = Store::at(scratch.0.join("store"));
        fs::write(&path, b"first").unwrap();
        let (original, file) = ScoreFile::open(&path).unwrap();
        let mut legacy_key = Sha256::new();
        for unit in original.path.as_os_str().encode_wide() {
            legacy_key.update(unit.to_le_bytes());
        }
        let legacy = store
            .root
            .join("markup")
            .join(format!("{:x}", legacy_key.finalize()));
        write_json(&legacy.join("0.json"), &[0]).unwrap();
        let original_page = store.page_path(&original, 0);
        assert!(!original_page.exists());
        write_json(&original_page, &[1]).unwrap();
        write_json(&store.bookmarks_path(&original), &["First"]).unwrap();
        assert_eq!(ScoreFile::open(&path).unwrap().0, original);

        let modified = file.metadata().unwrap().modified().unwrap();
        atomic_write(&path, |replacement| {
            replacement.write_all(b"other")?;
            replacement.set_times(fs::FileTimes::new().set_modified(modified))
        })
        .unwrap();
        let (replacement, file) = ScoreFile::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().modified().unwrap(), modified);
        assert_ne!(original.stamp.id, replacement.stamp.id);
        assert!(original.open_checked().is_err());
        // A save still queued for the original must never reach the replacement.
        write_json(&original_page, &[2]).unwrap();
        assert!(!store.page_path(&replacement, 0).exists());
        assert!(!store.bookmarks_path(&replacement).exists());

        write_json(&store.page_path(&replacement, 0), &[3]).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"other").unwrap();
        let (recreated, _) = ScoreFile::open(&path).unwrap();
        assert!(!store.page_path(&recreated, 0).exists());
        assert!(legacy.join("0.json").exists());
    }

    #[test]
    fn overwriting_the_same_file_starts_a_new_annotation_revision() {
        let scratch = Scratch::new();
        let path = scratch.0.join("score.pdf");
        let store = Store::at(scratch.0.join("store"));
        fs::write(&path, b"first").unwrap();
        let (original, file) = ScoreFile::open(&path).unwrap();
        write_json(&store.page_path(&original, 0), &[1]).unwrap();
        fs::write(&path, b"other").unwrap();
        let (updated, _) = ScoreFile::open(&path).unwrap();
        assert_eq!(original.stamp.id, updated.stamp.id);
        assert!(original.verify_revision(&file).is_err());
        assert!(!store.page_path(&updated, 0).exists());
    }

    #[test]
    fn creating_a_new_pdf_never_replaces_an_existing_file() {
        let scratch = Scratch::new();
        let destination = scratch.0.join("spartito.pdf");
        atomic_write_new(&destination, |file| file.write_all(b"prima copia")).unwrap();
        let error =
            atomic_write_new(&destination, |file| file.write_all(b"seconda copia")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&destination).unwrap(), b"prima copia");
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    }

    #[test]
    fn moves_legacy_presets_to_their_tool_without_sharing_slots() {
        let scratch = Scratch::new();
        let path = scratch.0.join("settings.json");
        fs::write(
            &path,
            r#"{"favorites":[
            {"highlighter":false,"style":{"color":100,"opacity":255,"width":0.004}},
            {"highlighter":true,"style":{"color":200,"opacity":90,"width":0.03}},
            null,
            {"highlighter":false,"style":{"color":300,"opacity":200,"width":0.007}}
        ]}"#,
        )
        .unwrap();
        let store = Store::at(scratch.0.clone());
        let mut settings = store.settings().unwrap();
        assert_eq!(settings.preset(PresetTool::Pen, 0).color, 100);
        assert_eq!(settings.preset(PresetTool::Highlighter, 1).color, 200);
        assert_eq!(settings.preset(PresetTool::Pen, 1).color, 300);
        *settings.preset_mut(PresetTool::Pen, 1) = Style {
            color: 400,
            opacity: 180,
            width: 0.009,
        };
        settings.active_tool = Tool::Highlighter;
        settings.active_preset = Some(1);
        store.save_settings(&settings).unwrap();
        let saved = store.settings().unwrap();
        assert_eq!(saved.selected_preset(), Some((PresetTool::Highlighter, 1)));
        let pen = saved.preset(PresetTool::Pen, 1);
        assert_eq!((pen.color, pen.opacity, pen.width), (400, 180, 0.009));
        assert_eq!(saved.preset(PresetTool::Highlighter, 1).color, 200);
    }

    #[test]
    fn rejects_stale_revision_and_recovers_previous_valid_page() {
        let scratch = Scratch::new();
        let path = scratch.0.join("page.json");
        let initial = write_page(&path, None, &vec![10]).unwrap();
        let current = write_page(&path, initial, &vec![20]).unwrap();
        assert_ne!(initial, current);
        assert_eq!(
            write_page(&path, initial, &vec![30]).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::write(&path, "incomplete").unwrap();
        let (restored, revision, recovered) = read_page::<Vec<i32>>(&path).unwrap();
        assert!(recovered);
        assert_eq!(restored, [10]);
        write_page(&path, revision, &vec![40]).unwrap();
        assert_eq!(read_page::<Vec<i32>>(&path).unwrap().0, [40]);
        assert_eq!(
            read_page::<Vec<i32>>(&path.with_extension("bak"))
                .unwrap()
                .0,
            [10]
        );
    }

    #[test]
    fn saves_order_and_preserves_links_when_deleting_a_setlist() {
        let scratch = Scratch::new();
        let first = scratch.0.join("prima.pdf");
        let second = scratch.0.join("seconda.pdf");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let store = Store {
            root: scratch.0.join("store"),
        };
        let mut setlist = Setlist::new(tr!("Concerto", "Concert").into());
        let copy = store.root.join("scores").join("legacy").join("seconda.pdf");
        fs::create_dir_all(copy.parent().unwrap()).unwrap();
        fs::copy(&second, &copy).unwrap();
        setlist.scores = vec![first.clone(), copy.clone()];
        store.save_setlist(&setlist).unwrap();
        setlist.scores.reverse();
        store.save_setlist(&setlist).unwrap();
        assert_eq!(
            store.setlists().unwrap()[0].scores,
            [copy.clone(), first.clone()]
        );
        store.delete_setlist(&setlist.id).unwrap();
        assert!(store.setlists().unwrap().is_empty());
        assert_eq!(fs::read_to_string(first).unwrap(), "first");
        assert_eq!(fs::read_to_string(second).unwrap(), "second");
        assert_eq!(fs::read_to_string(copy).unwrap(), "second");
    }
}
