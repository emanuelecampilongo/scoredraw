use crate::data::{ScoreEntry, ScoreFile};
use crate::pdf::{Frame, Score, open_engine, preview_dimensions};
use pdfium_render::prelude::Pdfium;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const RENDER_RESULT: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PageKey {
    pub session: u64,
    pub index: u32,
    pub width: u32,
    pub height: u32,
}

pub struct ScoreInfo {
    pub title: String,
    pub source: ScoreFile,
    pub first_page: u32,
    pub count: u32,
    pub pdf_start: u32,
    pub pdf_count: u32,
    pub entry: ScoreEntry,
}

impl ScoreInfo {
    pub fn pdf_page(&self, index: u32) -> u32 {
        index - self.first_page + self.pdf_start
    }

    pub fn index(&self, page: u32) -> Option<u32> {
        let offset = page.checked_sub(self.pdf_start)?;
        (offset < self.count).then(|| self.first_page + offset)
    }
}

pub fn render_plan(current: PageKey, fit: PageKey, total: u32, scores: &[ScoreInfo]) -> Vec<PageKey> {
    let at = |index| PageKey { index, ..fit };
    let mut pages = vec![current];
    if current.index + 1 < total {
        pages.push(at(current.index + 1));
    }
    if scores
        .iter()
        .find(|score| score.first_page > current.index)
        .is_some_and(|next| next.first_page - current.index == 2)
    {
        pages.push(at(current.index + 2));
    }
    if current.index > 0 {
        pages.push(at(current.index - 1));
    }
    pages
}

pub enum Request {
    Open(u64, Vec<ScoreEntry>),
    Close(u64),
    Thumbnails(Vec<PageKey>),
    Render {
        first_page_preview: bool,
        pages: Vec<PageKey>,
    },
}

pub enum Response {
    Opened(u64, Vec<ScoreInfo>, u32),
    Preview(PageKey, Arc<Frame>),
    Rendered(PageKey, Arc<Frame>),
    Thumbnail(PageKey, Option<Arc<Frame>>),
    OpenError(u64, String),
    RenderError(PageKey, String),
    Unavailable(String),
}

pub fn spawn(hwnd: HWND) -> std::io::Result<(Sender<Request>, Receiver<Response>)> {
    let (send_requests, receive_requests) = channel();
    let (send_results, receive_results) = channel();
    // HWND is used by the worker solely to post an asynchronous notification.
    let window_address = hwnd.0 as isize;
    std::thread::Builder::new()
        .name("render-pdf".into())
        .spawn(move || {
            let notify = |response| {
                if send_results.send(response).is_ok() {
                    let hwnd = HWND(window_address as _);
                    let _ =
                        unsafe { PostMessageW(Some(hwnd), RENDER_RESULT, WPARAM(0), LPARAM(0)) };
                }
            };
            // All PDFium calls and document handles stay on this thread.
            let engine = match open_engine() {
                Ok(engine) => engine,
                Err(error) => {
                    notify(Response::Unavailable(error.to_string()));
                    return;
                }
            };
            work(&engine, receive_requests, notify);
        })?;
    Ok((send_requests, receive_results))
}

fn work(engine: &Pdfium, inbox: Receiver<Request>, notify: impl Fn(Response)) {
    let mut pending: VecDeque<(PageKey, bool)> = VecDeque::new();
    let mut thumbnails = VecDeque::new();
    let mut session = 0;
    let mut scores = Vec::new();

    loop {
        let request = if pending.is_empty() && thumbnails.is_empty() {
            match inbox.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            }
        } else {
            match inbox.try_recv() {
                Ok(request) => Some(request),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        };
        match request {
            Some(Request::Close(epoch)) => {
                pending.clear();
                thumbnails.clear();
                session = epoch;
                scores.clear();
            }
            Some(Request::Open(epoch, files)) => {
                pending.clear();
                thumbnails.clear();
                session = epoch;
                scores.clear();
                match open_scores(engine, files) {
                    Ok((new_scores, info, page_count)) => {
                        scores = new_scores;
                        notify(Response::Opened(epoch, info, page_count));
                    }
                    Err(error) => notify(Response::OpenError(epoch, error)),
                }
            }
            Some(Request::Render {
                first_page_preview,
                pages,
            }) => {
                pending = pages.into_iter().map(|key| (key, false)).collect();
                if first_page_preview && let Some(&(first, _)) = pending.front() {
                    pending.push_front((first, true));
                }
            }
            Some(Request::Thumbnails(pages)) => thumbnails = pages.into(),
            None => {
                let Some((key, preview, thumbnail)) = pending
                    .pop_front()
                    .map(|(key, preview)| (key, preview, false))
                    .or_else(|| thumbnails.pop_front().map(|key| (key, false, true)))
                else {
                    continue;
                };
                if key.session != session {
                    continue;
                }
                if let Some((score, pdf_start)) = scores
                    .iter()
                    .rev()
                    .find(|(score, _)| score.first_page <= key.index)
                {
                    let pdf_page = key.index - score.first_page + pdf_start;
                    if thumbnail {
                        let frame = score.render_page(pdf_page, key.width, key.height).ok().map(Arc::new);
                        notify(Response::Thumbnail(key, frame));
                        continue;
                    }
                    if preview {
                        if let Some((width, height)) = preview_dimensions(key.width, key.height)
                            && let Ok(frame) = score.render_page(pdf_page, width, height)
                        {
                            notify(Response::Preview(key, Arc::new(frame)));
                        }
                        continue;
                    }
                    match score.render_page(pdf_page, key.width, key.height) {
                        Ok(frame) => notify(Response::Rendered(key, Arc::new(frame))),
                        Err(error) => notify(Response::RenderError(key, error.to_string())),
                    }
                }
            }
        }
    }
}

fn open_scores(
    engine: &Pdfium,
    files: Vec<ScoreEntry>,
) -> Result<(Vec<(Score<'_>, u32)>, Vec<ScoreInfo>, u32), String> {
    let mut scores = Vec::new();
    let mut info = Vec::new();
    let mut page_count = 0u32;
    for entry in files {
        let file = entry.path();
        let score = Score::open(engine, file, page_count)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        if score.count == 0 {
            return Err(trf!(
                "{} è privo di pagine.",
                "{} has no pages.",
                file.display()
            ));
        }
        let (start, end) = entry.range().unwrap_or((1, score.count));
        if start == 0 || start > end || end > score.count {
            return Err(trf!(
                "{}: intervallo {}–{} fuori dalle pagine 1–{}.",
                "{}: range {}–{} outside pages 1–{}.",
                file.display(), start, end, score.count
            ));
        }
        let count = end - start + 1;
        let mut title = score
            .source
            .path
            .file_stem()
            .unwrap_or(score.source.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        if entry.range().is_some() {
            title.push_str(&format!(" · {start}–{end}"));
        }
        info.push(ScoreInfo {
            title,
            source: score.source.clone(),
            first_page: page_count,
            count,
            pdf_start: start - 1,
            pdf_count: score.count,
            entry,
        });
        page_count = page_count.checked_add(count).ok_or(tr!(
            "Troppi spartiti nella sequenza.",
            "Too many scores in the playlist."
        ))?;
        scores.push((score, start - 1));
    }
    Ok((scores, info, page_count))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Scratch, Store};
    use lopdf::{Dictionary, Document, Object, Stream, dictionary};
    use std::time::{Duration, Instant};

    #[test]
    fn renders_the_first_physical_page_of_the_next_range() {
        let scratch = Scratch::new();
        let path = scratch.0.join("scan.pdf");
        let mut pdf = Document::with_version("1.4");
        let parent = pdf.new_object_id();
        let mut pages = Vec::new();
        for tone in [40u8, 100, 180] {
            // Uncompressed scan-sized images exercise decoding and rasterization.
            let image = pdf.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Image",
                    "Width" => 2200, "Height" => 3000, "BitsPerComponent" => 8,
                    "ColorSpace" => "DeviceGray",
                },
                vec![tone; 2200 * 3000],
            ));
            let contents = pdf.add_object(Stream::new(
                Dictionary::new(),
                b"q 220 0 0 300 0 0 cm /Scan Do Q".to_vec(),
            ));
            pages.push(Object::Reference(pdf.add_object(dictionary! {
                "Type" => "Page", "Parent" => Object::Reference(parent),
                "Contents" => Object::Reference(contents),
                "Resources" => dictionary! {
                    "XObject" => dictionary! { "Scan" => Object::Reference(image) },
                },
            })));
        }
        pdf.set_object(parent, dictionary! {
            "Type" => "Pages", "Count" => 3, "Kids" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 220.into(), 300.into()],
        });
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => Object::Reference(parent),
        });
        pdf.trailer.set("Root", Object::Reference(catalog));
        pdf.save(&path).unwrap();

        let (requests, inbox) = channel();
        let (outbox, responses) = channel();
        let worker = std::thread::spawn(move || {
            let engine = open_engine().unwrap();
            work(&engine, inbox, |response| outbox.send(response).unwrap());
        });
        requests
            .send(Request::Open(
                1,
                vec![
                    ScoreEntry::new(path.clone(), Some((2, 3))),
                    ScoreEntry::new(path, Some((2, 2))),
                ],
            ))
            .unwrap();
        let timeout = Duration::from_secs(10);
        let Response::Opened(1, scores, 3) = responses.recv_timeout(timeout).unwrap() else {
            panic!("Could not open ranged scores");
        };
        assert_eq!(scores[0].pdf_page(0), 1);
        assert_eq!(scores[0].index(0), None);
        assert_eq!(scores[1].index(1), Some(2));
        let store = Store::at(scratch.0.join("state"));
        assert_eq!(
            store.page_path(&scores[0].source, scores[0].pdf_page(0)),
            store.page_path(&scores[1].source, scores[1].pdf_page(2)),
        );
        let turn = |index| PageKey {
            session: 1,
            index,
            width: 1600,
            height: 2200,
        };
        let entering = render_plan(turn(0), turn(0), 3, &scores);
        assert_eq!(entering, [turn(0), turn(1), turn(2)]);
        let leaving = render_plan(turn(2), turn(2), 3, &scores);
        assert_eq!(leaving, [turn(2), turn(1)]);
        let started = Instant::now();
        let receive = |index, tone| {
            let Response::Rendered(key, frame) = responses.recv_timeout(timeout).unwrap() else {
                panic!("Could not render ranged page");
            };
            assert_eq!(key, turn(index));
            assert_eq!(frame.pixels[frame.pixels.len() / 2], tone);
        };
        requests
            .send(Request::Render {
                first_page_preview: false,
                pages: entering,
            })
            .unwrap();
        receive(0, 100);
        receive(1, 180);
        receive(2, 100);
        eprintln!("Scan-sized boundary renders: {:?}", started.elapsed());
        let thumbnail = PageKey { width: 160, height: 220, ..turn(2) };
        requests.send(Request::Thumbnails(vec![thumbnail])).unwrap();
        let Response::Thumbnail(key, Some(frame)) = responses.recv_timeout(timeout).unwrap() else {
            panic!("Could not render the ranged thumbnail");
        };
        assert_eq!(key, thumbnail);
        assert_eq!(frame.pixels[frame.pixels.len() / 2], 100);
        requests
            .send(Request::Render {
                first_page_preview: false,
                pages: leaving.into_iter().skip(1).collect(),
            })
            .unwrap();
        receive(1, 180);
        drop(requests);
        worker.join().unwrap();
    }
}
