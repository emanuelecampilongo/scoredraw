use super::Mark;
use crate::data::{self, Revision};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const SAVE_RESULT: u32 = WM_APP + 7;

#[derive(Clone, Debug, Default)]
pub enum SaveStatus {
    #[default]
    Idle,
    Saving,
    Failed(String),
    Recovered(Vec<PathBuf>),
}

pub struct Saver {
    jobs: mpsc::Sender<PathBuf>,
    ledger: Arc<Mutex<Ledger>>,
}

#[derive(Default)]
struct Ledger {
    sequence: u64,
    pages: HashMap<PathBuf, Entry>,
}

struct Entry {
    revision: Revision,
    pending: Option<Pending>,
    recovered: Option<Recovery>,
}

struct Recovery {
    path: PathBuf,
    marks: Arc<Vec<Mark>>,
}

struct Pending {
    id: u64,
    marks: Arc<Vec<Mark>>,
    error: Option<String>,
}

impl Saver {
    pub(crate) fn snapshots(&self) -> HashMap<PathBuf, Arc<Vec<Mark>>> {
        lock(&self.ledger)
            .pages
            .iter()
            .filter_map(|(path, entry)| {
                entry
                    .pending
                    .as_ref()
                    .map(|pending| &pending.marks)
                    .or_else(|| entry.recovered.as_ref().map(|recovery| &recovery.marks))
                    .map(|marks| (path.clone(), Arc::clone(marks)))
            })
            .collect()
    }

    pub fn start(hwnd: Option<HWND>) -> io::Result<Self> {
        let (jobs, queue) = mpsc::channel::<PathBuf>();
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let shared = ledger.clone();
        let window = hwnd.map(|window| window.0 as isize);
        std::thread::Builder::new()
            .name("salva-annotazioni".into())
            .spawn(move || {
                for path in queue {
                    persist(&shared, &path);
                    if let Some(window) = window {
                        let _ = unsafe {
                            PostMessageW(Some(HWND(window as _)), SAVE_RESULT, WPARAM(0), LPARAM(0))
                        };
                    }
                }
            })?;
        Ok(Self { jobs, ledger })
    }

    pub(super) fn load(&self, path: &Path) -> io::Result<(Vec<Mark>, Revision, bool)> {
        if let Some(entry) = lock(&self.ledger).pages.get(path) {
            if let Some(latest) = &entry.pending {
                return Ok((latest.marks.as_ref().clone(), entry.revision, false));
            }
            if let Some(recovery) = &entry.recovered {
                return Ok((recovery.marks.as_ref().clone(), entry.revision, false));
            }
        }
        let (marks, revision, recovered) = data::read_page(path)?;
        if let Some(entry) = lock(&self.ledger).pages.get_mut(path) {
            entry.revision = revision;
        }
        Ok((marks, revision, recovered))
    }

    pub(super) fn submit(
        &self,
        path: PathBuf,
        revision: Revision,
        marks: Vec<Mark>,
    ) -> io::Result<()> {
        let mut ledger = lock(&self.ledger);
        ledger.sequence += 1;
        let id = ledger.sequence;
        let entry = ledger.pages.entry(path.clone()).or_insert(Entry {
            revision,
            pending: None,
            recovered: None,
        });
        let previous = entry.pending.replace(Pending {
            id,
            marks: Arc::new(marks),
            error: None,
        });
        if let Err(error) = self.send(path) {
            entry.pending = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn status(&self) -> SaveStatus {
        let ledger = lock(&self.ledger);
        let mut failed = None;
        for entry in ledger.pages.values() {
            if let Some(pending) = &entry.pending {
                if pending.error.is_none() {
                    return SaveStatus::Saving;
                }
                failed = pending.error.as_deref();
            }
        }
        if let Some(failed) = failed {
            SaveStatus::Failed(failed.to_owned())
        } else {
            let mut paths: Vec<_> = ledger
                .pages
                .values()
                .filter_map(|entry| entry.recovered.as_ref())
                .map(|recovery| recovery.path.clone())
                .collect();
            if paths.is_empty() {
                SaveStatus::Idle
            } else {
                paths.sort_unstable();
                SaveStatus::Recovered(paths)
            }
        }
    }

    pub fn retry(&self) -> io::Result<()> {
        let mut ledger = lock(&self.ledger);
        for (path, entry) in &mut ledger.pages {
            if let Some(pending) = &mut entry.pending
                && pending.error.is_some()
            {
                self.send(path.clone())?;
                pending.error = None;
            }
        }
        Ok(())
    }

    fn send(&self, path: PathBuf) -> io::Result<()> {
        self.jobs.send(path).map_err(|_| {
            io::Error::other(tr!(
                "Il salvataggio in background è stato interrotto.",
                "Background saving stopped."
            ))
        })
    }
}

fn persist(shared: &Mutex<Ledger>, path: &Path) {
    let (id, revision, marks, recovery) = {
        let ledger = lock(shared);
        let Some(entry) = ledger.pages.get(path) else {
            return;
        };
        let Some(pending) = entry
            .pending
            .as_ref()
            .filter(|pending| pending.error.is_none())
        else {
            return;
        };
        (
            pending.id,
            entry.revision,
            pending.marks.clone(),
            entry
                .recovered
                .as_ref()
                .map(|recovery| recovery.path.clone()),
        )
    };
    let result = match recovery {
        Some(copy) => data::write_json(&copy, marks.as_ref()).map(|()| Saved::Recovery(copy)),
        None => persist_primary(path, revision, &marks),
    };
    let mut ledger = lock(shared);
    let Some(entry) = ledger.pages.get_mut(path) else {
        return;
    };
    match result {
        Ok(saved) => {
            match saved {
                Saved::Canonical(revision) => entry.revision = revision,
                Saved::Recovery(copy) => {
                    entry.recovered = Some(Recovery { path: copy, marks });
                }
            }
            if entry
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == id)
            {
                entry.pending = None;
            }
        }
        Err(error) => {
            if let Some(pending) = &mut entry.pending
                && pending.id == id
            {
                pending.error = Some(error.to_string());
            }
        }
    }
}

enum Saved {
    Canonical(Revision),
    Recovery(PathBuf),
}

fn persist_primary(path: &Path, revision: Revision, marks: &Arc<Vec<Mark>>) -> io::Result<Saved> {
    let mut outcome = data::write_page(path, revision, marks.as_ref());
    for delay in [15, 40, 80] {
        if !outcome
            .as_ref()
            .is_err_and(|error| matches!(error.raw_os_error(), Some(32 | 33)))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(delay));
        outcome = data::write_page(path, revision, marks.as_ref());
    }
    match outcome {
        Ok(updated) => Ok(Saved::Canonical(updated)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            data::write_recovery(path, marks).map(Saved::Recovery)
        }
        Err(error) => Err(error),
    }
}

fn lock(ledger: &Mutex<Ledger>) -> MutexGuard<'_, Ledger> {
    ledger.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovery_paths(saver: &Saver) -> Vec<PathBuf> {
        let started = std::time::Instant::now();
        loop {
            match saver.status() {
                SaveStatus::Saving => {
                    assert!(started.elapsed().as_secs() < 5);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                SaveStatus::Recovered(paths) => return paths,
                status => panic!("{status:?}"),
            }
        }
    }

    #[test]
    fn retains_conflicting_edits_when_revisiting_a_page_and_reuses_the_copy() {
        let scratch = data::Scratch::new();
        let path = scratch.0.join("0.json");
        data::write_json(&path, &Vec::<Mark>::new()).unwrap();
        let saver = Saver::start(None).unwrap();
        let mark = Mark::Stroke {
            style: data::Settings::default().pen,
            variable: true,
            points: Arc::new(Vec::new()),
        };
        saver
            .submit(path.clone(), None, vec![mark.clone()])
            .unwrap();
        let copies = recovery_paths(&saver);
        assert_eq!(copies.len(), 1);
        assert_ne!(path, copies[0]);
        assert_eq!(saver.load(&path).unwrap().0.len(), 1);

        saver
            .submit(path.clone(), None, vec![mark.clone(), mark])
            .unwrap();
        assert_eq!(recovery_paths(&saver), copies);
        assert_eq!(saver.load(&path).unwrap().0.len(), 2);
        assert_eq!(data::read_page::<Vec<Mark>>(&copies[0]).unwrap().0.len(), 2);
        assert!(data::read_page::<Vec<Mark>>(&path).unwrap().0.is_empty());

        let another = scratch.0.join("1.json");
        data::write_json(&another, &Vec::<Mark>::new()).unwrap();
        saver.submit(another, None, Vec::new()).unwrap();
        let both = recovery_paths(&saver);
        assert_eq!(both.len(), 2);
        assert!(both.contains(&copies[0]));
    }
}
