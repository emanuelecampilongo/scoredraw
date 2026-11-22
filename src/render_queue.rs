use crate::pdf::{Frame, Score, open_engine, preview_dimensions};
use pdfium_render::prelude::Pdfium;
use std::collections::VecDeque;
use std::path::PathBuf;
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
}

pub enum Request {
    Open(u64, Vec<PathBuf>),
    Render {
        first_page_preview: bool,
        pages: Vec<PageKey>,
    },
}

pub enum Response {
    Opened(u64, Vec<ScoreInfo>, u32),
    Preview(PageKey, Arc<Frame>),
    Rendered(PageKey, Arc<Frame>),
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
    let mut session = 0;
    let mut scores = Vec::new();

    loop {
        let request = if pending.is_empty() {
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
            Some(Request::Open(epoch, files)) => {
                pending.clear();
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
            None => {
                let Some((key, preview)) = pending.pop_front() else {
                    continue;
                };
                if key.session != session {
                    continue;
                }
                if let Some(score) = scores
                    .iter()
                    .rev()
                    .find(|score| score.first_page <= key.index)
                {
                    if preview {
                        if let Some((width, height)) = preview_dimensions(key.width, key.height)
                            && let Ok(frame) = score.render(key.index, width, height)
                        {
                            notify(Response::Preview(key, Arc::new(frame)));
                        }
                        continue;
                    }
                    match score.render(key.index, key.width, key.height) {
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
    files: Vec<PathBuf>,
) -> Result<(Vec<Score<'_>>, Vec<ScoreInfo>, u32), String> {
    let mut scores = Vec::new();
    let mut info = Vec::new();
    let mut page_count = 0u32;
    for file in files {
        let score = Score::open(engine, &file, page_count)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        if score.count == 0 {
            return Err(trf!(
                "{} è privo di pagine.",
                "{} has no pages.",
                file.display()
            ));
        }
        let title = score
            .source
            .path
            .file_stem()
            .unwrap_or(score.source.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        info.push(ScoreInfo {
            title,
            source: score.source.clone(),
            first_page: page_count,
            count: score.count,
        });
        page_count = page_count.checked_add(score.count).ok_or(tr!(
            "Troppi spartiti nella sequenza.",
            "Too many scores in the playlist."
        ))?;
        scores.push(score);
    }
    Ok((scores, info, page_count))
}
use crate::data::ScoreFile;
